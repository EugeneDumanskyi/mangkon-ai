//! Stage B in the store (M6a, ADR-0005, ADR-0006): runs, work units, the commit of a checked
//! reply, the re-run's switch.
//!
//! - `entity_mention` only ever holds what is visible. A first run is `active` from the start: each
//!   item's mentions commit with their per-key rebuild. A re-run is `pending`: its mentions wait
//!   in `pending_entity_mention`, and the switch moves them in, in one transaction with
//!   `rebuild_all`. It hangs off [`after_job_ended`], so every path that ends a job triggers it.
//! - An attempt token instead of aborting calls: pause, cancel and close move AI items out of In
//!   progress at once, and a reply that comes later fails its `(in_progress, attempts = N)` check
//!   and is dropped (NFR-REL-05, INV-PR-04). These paths never change `content.state`: a document
//!   is Done after Stage A (D9).

use std::collections::BTreeSet;

use mk_ai::check::{CheckedMention, IdentifierSpan, Piece};
use mk_ai::{RunIdentity, RunState, UnitPlan, UnitPlanner, unit_pieces};
use mk_jobs::{ItemState, JobState, Stage, Trigger, stage_b_priority};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::derive::{rebuild_all, rebuild_keys};
use crate::documents::meta;
use crate::manage::{set_job_state, set_meta};
use crate::ops::{complete_job_if_done, create_job, invalid, item_state, set_item_state};
use crate::table::TABLE_KINDS;
use crate::time::{now_iso, now_ms};

const STAGE: &str = "b";
const WAITING_UNITS: &str = "ai_waiting_units";

/// What a new run records (INV-AI-02): the model and where its data goes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunBinding {
    pub identity: RunIdentity,
    pub model_name: String,
    pub destination_host: String,
    pub destination_class: String,
    pub prompt_version: u32,
}

/// A stored run.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AiRunRow {
    pub id: i64,
    pub identity: RunIdentity,
    pub model_name: String,
    pub destination_host: String,
    pub destination_class: String,
    pub prompt_version: u32,
    pub state: RunState,
    pub mentions: u64,
    pub discarded: u64,
    pub invalid_responses: u64,
    pub identifier_dropped: u64,
}

const RUN_COLUMNS: &str = "id, model_identity, model_name, destination_host, destination_class,
    prompt_version, state, mention_count, discarded, invalid_responses, identifier_dropped";

fn run_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<AiRunRow> {
    let state: String = r.get(6)?;
    Ok(AiRunRow {
        id: r.get(0)?,
        identity: RunIdentity::from_stored(&r.get::<_, String>(1)?),
        model_name: r.get(2)?,
        destination_host: r.get(3)?,
        destination_class: r.get(4)?,
        prompt_version: r.get::<_, i64>(5)? as u32,
        state: RunState::from_name(&state).unwrap_or(RunState::Superseded),
        mentions: r.get::<_, i64>(7)? as u64,
        discarded: r.get::<_, i64>(8)? as u64,
        invalid_responses: r.get::<_, i64>(9)? as u64,
        identifier_dropped: r.get::<_, i64>(10)? as u64,
    })
}

fn run_in(conn: &Connection, state: RunState) -> Result<Option<AiRunRow>, StoreError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {RUN_COLUMNS} FROM ai_run WHERE stage = ?1 AND state = ?2"
        ))?
        .query_row(params![STAGE, state.as_str()], run_from)
        .optional()?)
}

/// The active run: what the screens show.
pub(crate) fn active_run(conn: &Connection) -> Result<Option<AiRunRow>, StoreError> {
    run_in(conn, RunState::Active)
}

/// The pending run: a re-run nobody sees yet.
pub(crate) fn pending_run(conn: &Connection) -> Result<Option<AiRunRow>, StoreError> {
    run_in(conn, RunState::Pending)
}

/// The run new work belongs to: a re-run under way takes it, so the switch leaves nothing behind.
fn current_run(conn: &Connection) -> Result<Option<AiRunRow>, StoreError> {
    Ok(match pending_run(conn)? {
        Some(r) => Some(r),
        None => active_run(conn)?,
    })
}

