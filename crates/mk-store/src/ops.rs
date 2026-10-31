//! Write operations. Each runs inside one writer transaction (ADR-0004): its rows, the state
//! changes and the derived rebuild of the keys it touched commit together (ADR-0005, INV-PR-04).

use std::collections::BTreeSet;
use std::path::PathBuf;

use mk_corpus::{IngestionState, PathChange};
use mk_graph::DecisionKind;
use mk_jobs::{ItemState, JobState, Stage, Trigger};
use mk_kernel::{ContentHash, IdentifierKey, IdentifierKind, Locator, OcrGeometry};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::codec;
use crate::derive::rebuild_keys;
use crate::time::{now_iso, now_ms};

/// One re-scan outcome with the file type of its content (from the path's extension).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RescanChange {
    pub change: PathChange,
    pub file_type: String,
}

/// What a re-scan commit did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RescanOutcome {
    /// Stage A job created for new work, if any.
    pub job: Option<i64>,
    pub new_items: usize,
    pub new_contents: usize,
    pub missing_contents: usize,
    pub found_contents: usize,
    pub replaced_contents: usize,
    /// New paths (US-01.3 AC5 "N added").
    pub added: usize,
    /// New paths whose content was already stored or came earlier in this re-scan: read once
    /// (US-01.3 AC4/AC5 "N duplicates").
    pub duplicates: usize,
    /// A connection row was added, removed or changed.
    pub connections_changed: bool,
}

/// A new content without a file reference, for benchmarks and tests that fill a project
/// through the store (spike S2c).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewContent {
    pub hash: ContentHash,
    pub file_type: String,
    pub size: u64,
}

/// Stage A output for one content: what the parser and the identifier step produced.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct StageAOutput {
    pub documents: Vec<DocumentOut>,
    pub mentions: Vec<MentionOut>,
    pub card_flags: Vec<CardFlagOut>,
    pub warnings: Vec<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct DocumentOut {
    pub kind: String,
    pub position: String,
    /// Index of the document this one is an attachment of; it comes earlier (INV-CO-05).
    pub parent: Option<u32>,
    pub segments: Vec<SegmentOut>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct SegmentOut {
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
    /// OCR'd pages: the word geometry, built from exactly `text` (US-02.2 AC3).
    pub ocr: Option<OcrGeometry>,
}

/// An identifier mention at `locator` (line + span, or row + column + span) of a segment.
#[derive(Clone, PartialEq, Debug)]
pub struct MentionOut {
    /// Index into [`StageAOutput::documents`].
    pub document: usize,
    /// Index into that document's segments.
    pub segment: usize,
    pub locator: Locator,
    pub raw: String,
    pub key: IdentifierKey,
    pub confidence: f32,
}

/// A card-like number at `locator` of a segment (US-04.6): a flag on its document with the last
/// four digits, never an identifier (INV-ID-06).
#[derive(Clone, PartialEq, Debug)]
pub struct CardFlagOut {
    /// Index into [`StageAOutput::documents`].
    pub document: usize,
    /// Index into that document's segments.
    pub segment: usize,
    pub locator: Locator,
    pub last4: String,
}

/// One piece of a staged Stage A result, in the order the worker sent it (protocol v4): a
/// document, then batches of its segments, each followed by the mentions and card-like flags
/// in that batch.
#[derive(Clone, PartialEq, Debug)]
pub enum StagedPart {
    /// Documents are numbered 0, 1, 2, … in order; `parent` is an earlier one.
    Document {
        kind: String,
        position: String,
        parent: Option<u32>,
    },
    Segments(Vec<SegmentOut>),
    /// Mentions in the `Segments` batch just before. `document` is the current document's
    /// number and `segment` the index in it.
    Mentions(Vec<MentionOut>),
    /// Card-like flags in the `Segments` batch just before, numbered as mentions are.
    CardFlags(Vec<CardFlagOut>),
}

impl StageAOutput {
    /// The output as staged parts: each document, all its segments, then its mentions and
    /// card-like flags.
    pub fn into_parts(self) -> impl Iterator<Item = StagedPart> {
        let mut flags: Vec<Vec<CardFlagOut>> = vec![Vec::new(); self.documents.len()];
        for f in self.card_flags {
            // Points at no document: kept so the commit rejects it.
            let i = f.document.min(flags.len().saturating_sub(1));
            if let Some(v) = flags.get_mut(i) {
                v.push(f);
            }
        }
        let mut by_doc: Vec<Vec<MentionOut>> = vec![Vec::new(); self.documents.len()];
        for m in self.mentions {
            if let Some(v) = by_doc.get_mut(m.document) {
                v.push(m);
            } else {
                // Points at no document: kept so the commit rejects it.
                if let Some(v) = by_doc.last_mut() {
                    v.push(m);
                }
            }
        }
        self.documents
            .into_iter()
            .zip(by_doc)
            .zip(flags)
            .flat_map(|((doc, mentions), flags)| {
                let mut parts = vec![StagedPart::Document {
                    kind: doc.kind,
                    position: doc.position,
                    parent: doc.parent,
                }];
                if !doc.segments.is_empty() {
                    parts.push(StagedPart::Segments(doc.segments));
                }
                if !mentions.is_empty() {
                    parts.push(StagedPart::Mentions(mentions));
                }
                if !flags.is_empty() {
                    parts.push(StagedPart::CardFlags(flags));
                }
                parts
            })
    }
}

/// What a Stage A commit did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CommitOutcome {
    pub segments: usize,
    pub mentions: usize,
    pub card_flags: usize,
    pub touched_keys: usize,
    pub connections_changed: bool,
}

