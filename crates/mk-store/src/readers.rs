//! Read connections (ADR-0004): a small pool of `query_only` connections. With WAL they read the
//! last committed state and never wait for the writer.
//!
//! Connections open on demand, up to the pool's size; [`Readers::release`] closes the idle ones
//! with their page caches after a run (NFR-RES-01), and the next reads open them again.

use std::path::PathBuf;
use std::sync::{Condvar, Mutex};

use rusqlite::Connection;

use crate::StoreError;

pub(crate) struct Readers {
    path: PathBuf,
    size: usize,
    pool: Mutex<Pool>,
    returned: Condvar,
}

struct Pool {
    free: Vec<Connection>,
    /// Connections open now, free or in use.
    open: usize,
}

impl Readers {
    /// A pool of at most `size` connections to `path`, `conns` open already.
    pub(crate) fn new(path: PathBuf, size: usize, conns: Vec<Connection>) -> Readers {
        Readers {
            path,
            size: size.max(1),
            pool: Mutex::new(Pool {
                open: conns.len(),
                free: conns,
            }),
            returned: Condvar::new(),
        }
    }

    /// Closes every idle connection but one, after `PRAGMA shrink_memory` on that one. Busy
    /// connections are left alone.
    pub(crate) fn release(&self) {
        let closed: Vec<Connection> = {
            let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
            let keep = pool.free.len().min(1);
            let closed: Vec<Connection> = pool.free.drain(keep..).collect();
            pool.open -= closed.len();
            if let Some(c) = pool.free.first() {
                let _ = c.execute_batch("PRAGMA shrink_memory");
            }
            closed
        };
        drop(closed);
    }

    /// Runs `f` on a free connection, opening one if the pool has room, else waiting for one.
    pub(crate) fn with<R>(
        &self,
        f: impl FnOnce(&Connection) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let conn = {
            let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(c) = pool.free.pop() {
                    break Some(c);
                }
                if pool.open < self.size {
                    pool.open += 1;
                    break None;
                }
                pool = self.returned.wait(pool).unwrap_or_else(|e| e.into_inner());
            }
        };
        let conn = match conn {
            Some(c) => c,
            None => crate::schema::open_read(&self.path).inspect_err(|_| {
                self.pool.lock().unwrap_or_else(|e| e.into_inner()).open -= 1;
                self.returned.notify_one();
            })?,
        };
        // Give the connection back even if `f` panics.
        struct Return<'a>(&'a Readers, Option<Connection>);
        impl Drop for Return<'_> {
            fn drop(&mut self) {
                if let Some(c) = self.1.take() {
                    self.0
                        .pool
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .free
                        .push(c);
                    self.0.returned.notify_one();
                }
            }
        }
        let guard = Return(self, Some(conn));
        let conn = guard.1.as_ref().expect("held");
        // One read transaction, so a multi-query read sees one committed state.
        conn.execute_batch("BEGIN")?;
        let result = f(conn);
        let _ = conn.execute_batch("COMMIT");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NFR-RES-01: released readers close, and reads open them again.
    #[test]
    fn release_closes_idle_readers_and_reads_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        let w = Connection::open(&path).unwrap();
        w.execute_batch(
            "PRAGMA journal_mode = WAL; CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (7);",
        )
        .unwrap();
        let conns = (0..3)
            .map(|_| crate::schema::open_read(&path).unwrap())
            .collect();
        let r = Readers::new(path, 3, conns);
        r.release();
        assert_eq!(r.pool.lock().unwrap().open, 1);
        let x: i64 = r
            .with(|c| Ok(c.query_row("SELECT x FROM t", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(x, 7);
        // Nested reads need a second connection: it is opened, not waited for.
        let n: i64 = r
            .with(|_| r.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))?)))
            .unwrap();
        assert_eq!((n, r.pool.lock().unwrap().open), (1, 2));
    }
}
