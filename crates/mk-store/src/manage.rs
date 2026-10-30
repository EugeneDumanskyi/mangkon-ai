//! M3c writes: pause, resume and cancel the queue, retry and re-process contents, remove files
//! and roots, relocate roots, settings. Each runs in one writer transaction (ADR-0004); the
//! derived rows of every key whose evidence changed are rebuilt in it (ADR-0005).

use std::collections::BTreeSet;

use mk_corpus::IngestionState;
use mk_jobs::{ItemState, JobState, Trigger};
use mk_workspace::ProjectSettings;
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::derive::rebuild_keys;
use crate::ops::{
    add_item, complete_job_if_done, content_keys, content_state, create_job, invalid,
    set_content_state, skip_pending,
};
use crate::time::{now_iso, now_ms};

/// Job states in which a job still has work to do.
const OPEN_JOBS: [JobState; 4] = [
    JobState::Queued,
    JobState::Running,
    JobState::Paused,
    JobState::Blocked,
];

pub(crate) fn set_meta(
    conn: &Connection,
    key: &str,
    value: Option<&str>,
) -> Result<(), StoreError> {
    match value {
        Some(v) => conn
            .prepare_cached(
                "INSERT INTO project_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?
            .execute(params![key, v])?,
        None => conn
            .prepare_cached("DELETE FROM project_meta WHERE key = ?1")?
            .execute([key])?,
    };
    Ok(())
}

fn jobs_in(conn: &Connection, states: &[JobState]) -> Result<Vec<(i64, JobState)>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT id, state FROM job ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, s) = row?;
        let s = JobState::from_name(&s).ok_or_else(|| invalid(format!("unknown job state {s}")))?;
        if states.contains(&s) {
            out.push((id, s));
        }
    }
    Ok(out)
}

fn set_job_state(
    conn: &Connection,
    job: i64,
    from: JobState,
    to: JobState,
) -> Result<(), StoreError> {
    from.move_to(to).map_err(|e| invalid(e.to_string()))?;
    conn.prepare_cached("UPDATE job SET state = ?2, paused_at = ?3 WHERE id = ?1")?
        .execute(params![
            job,
            to.as_str(),
            (to == JobState::Paused).then(now_ms)
        ])?;
    Ok(())
}

/// Pauses or resumes the project's queue (US-03.2, D11, INV-PR-06). Pause: Queued and Running
/// jobs become Paused, and jobs made while paused start Paused. Resume: Paused jobs run again;
/// one whose items all finished meanwhile completes. Returns the jobs moved.
pub(crate) fn set_queue_paused(conn: &Connection, paused: bool) -> Result<usize, StoreError> {
    set_meta(conn, "queue_paused", paused.then_some("1"))?;
    let mut moved = 0;
    if paused {
        for (job, s) in jobs_in(conn, &[JobState::Queued, JobState::Running])? {
            set_job_state(conn, job, s, JobState::Paused)?;
            moved += 1;
        }
    } else {
        for (job, s) in jobs_in(conn, &[JobState::Paused])? {
            set_job_state(conn, job, s, JobState::Running)?;
            complete_job_if_done(conn, job)?;
            moved += 1;
        }
    }
    Ok(moved)
}

/// What [`cancel_queue`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CancelOutcome {
    pub items: usize,
    /// Contents now Not processed (D10).
    pub contents: usize,
    pub jobs: usize,
}