/// A work item the Stage A lane has started: what to read and how to parse it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartedItem {
    pub item: i64,
    pub content: i64,
    pub hash: ContentHash,
    pub file_type: String,
    /// A present path of the content: source root path joined with the relative path. None
    /// when no path is present (a content without file references).
    pub path: Option<PathBuf>,
}

/// Items and contents put back by [`recover_after_restart`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Recovery {
    pub items: usize,
    pub contents: usize,
}

pub(crate) fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::Invalid(msg.into())
}

pub(crate) fn content_state(conn: &Connection, id: i64) -> Result<IngestionState, StoreError> {
    let s: String = conn
        .prepare_cached("SELECT state FROM content WHERE id = ?1")?
        .query_row([id], |r| r.get(0))?;
    IngestionState::from_name(&s).ok_or_else(|| invalid(format!("unknown content state {s}")))
}

pub(crate) fn set_content_state(
    conn: &Connection,
    id: i64,
    to: IngestionState,
) -> Result<IngestionState, StoreError> {
    let from = content_state(conn, id)?;
    from.move_to(to).map_err(|e| invalid(e.to_string()))?;
    conn.prepare_cached("UPDATE content SET state = ?2 WHERE id = ?1")?
        .execute(params![id, to.as_str()])?;
    Ok(from)
}

pub(crate) fn item_state(
    conn: &Connection,
    id: i64,
) -> Result<(ItemState, i64, Option<i64>), StoreError> {
    let (s, job, content): (String, i64, Option<i64>) = conn
        .prepare_cached("SELECT state, job_id, content_id FROM work_item WHERE id = ?1")?
        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let s = ItemState::from_name(&s).ok_or_else(|| invalid(format!("unknown item state {s}")))?;
    Ok((s, job, content))
}

pub(crate) fn set_item_state(
    conn: &Connection,
    id: i64,
    to: ItemState,
) -> Result<(i64, Option<i64>), StoreError> {
    let (from, job, content) = item_state(conn, id)?;
    from.move_to(to).map_err(|e| invalid(e.to_string()))?;
    conn.prepare_cached("UPDATE work_item SET state = ?2 WHERE id = ?1")?
        .execute(params![id, to.as_str()])?;
    Ok((job, content))
}

/// The identifier keys and entity name keys of a content's stored mentions.
pub(crate) fn content_keys(
    conn: &Connection,
    content: i64,
    keys: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT m.identifier_key
         FROM document d
         JOIN segment s ON s.document_id = d.id
         JOIN identifier_mention m ON m.segment_id = s.id
         WHERE d.content_id = ?1",
    )?;
    for k in stmt.query_map([content], |r| r.get::<_, String>(0))? {
        keys.insert(k?);
    }
    crate::table::content_entity_keys(conn, content, keys)
}

/// Marks a job Completed once none of its items is Pending or In progress (domain §7.2).
pub(crate) fn complete_job_if_done(conn: &Connection, job: i64) -> Result<(), StoreError> {
    let open: i64 = conn
        .prepare_cached("SELECT COUNT(*) FROM work_item WHERE job_id = ?1 AND state IN (?2, ?3)")?
        .query_row(
            params![
                job,
                ItemState::Pending.as_str(),
                ItemState::InProgress.as_str()
            ],
            |r| r.get(0),
        )?;
    if open > 0 {
        return Ok(());
    }
    let s: String = conn
        .prepare_cached("SELECT state FROM job WHERE id = ?1")?
        .query_row([job], |r| r.get(0))?;
    let state = JobState::from_name(&s).ok_or_else(|| invalid(format!("unknown job state {s}")))?;
    let to = match state {
        JobState::Running => JobState::Completed,
        // A job whose items were all skipped before it started (everything went missing).
        JobState::Queued => JobState::Cancelled,
        _ => return Ok(()),
    };
    conn.prepare_cached("UPDATE job SET state = ?2 WHERE id = ?1")?
        .execute(params![job, to.as_str()])?;
    Ok(())
}

/// A new Stage A job: Queued, or Paused while the project's queue is paused (INV-PR-06).
pub(crate) fn create_job(conn: &Connection, trigger: Trigger) -> Result<i64, StoreError> {
    conn.prepare_cached(
        "INSERT INTO job (trigger, stage, state, created_at) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        trigger.as_str(),
        Stage::A.as_str(),
        JobState::Queued.as_str(),
        now_iso()
    ])?;
    let job = conn.last_insert_rowid();
    if crate::documents::queue_paused(conn)? {
        JobState::Queued
            .move_to(JobState::Paused)
            .map_err(|e| invalid(e.to_string()))?;
        conn.prepare_cached("UPDATE job SET state = ?2, paused_at = ?3 WHERE id = ?1")?
            .execute(params![job, JobState::Paused.as_str(), now_ms()])?;
    }
    Ok(job)
}

pub(crate) fn add_item(
    conn: &Connection,
    job: i64,
    content: i64,
    size: u64,
) -> Result<i64, StoreError> {
    // M2b parses only text-native formats.
    let priority = mk_jobs::stage_a_priority(true, size);
    conn.prepare_cached(
        "INSERT INTO work_item (job_id, content_id, priority, state) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![job, content, priority, ItemState::Pending.as_str()])?;
    Ok(conn.last_insert_rowid())
}

/// Pending items of a content become Skipped: its target went missing or away before it
/// started (domain §7.2).
pub(crate) fn skip_pending(conn: &Connection, content: i64) -> Result<(), StoreError> {
    let jobs: Vec<i64> = {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT job_id FROM work_item WHERE content_id = ?1 AND state = ?2",
        )?;
        stmt.query_map(params![content, ItemState::Pending.as_str()], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    conn.prepare_cached("UPDATE work_item SET state = ?3 WHERE content_id = ?1 AND state = ?2")?
        .execute(params![
            content,
            ItemState::Pending.as_str(),
            ItemState::Skipped.as_str()
        ])?;
    for job in jobs {
        complete_job_if_done(conn, job)?;
    }
    Ok(())
}

fn content_id(conn: &Connection, hash: &ContentHash) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT id FROM content WHERE hash = ?1")?
        .query_row([&hash.as_bytes()[..]], |r| r.get(0))
        .optional()?)
}