fn insert_run(conn: &Connection, b: &RunBinding, state: RunState) -> Result<i64, StoreError> {
    conn.prepare_cached(
        "INSERT INTO ai_run (stage, model_identity, model_name, destination_host,
                             destination_class, prompt_version, state, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?
    .execute(params![
        STAGE,
        b.identity.as_str(),
        b.model_name,
        b.destination_host,
        b.destination_class,
        i64::from(b.prompt_version),
        state.as_str(),
        now_iso()
    ])?;
    Ok(conn.last_insert_rowid())
}

/// The run jobs without one work for: the current one, or a first run made now, `active` from the
/// start. Open jobs made before a model was bound join it; ended ones, such as a cancelled
/// re-run's, stay in the history without a run.
pub(crate) fn bind_run(conn: &Connection, b: &RunBinding) -> Result<i64, StoreError> {
    let id = match current_run(conn)? {
        Some(r) => r.id,
        None => insert_run(conn, b, RunState::Active)?,
    };
    conn.prepare_cached(
        "UPDATE job SET ai_run_id = ?1
         WHERE stage = ?2 AND ai_run_id IS NULL AND state IN (?3, ?4, ?5, ?6)",
    )?
    .execute(params![
        id,
        STAGE,
        JobState::Queued.as_str(),
        JobState::Running.as_str(),
        JobState::Paused.as_str(),
        JobState::Blocked.as_str()
    ])?;
    Ok(id)
}

/// Which units a schedule is about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UnitScope {
    /// The automatic start: a unit ended by Cancel or Fail stays ended until the user asks.
    Auto,
    /// The user chose "all": cancelled and failed units run again.
    All,
    /// The user picked these contents.
    Contents(Vec<i64>),
}

impl UnitScope {
    /// The item states that mean "this unit is taken care of".
    fn covered_states(&self) -> &'static str {
        match self {
            UnitScope::Auto => "'pending', 'in_progress', 'done', 'failed', 'cancelled'",
            _ => "'pending', 'in_progress', 'done'",
        }
    }
}

/// Stored documents Stage B reads, with their contents: read by Stage A (D9), not tables, whose
/// names come through column mapping (US-05.3, `09-model-check.md` §3.1).
fn unit_documents(conn: &Connection, scope: &UnitScope) -> Result<Vec<(i64, i64)>, StoreError> {
    let base = format!(
        "SELECT d.id, d.content_id FROM document d JOIN content c ON c.id = d.content_id
         WHERE c.state = 'done' AND d.kind NOT IN ('{}', '{}')",
        TABLE_KINDS[0], TABLE_KINDS[1]
    );
    match scope {
        UnitScope::Contents(ids) => {
            let mut out = Vec::new();
            let mut stmt =
                conn.prepare_cached(&format!("{base} AND d.content_id = ?1 ORDER BY d.id"))?;
            for id in ids {
                let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                for row in rows {
                    out.push(row?);
                }
            }
            Ok(out)
        }
        _ => {
            let mut stmt = conn.prepare_cached(&format!("{base} ORDER BY d.id"))?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        }
    }
}

/// The units of `document` that are not covered yet for `run`.
fn uncovered_units(
    conn: &Connection,
    document: i64,
    run: Option<i64>,
    scope: &UnitScope,
) -> Result<Vec<UnitPlan>, StoreError> {
    let mut units = Vec::new();
    {
        let mut planner = UnitPlanner::new(mk_ai::MAX_UNIT_CHARS);
        let mut stmt = conn.prepare_cached(
            "SELECT id, text FROM segment WHERE document_id = ?1 ORDER BY ordinal",
        )?;
        let mut rows = stmt.query([document])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let text = r.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?;
            planner.push(id, text, &mut |u| units.push(u));
        }
        planner.finish(&mut |u| units.push(u));
    }
    let mut covered = conn.prepare_cached(&format!(
        "SELECT EXISTS (SELECT 1 FROM work_item w JOIN job j ON j.id = w.job_id
                        WHERE w.segment_first = ?1 AND w.segment_last = ?2
                          AND w.char_start IS ?3 AND w.char_end IS ?4
                          AND j.stage = ?5
                          AND (j.ai_run_id IS ?6
                               OR (j.ai_run_id IS NULL AND w.state IN ('failed', 'cancelled')))
                          AND w.state IN ({}))",
        scope.covered_states()
    ))?;
    let mut out = Vec::new();
    for u in units {
        let hit: bool = covered.query_row(
            params![
                u.first,
                u.last,
                u.window.map(|w| w.0 as i64),
                u.window.map(|w| w.1 as i64),
                STAGE,
                run
            ],
            |r| r.get(0),
        )?;
        if !hit {
            out.push(u);
        }
    }
    Ok(out)
}

/// How many units a schedule of `scope` would make. Reads every segment in scope: the caller runs
/// it on a read connection, away from the writer.
pub(crate) fn count_units(conn: &Connection, scope: &UnitScope) -> Result<u64, StoreError> {
    let run = current_run(conn)?.map(|r| r.id);
    let mut n = 0;
    for (document, _) in unit_documents(conn, scope)? {
        n += uncovered_units(conn, document, run, scope)?.len() as u64;
    }
    Ok(n)
}

/// What a schedule did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ScheduleOutcome {
    pub job: Option<i64>,
    pub units: u64,
}

