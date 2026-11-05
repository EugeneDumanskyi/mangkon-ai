//! The embeddings lane in the store (M6b, BC-10, ADR-0010): schedule, items, the commit of one
//! item's vectors, the re-run's switch, and what meaning search reads.
//!
//! Built on Stage B's runs and items (`ai.rs`): the attempt token, pause and cancel, the job's
//! run. The differences: an item's vectors are all it writes, a re-run's vectors wait beside the
//! active run's in `segment_embedding` (reads go by run id), and the switch needs no rebuild.

use mk_ai::RunState;
use mk_jobs::{ItemState, JobState, Stage, Trigger};
use mk_search::meaning::{ItemPlan, ItemPlanner, char_slice, plan_windows};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::ai::{
    AiCancelled, AiRunRow, RUN_COLUMNS, RunBinding, ScheduleOutcome, UnitScope, cancel_jobs_of,
    cancel_run, current_run, drop_run, insert_run, pending_run, run_documents, run_from,
    token_holds,
};
use crate::documents::meta;
use crate::manage::{set_job_state, set_meta};
use crate::ops::{complete_job_if_done, create_job, invalid, set_item_state};
use crate::time::{now_iso, now_ms};

pub(crate) const STAGE: &str = "embeddings";
pub(crate) const WAITING: &str = "embedding_waiting_windows";
const GENERATION: &str = "embedding_generation";

/// Stored documents with their contents: read by Stage A (D9), all kinds, tables too.
fn documents(conn: &Connection, scope: &UnitScope) -> Result<Vec<(i64, i64)>, StoreError> {
    const BASE: &str = "SELECT d.id, d.content_id FROM document d JOIN content c
                        ON c.id = d.content_id WHERE c.state = 'done'";
    match scope {
        UnitScope::Contents(ids) => {
            let mut out = Vec::new();
            let mut stmt =
                conn.prepare_cached(&format!("{BASE} AND d.content_id = ?1 ORDER BY d.id"))?;
            for id in ids {
                let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                for row in rows {
                    out.push(row?);
                }
            }
            Ok(out)
        }
        _ => {
            let mut stmt = conn.prepare_cached(&format!("{BASE} ORDER BY d.id"))?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        }
    }
}

/// The items of `document` that are not covered yet for `run`.
fn uncovered_items(
    conn: &Connection,
    document: i64,
    run: Option<i64>,
    scope: &UnitScope,
) -> Result<Vec<ItemPlan>, StoreError> {
    let mut planner = ItemPlanner::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT id, text FROM segment WHERE document_id = ?1 ORDER BY ordinal",
        )?;
        let mut rows = stmt.query([document])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let text = r.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?;
            planner.push(id, plan_windows(text).len());
        }
    }
    let mut covered = conn.prepare_cached(&format!(
        "SELECT EXISTS (SELECT 1 FROM work_item w JOIN job j ON j.id = w.job_id
                        WHERE w.segment_first = ?1 AND w.segment_last = ?2 AND j.stage = ?3
                          AND (j.ai_run_id IS ?4
                               OR (j.ai_run_id IS NULL AND w.state IN ('failed', 'cancelled')))
                          AND w.state IN ({}))",
        scope.covered_states()
    ))?;
    let mut out = planner.finish();
    let mut err = None;
    out.retain(|item| {
        match covered.query_row(params![item.first, item.last, STAGE, run], |r| {
            r.get::<_, bool>(0)
        }) {
            Ok(hit) => !hit,
            Err(e) => {
                err = Some(e);
                false
            }
        }
    });
    match err {
        Some(e) => Err(e.into()),
        None => Ok(out),
    }
}

/// How many windows a schedule of `scope` would embed. Reads every segment in scope: the caller
/// runs it on a read connection, away from the writer.
pub(crate) fn count_windows(conn: &Connection, scope: &UnitScope) -> Result<u64, StoreError> {
    let run = current_run(conn, STAGE)?.map(|r| r.id);
    let mut n = 0;
    for (document, _) in documents(conn, scope)? {
        n += uncovered_items(conn, document, run, scope)?
            .iter()
            .map(|i| i.windows as u64)
            .sum::<u64>();
    }
    Ok(n)
}

