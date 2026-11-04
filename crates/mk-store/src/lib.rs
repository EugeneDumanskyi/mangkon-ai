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

mod app_store;
mod codec;
mod derive;
mod documents;
mod egress;
mod manage;
mod mapping;
mod ops;
mod read;
mod readers;
mod schema;
mod search;
mod table;
mod time;
mod writer;

use std::fmt;
use std::path::{Path, PathBuf};

use mk_corpus::StoredRef;
use mk_graph::DecisionKind;
use mk_graph::model::{DerivedState, EntityRow, Facts};
use mk_jobs::Trigger;
use mk_kernel::ContentHash;
use mk_workspace::ProjectSettings;

pub use app_store::{APP_MIGRATIONS, AppStore, EndpointRow, EndpointTestRow, ModelDownloadRow};
pub use documents::{DocumentCursor, DocumentFilter, DocumentRow, ProgressDetail, STATUS_MISSING};
pub use egress::{EgressRecordRow, PermissionRow};
pub use manage::{CancelOutcome, RemoveOutcome};
pub use mapping::{ColumnRow, MappingView, ReuseCandidate};
pub use ops::{
    CardFlagOut, CommitOutcome, DocumentOut, FoundFlag, FoundMention, MentionOut, NewContent,
    Recovery, RerunOutcome, RerunStep, RescanChange, RescanOutcome, SegmentFinds, SegmentOut,
    StageAOutput, StagedPart, StartedItem, StoredSegment,
};
pub use read::{
    CardFlagRow, ConnectionQuery, ContentRow, DEFAULT_OCR_LANGUAGES, DocumentInfo, FailedFile,
    KnownFile, ListCursor, ListedConnection, MentionRow, OccurrenceRow, PendingWork, Progress,
    RecordLinkRow, RootRow, SegmentRow, SegmentWindow, SortColumn, WindowSegment,
};
pub use schema::{MIGRATIONS, Migration, OpenReport};
pub use search::{SearchCursor, SearchFilter, SearchRow};
pub use table::SHOWN_SAMPLES;