/// Cancels every open job (US-03.3, D10): its Pending and In-progress items become Cancelled,
/// their contents Not processed, the jobs Cancelled. Done items and their results are kept; an
/// item in progress committed nothing (INV-PR-04). The queue is no longer paused.
pub(crate) fn cancel_queue(conn: &Connection) -> Result<CancelOutcome, StoreError> {
    let mut out = CancelOutcome::default();
    for (job, s) in jobs_in(conn, &OPEN_JOBS)? {
        let items: Vec<(i64, String, Option<i64>)> = {
            let mut stmt = conn.prepare_cached(
                "SELECT id, state, content_id FROM work_item
                 WHERE job_id = ?1 AND state IN (?2, ?3)",
            )?;
            stmt.query_map(
                params![
                    job,
                    ItemState::Pending.as_str(),
                    ItemState::InProgress.as_str()
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?
            .collect::<Result<_, _>>()?
        };
        for (item, state, content) in items {
            let from = ItemState::from_name(&state)
                .ok_or_else(|| invalid(format!("unknown item state {state}")))?;
            from.move_to(ItemState::Cancelled)
                .map_err(|e| invalid(e.to_string()))?;
            conn.prepare_cached("UPDATE work_item SET state = ?2, finished_at = ?3 WHERE id = ?1")?
                .execute(params![item, ItemState::Cancelled.as_str(), now_ms()])?;
            out.items += 1;
            let Some(content) = content else { continue };
            if content_state(conn, content)? == IngestionState::Processing {
                set_content_state(conn, content, IngestionState::Queued)?;
            }
            if content_state(conn, content)? == IngestionState::Queued {
                set_content_state(conn, content, IngestionState::NotProcessed)?;
                out.contents += 1;
            }
        }
        set_job_state(conn, job, s, JobState::Cancelled)?;
        out.jobs += 1;
    }
    set_meta(conn, "queue_paused", None)?;
    Ok(out)
}

/// Queues `contents` again under one new job with `trigger`, each from a state in `from`.
/// Others are left out. Returns the job and the contents queued.
fn requeue(
    conn: &Connection,
    contents: &[i64],
    from: &[IngestionState],
    trigger: Trigger,
) -> Result<(Option<i64>, usize), StoreError> {
    let mut job = None;
    let mut n = 0;
    for &id in contents {
        let Some((state, size)): Option<(String, i64)> = conn
            .prepare_cached("SELECT state, size FROM content WHERE id = ?1")?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
        else {
            continue;
        };
        let state = IngestionState::from_name(&state)
            .ok_or_else(|| invalid(format!("unknown content state {state}")))?;
        if !from.contains(&state) {
            continue;
        }
        set_content_state(conn, id, IngestionState::Queued)?;
        conn.prepare_cached(
            "UPDATE content SET failure_code = NULL, failure_reason = NULL, failure_detail = NULL
             WHERE id = ?1",
        )?
        .execute([id])?;
        let j = match job {
            Some(j) => j,
            None => *job.insert(create_job(conn, trigger)?),
        };
        add_item(conn, j, id, size as u64)?;
        n += 1;
    }
    Ok((job, n))
}

/// Retries failed and not-processed contents under a new Retry job (US-02.7 AC3, D10). Only
/// those contents run again.
pub(crate) fn retry_contents(
    conn: &Connection,
    contents: &[i64],
) -> Result<(Option<i64>, usize), StoreError> {
    requeue(
        conn,
        contents,
        &[IngestionState::Failed, IngestionState::NotProcessed],
        Trigger::Retry,
    )
}

/// Failure codes of contents that OCR could not read for want of a language.
const OCR_LANGUAGE_FAILURES: [&str; 2] = ["ocr_language_missing", "ocr_unavailable"];

/// After the OCR languages change (US-01.6 AC3, INV-WS-07): Done contents with an OCR'd page,
/// and contents that failed for a missing OCR language, go back to Queued under one Settings
/// job. Nothing else runs again. Their old results stay counted until the new ones commit.
pub(crate) fn requeue_for_ocr(conn: &Connection) -> Result<(Option<i64>, usize), StoreError> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT c.id FROM content c
             WHERE (c.state = ?1 AND EXISTS (
                        SELECT 1 FROM document d JOIN segment s ON s.document_id = d.id
                        WHERE d.content_id = c.id AND s.loc_kind = 'ocr_text'))
                OR (c.state = ?2 AND c.failure_code IN (?3, ?4))
             ORDER BY c.id",
        )?;
        stmt.query_map(
            params![
                IngestionState::Done.as_str(),
                IngestionState::Failed.as_str(),
                OCR_LANGUAGE_FAILURES[0],
                OCR_LANGUAGE_FAILURES[1]
            ],
            |r| r.get(0),
        )?
        .collect::<Result<_, _>>()?
    };
    requeue(
        conn,
        &ids,
        &[IngestionState::Done, IngestionState::Failed],
        Trigger::Settings,
    )
}