fn add_items(
    conn: &Connection,
    job: i64,
    content: i64,
    items: &[ItemPlan],
) -> Result<(), StoreError> {
    let mut insert = conn.prepare_cached(
        "INSERT INTO work_item (job_id, content_id, priority, state, segment_first, segment_last)
         VALUES (?1, ?2, 0, ?3, ?4, ?5)",
    )?;
    for item in items {
        insert.execute(params![
            job,
            content,
            ItemState::Pending.as_str(),
            item.first,
            item.last
        ])?;
    }
    Ok(())
}

/// One new job over the uncovered items of `scope`, for the current run (none yet: bound later).
/// Nothing to do makes no job. The waiting count goes: the user or the rule has decided.
pub(crate) fn schedule(
    conn: &Connection,
    scope: &UnitScope,
    trigger: Trigger,
) -> Result<ScheduleOutcome, StoreError> {
    let run = current_run(conn, STAGE)?.map(|r| r.id);
    let mut out = ScheduleOutcome::default();
    for (document, content) in documents(conn, scope)? {
        let items = uncovered_items(conn, document, run, scope)?;
        if items.is_empty() {
            continue;
        }
        let job = match out.job {
            Some(j) => j,
            None => {
                let j = create_job(conn, trigger, Stage::Embeddings)?;
                conn.prepare_cached("UPDATE job SET ai_run_id = ?2 WHERE id = ?1")?
                    .execute(params![j, run])?;
                *out.job.insert(j)
            }
        };
        add_items(conn, job, content, &items)?;
        out.units += items.len() as u64;
    }
    set_meta(conn, WAITING, None)?;
    Ok(out)
}

/// Remembers how many windows wait for the user's start.
pub(crate) fn set_waiting(conn: &Connection, windows: u64) -> Result<(), StoreError> {
    set_meta(
        conn,
        WAITING,
        (windows > 0).then(|| windows.to_string()).as_deref(),
    )
}

/// One window to embed: where it is, and its text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WindowText {
    pub segment: i64,
    pub char_start: usize,
    pub char_end: usize,
    pub text: String,
}

/// An embeddings item the lane has started: what to send, and its token.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartedEmbeddingItem {
    pub item: i64,
    /// The token: vectors count only while the item is still In progress with this attempt.
    pub attempt: i64,
    pub run: i64,
    pub identity: mk_ai::RunIdentity,
    pub windows: Vec<WindowText>,
}

/// Starts the next Pending item of a job that may run. An item whose segments went is Skipped on
/// the way. `None` when nothing may start now.
pub(crate) fn start_next_item(
    conn: &Connection,
) -> Result<Option<StartedEmbeddingItem>, StoreError> {
    loop {
        let next: Option<(i64, i64, Option<i64>, Option<i64>)> = conn
            .prepare_cached(
                "SELECT w.id, w.job_id, w.segment_first, w.segment_last
                 FROM work_item w JOIN job j ON j.id = w.job_id
                 WHERE w.state = ?1 AND j.stage = ?2 AND j.state IN (?3, ?4)
                 ORDER BY w.priority, w.id LIMIT 1",
            )?
            .query_row(
                params![
                    ItemState::Pending.as_str(),
                    STAGE,
                    JobState::Queued.as_str(),
                    JobState::Running.as_str()
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((item, job, first, last)) = next else {
            return Ok(None);
        };
        let (Some(first), Some(last)) = (first, last) else {
            set_item_state(conn, item, ItemState::Skipped)?;
            complete_job_if_done(conn, job)?;
            continue;
        };
        let run: Option<AiRunRow> = conn
            .prepare_cached(&format!(
                "SELECT {RUN_COLUMNS} FROM ai_run
                 WHERE id = (SELECT ai_run_id FROM job WHERE id = ?1)"
            ))?
            .query_row([job], run_from)
            .optional()?;
        // Bound by the caller first; a job made since waits for the next look.
        let Some(run) = run else {
            return Ok(None);
        };
        set_item_state(conn, item, ItemState::InProgress)?;
        conn.prepare_cached(
            "UPDATE work_item SET attempts = attempts + 1, started_at = ?2, finished_at = NULL
             WHERE id = ?1",
        )?
        .execute(params![item, now_ms()])?;
        conn.prepare_cached("UPDATE job SET state = ?2 WHERE id = ?1 AND state = ?3")?
            .execute(params![
                job,
                JobState::Running.as_str(),
                JobState::Queued.as_str()
            ])?;
        let attempt: i64 = conn
            .prepare_cached("SELECT attempts FROM work_item WHERE id = ?1")?
            .query_row([item], |r| r.get(0))?;
        let mut windows = Vec::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT id, text FROM segment WHERE id BETWEEN ?1 AND ?2 ORDER BY id",
            )?;
            let mut rows = stmt.query(params![first, last])?;
            while let Some(r) = rows.next()? {
                let segment: i64 = r.get(0)?;
                let text = r.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?;
                for (char_start, char_end) in plan_windows(text) {
                    windows.push(WindowText {
                        segment,
                        char_start,
                        char_end,
                        text: char_slice(text, char_start, char_end).to_string(),
                    });
                }
            }
        }
        return Ok(Some(StartedEmbeddingItem {
            item,
            attempt,
            run: run.id,
            identity: run.identity,
            windows,
        }));
    }
}