/// Read connections (architecture §7).
pub const READERS: usize = 4;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// The project was written by a newer app. It was not opened or changed (INV-WS-03).
    /// `app_version` is the app that wrote it, when the project says (schema v4 on).
    TooNew {
        found: u32,
        supported: u32,
        app_version: Option<String>,
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
    /// Another registered project has this name, ignoring case (INV-WS-01).
    NameTaken(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "database error: {e}"),
            StoreError::Io(e) => write!(f, "file error: {e}"),
            StoreError::TooNew {
                found,
                supported,
                app_version,
            } => {
                match app_version {
                    Some(v) => write!(f, "This project needs Mangkon AI {v} or newer")?,
                    None => {
                        f.write_str("This project was made by a newer version of Mangkon AI")?
                    }
                }
                write!(
                    f,
                    " (schema {found}; this version reads up to {supported}). It was not opened \
                     or changed."
                )
            }
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
            StoreError::NameTaken(_) => f.write_str("A project with this name already exists"),
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
                readers: readers::Readers::new(path.to_path_buf(), READERS, readers),
                writer: writer::Writer::start(conn),
                path: path.to_path_buf(),
            },
            report,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Gives memory back after a run (NFR-RES-01): shrinks the writer's page cache and closes
    /// idle readers but one. Reads afterwards open readers again as needed.
    pub fn release_memory(&self) -> Result<(), StoreError> {
        self.readers.release();
        self.writer.shrink()
    }

    // ---- writes: one transaction each ----

    pub fn add_source_root(&self, path: &str, kind: &str) -> Result<i64, StoreError> {
        let (path, kind) = (path.to_string(), kind.to_string());
        self.writer
            .call(move |tx| ops::add_source_root(tx, &path, &kind))
    }

    /// Sets the project's OCR languages, `+`-joined (US-02.2 AC4). The caller checks the codes.
    pub fn set_ocr_languages(&self, languages: &str) -> Result<(), StoreError> {
        let languages = languages.to_string();
        self.writer
            .call(move |tx| ops::set_ocr_languages(tx, &languages))
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
        mut output: StageAOutput,
    ) -> Result<CommitOutcome, StoreError> {
        let warnings = std::mem::take(&mut output.warnings);
        self.commit_stage_a_staged(item, output.into_parts().map(Ok), warnings)
    }

    /// Commits a staged Stage A result, read part by part in one transaction: memory stays
    /// O(batch) whatever the file's size (§4 #16, ADR-0016). An `Err` part rolls it all back.
    pub fn commit_stage_a_staged(
        &self,
        item: i64,
        parts: impl Iterator<Item = Result<StagedPart, StoreError>> + Send + 'static,
        warnings: Vec<String>,
    ) -> Result<CommitOutcome, StoreError> {
        self.writer
            .call(move |tx| ops::commit_stage_a(tx, item, parts, &warnings))
    }

    /// Fails an item with a stable code, a plain reason and the technical detail behind "Show
    /// details" (US-02.7 AC2).
    pub fn fail_item(
        &self,
        item: i64,
        code: &str,
        reason: &str,
        detail: &str,
    ) -> Result<(), StoreError> {
        let (code, reason, detail) = (code.to_string(), reason.to_string(), detail.to_string());
        self.writer
            .call(move |tx| ops::fail_item(tx, item, &code, &reason, &detail))
    }

    /// An item stopped by pause, cancel or close goes back to Pending (domain §7.2).
    pub fn release_item(&self, item: i64) -> Result<(), StoreError> {
        self.writer.call(move |tx| ops::release_item(tx, item))
    }

    /// Pauses (or resumes) the project's queue: open jobs Paused (or Running again), and new
    /// jobs start Paused while it is (US-03.2, INV-PR-06). Returns the jobs moved.
    pub fn set_queue_paused(&self, paused: bool) -> Result<usize, StoreError> {
        self.writer
            .call(move |tx| manage::set_queue_paused(tx, paused))
    }

    /// Cancels every open job; their unfinished contents become Not processed (US-03.3, D10).
    pub fn cancel_queue(&self) -> Result<CancelOutcome, StoreError> {
        self.writer.call(|tx| manage::cancel_queue(tx))
    }

    /// Failed and not-processed `contents` run again under a new Retry job (US-02.7 AC3).
    /// Returns how many were queued.
    pub fn retry_contents(&self, contents: Vec<i64>) -> Result<usize, StoreError> {
        self.writer
            .call(move |tx| manage::retry_contents(tx, &contents).map(|(_, n)| n))
    }

    /// Contents OCR read, or could not for a missing language, run again (US-01.6 AC3).
    pub fn requeue_for_ocr(&self) -> Result<usize, StoreError> {
        self.writer
            .call(|tx| manage::requeue_for_ocr(tx).map(|(_, n)| n))
    }

    /// Removes file references; contents left without one are Removed (US-01.5, INV-CO-08).
    pub fn remove_file_refs(&self, refs: Vec<i64>) -> Result<RemoveOutcome, StoreError> {
        self.writer
            .call(move |tx| manage::remove_file_refs(tx, &refs))
    }

    /// Removes a source root with all its paths.
    pub fn remove_root(&self, root: i64) -> Result<RemoveOutcome, StoreError> {
        self.writer.call(move |tx| manage::remove_root(tx, root))
    }

    /// Points a source root at a new absolute path (D12).
    pub fn relocate_root(&self, root: i64, path: &str) -> Result<(), StoreError> {
        let path = path.to_string();
        self.writer
            .call(move |tx| manage::relocate_root(tx, root, &path))
    }

    /// Removes the exclusions at and under `rel_path` of `root` (the path was added again).
    pub fn clear_exclusions(&self, root: i64, rel_path: &str) -> Result<usize, StoreError> {
        let rel_path = rel_path.to_string();
        self.writer
            .call(move |tx| manage::clear_exclusions(tx, root, &rel_path))
    }

    /// Stores checked settings (US-01.6); a stop-list edit rebuilds the domains it adds or
    /// drops (INV-LK-03). Returns whether a connection row changed.
    pub fn set_settings(&self, settings: ProjectSettings) -> Result<bool, StoreError> {
        self.writer
            .call(move |tx| manage::set_settings(tx, &settings))
    }

    /// Writes the project's name (when given) and the app version that wrote it (US-01.2 AC3).
    pub fn stamp(&self, name: Option<&str>, app_version: &str) -> Result<(), StoreError> {
        let (name, version) = (name.map(str::to_string), app_version.to_string());
        self.writer
            .call(move |tx| manage::stamp(tx, name.as_deref(), &version))
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

    /// Finds one content's identifiers of `step` again from its stored segments with `find`,
    /// replacing that step's mentions (and, for the built-in step, its card-like flags), running
    /// its table step again and rebuilding the keys, in one transaction (M4b, M4c; see
    /// `ops::rerun_step`). Runs on the writer thread. `None` if the re-run was started again
    /// since `after`.
    pub fn rerun<F>(
        &self,
        step: RerunStep,
        after: i64,
        content: i64,
        mut find: F,
    ) -> Result<Option<RerunOutcome>, StoreError>
    where
        F: FnMut(&StoredSegment) -> SegmentFinds + Send + 'static,
    {
        self.writer
            .call(move |tx| ops::rerun_step(tx, step, after, content, &mut find))
    }

    /// Marks a re-run of `step` over every content as started.
    pub fn begin_rerun(&self, step: RerunStep) -> Result<(), StoreError> {
        self.writer.call(move |tx| ops::begin_rerun(tx, step))
    }

    /// Marks the re-run of `step` at cursor `after` done: the mentions follow `stamp` (the
    /// rules version, or the pattern set's hash). `false` if it was started again since.
    pub fn finish_rerun(
        &self,
        step: RerunStep,
        after: Option<i64>,
        stamp: &str,
    ) -> Result<bool, StoreError> {
        let stamp = stamp.to_string();
        self.writer
            .call(move |tx| ops::finish_rerun(tx, step, after, &stamp))
    }

    /// [`rerun`](Self::rerun) of the built-in step.
    pub fn rerun_identifiers<F>(
        &self,
        after: i64,
        content: i64,
        find: F,
    ) -> Result<Option<RerunOutcome>, StoreError>
    where
        F: FnMut(&StoredSegment) -> SegmentFinds + Send + 'static,
    {
        self.rerun(RerunStep::Builtin, after, content, find)
    }

    /// Marks an identifier re-run of every content as started (a region change).
    pub fn begin_identifier_rerun(&self) -> Result<(), StoreError> {
        self.begin_rerun(RerunStep::Builtin)
    }

    /// Marks the identifier re-run at cursor `after` done: the mentions follow rules `version`.
    /// `false` if it was started again since.
    pub fn finish_identifier_rerun(
        &self,
        after: Option<i64>,
        version: u32,
    ) -> Result<bool, StoreError> {
        self.finish_rerun(RerunStep::Builtin, after, &version.to_string())
    }

    /// Replaces the custom patterns (checked by the caller) and starts their re-run, in one
    /// transaction (US-04.7 AC3).
    pub fn set_custom_patterns(&self, patterns: Vec<(String, String)>) -> Result<(), StoreError> {
        self.writer
            .call(move |tx| ops::set_custom_patterns(tx, &patterns))
    }

    /// Confirms the column mapping of the file at `file_ref` and applies it, in one transaction
    /// (US-05.2 AC1). Returns the decision's `seq` and whether a connection changed.
    pub fn confirm_mapping(
        &self,
        file_ref: i64,
        assignments: mk_mapping::Assignments,
    ) -> Result<(i64, bool), StoreError> {
        self.writer
            .call(move |tx| ops::confirm_mapping(tx, file_ref, &assignments))
    }

    /// Drops and rebuilds every derived table (ADR-0005 "full rebuild").
    pub fn rebuild_all(&self) -> Result<(), StoreError> {
        self.writer.call(|tx| derive::rebuild_all(tx))
    }

    /// Refills the keyword index from the stored segment text (FTS5 'rebuild', ADR-0010).
    pub fn rebuild_search_index(&self) -> Result<(), StoreError> {
        self.writer.call(|tx| search::rebuild_index(tx))
    }

    /// Checks the keyword index against the stored segment text (FTS5 'integrity-check',
    /// NFR-REL-02). Runs on the writer: FTS5 commands are writes, even this one.
    pub fn check_search_index(&self) -> Result<(), StoreError> {
        self.writer.call(|tx| search::check_index(tx))
    }

    /// In-progress items back to Pending, Processing contents back to Queued (ADR-0006).
    pub fn recover_after_restart(&self) -> Result<Recovery, StoreError> {
        self.writer.call(|tx| ops::recover_after_restart(tx))
    }

    // ---- egress (schema v8, BC-11) ----

    /// Stores a confirmed permission for a non-local endpoint (US-09.7 AC2).
    pub fn allow_endpoint(&self, id: &str, origin: &str, class: &str) -> Result<(), StoreError> {
        let (id, origin, class) = (id.to_string(), origin.to_string(), class.to_string());
        self.writer
            .call(move |tx| egress::allow(tx, &id, &origin, &class))
    }

    /// Moves an allowed permission to "needs reconfirmation" (domain §7.6).
    pub fn permission_needs_reconfirmation(
        &self,
        id: &str,
        reason: &str,
    ) -> Result<bool, StoreError> {
        let (id, reason) = (id.to_string(), reason.to_string());
        self.writer
            .call(move |tx| egress::needs_reconfirmation(tx, &id, &reason))
    }

    /// Revokes a permission: Not allowed again.
    pub fn revoke_endpoint(&self, id: &str) -> Result<bool, StoreError> {
        let id = id.to_string();
        self.writer.call(move |tx| egress::revoke(tx, &id))
    }

    /// Upserts the egress record before a send (INV-EG-05).
    pub fn record_egress(&self, host: &str, class: &str, purpose: &str) -> Result<(), StoreError> {
        let (host, class, purpose) = (host.to_string(), class.to_string(), purpose.to_string());
        self.writer
            .call(move |tx| egress::record(tx, &host, &class, &purpose))
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

    /// The project's OCR languages; [`DEFAULT_OCR_LANGUAGES`] until set.
    pub fn ocr_languages(&self) -> Result<String, StoreError> {
        self.readers.with(read::ocr_languages)
    }

    /// The project's settings, with the defaults for what was never set (INV-WS-05).
    pub fn settings(&self) -> Result<ProjectSettings, StoreError> {
        self.readers.with(documents::settings)
    }

    /// A project-meta value, such as `name` or `app_version`.
    pub fn meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.readers.with(|c| documents::meta(c, key))
    }

    /// The rules version of the stored identifiers (`None` before M4b) and, during a re-run,
    /// the last content it committed.
    pub fn identifier_state(&self) -> Result<(Option<u32>, Option<i64>), StoreError> {
        self.readers.with(read::identifier_state)
    }

    /// Where a re-run step stands: the stamp of the stored mentions and, during a re-run, the
    /// last content it committed.
    pub fn rerun_state(
        &self,
        step: RerunStep,
    ) -> Result<(Option<String>, Option<i64>), StoreError> {
        self.readers.with(|c| read::rerun_state(c, step))
    }

    /// Each custom pattern as `(name, expression, stored matches)`, in the order added.
    pub fn pattern_counts(&self) -> Result<Vec<(String, String, u64)>, StoreError> {
        self.readers.with(read::pattern_counts)
    }

    /// The mapping screen of a file reference (US-05.1, US-05.2); `None` if it is gone or was
    /// not read as a table.
    pub fn mapping_view(&self, ref_id: i64) -> Result<Option<MappingView>, StoreError> {
        self.readers.with(|c| mapping::mapping_view(c, ref_id))
    }

    /// The entity rows of `keys` that exist.
    pub fn entities(
        &self,
        keys: &[String],
    ) -> Result<std::collections::BTreeMap<String, EntityRow>, StoreError> {
        self.readers.with(|c| read::entities(c, keys))
    }

    /// A content's record links (INV-CM-05).
    pub fn record_links(&self, content: i64) -> Result<Vec<RecordLinkRow>, StoreError> {
        self.readers.with(|c| read::record_links(c, content))
    }

    /// Contents with documents after `after`, in id order, at most `limit`.
    pub fn rerun_contents(&self, after: i64, limit: usize) -> Result<Vec<i64>, StoreError> {
        self.readers.with(|c| read::rerun_contents(c, after, limit))
    }

    /// A document's card-like flags in reading order (US-04.6 AC1).
    pub fn card_flags(&self, document: i64) -> Result<Vec<CardFlagRow>, StoreError> {
        self.readers.with(|c| read::card_flags(c, document))
    }

    /// Every card-like flag as `(content, document, last4)`.
    pub fn all_card_flags(&self) -> Result<Vec<(i64, i64, String)>, StoreError> {
        self.readers.with(read::all_card_flags)
    }

    /// Paths removed from `root` (US-01.5).
    pub fn exclusions(&self, root: i64) -> Result<std::collections::BTreeSet<String>, StoreError> {
        self.readers.with(|c| documents::exclusions(c, root))
    }

    /// One page of the documents list in path order (US-02.8).
    pub fn documents_page(
        &self,
        filter: &DocumentFilter,
        after: Option<&DocumentCursor>,
        limit: usize,
    ) -> Result<Vec<DocumentRow>, StoreError> {
        self.readers
            .with(|c| documents::documents_page(c, filter, after, limit))
    }

    pub fn documents_count(&self, filter: &DocumentFilter) -> Result<u64, StoreError> {
        self.readers.with(|c| documents::documents_count(c, filter))
    }

    /// Paths per file type.
    pub fn document_type_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<String, u64>, StoreError> {
        self.readers.with(documents::document_type_counts)
    }

    /// Connections that lose evidence if `refs` are removed (US-01.5 AC2).
    pub fn removal_impact(&self, refs: &[i64]) -> Result<u64, StoreError> {
        self.readers.with(|c| documents::removal_impact(c, refs))
    }

    /// The Stage A queue since `since` (UTC ms), for the progress panel (US-03.1).
    pub fn progress_detail(&self, since: Option<i64>) -> Result<ProgressDetail, StoreError> {
        self.readers.with(|c| documents::progress_detail(c, since))
    }

    /// Whether a run would start an item now (a Pending item in a Queued or Running job).
    pub fn has_runnable_work(&self) -> Result<bool, StoreError> {
        self.readers.with(documents::has_runnable_work)
    }

    pub fn stored_refs(&self) -> Result<Vec<StoredRef>, StoreError> {
        self.readers.with(read::stored_refs)
    }

    pub fn pending_items(&self) -> Result<Vec<PendingWork>, StoreError> {
        self.readers.with(read::pending_items)
    }

    /// Connections in the query's order (default: domain §9.2), keyset-paged after `after`.
    pub fn connections_page(
        &self,
        query: &read::ConnectionQuery,
        after: Option<&read::ListCursor>,
        limit: usize,
    ) -> Result<Vec<read::ListedConnection>, StoreError> {
        self.readers
            .with(|c| read::connections_page(c, query, after, limit))
    }

    /// Number of connection rows that match the query (the list length).
    pub fn connection_count(&self, query: &read::ConnectionQuery) -> Result<u64, StoreError> {
        self.readers.with(|c| read::connection_count(c, query))
    }

    /// Connection keys with fewer than two counted documents (NFR-QA-10), at most `limit`.
    pub fn weak_connections(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        self.readers.with(|c| read::weak_connections(c, limit))
    }

    /// Number of ignored connection rows.
    pub fn ignored_count(&self) -> Result<u64, StoreError> {
        self.readers.with(read::ignored_count)
    }

    /// The counted mentions of an identifier key or an entity name key, at most `limit`
    /// (US-07.3 AC2). Entity mentions have negative ids.
    pub fn mentions_of(&self, key: &str, limit: usize) -> Result<Vec<OccurrenceRow>, StoreError> {
        self.readers.with(|c| read::mentions_of(c, key, limit))
    }

    /// The counted mentions of `key` in one document, in reading order (US-08.2 AC1).
    pub fn mentions_in_document(&self, key: &str, document: i64) -> Result<Vec<i64>, StoreError> {
        self.readers
            .with(|c| read::mentions_in_document(c, key, document))
    }

    /// Failed contents with their reasons, at most `limit` (NFR-USE-05).
    pub fn failed_files(&self, limit: usize) -> Result<Vec<FailedFile>, StoreError> {
        self.readers.with(|c| read::failed_files(c, limit))
    }

    /// One mention by id.
    pub fn mention(&self, id: i64) -> Result<Option<OccurrenceRow>, StoreError> {
        self.readers.with(|c| read::mention(c, id))
    }

    /// A segment and its neighbours in its document (US-08.1).
    pub fn segment_window(
        &self,
        segment: i64,
        before: usize,
        after: usize,
    ) -> Result<Option<SegmentWindow>, StoreError> {
        self.readers
            .with(|c| read::segment_window(c, segment, before, after))
    }

    pub fn document_info(&self, document: i64) -> Result<Option<DocumentInfo>, StoreError> {
        self.readers.with(|c| read::document_info(c, document))
    }

    /// The first `limit` segments of a document, in order.
    pub fn document_segments(
        &self,
        document: i64,
        limit: usize,
    ) -> Result<Vec<WindowSegment>, StoreError> {
        self.readers
            .with(|c| read::document_segments(c, document, limit))
    }

    /// The text segment of one page of a PDF or image document (US-08.3).
    pub fn page_segment(
        &self,
        document: i64,
        page: u32,
    ) -> Result<Option<WindowSegment>, StoreError> {
        self.readers.with(|c| read::page_segment(c, document, page))
    }

    /// The last page of a PDF or image document with stored text, or 0.
    pub fn last_text_page(&self, document: i64) -> Result<u32, StoreError> {
        self.readers.with(|c| read::last_text_page(c, document))
    }

    /// A document's attachments: `(id, kind, position)` in order.
    pub fn child_documents(&self, document: i64) -> Result<Vec<(i64, String, String)>, StoreError> {
        self.readers.with(|c| read::child_documents(c, document))
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

    /// One page of keyword search results, best first (US-06.1). `fts` is an FTS5 query made
    /// by `mk-search` (`Query::to_fts5`); `highlight_query` gives the query that marks the hits
    /// in the page's texts (`Query::highlight_fts5`, or `fts` again).
    pub fn search_page(
        &self,
        fts: &str,
        highlight_query: impl FnOnce(&[&str]) -> String,
        filter: &SearchFilter,
        after: Option<&SearchCursor>,
        limit: usize,
    ) -> Result<Vec<SearchRow>, StoreError> {
        self.readers
            .with(|c| search::search_page(c, fts, highlight_query, filter, after, limit))
    }

    /// Rows in the keyword index; equals the segment count while it is in step.
    pub fn search_index_rows(&self) -> Result<u64, StoreError> {
        self.readers.with(search::index_rows)
    }

    /// The stored permission for an endpoint; `None` is Not allowed (INV-EG-01).
    pub fn endpoint_permission(&self, id: &str) -> Result<Option<PermissionRow>, StoreError> {
        self.readers.with(|c| egress::permission(c, id))
    }

    pub fn endpoint_permissions(&self) -> Result<Vec<PermissionRow>, StoreError> {
        self.readers.with(egress::permissions)
    }

    /// Every destination that received this project's data, the most recent first.
    pub fn egress_records(&self) -> Result<Vec<EgressRecordRow>, StoreError> {
        self.readers.with(egress::records)
    }

    /// Runs `f` on a read connection. For tests.
    pub fn read_raw<R>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        self.readers.with(f)
    }
}
