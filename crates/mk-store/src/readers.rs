//! Read connections (ADR-0004): a small pool of `query_only` connections. With WAL they read the
//! last committed state and never wait for the writer.

use std::sync::{Condvar, Mutex};

use rusqlite::Connection;

use crate::StoreError;

pub(crate) struct Readers {
    free: Mutex<Vec<Connection>>,
    returned: Condvar,
}

impl Readers {
    pub(crate) fn new(conns: Vec<Connection>) -> Readers {
        Readers {
            free: Mutex::new(conns),
            returned: Condvar::new(),
        }
    }

    /// Runs `f` on a free connection, waiting for one if all are busy.
    pub(crate) fn with<R>(
        &self,
        f: impl FnOnce(&Connection) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        let conn = {
            let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(c) = free.pop() {
                    break c;
                }
                free = self.returned.wait(free).unwrap_or_else(|e| e.into_inner());
            }
        };
        // Give the connection back even if `f` panics.
        struct Return<'a>(&'a Readers, Option<Connection>);
        impl Drop for Return<'_> {
            fn drop(&mut self) {
                if let Some(c) = self.1.take() {
                    self.0
                        .free
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
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