/// The content for `hash`, created Queued if new. Returns `(id, created)`.
fn ensure_content(
    conn: &Connection,
    hash: &ContentHash,
    file_type: &str,
    size: u64,
) -> Result<(i64, bool), StoreError> {
    if let Some(id) = content_id(conn, hash)? {
        return Ok((id, false));
    }
    conn.prepare_cached(
        "INSERT INTO content (hash, file_type, size, state) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        &hash.as_bytes()[..],
        file_type,
        size as i64,
        IngestionState::Queued.as_str()
    ])?;
    Ok((conn.last_insert_rowid(), true))
}

/// Adds a source root, or returns the id of the one with the same path.
pub(crate) fn set_ocr_languages(conn: &Connection, languages: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO project_meta (key, value) VALUES ('ocr_languages', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [languages],
    )?;
    Ok(())
}

pub(crate) fn add_source_root(
    conn: &Connection,
    path: &str,
    kind: &str,
) -> Result<i64, StoreError> {
    if let Some(id) = conn
        .query_row("SELECT id FROM source_root WHERE path = ?1", [path], |r| {
            r.get(0)
        })
        .optional()?
    {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO source_root (path, kind, added_at) VALUES (?1, ?2, ?3)",
        params![path, kind, now_iso()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Applies a re-scan (US-01.3, US-01.4, architecture §5.4) in one transaction:
/// - file references are added, moved to their new content, marked missing or present;
/// - a content with no file reference left is Replaced: its pending work is skipped and its
///   documents, segments and mentions are deleted (INV-CO-06, ASSUMPTION C6);
/// - a content whose references are all missing becomes Missing and stops counting
///   (INV-CO-07); found again, it returns to Done, or to Queued with new work;
/// - new contents get a Stage A work item in one new job (INV-CO-02, INV-PR-08);
/// - the derived rows of every key whose evidence changed are rebuilt (ADR-0005).
pub(crate) fn apply_rescan(
    conn: &Connection,
    changes: &[RescanChange],
    trigger: Trigger,
) -> Result<RescanOutcome, StoreError> {
    let now = now_iso();
    let mut out = RescanOutcome::default();
    let mut affected: BTreeSet<i64> = BTreeSet::new();
    let mut created: BTreeSet<i64> = BTreeSet::new();
    // Stored contents that gained or lost a path: the confirm decisions on their paths changed.
    let mut paths_changed: BTreeSet<i64> = BTreeSet::new();
    let lookup = |hash: &ContentHash| -> Result<i64, StoreError> {
        content_id(conn, hash)?.ok_or_else(|| invalid(format!("no content {hash}")))
    };
    for RescanChange { change, file_type } in changes {
        match change {
            PathChange::New {
                root,
                rel_path,
                meta,
                hash,
            } => {
                let (id, new) = ensure_content(conn, hash, file_type, meta.size)?;
                if new {
                    created.insert(id);
                } else {
                    out.duplicates += 1;
                    paths_changed.insert(id);
                }
                out.added += 1;
                conn.prepare_cached(
                    "INSERT INTO file_ref (root_id, rel_path, content_id, size, mtime_ns, present,
                                           last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                )?
                .execute(params![
                    root,
                    rel_path,
                    id,
                    meta.size as i64,
                    meta.mtime_ns,
                    now
                ])?;
                affected.insert(id);
            }
            PathChange::Changed {
                root,
                rel_path,
                meta,
                old,
                new,
            } => {
                let old_id = lookup(old)?;
                let (id, is_new) = ensure_content(conn, new, file_type, meta.size)?;
                if is_new {
                    created.insert(id);
                } else {
                    paths_changed.insert(id);
                }
                paths_changed.insert(old_id);
                conn.prepare_cached(
                    "UPDATE file_ref SET content_id = ?3, size = ?4, mtime_ns = ?5, present = 1,
                                         last_seen_at = ?6
                     WHERE root_id = ?1 AND rel_path = ?2",
                )?
                .execute(params![
                    root,
                    rel_path,
                    id,
                    meta.size as i64,
                    meta.mtime_ns,
                    now
                ])?;
                affected.extend([old_id, id]);
            }
            PathChange::Touched {
                root,
                rel_path,
                meta,
                ..
            } => {
                conn.prepare_cached(
                    "UPDATE file_ref SET size = ?3, mtime_ns = ?4, last_seen_at = ?5
                     WHERE root_id = ?1 AND rel_path = ?2",
                )?
                .execute(params![
                    root,
                    rel_path,
                    meta.size as i64,
                    meta.mtime_ns,
                    now
                ])?;
            }
            PathChange::Missing {
                root,
                rel_path,
                hash,
            } => {
                conn.prepare_cached(
                    "UPDATE file_ref SET present = 0 WHERE root_id = ?1 AND rel_path = ?2",
                )?
                .execute(params![root, rel_path])?;
                affected.insert(lookup(hash)?);
            }
            PathChange::FoundAgain {
                root,
                rel_path,
                meta,
                hash,
            } => {
                conn.prepare_cached(
                    "UPDATE file_ref SET present = 1, size = ?3, mtime_ns = ?4, last_seen_at = ?5
                     WHERE root_id = ?1 AND rel_path = ?2",
                )?
                .execute(params![
                    root,
                    rel_path,
                    meta.size as i64,
                    meta.mtime_ns,
                    now
                ])?;
                affected.insert(lookup(hash)?);
            }
        }
    }

    let mut touched: BTreeSet<String> = BTreeSet::new();
    let mut needs_work: Vec<(i64, u64)> = Vec::new();
    for &id in &affected {
        let (refs, present, size): (i64, i64, i64) = conn
            .prepare_cached(
                "SELECT (SELECT COUNT(*) FROM file_ref WHERE content_id = ?1),
                        (SELECT COUNT(*) FROM file_ref WHERE content_id = ?1 AND present = 1),
                        (SELECT size FROM content WHERE id = ?1)",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let state = content_state(conn, id)?;
        if refs == 0 {
            state
                .move_to(IngestionState::Replaced)
                .map_err(|e| invalid(e.to_string()))?;
            skip_pending(conn, id)?;
            content_keys(conn, id, &mut touched)?;
            conn.prepare_cached("DELETE FROM document WHERE content_id = ?1")?
                .execute([id])?;
            conn.prepare_cached("DELETE FROM content WHERE id = ?1")?
                .execute([id])?;
            out.replaced_contents += 1;
        } else if present == 0 && state != IngestionState::Missing {
            set_content_state(conn, id, IngestionState::Missing)?;
            conn.prepare_cached("UPDATE content SET state_before_missing = ?2 WHERE id = ?1")?
                .execute(params![id, state.as_str()])?;
            skip_pending(conn, id)?;
            content_keys(conn, id, &mut touched)?;
            out.missing_contents += 1;
        } else if present > 0 && state == IngestionState::Missing {
            let before: Option<String> = conn
                .prepare_cached("SELECT state_before_missing FROM content WHERE id = ?1")?
                .query_row([id], |r| r.get(0))?;
            let before = before
                .as_deref()
                .and_then(IngestionState::from_name)
                .unwrap_or(IngestionState::Queued);
            let to = IngestionState::found_again(before);
            set_content_state(conn, id, to)?;
            conn.prepare_cached("UPDATE content SET state_before_missing = NULL WHERE id = ?1")?
                .execute([id])?;
            if to == IngestionState::Queued {
                needs_work.push((id, size as u64));
            }
            content_keys(conn, id, &mut touched)?;
            out.found_contents += 1;
        } else if created.contains(&id) {
            needs_work.push((id, size as u64));
            out.new_contents += 1;
        }
    }
    if !needs_work.is_empty() {
        let job = create_job(conn, trigger)?;
        for (id, size) in &needs_work {
            add_item(conn, job, *id, *size)?;
        }
        out.job = Some(job);
        out.new_items = needs_work.len();
    }
    rerun_table_steps(conn, &paths_changed, &mut touched)?;
    out.connections_changed = rebuild_keys(conn, &touched)?;
    Ok(out)
}

/// The table step again for each of `contents` still stored (see [`crate::table`]).
pub(crate) fn rerun_table_steps(
    conn: &Connection,
    contents: &BTreeSet<i64>,
    touched: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    for &id in contents {
        let stored: Option<i64> = conn
            .prepare_cached("SELECT id FROM content WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        if stored.is_some() {
            crate::table::apply_table_step(conn, id, touched)?;
        }
    }
    Ok(())
}

/// Adds contents without file references, each with a Stage A item in one new job. For
/// benchmarks and tests (spike S2c); the app adds contents through [`apply_rescan`].
pub(crate) fn enqueue_contents(
    conn: &Connection,
    contents: &[NewContent],
) -> Result<Vec<i64>, StoreError> {
    let job = create_job(conn, Trigger::Add)?;
    let mut items = Vec::with_capacity(contents.len());
    for c in contents {
        let (id, created) = ensure_content(conn, &c.hash, &c.file_type, c.size)?;
        if !created {
            return Err(invalid(format!("content {} exists", c.hash)));
        }
        items.push(add_item(conn, job, id, c.size)?);
    }
    Ok(items)
}

/// Starts a Pending item: item In progress, content Processing, job Running. `None` if the item
/// is no longer Pending.
pub(crate) fn start_item(conn: &Connection, item: i64) -> Result<Option<StartedItem>, StoreError> {
    let (state, job, content) = item_state(conn, item)?;
    if state != ItemState::Pending {
        return Ok(None);
    }
    let content = content.ok_or_else(|| invalid(format!("item {item} has no content")))?;
    set_item_state(conn, item, ItemState::InProgress)?;
    conn.prepare_cached(
        "UPDATE work_item SET attempts = attempts + 1, started_at = ?2, finished_at = NULL
         WHERE id = ?1",
    )?
    .execute(params![item, now_ms()])?;
    set_content_state(conn, content, IngestionState::Processing)?;
    let job_state: String = conn
        .prepare_cached("SELECT state FROM job WHERE id = ?1")?
        .query_row([job], |r| r.get(0))?;
    if job_state == JobState::Queued.as_str() {
        JobState::Queued
            .move_to(JobState::Running)
            .map_err(|e| invalid(e.to_string()))?;
        conn.prepare_cached("UPDATE job SET state = ?2 WHERE id = ?1")?
            .execute(params![job, JobState::Running.as_str()])?;
    }
    let (hash, file_type): (Vec<u8>, String) = conn
        .prepare_cached("SELECT hash, file_type FROM content WHERE id = ?1")?
        .query_row([content], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let path: Option<(String, String)> = conn
        .prepare_cached(
            "SELECT r.path, f.rel_path FROM file_ref f JOIN source_root r ON r.id = f.root_id
             WHERE f.content_id = ?1 AND f.present = 1 ORDER BY f.id LIMIT 1",
        )?
        .query_row([content], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(Some(StartedItem {
        item,
        content,
        hash: codec::hash_from(&hash)?,
        file_type,
        path: path.map(|(root, rel)| mk_corpus::fs::resolve(root.as_ref(), &rel)),
    }))
}

const INSERT_MENTION: &str = "INSERT INTO identifier_mention (segment_id, document_id, content_id,
         identifier_key, span_start, span_end, column_index, column_name, raw, confidence,
         found_by)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";
const INSERT_FLAG: &str = "INSERT INTO sensitive_flag (segment_id, document_id, content_id,
         span_start, span_end, column_index, column_name, last4)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

/// The checked place of a mention or flag in its segment: span and CSV column.
struct Place {
    start: i64,
    end: i64,
    column_index: Option<i64>,
    column_name: Option<String>,
}

impl Place {
    /// Checks that `locator`'s span lies in a segment of `len` characters (INV-CO-04) and finds
    /// its cell among `cells`.
    fn check(locator: &Locator, len: usize, cells: &[u32]) -> Result<Place, StoreError> {
        let span = locator
            .span()
            .ok_or_else(|| invalid("mention locator has no span"))?;
        if span.end() > len {
            return Err(invalid("mention span is past its segment text"));
        }
        let cell = (!cells.is_empty()).then(|| {
            cells
                .partition_point(|s| *s as usize <= span.start())
                .saturating_sub(1)
        });
        let (column_index, column_name) = codec::column(locator, cell);
        Ok(Place {
            start: span.start() as i64,
            end: span.end() as i64,
            column_index,
            column_name,
        })
    }
}

/// Which step found a mention of `key`: `custom` for a custom pattern's type, else `builtin`.
fn found_by(key: &IdentifierKey) -> RerunStep {
    if key.kind() == IdentifierKind::Custom {
        RerunStep::Custom
    } else {
        RerunStep::Builtin
    }
}

/// Inserts one mention; returns its key as stored.
#[allow(clippy::too_many_arguments)]
fn insert_mention(
    stmt: &mut rusqlite::CachedStatement<'_>,
    segment: i64,
    document: i64,
    content: i64,
    place: &Place,
    raw: &str,
    key: &IdentifierKey,
    confidence: f32,
) -> Result<String, StoreError> {
    if !(0.0..=1.0).contains(&confidence) {
        return Err(invalid("confidence outside 0..1"));
    }
    let by = found_by(key).as_str();
    let key = key.to_string();
    stmt.execute(params![
        segment,
        document,
        content,
        key,
        place.start,
        place.end,
        place.column_index,
        place.column_name,
        raw,
        f64::from(confidence),
        by
    ])?;
    Ok(key)
}

fn insert_flag(
    stmt: &mut rusqlite::CachedStatement<'_>,
    segment: i64,
    document: i64,
    content: i64,
    place: &Place,
    last4: &str,
) -> Result<(), StoreError> {
    if last4.len() != 4 || !last4.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(
            "a card-like flag keeps exactly its last four digits",
        ));
    }
    stmt.execute(params![
        segment,
        document,
        content,
        place.start,
        place.end,
        place.column_index,
        place.column_name,
        last4
    ])?;
    Ok(())
}

/// Commits a Stage A result (INV-PR-04, INV-CO-06, ADR-0005): the content's old documents,
/// segments and mentions are replaced by the new ones, the touched keys are rebuilt, and the
/// item and content become Done, all in this one transaction.
///
/// The parts are read in order and written as they come; only the current segment batch is
/// held, so a streamed file of any size commits in O(batch) memory (§4 #16, ADR-0016).
pub(crate) fn commit_stage_a(
    conn: &Connection,
    item: i64,
    parts: impl Iterator<Item = Result<StagedPart, StoreError>>,
    warnings: &[String],
) -> Result<CommitOutcome, StoreError> {
    let (job, content) = set_item_state(conn, item, ItemState::Done)?;
    let content = content.ok_or_else(|| invalid(format!("item {item} has no content")))?;
    set_content_state(conn, content, IngestionState::Done)?;

    let mut touched = BTreeSet::new();
    content_keys(conn, content, &mut touched)?;
    conn.prepare_cached("DELETE FROM document WHERE content_id = ?1")?
        .execute([content])?;

    let mut out = CommitOutcome::default();
    let mut doc_ids: Vec<i64> = Vec::new();
    // The current document's segment count, and the last batch: its first index and, per
    // segment, (id, character count, cell starts).
    let mut ordinal = 0usize;
    let mut batch_start = 0usize;
    let mut batch: Vec<(i64, usize, Vec<u32>)> = Vec::new();
    let mut insert_doc = conn.prepare_cached(
        "INSERT INTO document (content_id, parent_id, position, kind) VALUES (?1, ?2, ?3, ?4)",
    )?;
    let mut insert_seg = conn.prepare_cached(
        "INSERT INTO segment (document_id, ordinal, loc_kind, loc_pos, loc_ref, text, malformed,
                              cell_starts, ocr)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    let mut insert_mention_stmt = conn.prepare_cached(INSERT_MENTION)?;
    let mut insert_flag_stmt = conn.prepare_cached(INSERT_FLAG)?;
    for part in parts {
        match part? {
            StagedPart::Document {
                kind,
                position,
                parent,
            } => {
                let parent_id = match parent {
                    Some(p) => Some(
                        *doc_ids
                            .get(p as usize)
                            .ok_or_else(|| invalid("document parent is not an earlier one"))?,
                    ),
                    None => None,
                };
                insert_doc.execute(params![content, parent_id, position, kind])?;
                doc_ids.push(conn.last_insert_rowid());
                ordinal = 0;
                batch_start = 0;
                batch.clear();
            }
            StagedPart::Segments(segments) => {
                let doc_id = *doc_ids
                    .last()
                    .ok_or_else(|| invalid("segments before any document"))?;
                batch_start = ordinal;
                batch.clear();
                for seg in segments {
                    let loc = codec::segment_locator(&seg.locator)?;
                    // An OCR segment has the geometry of exactly its text (INV-CO-04); others
                    // have none.
                    let ocr = match (&seg.locator, &seg.ocr) {
                        (Locator::OcrText { .. }, Some(g))
                            if g.text_hash() == ContentHash::of(seg.text.as_bytes()) =>
                        {
                            Some(codec::ocr_to_blob(g))
                        }
                        (Locator::OcrText { .. }, _) => {
                            return Err(invalid("OCR segment without the geometry of its text"));
                        }
                        (_, None) => None,
                        (_, Some(_)) => return Err(invalid("OCR geometry on a non-OCR segment")),
                    };
                    insert_seg.execute(params![
                        doc_id,
                        ordinal as i64,
                        loc.kind,
                        loc.pos,
                        loc.loc_ref,
                        seg.text,
                        seg.malformed,
                        codec::cells_to_blob(&seg.cell_starts),
                        ocr
                    ])?;
                    batch.push((
                        conn.last_insert_rowid(),
                        seg.text.chars().count(),
                        seg.cell_starts,
                    ));
                    ordinal += 1;
                }
                out.segments += batch.len();
            }
            StagedPart::Mentions(mentions) => {
                for m in mentions {
                    if m.document + 1 != doc_ids.len() {
                        return Err(invalid("mention points to no segment"));
                    }
                    let seg = m
                        .segment
                        .checked_sub(batch_start)
                        .and_then(|i| batch.get(i))
                        .ok_or_else(|| invalid("mention points to no segment"))?;
                    let place = Place::check(&m.locator, seg.1, &seg.2)?;
                    let key = insert_mention(
                        &mut insert_mention_stmt,
                        seg.0,
                        doc_ids[m.document],
                        content,
                        &place,
                        &m.raw,
                        &m.key,
                        m.confidence,
                    )?;
                    touched.insert(key);
                    out.mentions += 1;
                }
            }
            StagedPart::CardFlags(flags) => {
                for f in flags {
                    if f.document + 1 != doc_ids.len() {
                        return Err(invalid("card-like flag points to no segment"));
                    }
                    let seg = f
                        .segment
                        .checked_sub(batch_start)
                        .and_then(|i| batch.get(i))
                        .ok_or_else(|| invalid("card-like flag points to no segment"))?;
                    let place = Place::check(&f.locator, seg.1, &seg.2)?;
                    insert_flag(
                        &mut insert_flag_stmt,
                        seg.0,
                        doc_ids[f.document],
                        content,
                        &place,
                        &f.last4,
                    )?;
                    out.card_flags += 1;
                }
            }
        }
    }
    // The keyword index (ADR-0010), in this transaction: one statement for the content's
    // segments, not a trigger per row (S4: 1M rows in 2.8 s instead of 13.2 s).
    conn.prepare_cached(
        "INSERT INTO segment_fts (rowid, text)
         SELECT s.id, s.text FROM document d JOIN segment s ON s.document_id = d.id
         WHERE d.content_id = ?1",
    )?
    .execute([content])?;
    // Suggestions, and a confirmed mapping applied before the results show (INV-CM-04).
    crate::table::apply_table_step(conn, content, &mut touched)?;
    conn.prepare_cached(
        "UPDATE content SET warnings = ?2, failure_code = NULL, failure_reason = NULL,
                            failure_detail = NULL
         WHERE id = ?1",
    )?
    .execute(params![content, warnings.join("\n")])?;
    conn.prepare_cached("UPDATE work_item SET finished_at = ?2 WHERE id = ?1")?
        .execute(params![item, now_ms()])?;
    out.touched_keys = touched.len();
    out.connections_changed = rebuild_keys(conn, &touched)?;
    complete_job_if_done(conn, job)?;
    Ok(out)
}

/// Fails an item and its content with a stable code, a plain reason and the technical detail
/// behind "Show details" (INV-CO-09, US-02.7 AC2). No other content changes.
pub(crate) fn fail_item(
    conn: &Connection,
    item: i64,
    code: &str,
    reason: &str,
    detail: &str,
) -> Result<(), StoreError> {
    let (job, content) = set_item_state(conn, item, ItemState::Failed)?;
    conn.prepare_cached(
        "UPDATE work_item SET failure_code = ?2, failure_reason = ?3, finished_at = ?4
         WHERE id = ?1",
    )?
    .execute(params![item, code, reason, now_ms()])?;
    if let Some(content) = content {
        set_content_state(conn, content, IngestionState::Failed)?;
        conn.prepare_cached(
            "UPDATE content SET failure_code = ?2, failure_reason = ?3, failure_detail = ?4
             WHERE id = ?1",
        )?
        .execute(params![content, code, reason, detail])?;
    }
    complete_job_if_done(conn, job)?;
    Ok(())
}

/// Puts an item stopped by pause, cancel or close back to Pending, and its content back to
/// Queued: nothing of it was committed (INV-PR-04, domain §7.2 "forced stop on pause").
pub(crate) fn release_item(conn: &Connection, item: i64) -> Result<(), StoreError> {
    let (_, content) = set_item_state(conn, item, ItemState::Pending)?;
    conn.prepare_cached("UPDATE work_item SET started_at = NULL WHERE id = ?1")?
        .execute([item])?;
    if let Some(content) = content {
        set_content_state(conn, content, IngestionState::Queued)?;
    }
    Ok(())
}

/// Appends a decision to the log (INV-CU-01) and rebuilds its anchor (ADR-0005). Returns the
/// decision's `seq` and whether a connection row changed.
pub(crate) fn append_decision(
    conn: &Connection,
    kind: DecisionKind,
    anchor: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<(i64, bool), StoreError> {
    conn.prepare_cached(
        "INSERT INTO decision (kind, anchor, before, after, decided_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![kind.as_str(), anchor, before, after, now_iso()])?;
    let seq = conn.last_insert_rowid();
    let changed =
        kind.on_connection() && rebuild_keys(conn, &BTreeSet::from([anchor.to_string()]))?;
    Ok((seq, changed))
}

/// After a crash or quit (ADR-0006, INV-PR-05/07): items In progress go back to Pending and
/// their contents from Processing to Queued. Done items and job states are kept.
pub(crate) fn recover_after_restart(conn: &Connection) -> Result<Recovery, StoreError> {
    let items = conn.execute(
        "UPDATE work_item SET state = ?2 WHERE state = ?1",
        params![
            ItemState::InProgress.as_str(),
            ItemState::InProgress.after_restart().as_str()
        ],
    )?;
    let contents = conn.execute(
        "UPDATE content SET state = ?2 WHERE state = ?1",
        params![
            IngestionState::Processing.as_str(),
            IngestionState::Processing.after_restart().as_str()
        ],
    )?;
    Ok(Recovery { items, contents })
}

/// A stored segment as an identifier re-run sees it (M4b): enough to find identifiers in it
/// and build their locators, as the worker did.
#[derive(Clone, PartialEq, Debug)]
pub struct StoredSegment {
    pub document_id: i64,
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
    pub ocr: Option<OcrGeometry>,
}

/// A mention found again in a stored segment.
#[derive(Clone, PartialEq, Debug)]
pub struct FoundMention {
    pub locator: Locator,
    pub raw: String,
    pub key: IdentifierKey,
    pub confidence: f32,
}

/// A card-like number found again in a stored segment.
#[derive(Clone, PartialEq, Debug)]
pub struct FoundFlag {
    pub locator: Locator,
    pub last4: String,
}

/// What a re-run found in one segment.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SegmentFinds {
    pub mentions: Vec<FoundMention>,
    pub card_flags: Vec<FoundFlag>,
}

/// What re-running one content's identifiers did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RerunOutcome {
    pub mentions: usize,
    pub card_flags: usize,
    pub connections_changed: bool,
}

/// The identifier steps that re-run from stored segments, each alone (INV-ID-08): the built-in
/// finders and card-like flags (M4b, on a region change or newer rules), or the custom patterns
/// (M4c, when the pattern set changes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RerunStep {
    Builtin,
    Custom,
}

impl RerunStep {
    /// The `found_by` of the mentions this step makes.
    pub fn as_str(&self) -> &'static str {
        match self {
            RerunStep::Builtin => "builtin",
            RerunStep::Custom => "custom",
        }
    }

    /// The `project_meta` key of the step's unfinished re-run: the last content it committed.
    pub(crate) fn cursor_key(&self) -> &'static str {
        match self {
            RerunStep::Builtin => RERUN_AFTER,
            RerunStep::Custom => CUSTOM_RERUN_AFTER,
        }
    }

    /// The `project_meta` key of what the stored mentions were made with: the rules version, or
    /// a hash of the pattern set.
    pub(crate) fn stamp_key(&self) -> &'static str {
        match self {
            RerunStep::Builtin => IDENTIFIER_VERSION,
            RerunStep::Custom => CUSTOM_PATTERNS_APPLIED,
        }
    }
}

