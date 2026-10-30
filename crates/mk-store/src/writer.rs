//! The one writer thread (ADR-0004): it owns the only write connection and runs each job in its
//! own transaction, so a job's writes commit all together or not at all (INV-PR-04).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread::JoinHandle;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::StoreError;

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

pub(crate) struct Writer {
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn start(mut conn: Connection) -> Writer {
        let (jobs, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("mk-store-writer".into())
            .spawn(move || {
                for job in rx {
                    job(&mut conn);
                }
            })
            .expect("writer thread starts");
        Writer {
            jobs: Some(jobs),
            thread: Some(thread),
        }
    }

    /// Runs `f` in one immediate transaction on the writer thread and waits for it. `Ok` commits;
    /// `Err` or a panic rolls everything back and leaves the writer running.
    pub(crate) fn call<R, F>(&self, f: F) -> Result<R, StoreError>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction) -> Result<R, StoreError> + Send + 'static,
    {
        let (reply, answer) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |conn| {
            let result = run(conn, f);
            let _ = reply.send(result);
        });
        self.jobs
            .as_ref()
            .ok_or(StoreError::WriterGone)?
            .send(job)
            .map_err(|_| StoreError::WriterGone)?;
        answer.recv().map_err(|_| StoreError::WriterGone)?
    }
}

impl Writer {
    /// `PRAGMA shrink_memory` on the write connection, outside any transaction: frees its page
    /// cache after a run (NFR-RES-01). Waits for the jobs queued before it.
    pub(crate) fn shrink(&self) -> Result<(), StoreError> {
        let (reply, answer) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |conn| {
            let _ = reply.send(conn.execute_batch("PRAGMA shrink_memory"));
        });
        self.jobs
            .as_ref()
            .ok_or(StoreError::WriterGone)?
            .send(job)
            .map_err(|_| StoreError::WriterGone)?;
        Ok(answer.recv().map_err(|_| StoreError::WriterGone)??)
    }
}

fn run<R, F>(conn: &mut Connection, f: F) -> Result<R, StoreError>
where
    F: FnOnce(&Transaction) -> Result<R, StoreError>,
{
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match catch_unwind(AssertUnwindSafe(|| f(&tx))) {
        Ok(Ok(r)) => {
            tx.commit()?;
            Ok(r)
        }
        // Dropping the transaction rolls it back.
        Ok(Err(e)) => Err(e),
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(StoreError::Invalid(format!("write job panicked: {msg}")))
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Closing the channel ends the thread once queued jobs are done.
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