/// One window's vector, ready to store.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmbeddingRow {
    pub segment: i64,
    pub char_start: usize,
    pub char_end: usize,
    /// Half floats, normalized (`mk_search::meaning::encode_f16`).
    pub vector: Vec<u8>,
}

/// One item's vectors, in one transaction (INV-PR-04): the token check, every vector, Done. False:
/// the item left In progress since it started, and they were dropped.
pub(crate) fn commit_item(
    conn: &Connection,
    item: i64,
    attempt: i64,
    rows: &[EmbeddingRow],
) -> Result<bool, StoreError> {
    if !token_holds(conn, item, attempt)? {
        return Ok(false);
    }
    let job: i64 = conn
        .prepare_cached("SELECT job_id FROM work_item WHERE id = ?1")?
        .query_row([item], |r| r.get(0))?;
    let (run, stored): (i64, Option<i64>) = conn
        .prepare_cached(
            "SELECT a.id, a.dims FROM job j JOIN ai_run a ON a.id = j.ai_run_id WHERE j.id = ?1",
        )?
        .query_row([job], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| invalid("an embeddings item's job has no run"))?;
    if let Some(first) = rows.first() {
        let dims = (first.vector.len() / 2) as i64;
        if dims == 0 || rows.iter().any(|r| r.vector.len() != first.vector.len()) {
            return Err(invalid("The vectors of this part differ in size"));
        }
        match stored {
            Some(d) if d != dims => {
                return Err(invalid(format!(
                    "The model sent vectors of {dims} dimensions, not {d}"
                )));
            }
            Some(_) => {}
            None => {
                conn.prepare_cached("UPDATE ai_run SET dims = ?2 WHERE id = ?1")?
                    .execute(params![run, dims])?;
            }
        }
    }
    let mut insert = conn.prepare_cached(
        "INSERT INTO segment_embedding (ai_run_id, segment_id, char_start, char_end, vector)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for r in rows {
        insert.execute(params![
            run,
            r.segment,
            r.char_start as i64,
            r.char_end as i64,
            r.vector
        ])?;
    }
    set_item_state(conn, item, ItemState::Done)?;
    conn.prepare_cached("UPDATE work_item SET finished_at = ?2 WHERE id = ?1")?
        .execute(params![item, now_ms()])?;
    complete_job_if_done(conn, job)?;
    Ok(true)
}