/// The `project_meta` key of an unfinished identifier re-run: the last content it committed.
pub(crate) const RERUN_AFTER: &str = "identifier_rerun_after";
pub(crate) const IDENTIFIER_VERSION: &str = "identifier_version";
pub(crate) const CUSTOM_RERUN_AFTER: &str = "custom_rerun_after";
pub(crate) const CUSTOM_PATTERNS_APPLIED: &str = "custom_patterns_applied";

/// Finds a content's identifiers of one step again from its stored segments: its mentions of
/// that step (and, for the built-in step, its card-like flags) are replaced by what `find` gives
/// for each segment, in document and segment order; the table step runs again so record links
/// follow (M4c); the keys of the old and new mentions are rebuilt (ADR-0005), all in this one
/// transaction. The re-run's cursor moves past the content in it too, so a kill loses at most
/// this content's work (INV-PR-04/05). The segments are read one at a time: memory stays
/// O(segment) whatever the content's size. Segments, the keyword index, the other step's
/// mentions and decisions are not touched (INV-ID-08). A content with no documents (gone,
/// failed, not read yet) changes nothing.
///
/// `after` is the cursor the caller read; if the re-run was started again since (another region
/// or pattern change), nothing is done and `None` comes back, so the caller starts over with the
/// new rules.
pub(crate) fn rerun_step(
    conn: &Connection,
    step: RerunStep,
    after: i64,
    content: i64,
    find: &mut dyn FnMut(&StoredSegment) -> SegmentFinds,
) -> Result<Option<RerunOutcome>, StoreError> {
    if rerun_cursor(conn, step)? != Some(after) {
        return Ok(None);
    }
    let mut out = RerunOutcome::default();
    let mut touched = BTreeSet::new();
    content_keys(conn, content, &mut touched)?;
    conn.prepare_cached("DELETE FROM identifier_mention WHERE content_id = ?1 AND found_by = ?2")?
        .execute(params![content, step.as_str()])?;
    if step == RerunStep::Builtin {
        conn.prepare_cached("DELETE FROM sensitive_flag WHERE content_id = ?1")?
            .execute([content])?;
    }
    let mut insert_mention_stmt = conn.prepare_cached(INSERT_MENTION)?;
    let mut insert_flag_stmt = conn.prepare_cached(INSERT_FLAG)?;
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.document_id, s.loc_kind, s.loc_pos, s.loc_ref, s.text, s.malformed,
                s.cell_starts, s.ocr
         FROM document d JOIN segment s ON s.document_id = d.id
         WHERE d.content_id = ?1 ORDER BY d.id, s.ordinal",
    )?;
    let mut rows = stmt.query([content])?;
    let mut any = false;
    while let Some(r) = rows.next()? {
        any = true;
        let id: i64 = r.get(0)?;
        let text: String = r.get(5)?;
        let ocr: Option<Vec<u8>> = r.get(8)?;
        let seg = StoredSegment {
            document_id: r.get(1)?,
            locator: codec::segment_from(
                &r.get::<_, String>(2)?,
                r.get(3)?,
                r.get(4)?,
                &text,
                ocr.as_deref(),
            )?,
            malformed: r.get(6)?,
            cell_starts: codec::cells_from_blob(r.get(7)?),
            ocr: ocr.map(|b| codec::ocr_from_blob(&b, &text)).transpose()?,
            text,
        };
        let finds = find(&seg);
        let len = seg.text.chars().count();
        for m in finds.mentions {
            if found_by(&m.key) != step {
                return Err(invalid(format!(
                    "a {} re-run found a mention of another step",
                    step.as_str()
                )));
            }
            let place = Place::check(&m.locator, len, &seg.cell_starts)?;
            let key = insert_mention(
                &mut insert_mention_stmt,
                id,
                seg.document_id,
                content,
                &place,
                &m.raw,
                &m.key,
                m.confidence,
            )?;
            touched.insert(key);
            out.mentions += 1;
        }
        if step == RerunStep::Custom && !finds.card_flags.is_empty() {
            return Err(invalid("a custom-pattern re-run makes no card-like flags"));
        }
        for f in finds.card_flags {
            let place = Place::check(&f.locator, len, &seg.cell_starts)?;
            insert_flag(
                &mut insert_flag_stmt,
                id,
                seg.document_id,
                content,
                &place,
                &f.last4,
            )?;
            out.card_flags += 1;
        }
    }
    drop(rows);
    if any {
        crate::table::apply_table_step(conn, content, &mut touched)?;
    }
    crate::manage::set_meta(conn, step.cursor_key(), Some(&content.to_string()))?;
    out.connections_changed = rebuild_keys(conn, &touched)?;
    Ok(Some(out))
}