/// What a removal did (US-01.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RemoveOutcome {
    pub refs: usize,
    /// Contents with no path left: Removed, their data deleted (INV-CO-08).
    pub contents: usize,
    pub connections_changed: bool,
}

/// After the paths of `contents` changed: a content with no path left is Removed and its
/// documents, segments and mentions deleted (INV-CO-08); one whose remaining paths are all
/// missing becomes Missing (INV-CO-07). A content being read is refused.
fn settle_contents(
    conn: &Connection,
    contents: &BTreeSet<i64>,
    touched: &mut BTreeSet<String>,
) -> Result<usize, StoreError> {
    let mut removed = 0;
    for &id in contents {
        let (refs, present): (i64, i64) = conn
            .prepare_cached(
                "SELECT COUNT(*), COALESCE(SUM(present), 0) FROM file_ref WHERE content_id = ?1",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let state = content_state(conn, id)?;
        if state == IngestionState::Processing && present == 0 {
            return Err(invalid(
                "A file is being read right now. Pause, then remove it.",
            ));
        }
        if refs == 0 {
            state
                .move_to(IngestionState::Removed)
                .map_err(|e| invalid(e.to_string()))?;
            skip_pending(conn, id)?;
            content_keys(conn, id, touched)?;
            conn.prepare_cached("DELETE FROM document WHERE content_id = ?1")?
                .execute([id])?;
            conn.prepare_cached("DELETE FROM content WHERE id = ?1")?
                .execute([id])?;
            removed += 1;
        } else if present == 0 && state.can_move_to(IngestionState::Missing) {
            set_content_state(conn, id, IngestionState::Missing)?;
            conn.prepare_cached("UPDATE content SET state_before_missing = ?2 WHERE id = ?1")?
                .execute(params![id, state.as_str()])?;
            skip_pending(conn, id)?;
            content_keys(conn, id, touched)?;
        }
    }
    Ok(removed)
}

/// Removes file references from the project (US-01.5 AC1, INV-CO-08, D3). A path inside a
/// folder root is excluded, so re-scan does not add it again; a single-file root goes with its
/// path. The originals are never touched (INV-CO-01).
pub(crate) fn remove_file_refs(
    conn: &Connection,
    refs: &[i64],
) -> Result<RemoveOutcome, StoreError> {
    let mut out = RemoveOutcome::default();
    let mut contents = BTreeSet::new();
    for &id in refs {
        let Some((root, rel, content, kind)): Option<(i64, String, i64, String)> = conn
            .prepare_cached(
                "SELECT f.root_id, f.rel_path, f.content_id, r.kind
                 FROM file_ref f JOIN source_root r ON r.id = f.root_id WHERE f.id = ?1",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
        else {
            continue;
        };
        conn.prepare_cached("DELETE FROM file_ref WHERE id = ?1")?
            .execute([id])?;
        if kind == "file" {
            conn.prepare_cached("DELETE FROM root_exclusion WHERE root_id = ?1")?
                .execute([root])?;
            conn.prepare_cached("DELETE FROM source_root WHERE id = ?1")?
                .execute([root])?;
        } else {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO root_exclusion (root_id, rel_path) VALUES (?1, ?2)",
            )?
            .execute(params![root, rel])?;
        }
        contents.insert(content);
        out.refs += 1;
    }
    let mut touched = BTreeSet::new();
    out.contents = settle_contents(conn, &contents, &mut touched)?;
    out.connections_changed = rebuild_keys(conn, &touched)?;
    Ok(out)
}

/// Removes a source root with all its paths and exclusions (US-01.5 for a whole root).
pub(crate) fn remove_root(conn: &Connection, root: i64) -> Result<RemoveOutcome, StoreError> {
    let mut out = RemoveOutcome::default();
    let contents: BTreeSet<i64> = {
        let mut stmt =
            conn.prepare_cached("SELECT DISTINCT content_id FROM file_ref WHERE root_id = ?1")?;
        stmt.query_map([root], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    out.refs = conn
        .prepare_cached("DELETE FROM file_ref WHERE root_id = ?1")?
        .execute([root])?;
    conn.prepare_cached("DELETE FROM root_exclusion WHERE root_id = ?1")?
        .execute([root])?;
    if conn
        .prepare_cached("DELETE FROM source_root WHERE id = ?1")?
        .execute([root])?
        == 0
    {
        return Err(invalid(format!("no source root {root}")));
    }
    let mut touched = BTreeSet::new();
    out.contents = settle_contents(conn, &contents, &mut touched)?;
    out.connections_changed = rebuild_keys(conn, &touched)?;
    Ok(out)
}

/// Points a source root at a new path (D12): its paths stay relative to it, so the next re-scan
/// finds the same files there (same size and time: unchanged; else hashed, same hash: found
/// again with nothing redone).
pub(crate) fn relocate_root(conn: &Connection, root: i64, path: &str) -> Result<(), StoreError> {
    let other: Option<i64> = conn
        .prepare_cached("SELECT id FROM source_root WHERE path = ?1 AND id != ?2")?
        .query_row(params![path, root], |r| r.get(0))
        .optional()?;
    if other.is_some() {
        return Err(invalid(format!(
            "{path} is already a source of this project"
        )));
    }
    if conn
        .prepare_cached("UPDATE source_root SET path = ?2 WHERE id = ?1")?
        .execute(params![root, path])?
        == 0
    {
        return Err(invalid(format!("no source root {root}")));
    }
    Ok(())
}

/// Removes the exclusions of `root` at `rel_path` and under it: the user added that path again.
/// An empty `rel_path` clears the whole root. Returns how many were removed.
pub(crate) fn clear_exclusions(
    conn: &Connection,
    root: i64,
    rel_path: &str,
) -> Result<usize, StoreError> {
    let n = if rel_path.is_empty() {
        conn.prepare_cached("DELETE FROM root_exclusion WHERE root_id = ?1")?
            .execute([root])?
    } else {
        conn.prepare_cached(
            "DELETE FROM root_exclusion
             WHERE root_id = ?1 AND (rel_path = ?2 OR substr(rel_path, 1, length(?2) + 1) = ?2 || '/')",
        )?
        .execute(params![root, rel_path])?
    };
    Ok(n)
}

/// Stores the settings (US-01.6). The caller checked them ([`ProjectSettings::normalized`]).
/// Stores settings. A stop-list edit rebuilds the domains added to or taken off the list in the
/// same transaction (INV-LK-03, ADR-0005). Returns whether a connection row changed.
pub(crate) fn set_settings(conn: &Connection, s: &ProjectSettings) -> Result<bool, StoreError> {
    if s.ocr_languages.is_empty() {
        return Err(invalid("a project needs at least one OCR language"));
    }
    let before = crate::derive::stop_list(conn)?;
    set_meta(conn, "ocr_languages", Some(&s.ocr_languages_joined()))?;
    set_meta(conn, "phone_region", s.phone_region.as_deref())?;
    set_meta(
        conn,
        "domain_stop_list",
        Some(&s.domain_stop_list.join("\n")),
    )?;
    let after = crate::derive::stop_list(conn)?;
    crate::derive::rebuild_keys(conn, &before.changed_keys(&after))
}

/// Writes the project name and the app version on create and after a migration (US-01.2 AC3).
pub(crate) fn stamp(
    conn: &Connection,
    name: Option<&str>,
    app_version: &str,
) -> Result<(), StoreError> {
    if let Some(name) = name {
        set_meta(conn, "name", Some(name))?;
    }
    set_meta(conn, "app_version", Some(app_version))?;
    set_meta(conn, "stamped_at", Some(&now_iso()))
}
