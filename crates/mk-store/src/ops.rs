//! Write operations. Each runs inside one writer transaction (ADR-0004): its rows, the state
//! changes and the derived rebuild of the keys it touched commit together (ADR-0005, INV-PR-04).

use std::collections::BTreeSet;
use std::path::PathBuf;

use mk_corpus::{IngestionState, PathChange};
use mk_graph::DecisionKind;
use mk_jobs::{ItemState, JobState, Stage, Trigger};
use mk_kernel::{ContentHash, IdentifierKey, Locator};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::codec;
use crate::derive::rebuild_keys;
use crate::time::now_iso;

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
    pub warnings: Vec<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct DocumentOut {
    pub kind: String,
    pub position: String,
    pub segments: Vec<SegmentOut>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct SegmentOut {
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
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

/// What a Stage A commit did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CommitOutcome {
    pub segments: usize,
    pub mentions: usize,
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

fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::Invalid(msg.into())
}

fn content_state(conn: &Connection, id: i64) -> Result<IngestionState, StoreError> {
    let s: String = conn
        .prepare_cached("SELECT state FROM content WHERE id = ?1")?
        .query_row([id], |r| r.get(0))?;
    IngestionState::from_name(&s).ok_or_else(|| invalid(format!("unknown content state {s}")))
}

fn set_content_state(
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

fn item_state(conn: &Connection, id: i64) -> Result<(ItemState, i64, Option<i64>), StoreError> {
    let (s, job, content): (String, i64, Option<i64>) = conn
        .prepare_cached("SELECT state, job_id, content_id FROM work_item WHERE id = ?1")?
        .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let s = ItemState::from_name(&s).ok_or_else(|| invalid(format!("unknown item state {s}")))?;
    Ok((s, job, content))
}

fn set_item_state(
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

/// The identifier keys of a content's stored mentions.
fn content_keys(
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
    Ok(())
}

/// Marks a job Completed once none of its items is Pending or In progress (domain §7.2).
fn complete_job_if_done(conn: &Connection, job: i64) -> Result<(), StoreError> {
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

fn create_job(conn: &Connection, trigger: Trigger) -> Result<i64, StoreError> {
    conn.prepare_cached(
        "INSERT INTO job (trigger, stage, state, created_at) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        trigger.as_str(),
        Stage::A.as_str(),
        JobState::Queued.as_str(),
        now_iso()
    ])?;
    Ok(conn.last_insert_rowid())
}

fn add_item(conn: &Connection, job: i64, content: i64, size: u64) -> Result<i64, StoreError> {
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
fn skip_pending(conn: &Connection, content: i64) -> Result<(), StoreError> {
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
    let mut affected: BTreeSet<i64> = BTreeSet::new();
    let mut created: BTreeSet<i64> = BTreeSet::new();
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
                }
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
                }
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

    let mut out = RescanOutcome::default();
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
    out.connections_changed = rebuild_keys(conn, &touched)?;
    Ok(out)
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
    conn.prepare_cached("UPDATE work_item SET attempts = attempts + 1 WHERE id = ?1")?
        .execute([item])?;
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

/// Commits a Stage A result (INV-PR-04, INV-CO-06, ADR-0005): the content's old documents,
/// segments and mentions are replaced by the new ones, the touched keys are rebuilt, and the
/// item and content become Done, all in this one transaction.
pub(crate) fn commit_stage_a(
    conn: &Connection,
    item: i64,
    output: &StageAOutput,
) -> Result<CommitOutcome, StoreError> {
    let (job, content) = set_item_state(conn, item, ItemState::Done)?;
    let content = content.ok_or_else(|| invalid(format!("item {item} has no content")))?;
    set_content_state(conn, content, IngestionState::Done)?;

    let mut touched = BTreeSet::new();
    content_keys(conn, content, &mut touched)?;
    conn.prepare_cached("DELETE FROM document WHERE content_id = ?1")?
        .execute([content])?;

    let mut out = CommitOutcome::default();
    let mut doc_ids: Vec<i64> = Vec::with_capacity(output.documents.len());
    let mut segment_ids: Vec<Vec<i64>> = Vec::with_capacity(output.documents.len());
    {
        let mut insert_doc = conn.prepare_cached(
            "INSERT INTO document (content_id, position, kind) VALUES (?1, ?2, ?3)",
        )?;
        let mut insert_seg = conn.prepare_cached(
            "INSERT INTO segment (document_id, ordinal, loc_kind, loc_pos, text, malformed,
                                  cell_starts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for doc in &output.documents {
            insert_doc.execute(params![content, doc.position, doc.kind])?;
            let doc_id = conn.last_insert_rowid();
            doc_ids.push(doc_id);
            let mut ids = Vec::with_capacity(doc.segments.len());
            for (ordinal, seg) in doc.segments.iter().enumerate() {
                let (kind, pos) = codec::segment_locator(&seg.locator)?;
                insert_seg.execute(params![
                    doc_id,
                    ordinal as i64,
                    kind,
                    pos,
                    seg.text,
                    seg.malformed,
                    codec::cells_to_blob(&seg.cell_starts)
                ])?;
                ids.push(conn.last_insert_rowid());
            }
            out.segments += ids.len();
            segment_ids.push(ids);
        }
    }
    {
        let mut insert = conn.prepare_cached(
            "INSERT INTO identifier_mention (segment_id, document_id, content_id, identifier_key,
                 span_start, span_end, column_index, column_name, raw, confidence, found_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'builtin')",
        )?;
        for m in &output.mentions {
            let seg_id = *segment_ids
                .get(m.document)
                .and_then(|s| s.get(m.segment))
                .ok_or_else(|| invalid("mention points to no segment"))?;
            let seg = &output.documents[m.document].segments[m.segment];
            let span = m
                .locator
                .span()
                .ok_or_else(|| invalid("mention locator has no span"))?;
            // INV-CO-04: the span lies in the segment's stored text.
            if span.end() > seg.text.chars().count() {
                return Err(invalid("mention span is past its segment text"));
            }
            let cell = (!seg.cell_starts.is_empty()).then(|| {
                seg.cell_starts
                    .partition_point(|s| *s as usize <= span.start())
                    .saturating_sub(1)
            });
            let (column_index, column_name) = codec::column(&m.locator, cell);
            if !(0.0..=1.0).contains(&m.confidence) {
                return Err(invalid("confidence outside 0..1"));
            }
            let key = m.key.to_string();
            insert.execute(params![
                seg_id,
                doc_ids[m.document],
                content,
                key,
                span.start() as i64,
                span.end() as i64,
                column_index,
                column_name,
                m.raw,
                f64::from(m.confidence)
            ])?;
            touched.insert(key);
        }
    }
    out.mentions = output.mentions.len();
    conn.prepare_cached(
        "UPDATE content SET warnings = ?2, failure_code = NULL, failure_reason = NULL
         WHERE id = ?1",
    )?
    .execute(params![content, output.warnings.join("\n")])?;
    out.touched_keys = touched.len();
    out.connections_changed = rebuild_keys(conn, &touched)?;
    complete_job_if_done(conn, job)?;
    Ok(out)
}

/// Fails an item and its content with a stable code and a plain reason (INV-CO-09, US-02.7).
/// No other content changes.
pub(crate) fn fail_item(
    conn: &Connection,
    item: i64,
    code: &str,
    reason: &str,
) -> Result<(), StoreError> {
    let (job, content) = set_item_state(conn, item, ItemState::Failed)?;
    conn.prepare_cached(
        "UPDATE work_item SET failure_code = ?2, failure_reason = ?3 WHERE id = ?1",
    )?
    .execute(params![item, code, reason])?;
    if let Some(content) = content {
        set_content_state(conn, content, IngestionState::Failed)?;
        conn.prepare_cached(
            "UPDATE content SET failure_code = ?2, failure_reason = ?3 WHERE id = ?1",
        )?
        .execute(params![content, code, reason])?;
    }
    complete_job_if_done(conn, job)?;
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
    let changed = rebuild_keys(conn, &BTreeSet::from([anchor.to_string()]))?;
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