/// Starts a re-run: a `pending` run over the documents the active run covered. The active run's
/// unfinished work ends: this run covers it. A re-run already under way is cancelled first. The
/// old vectors stay until the switch.
pub(crate) fn start_rerun(
    conn: &Connection,
    b: &RunBinding,
) -> Result<(ScheduleOutcome, u64), StoreError> {
    if let Some(old) = pending_run(conn, STAGE)? {
        cancel_jobs_of(conn, old.id)?;
        drop_run(conn, old.id)?;
    }
    let active = crate::ai::active_run(conn, STAGE)?
        .ok_or_else(|| invalid("There is nothing to re-run yet"))?;
    if active.identity == b.identity {
        return Err(invalid(format!(
            "The current vectors already come from {}",
            active.model_name
        )));
    }
    let docs = run_documents(conn, active.id)?;
    cancel_jobs_of(conn, active.id)?;
    let run = insert_run(conn, STAGE, b, RunState::Pending)?;
    let job = create_job(conn, Trigger::ModelRerun, Stage::Embeddings)?;
    conn.prepare_cached("UPDATE job SET ai_run_id = ?2 WHERE id = ?1")?
        .execute(params![job, run])?;
    let (mut units, mut windows) = (0, 0);
    for (document, content) in docs {
        let items = uncovered_items(conn, document, Some(run), &UnitScope::All)?;
        add_items(conn, job, content, &items)?;
        units += items.len() as u64;
        windows += items.iter().map(|i| i.windows as u64).sum::<u64>();
    }
    if units == 0 {
        set_job_state(conn, job, JobState::Queued, JobState::Cancelled)?;
        drop_run(conn, run)?;
        return Err(invalid("There is nothing to re-run yet"));
    }
    set_meta(conn, WAITING, None)?;
    Ok((
        ScheduleOutcome {
            job: Some(job),
            units,
        },
        windows,
    ))
}

/// Cancels the re-run under way, or else the current run's remaining work. A first run left with
/// no vector goes too: otherwise its model would hold new work back with nothing to show for it.
pub(crate) fn cancel(conn: &Connection) -> Result<AiCancelled, StoreError> {
    let out = cancel_run(conn, STAGE, WAITING)?;
    if let Some(run) = crate::ai::active_run(conn, STAGE)?
        && run.dims.is_none()
    {
        drop_run(conn, run.id)?;
    }
    Ok(out)
}

/// The switch (ADR-0005), in the transaction of the last item: the old run is superseded, its
/// vectors go, this run is active. A reader sees one model or the other.
pub(crate) fn switch_run(conn: &Connection, run: i64) -> Result<(), StoreError> {
    let now = now_iso();
    let old: Option<i64> = conn
        .prepare_cached("SELECT id FROM ai_run WHERE stage = ?1 AND state = ?2")?
        .query_row(params![STAGE, RunState::Active.as_str()], |r| r.get(0))
        .optional()?;
    if let Some(old) = old {
        conn.prepare_cached("UPDATE ai_run SET state = ?2, finished_at = ?3 WHERE id = ?1")?
            .execute(params![old, RunState::Superseded.as_str(), now])?;
        conn.prepare_cached("DELETE FROM segment_embedding WHERE ai_run_id = ?1")?
            .execute([old])?;
    }
    conn.prepare_cached("UPDATE ai_run SET state = ?2, finished_at = ?3 WHERE id = ?1")?
        .execute(params![run, RunState::Active.as_str(), now])?;
    Ok(())
}

/// The value the index file's header is compared with.
pub(crate) fn generation(conn: &Connection) -> Result<u64, StoreError> {
    Ok(meta(conn, GENERATION)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// A page of a run's vectors after id `after`, in id order: what the index is built from.
pub(crate) fn rows_page(
    conn: &Connection,
    run: i64,
    after: i64,
    limit: usize,
) -> Result<Vec<(i64, Vec<u8>)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, vector FROM segment_embedding
         WHERE ai_run_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![run, after, limit as i64], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The vectors of `ids`, only those of `run` (INV-SR-02).
pub(crate) fn load_vectors(
    conn: &Connection,
    run: i64,
    ids: &[i64],
) -> Result<Vec<(i64, Vec<u8>)>, StoreError> {
    let mut stmt = conn
        .prepare_cached("SELECT vector FROM segment_embedding WHERE id = ?1 AND ai_run_id = ?2")?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Some(v) = stmt
            .query_row(params![id, run], |r| r.get::<_, Vec<u8>>(0))
            .optional()?
        {
            out.push((id, v));
        }
    }
    Ok(out)
}

/// A meaning result: its vector's id, its window (characters) and its segment's row.
pub type EmbeddingSearchRow = (i64, (usize, usize), crate::SearchRow);

/// Where a vector is: its segment and window.
pub(crate) fn window_of(
    conn: &Connection,
    id: i64,
) -> Result<Option<(i64, usize, usize)>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT segment_id, char_start, char_end FROM segment_embedding WHERE id = ?1",
        )?
        .query_row([id], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, i64>(1)? as usize,
                r.get::<_, i64>(2)? as usize,
            ))
        })
        .optional()?)
}