fn add_units(
    conn: &Connection,
    job: i64,
    content: i64,
    units: &[UnitPlan],
) -> Result<(), StoreError> {
    let mut has_identifiers = conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM identifier_mention WHERE segment_id BETWEEN ?1 AND ?2)",
    )?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO work_item (job_id, content_id, priority, state, segment_first, segment_last,
                                char_start, char_end)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for u in units {
        let ids: bool = has_identifiers.query_row(params![u.first, u.last], |r| r.get(0))?;
        insert.execute(params![
            job,
            content,
            stage_b_priority(ids),
            ItemState::Pending.as_str(),
            u.first,
            u.last,
            u.window.map(|w| w.0 as i64),
            u.window.map(|w| w.1 as i64)
        ])?;
    }
    Ok(())
}

/// One new job over the uncovered units of `scope`, for the current run (none yet: bound later).
/// Nothing to do makes no job. The waiting count goes: the user or the rule has decided.
pub(crate) fn schedule_units(
    conn: &Connection,
    scope: &UnitScope,
    trigger: Trigger,
) -> Result<ScheduleOutcome, StoreError> {
    let run = current_run(conn)?.map(|r| r.id);
    let mut out = ScheduleOutcome::default();
    for (document, content) in unit_documents(conn, scope)? {
        let units = uncovered_units(conn, document, run, scope)?;
        if units.is_empty() {
            continue;
        }
        let job = match out.job {
            Some(j) => j,
            None => {
                let j = create_job(conn, trigger, Stage::B)?;
                conn.prepare_cached("UPDATE job SET ai_run_id = ?2 WHERE id = ?1")?
                    .execute(params![j, run])?;
                *out.job.insert(j)
            }
        };
        add_units(conn, job, content, &units)?;
        out.units += units.len() as u64;
    }
    set_meta(conn, WAITING_UNITS, None)?;
    Ok(out)
}

/// Remembers how many units wait for the user's pick (gap A3).
pub(crate) fn set_waiting_units(conn: &Connection, units: u64) -> Result<(), StoreError> {
    set_meta(
        conn,
        WAITING_UNITS,
        (units > 0).then(|| units.to_string()).as_deref(),
    )
}

/// An AI item the lane has started: what to send, what the checks need, and its token.
#[derive(Clone, PartialEq, Debug)]
pub struct StartedAiItem {
    pub item: i64,
    /// The token: a reply counts only while the item is still In progress with this attempt.
    pub attempt: i64,
    pub run: i64,
    pub identity: RunIdentity,
    pub pieces: Vec<Piece>,
    pub identifiers: Vec<IdentifierSpan>,
}

/// An item's id, its job, its first and last segment and its character range.
type NextItem = (i64, i64, Option<i64>, Option<i64>, Option<i64>, Option<i64>);

