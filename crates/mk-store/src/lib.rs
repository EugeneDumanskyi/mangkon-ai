//! SQLite adapters, schema and migrations (ADR-0004): one writer thread, four readers, WAL.
//!
//! - [`Store::open`] checks the schema version, backs up and migrates (domain §7.9), and
//!   rebuilds the derived tables when needed.
//! - Every write method is **one** transaction on the writer thread. A Stage A commit holds the
//!   content's documents, segments and mentions, the per-key rebuild of the derived rows, and
//!   the item's Done state (INV-PR-04, ADR-0005, ADR-0006).
//! - Reads run on a pool of `query_only` connections and see the last commit.
//!
//! Plain `std` threads and channels, no async runtime.

mod codec;
mod derive;
mod ops;
mod read;
mod readers;
mod schema;
mod time;
mod writer;

use std::fmt;
use std::path::{Path, PathBuf};

use mk_corpus::StoredRef;
use mk_graph::model::{DerivedState, Facts};
use mk_graph::{ConnectionRow, DecisionKind, RankKey};
use mk_jobs::{PendingItem, Trigger};
use mk_kernel::ContentHash;

pub use ops::{
    CommitOutcome, DocumentOut, MentionOut, NewContent, Recovery, RescanChange, RescanOutcome,
    SegmentOut, StageAOutput, StartedItem,
};
pub use read::{ContentRow, MentionRow, Progress, RootRow, SegmentRow};
pub use schema::{MIGRATIONS, Migration, OpenReport};