pub(crate) fn rerun_cursor(conn: &Connection, step: RerunStep) -> Result<Option<i64>, StoreError> {
    Ok(crate::documents::meta(conn, step.cursor_key())?.and_then(|v| v.parse().ok()))
}

/// Starts a re-run of `step` over every content from the first (a region change, US-04.2 AC2;
/// a pattern change, US-04.7 AC3).
pub(crate) fn begin_rerun(conn: &Connection, step: RerunStep) -> Result<(), StoreError> {
    crate::manage::set_meta(conn, step.cursor_key(), Some("0"))
}

/// Ends a re-run of `step` whose cursor is still `after` (`None`: no re-run was started, an
/// empty project): the stored mentions now follow `stamp` (the rules version, or the pattern
/// set's hash). `false` if the re-run was started again since, which then stays to do.
pub(crate) fn finish_rerun(
    conn: &Connection,
    step: RerunStep,
    after: Option<i64>,
    stamp: &str,
) -> Result<bool, StoreError> {
    if rerun_cursor(conn, step)? != after {
        return Ok(false);
    }
    crate::manage::set_meta(conn, step.cursor_key(), None)?;
    crate::manage::set_meta(conn, step.stamp_key(), Some(stamp))?;
    Ok(true)
}

/// Replaces the project's custom patterns (US-04.7, checked by the caller: names unique
/// ignoring case, each compiles) and starts a custom-pattern re-run, in one transaction: the
/// re-run starts when the change is saved (AC3). A pattern kept by name keeps its `created_at`.
pub(crate) fn set_custom_patterns(
    conn: &Connection,
    patterns: &[(String, String)],
) -> Result<(), StoreError> {
    let created: std::collections::HashMap<String, String> = {
        let mut stmt = conn.prepare_cached("SELECT name, created_at FROM custom_pattern")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    conn.execute("DELETE FROM custom_pattern", [])?;
    let now = now_iso();
    let mut insert = conn.prepare_cached(
        "INSERT INTO custom_pattern (name, expression, created_at) VALUES (?1, ?2, ?3)",
    )?;
    for (name, expression) in patterns {
        let at = created.get(name).unwrap_or(&now);
        insert.execute(params![name, expression, at])?;
    }
    begin_rerun(conn, RerunStep::Custom)
}

/// Confirms a table's column mapping (US-05.2 AC1): the decision (anchor: the path and the
/// header signature) and its application to the content — entity mentions, record links, the
/// rebuild of their keys — in one transaction. Returns the decision's `seq` and whether a
/// connection changed.
pub(crate) fn confirm_mapping(
    conn: &Connection,
    file_ref: i64,
    assignments: &mk_mapping::Assignments,
) -> Result<(i64, bool), StoreError> {
    let (root, rel, content): (i64, String, i64) = conn
        .prepare_cached("SELECT root_id, rel_path, content_id FROM file_ref WHERE id = ?1")?
        .query_row([file_ref], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
        .ok_or_else(|| invalid("This file is no longer in the project"))?;
    let signature: String = conn
        .prepare_cached(
            "SELECT signature FROM table_document WHERE content_id = ?1
             ORDER BY document_id LIMIT 1",
        )?
        .query_row([content], |r| r.get(0))
        .optional()?
        .ok_or_else(|| invalid("This file has no columns to map"))?;
    let anchor = mk_mapping::mapping_anchor(root, &rel, &signature);
    let (seq, _) = append_decision(
        conn,
        DecisionKind::ConfirmMapping,
        &anchor,
        None,
        Some(&assignments.to_json()),
    )?;
    let mut touched = BTreeSet::new();
    crate::table::apply_table_step(conn, content, &mut touched)?;
    Ok((seq, rebuild_keys(conn, &touched)?))
}