/// Starts the next Pending Stage B item of a job that may run (INV-PR-06, INV-PR-09), highest
/// priority first. An item whose segments went is Skipped on the way (domain §7.2). `None` when
/// nothing may start now.
pub(crate) fn start_next_ai_item(conn: &Connection) -> Result<Option<StartedAiItem>, StoreError> {
    loop {
        let next: Option<NextItem> = conn
            .prepare_cached(
                "SELECT w.id, w.job_id, w.segment_first, w.segment_last, w.char_start, w.char_end
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
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((item, job, first, last, char_start, char_end)) = next else {
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
        let segments: Vec<(i64, String)> = {
            let mut stmt = conn.prepare_cached(
                "SELECT id, text FROM segment WHERE id BETWEEN ?1 AND ?2 ORDER BY id",
            )?;
            stmt.query_map(params![first, last], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let plan = UnitPlan {
            first,
            last,
            window: char_start
                .zip(char_end)
                .map(|(s, e)| (s as usize, e as usize)),
        };
        let identifiers: Vec<IdentifierSpan> = {
            let mut stmt = conn.prepare_cached(
                "SELECT segment_id, span_start, span_end FROM identifier_mention
                 WHERE segment_id BETWEEN ?1 AND ?2",
            )?;
            stmt.query_map(params![first, last], |r| {
                Ok(IdentifierSpan {
                    segment: r.get(0)?,
                    start: r.get::<_, i64>(1)? as usize,
                    end: r.get::<_, i64>(2)? as usize,
                })
            })?
            .collect::<Result<_, _>>()?
        };
        return Ok(Some(StartedAiItem {
            item,
            attempt,
            run: run.id,
            identity: run.identity,
            pieces: unit_pieces(&plan, segments),
            identifiers,
        }));
    }
}

/// The token: the item is still In progress with this attempt.
fn token_holds(conn: &Connection, item: i64, attempt: i64) -> Result<bool, StoreError> {
    let row: Option<(String, i64)> = conn
        .prepare_cached("SELECT state, attempts FROM work_item WHERE id = ?1")?
        .query_row([item], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.is_some_and(|(s, a)| s == ItemState::InProgress.as_str() && a == attempt))
}

/// What one item's checks found.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct AiItemResult {
    pub mentions: Vec<CheckedMention>,
    pub discarded: u64,
    pub identifier_dropped: u64,
    /// 1 when the reply was still invalid after its retry (INV-AI-03).
    pub invalid_responses: u64,
}

/// What a commit did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AiCommitOutcome {
    /// False: the item left In progress since it started, and the reply was dropped.
    pub committed: bool,
    pub mentions: usize,
    pub connections_changed: bool,
}

/// A checked reply, in one transaction (INV-PR-04): the token check, the mentions (visible with
/// their per-key rebuild in an active run, set aside in a pending one), the run's counts, Done.
pub(crate) fn commit_ai_item(
    conn: &Connection,
    item: i64,
    attempt: i64,
    result: &AiItemResult,
) -> Result<AiCommitOutcome, StoreError> {
    if !token_holds(conn, item, attempt)? {
        return Ok(AiCommitOutcome::default());
    }
    let (job, _) = item_state_job(conn, item)?;
    let run: i64 = conn
        .prepare_cached("SELECT ai_run_id FROM job WHERE id = ?1")?
        .query_row([job], |r| r.get::<_, Option<i64>>(0))?
        .ok_or_else(|| invalid("an AI item's job has no run"))?;
    let state: String = conn
        .prepare_cached("SELECT state FROM ai_run WHERE id = ?1")?
        .query_row([run], |r| r.get(0))?;
    let active = state == RunState::Active.as_str();
    let mut touched = BTreeSet::new();
    let mut place = conn.prepare_cached(
        "SELECT s.document_id, d.content_id, length(s.text) FROM segment s
         JOIN document d ON d.id = s.document_id WHERE s.id = ?1",
    )?;
    let mut visible = conn.prepare_cached(
        "INSERT INTO entity_mention (segment_id, document_id, content_id, name_key, entity_type,
             raw, span_start, span_end, column_key, record, confidence, producer, ai_run_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', '', ?9, 'model', ?10)",
    )?;
    let mut pending = conn.prepare_cached(
        "INSERT INTO pending_entity_mention (segment_id, document_id, content_id, name_key,
             entity_type, raw, span_start, span_end, confidence, ai_run_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for m in &result.mentions {
        let (document, content, len): (i64, i64, i64) =
            place.query_row([m.segment], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let (start, end) = (m.span.0 as i64, m.span.1 as i64);
        if start >= end || end > len {
            return Err(invalid("an AI mention's span is past its segment text"));
        }
        let key = m.key.to_string();
        let args = params![
            m.segment,
            document,
            content,
            key,
            m.entity_type.as_str(),
            m.raw,
            start,
            end,
            f64::from(m.confidence.clamp(0.0, 1.0)),
            run
        ];
        if active {
            visible.execute(args)?;
            touched.insert(key);
        } else {
            pending.execute(args)?;
        }
    }
    conn.prepare_cached(
        "UPDATE ai_run SET mention_count = mention_count + ?2, discarded = discarded + ?3,
             identifier_dropped = identifier_dropped + ?4,
             invalid_responses = invalid_responses + ?5
         WHERE id = ?1",
    )?
    .execute(params![
        run,
        result.mentions.len() as i64,
        result.discarded as i64,
        result.identifier_dropped as i64,
        result.invalid_responses as i64
    ])?;
    set_item_state(conn, item, ItemState::Done)?;
    conn.prepare_cached("UPDATE work_item SET finished_at = ?2 WHERE id = ?1")?
        .execute(params![item, now_ms()])?;
    let connections_changed = rebuild_keys(conn, &touched)?;
    complete_job_if_done(conn, job)?;
    Ok(AiCommitOutcome {
        committed: true,
        mentions: result.mentions.len(),
        connections_changed,
    })
}

fn item_state_job(conn: &Connection, item: i64) -> Result<(i64, ItemState), StoreError> {
    let (state, job, _) = item_state(conn, item)?;
    Ok((job, state))
}

/// Puts one item back to Pending (a network error: it runs again). False: the token failed.
pub(crate) fn release_ai_item(
    conn: &Connection,
    item: i64,
    attempt: i64,
) -> Result<bool, StoreError> {
    if !token_holds(conn, item, attempt)? {
        return Ok(false);
    }
    set_item_state(conn, item, ItemState::Pending)?;
    conn.prepare_cached("UPDATE work_item SET started_at = NULL WHERE id = ?1")?
        .execute([item])?;
    Ok(true)
}

/// Fails one item with a plain reason; the rest goes on. False: the token failed.
pub(crate) fn fail_ai_item(
    conn: &Connection,
    item: i64,
    attempt: i64,
    code: &str,
    reason: &str,
) -> Result<bool, StoreError> {
    if !token_holds(conn, item, attempt)? {
        return Ok(false);
    }
    let (job, _) = set_item_state(conn, item, ItemState::Failed)?;
    conn.prepare_cached(
        "UPDATE work_item SET failure_code = ?2, failure_reason = ?3, finished_at = ?4
         WHERE id = ?1",
    )?
    .execute(params![item, code, reason, now_ms()])?;
    complete_job_if_done(conn, job)?;
    Ok(true)
}

/// Every Stage B item In progress goes back to Pending: what pause and close do at once, in the
/// transaction that changes the job (NFR-REL-05). Replies still on their way fail their token.
pub(crate) fn release_all_ai_items(conn: &Connection) -> Result<usize, StoreError> {
    Ok(conn
        .prepare_cached(
            "UPDATE work_item SET state = ?1, started_at = NULL
             WHERE state = ?2 AND job_id IN (SELECT id FROM job WHERE stage = ?3)",
        )?
        .execute(params![
            ItemState::Pending.as_str(),
            ItemState::InProgress.as_str(),
            STAGE
        ])?)
}

/// AI items in progress on `content` go back to Pending, so its caller can skip them.
pub(crate) fn release_ai_items_of(conn: &Connection, content: i64) -> Result<(), StoreError> {
    conn.prepare_cached(
        "UPDATE work_item SET state = ?1, started_at = NULL
         WHERE content_id = ?2 AND state = ?3 AND segment_first IS NOT NULL",
    )?
    .execute(params![
        ItemState::Pending.as_str(),
        content,
        ItemState::InProgress.as_str()
    ])?;
    Ok(())
}

/// The content is read again and its segments go: unfinished AI items on them are Skipped.
pub(crate) fn skip_ai_items_of(conn: &Connection, content: i64) -> Result<(), StoreError> {
    release_ai_items_of(conn, content)?;
    let jobs: Vec<i64> = {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT job_id FROM work_item
             WHERE content_id = ?1 AND state = ?2 AND segment_first IS NOT NULL",
        )?;
        stmt.query_map(params![content, ItemState::Pending.as_str()], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    if jobs.is_empty() {
        return Ok(());
    }
    conn.prepare_cached(
        "UPDATE work_item SET state = ?3
         WHERE content_id = ?1 AND state = ?2 AND segment_first IS NOT NULL",
    )?
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

/// Starts a re-run (US-10.4): a `pending` run over the documents the active run covered, one job
/// with one item per unit. The active run's unfinished work ends: this run covers it. A re-run
/// already under way is cancelled first. Nothing visible changes until the switch.
pub(crate) fn start_rerun(
    conn: &Connection,
    b: &RunBinding,
) -> Result<ScheduleOutcome, StoreError> {
    if let Some(old) = pending_run(conn)? {
        cancel_jobs_of(conn, old.id)?;
        drop_run(conn, old.id)?;
    }
    let active = active_run(conn)?.ok_or_else(|| invalid("There is nothing to re-run yet"))?;
    if active.identity == b.identity {
        return Err(invalid(format!(
            "The current results already come from {}",
            active.model_name
        )));
    }
    let documents: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT s.document_id, w.content_id
             FROM work_item w
             JOIN job j ON j.id = w.job_id
             JOIN segment s ON s.id = w.segment_first
             WHERE j.ai_run_id = ?1 AND w.state IN (?2, ?3, ?4, ?5) AND w.content_id IS NOT NULL
             ORDER BY s.document_id",
        )?;
        stmt.query_map(
            params![
                active.id,
                ItemState::Pending.as_str(),
                ItemState::InProgress.as_str(),
                ItemState::Done.as_str(),
                ItemState::Failed.as_str()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<Result<_, _>>()?
    };
    cancel_jobs_of(conn, active.id)?;
    let run = insert_run(conn, b, RunState::Pending)?;
    let job = create_job(conn, Trigger::ModelRerun, Stage::B)?;
    conn.prepare_cached("UPDATE job SET ai_run_id = ?2 WHERE id = ?1")?
        .execute(params![job, run])?;
    let mut units = 0;
    for (document, content) in documents {
        let plan = uncovered_units(conn, document, Some(run), &UnitScope::All)?;
        add_units(conn, job, content, &plan)?;
        units += plan.len() as u64;
    }
    if units == 0 {
        // Nothing left of what the old run read (all removed since). Nothing to switch to.
        set_job_state(conn, job, JobState::Queued, JobState::Cancelled)?;
        drop_run(conn, run)?;
        return Err(invalid("There is nothing to re-run yet"));
    }
    Ok(ScheduleOutcome {
        job: Some(job),
        units,
    })
}

/// Open Stage B jobs of `run`: Pending and In-progress items Cancelled, the job Cancelled.
fn cancel_jobs_of(conn: &Connection, run: i64) -> Result<(), StoreError> {
    let jobs: Vec<(i64, String)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, state FROM job WHERE ai_run_id = ?1 AND state IN (?2, ?3, ?4, ?5)",
        )?;
        stmt.query_map(
            params![
                run,
                JobState::Queued.as_str(),
                JobState::Running.as_str(),
                JobState::Paused.as_str(),
                JobState::Blocked.as_str()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<Result<_, _>>()?
    };
    for (job, state) in jobs {
        let from =
            JobState::from_name(&state).ok_or_else(|| invalid(format!("unknown state {state}")))?;
        conn.prepare_cached(
            "UPDATE work_item SET state = ?2, finished_at = ?3
             WHERE job_id = ?1 AND state IN (?4, ?5)",
        )?
        .execute(params![
            job,
            ItemState::Cancelled.as_str(),
            now_ms(),
            ItemState::Pending.as_str(),
            ItemState::InProgress.as_str()
        ])?;
        set_job_state(conn, job, from, JobState::Cancelled)?;
    }
    Ok(())
}

/// A pending run and its output go; nothing visible changes (ADR-0005). Its jobs stay in the
/// history, without a run.
/// Its pending mentions go with it (`ON DELETE CASCADE`).
fn drop_run(conn: &Connection, run: i64) -> Result<(), StoreError> {
    conn.prepare_cached("DELETE FROM ai_run WHERE id = ?1")?
        .execute([run])?;
    Ok(())
}

/// Drops a pending run whose jobs were cancelled with the whole queue (US-03.3).
pub(crate) fn drop_pending_runs(conn: &Connection) -> Result<(), StoreError> {
    if let Some(run) = pending_run(conn)? {
        drop_run(conn, run.id)?;
    }
    Ok(())
}

/// What "Cancel" ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiCancelled {
    /// A re-run: its output is gone and nothing visible changed.
    Rerun,
    /// A first run's remaining work: what it found stays.
    Run,
    Nothing,
}

/// Cancels the re-run under way, or else the current run's remaining work.
pub(crate) fn cancel_ai_run(conn: &Connection) -> Result<AiCancelled, StoreError> {
    if let Some(run) = pending_run(conn)? {
        cancel_jobs_of(conn, run.id)?;
        drop_run(conn, run.id)?;
        return Ok(AiCancelled::Rerun);
    }
    let open_unbound: Vec<(i64, String)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, state FROM job WHERE stage = ?1 AND ai_run_id IS NULL
             AND state IN (?2, ?3, ?4, ?5)",
        )?;
        stmt.query_map(
            params![
                STAGE,
                JobState::Queued.as_str(),
                JobState::Running.as_str(),
                JobState::Paused.as_str(),
                JobState::Blocked.as_str()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<Result<_, _>>()?
    };
    let mut any = !open_unbound.is_empty();
    for (job, state) in open_unbound {
        let from =
            JobState::from_name(&state).ok_or_else(|| invalid(format!("unknown state {state}")))?;
        conn.prepare_cached(
            "UPDATE work_item SET state = ?2 WHERE job_id = ?1 AND state IN (?3, ?4)",
        )?
        .execute(params![
            job,
            ItemState::Cancelled.as_str(),
            ItemState::Pending.as_str(),
            ItemState::InProgress.as_str()
        ])?;
        set_job_state(conn, job, from, JobState::Cancelled)?;
    }
    if let Some(run) = active_run(conn)? {
        let open: i64 = conn
            .prepare_cached(
                "SELECT COUNT(*) FROM job WHERE ai_run_id = ?1 AND state IN (?2, ?3, ?4, ?5)",
            )?
            .query_row(
                params![
                    run.id,
                    JobState::Queued.as_str(),
                    JobState::Running.as_str(),
                    JobState::Paused.as_str(),
                    JobState::Blocked.as_str()
                ],
                |r| r.get(0),
            )?;
        any |= open > 0;
        cancel_jobs_of(conn, run.id)?;
    }
    set_meta(conn, WAITING_UNITS, None)?;
    Ok(if any {
        AiCancelled::Run
    } else {
        AiCancelled::Nothing
    })
}

/// A job ended. When it was the last open job of a pending run, the run is done: it switches, or,
/// if no item of it finished (all failed or skipped), it goes and the old results stay.
pub(crate) fn after_job_ended(conn: &Connection, job: i64) -> Result<(), StoreError> {
    let run: Option<i64> = conn
        .prepare_cached("SELECT ai_run_id FROM job WHERE id = ?1")?
        .query_row([job], |r| r.get(0))?;
    let Some(run) = run else {
        return Ok(());
    };
    let state: String = conn
        .prepare_cached("SELECT state FROM ai_run WHERE id = ?1")?
        .query_row([run], |r| r.get(0))?;
    if state != RunState::Pending.as_str() {
        return Ok(());
    }
    let (open, done): (i64, i64) = conn
        .prepare_cached(
            "SELECT
               (SELECT COUNT(*) FROM job WHERE ai_run_id = ?1 AND state IN (?2, ?3, ?4, ?5)),
               (SELECT COUNT(*) FROM work_item w JOIN job j ON j.id = w.job_id
                WHERE j.ai_run_id = ?1 AND w.state = ?6)",
        )?
        .query_row(
            params![
                run,
                JobState::Queued.as_str(),
                JobState::Running.as_str(),
                JobState::Paused.as_str(),
                JobState::Blocked.as_str(),
                ItemState::Done.as_str()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    if open > 0 {
        return Ok(());
    }
    if done == 0 {
        return drop_run(conn, run);
    }
    switch_run(conn, run)
}

/// The switch (ADR-0005): in one transaction, the visible AI mentions are replaced by the run's,
/// the old run is superseded, and every derived table is rebuilt with the decisions (INV-AI-04,
/// INV-CU-03). A reader sees all-old or all-new.
fn switch_run(conn: &Connection, run: i64) -> Result<(), StoreError> {
    let now = now_iso();
    conn.execute("DELETE FROM entity_mention WHERE producer = 'model'", [])?;
    conn.execute(
        "INSERT INTO entity_mention (segment_id, document_id, content_id, name_key, entity_type,
             raw, span_start, span_end, column_key, record, confidence, producer, ai_run_id)
         SELECT segment_id, document_id, content_id, name_key, entity_type, raw, span_start,
                span_end, '', '', confidence, 'model', ai_run_id
         FROM pending_entity_mention WHERE ai_run_id = ?1 ORDER BY id",
        [run],
    )?;
    conn.execute(
        "DELETE FROM pending_entity_mention WHERE ai_run_id = ?1",
        [run],
    )?;
    conn.execute(
        "UPDATE ai_run SET state = ?1, finished_at = ?2 WHERE stage = ?3 AND state = ?4",
        params![
            RunState::Superseded.as_str(),
            now,
            STAGE,
            RunState::Active.as_str()
        ],
    )?;
    conn.execute(
        "UPDATE ai_run SET state = ?1, finished_at = ?2 WHERE id = ?3",
        params![RunState::Active.as_str(), now, run],
    )?;
    rebuild_all(conn)
}

/// Stage B for the panel: the runs and the current run's items.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct AiProgress {
    pub active: Option<AiRunRow>,
    pub pending: Option<AiRunRow>,
    /// Items of the current run's jobs (or of jobs still without a run).
    pub open: u64,
    pub in_progress: u64,
    pub done: u64,
    pub failed: u64,
    /// The most telling state among its open jobs: `blocked`, `paused`, `running` or `queued`.
    pub job_state: Option<String>,
    pub blocked_reason: Option<String>,
    /// Units over the start rule's limit, waiting for the user's pick (gap A3).
    pub waiting_units: u64,
}

pub(crate) fn ai_progress(conn: &Connection) -> Result<AiProgress, StoreError> {
    let active = active_run(conn)?;
    let pending = pending_run(conn)?;
    let run = pending.as_ref().or(active.as_ref()).map(|r| r.id);
    let mut out = AiProgress {
        waiting_units: meta(conn, WAITING_UNITS)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        ..AiProgress::default()
    };
    {
        let mut stmt = conn.prepare_cached(
            "SELECT w.state, COUNT(*) FROM job j JOIN work_item w ON w.job_id = j.id
             WHERE j.stage = ?1 AND j.ai_run_id IS ?2 GROUP BY w.state",
        )?;
        let rows = stmt.query_map(params![STAGE, run], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        for row in rows {
            let (state, n) = row?;
            match ItemState::from_name(&state) {
                Some(ItemState::Pending) => out.open += n,
                Some(ItemState::InProgress) => {
                    out.open += n;
                    out.in_progress = n;
                }
                Some(ItemState::Done) => out.done = n,
                Some(ItemState::Failed) => out.failed = n,
                _ => {}
            }
        }
    }
    let jobs: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT state, blocked_reason FROM job
             WHERE stage = ?1 AND ai_run_id IS ?2 AND state IN (?3, ?4, ?5, ?6)",
        )?;
        stmt.query_map(
            params![
                STAGE,
                run,
                JobState::Queued.as_str(),
                JobState::Running.as_str(),
                JobState::Paused.as_str(),
                JobState::Blocked.as_str()
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<Result<_, _>>()?
    };
    for state in [
        JobState::Blocked,
        JobState::Paused,
        JobState::Running,
        JobState::Queued,
    ] {
        if let Some((s, reason)) = jobs.iter().find(|(s, _)| s == state.as_str()) {
            out.job_state = Some(s.clone());
            out.blocked_reason = reason.clone();
            break;
        }
    }
    out.active = active;
    out.pending = pending;
    Ok(out)
}

/// Open Stage B jobs with the identity of their run (`None`: no run bound yet) and state.
pub(crate) fn open_ai_jobs(
    conn: &Connection,
) -> Result<Vec<(i64, Option<RunIdentity>, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT j.id, a.model_identity, j.state FROM job j
         LEFT JOIN ai_run a ON a.id = j.ai_run_id
         WHERE j.stage = ?1 AND j.state IN (?2, ?3, ?4, ?5) ORDER BY j.id",
    )?;
    let rows = stmt.query_map(
        params![
            STAGE,
            JobState::Queued.as_str(),
            JobState::Running.as_str(),
            JobState::Paused.as_str(),
            JobState::Blocked.as_str()
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?
                    .map(|s| RunIdentity::from_stored(&s)),
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Blocks one AI job with `reason` (see `manage::block_jobs`); runs it again with `None`.
pub(crate) fn set_ai_job_block(
    conn: &Connection,
    job: i64,
    reason: Option<&str>,
) -> Result<bool, StoreError> {
    let (state, old): (String, Option<String>) = conn
        .prepare_cached("SELECT state, blocked_reason FROM job WHERE id = ?1")?
        .query_row([job], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let state =
        JobState::from_name(&state).ok_or_else(|| invalid(format!("unknown state {state}")))?;
    match (state, reason) {
        (JobState::Queued | JobState::Running, Some(reason)) => {
            conn.prepare_cached("UPDATE job SET state = ?2, blocked_reason = ?3 WHERE id = ?1")?
                .execute(params![job, JobState::Blocked.as_str(), reason])?;
            Ok(true)
        }
        (JobState::Blocked, Some(reason)) if old.as_deref() != Some(reason) => {
            conn.prepare_cached("UPDATE job SET blocked_reason = ?2 WHERE id = ?1")?
                .execute(params![job, reason])?;
            Ok(true)
        }
        (JobState::Blocked, None) => {
            state
                .move_to(JobState::Running)
                .map_err(|e| invalid(e.to_string()))?;
            conn.prepare_cached("UPDATE job SET state = ?2, blocked_reason = NULL WHERE id = ?1")?
                .execute(params![job, JobState::Running.as_str()])?;
            complete_job_if_done(conn, job)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Whether the lane would start an item now.
pub(crate) fn has_runnable_ai_work(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM work_item w JOIN job j ON j.id = w.job_id
                            WHERE w.state = ?1 AND j.stage = ?2 AND j.state IN (?3, ?4))",
        )?
        .query_row(
            params![
                ItemState::Pending.as_str(),
                STAGE,
                JobState::Queued.as_str(),
                JobState::Running.as_str()
            ],
            |r| r.get(0),
        )?)
}

/// What the kill tests look for (NFR-QA-11, ADR-0005).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AiHealth {
    /// AI mentions whose span does not hold their name in the segment's text.
    pub invented_evidence: u64,
    /// AI mentions outside the one active run, pending mentions outside the one pending run.
    pub mixed_runs: u64,
}

pub(crate) fn ai_health(conn: &Connection) -> Result<AiHealth, StoreError> {
    let mut out = AiHealth::default();
    let mut stmt = conn.prepare(
        "SELECT s.text, m.span_start, m.span_end, m.raw
         FROM entity_mention m JOIN segment s ON s.id = m.segment_id
         WHERE m.producer = 'model'
         UNION ALL
         SELECT s.text, m.span_start, m.span_end, m.raw
         FROM pending_entity_mention m JOIN segment s ON s.id = m.segment_id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let text = r.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
        let span = (r.get::<_, i64>(1)? as usize, r.get::<_, i64>(2)? as usize);
        let raw = r.get_ref(3)?.as_str().map_err(rusqlite::Error::from)?;
        if !mk_ai::verify_mention(text, span, raw) {
            out.invented_evidence += 1;
        }
    }
    let mixed: i64 = conn.query_row(
        "SELECT
           (SELECT COUNT(*) FROM entity_mention m
            WHERE m.producer = 'model' AND m.ai_run_id IS NOT
                  (SELECT id FROM ai_run WHERE state = 'active'))
         + (SELECT COUNT(*) FROM pending_entity_mention m
            WHERE m.ai_run_id IS NOT (SELECT id FROM ai_run WHERE state = 'pending'))",
        [],
        |r| r.get(0),
    )?;
    out.mixed_runs = mixed as u64;
    Ok(out)
}