/// Read connections (architecture §7).
pub const READERS: usize = 4;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// The project was written by a newer app. It was not opened or changed (INV-WS-03).
    TooNew {
        found: u32,
        supported: u32,
    },
    /// A migration failed and was rolled back; the project is still at `from`, and the backup
    /// is kept (INV-WS-04).
    MigrationFailed {
        from: u32,
        to: u32,
        backup: Option<PathBuf>,
        reason: String,
    },
    /// A rule was broken (an illegal state move, a bad locator, …). The transaction rolled back.
    Invalid(String),
    /// The writer thread is gone.
    WriterGone,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "database error: {e}"),
            StoreError::Io(e) => write!(f, "file error: {e}"),
            StoreError::TooNew { found, supported } => write!(
                f,
                "This project was made by a newer version of Mangkon AI (schema {found}; this \
                 version reads up to {supported}). It was not opened or changed."
            ),
            StoreError::MigrationFailed {
                from,
                backup,
                reason,
                ..
            } => {
                write!(
                    f,
                    "The project could not be upgraded and was left at schema {from}"
                )?;
                if let Some(b) = backup {
                    write!(f, "; a backup is at {}", b.display())?;
                }
                write!(f, " ({reason})")
            }
            StoreError::Invalid(m) => f.write_str(m),
            StoreError::WriterGone => f.write_str("the database writer stopped"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// An open project database.
pub struct Store {
    // Readers first: they close before the writer checkpoints on drop.
    readers: readers::Readers,
    writer: writer::Writer,
    path: PathBuf,
}

impl Store {
    /// Opens or creates `path` at the app's schema ([`MIGRATIONS`]). Backups go to `backups`.
    pub fn open(path: &Path, backups: &Path) -> Result<(Store, OpenReport), StoreError> {
        Self::open_with(path, backups, MIGRATIONS)
    }

    /// [`open`](Self::open) with another migration list (tests inject a v2 and a broken v2).
    pub fn open_with(
        path: &Path,
        backups: &Path,
        migrations: &[Migration],
    ) -> Result<(Store, OpenReport), StoreError> {
        let (conn, report) = schema::open_write(path, backups, migrations)?;
        let readers = (0..READERS)
            .map(|_| schema::open_read(path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((
            Store {
                readers: readers::Readers::new(readers),
                writer: writer::Writer::start(conn),
                path: path.to_path_buf(),
            },
            report,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // ---- writes: one transaction each ----

    pub fn add_source_root(&self, path: &str, kind: &str) -> Result<i64, StoreError> {
        let (path, kind) = (path.to_string(), kind.to_string());
        self.writer
            .call(move |tx| ops::add_source_root(tx, &path, &kind))
    }

    /// See `ops::apply_rescan`.
    pub fn apply_rescan(
        &self,
        changes: Vec<RescanChange>,
        trigger: Trigger,
    ) -> Result<RescanOutcome, StoreError> {
        self.writer
            .call(move |tx| ops::apply_rescan(tx, &changes, trigger))
    }

    /// Contents without file references, each with a Stage A item (benchmarks and tests).
    pub fn enqueue_contents(&self, contents: Vec<NewContent>) -> Result<Vec<i64>, StoreError> {
        self.writer
            .call(move |tx| ops::enqueue_contents(tx, &contents))
    }

    pub fn start_item(&self, item: i64) -> Result<Option<StartedItem>, StoreError> {
        self.writer.call(move |tx| ops::start_item(tx, item))
    }

    /// Commits a Stage A result with its derived rebuild and the item's Done state, in one
    /// transaction (INV-PR-04, ADR-0005).
    pub fn commit_stage_a(
        &self,
        item: i64,
        output: StageAOutput,
    ) -> Result<CommitOutcome, StoreError> {
        self.writer
            .call(move |tx| ops::commit_stage_a(tx, item, &output))
    }

    pub fn fail_item(&self, item: i64, code: &str, reason: &str) -> Result<(), StoreError> {
        let (code, reason) = (code.to_string(), reason.to_string());
        self.writer
            .call(move |tx| ops::fail_item(tx, item, &code, &reason))
    }

    /// Appends a decision and rebuilds its anchor. Returns its `seq` and whether a connection
    /// changed.
    pub fn append_decision(
        &self,
        kind: DecisionKind,
        anchor: &str,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<(i64, bool), StoreError> {
        let anchor = anchor.to_string();
        let (before, after) = (before.map(str::to_string), after.map(str::to_string));
        self.writer.call(move |tx| {
            ops::append_decision(tx, kind, &anchor, before.as_deref(), after.as_deref())
        })
    }

    /// Drops and rebuilds every derived table (ADR-0005 "full rebuild").
    pub fn rebuild_all(&self) -> Result<(), StoreError> {
        self.writer.call(|tx| derive::rebuild_all(tx))
    }

    /// In-progress items back to Pending, Processing contents back to Queued (ADR-0006).
    pub fn recover_after_restart(&self) -> Result<Recovery, StoreError> {
        self.writer.call(|tx| ops::recover_after_restart(tx))
    }

    /// Runs `f` in one writer transaction. For tests that need a raw write, such as checking
    /// the decision log triggers.
    pub fn write_raw<R, F>(&self, f: F) -> Result<R, StoreError>
    where
        R: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<R, StoreError> + Send + 'static,
    {
        self.writer.call(move |tx| f(tx))
    }

    // ---- reads ----

    pub fn source_roots(&self) -> Result<Vec<RootRow>, StoreError> {
        self.readers.with(read::source_roots)
    }

    pub fn stored_refs(&self) -> Result<Vec<StoredRef>, StoreError> {
        self.readers.with(read::stored_refs)
    }

    pub fn pending_items(&self) -> Result<Vec<PendingItem>, StoreError> {
        self.readers.with(read::pending_items)
    }

    /// Connections in ranking order (domain §9.2), keyset-paged after `after`.
    pub fn connections_page(
        &self,
        after: Option<&RankKey>,
        limit: usize,
        include_ignored: bool,
    ) -> Result<Vec<ConnectionRow>, StoreError> {
        self.readers
            .with(|c| read::connections_page(c, after, limit, include_ignored))
    }

    pub fn progress(&self) -> Result<Progress, StoreError> {
        self.readers.with(read::progress)
    }

    pub fn content(&self, hash: &ContentHash) -> Result<Option<ContentRow>, StoreError> {
        self.readers.with(|c| read::content(c, hash))
    }

    pub fn segments(&self, content: i64) -> Result<Vec<SegmentRow>, StoreError> {
        self.readers.with(|c| read::segments(c, content))
    }

    pub fn mentions(&self, content: i64) -> Result<Vec<MentionRow>, StoreError> {
        self.readers.with(|c| read::mentions(c, content))
    }

    pub fn derived_state(&self) -> Result<DerivedState, StoreError> {
        self.readers.with(read::derived_state)
    }

    pub fn facts(&self) -> Result<Facts, StoreError> {
        self.readers.with(read::facts)
    }

    /// Runs `f` on a read connection. For tests.
    pub fn read_raw<R>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        self.readers.with(f)
    }
}
