//! The embeddings lane (M6b, BC-10, ADR-0005, ADR-0006, ADR-0010): every window's vector, then the
//! index meaning search reads.
//!
//! - [`Workspace::schedule_embeddings`]: the items not covered yet, then the start rule: at most
//!   [`mk_jobs::EMBEDDING_AUTO_WINDOWS`] windows start by themselves, more wait for a click.
//! - [`Workspace::run_embeddings`]: one request in flight (ADR-0006): next item → identity check →
//!   vectors → commit. Like Stage B it holds no `Project.run`: pause and cancel move the item out of
//!   In progress, and vectors that come later fail its token (NFR-REL-05).
//! - Failures as Stage B: a network error puts the item back with a backoff, three in a row Block
//!   the job; a refusal Blocks at once; an HTTP error or an unreadable reply fails that item alone.
//! - A re-run's vectors wait beside the active run's and the switch is in its last commit, so a
//!   reader sees one model or the other (INV-SR-02). Meaning search is not ready during a re-run: a
//!   query must use the vectors' own model (US-06.2 AC2).
//! - The index is made after the lane ends, off the writer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mk_ai::RunIdentity;
use mk_egress::{DestinationClass, ErrorCode, GateError};
use mk_jobs::{Stage, Trigger, embedding_estimate};
use mk_models::Role;
use mk_search::meaning::{ITEM_WINDOWS, encode_f16, normalize};
use mk_store::{
    AiCancelled, AiProgress, EmbeddingRow, ScheduleOutcome, StartedEmbeddingItem, StoreError,
    UnitScope,
};

use crate::ai::{AiActivity, AiClient};
use crate::manage::Eta;
use crate::project::{Project, ProjectError};
use crate::stage_b::{CallFailure, MAX_NETWORK_FAILURES, backoff_wait, classify};
use crate::workspace::Workspace;

/// A project's embeddings lane.
#[derive(Default)]
pub(crate) struct EmbeddingLane {
    busy: AtomicBool,
    /// A start came while the lane ran: it looks for work once more before it ends.
    again: AtomicBool,
    stopped: AtomicBool,
    /// When this lane started, and the items finished before it.
    started: Mutex<Option<(Instant, u64)>>,
}

impl EmbeddingLane {
    /// The project is closing: no new item starts and no index is made.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// What a lane run did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct EmbeddingSummary {
    pub done: u64,
    pub failed: u64,
    pub windows: u64,
    /// Vectors that came after their item left In progress (pause, cancel, close).
    pub dropped_replies: u64,
    /// The reason, when the lane stopped because the job is Blocked.
    pub blocked: Option<String>,
}

/// A number of windows and how long they would take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WindowEstimate {
    pub windows: u64,
    pub estimate: Duration,
}

impl WindowEstimate {
    fn of(windows: u64) -> WindowEstimate {
        WindowEstimate {
            windows,
            estimate: embedding_estimate(windows),
        }
    }
}

/// What meaning search can do now (US-06.2 AC2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeaningState {
    /// Nothing has run.
    Idle,
    /// More windows than the start rule allows: the user starts it.
    WaitingForStart,
    Running,
    Paused,
    Blocked,
    /// The vectors come from another model than the one chosen.
    ModelChanged,
    /// The vectors are done and the index is being made.
    Preparing,
    Ready,
}

impl MeaningState {
    pub fn as_str(self) -> &'static str {
        match self {
            MeaningState::Idle => "idle",
            MeaningState::WaitingForStart => "waiting_for_start",
            MeaningState::Running => "running",
            MeaningState::Paused => "paused",
            MeaningState::Blocked => "blocked",
            MeaningState::ModelChanged => "model_changed",
            MeaningState::Preparing => "preparing",
            MeaningState::Ready => "ready",
        }
    }
}

/// The Meaning tab's status and the progress panel's row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MeaningView {
    pub state: MeaningState,
    /// Why a query cannot run now, as the Meaning tab says it. `None` when it can.
    pub reason: Option<String>,
    /// Items finished and all items of the run the numbers are about.
    pub items_done: u64,
    pub items_total: u64,
    pub items_failed: u64,
    pub eta: Eta,
    /// Where its data goes while it runs, and where a query goes (US-06.2 AC3).
    pub destination: Option<String>,
    /// The model of the vectors a query would search.
    pub vectors_model: Option<String>,
    /// The model of a re-run under way.
    pub rerun_model: Option<String>,
    /// "Re-run with Y": the model now chosen, when it is not the one behind the vectors.
    pub rerun_with: Option<String>,
    /// Windows over the start rule's limit, waiting for the user's start.
    pub waiting: Option<WindowEstimate>,
}

/// What the tab says when the vectors are not the chosen model's.
pub(crate) fn model_changed(vectors: &str, chosen: &str) -> String {
    format!("Vectors come from {vectors}. Re-run with {chosen}, or choose {vectors} again.")
}

fn percent(p: &AiProgress) -> u64 {
    let done = p.done + p.failed;
    (done * 100).checked_div(done + p.open).unwrap_or(0)
}

/// One item through the model, in as many requests as it takes. `Ok(None)`: the item left In
/// progress between two requests, so nothing more of it is sent (NFR-REL-05).
fn embed_item(
    project: &Project,
    client: &AiClient<'_>,
    item: &StartedEmbeddingItem,
    prefix: &str,
) -> Result<Option<Vec<EmbeddingRow>>, CallFailure> {
    let mut rows = Vec::with_capacity(item.windows.len());
    for (n, part) in item.windows.chunks(ITEM_WINDOWS).enumerate() {
        if n > 0
            && !project
                .store()
                .ai_item_token_holds(item.item, item.attempt)
                .unwrap_or(false)
        {
            return Ok(None);
        }
        let inputs: Vec<String> = part.iter().map(|w| format!("{prefix}{}", w.text)).collect();
        let reply = client.client.embeddings(&inputs).map_err(|e| match &e {
            GateError::Failed(f) if f.code == ErrorCode::BadReply => {
                CallFailure::Item(f.code.as_str().to_string(), f.message.clone())
            }
            _ => classify(e),
        })?;
        client.succeeded();
        // The gate has refused a reply with any other number of vectors than texts.
        for (w, mut vector) in part.iter().zip(reply.vectors) {
            normalize(&mut vector);
            rows.push(EmbeddingRow {
                segment: w.segment,
                char_start: w.char_start,
                char_end: w.char_end,
                vector: encode_f16(&vector),
            });
        }
    }
    Ok(Some(rows))
}

impl Workspace {
    /// The items not covered yet, then the start rule (decided in M6b planning): at most
    /// [`mk_jobs::EMBEDDING_AUTO_WINDOWS`] get a job, more wait for a click and nothing starts. A
    /// model's absence or change Blocks the new job with a reason.
    pub fn schedule_embeddings(&self, project: &Project) -> Result<ScheduleOutcome, ProjectError> {
        let store = project.store();
        let windows = store.count_embedding_windows(&UnitScope::Auto)?;
        if windows > mk_jobs::EMBEDDING_AUTO_WINDOWS {
            store.set_embedding_waiting(windows)?;
            return Ok(ScheduleOutcome::default());
        }
        // Nothing to plan: no second scan on the writer, whose large page cache would stay filled
        // on a project that only opens (64 MiB on TD-2, NFR-RES-01).
        let out = if windows == 0 {
            store.set_embedding_waiting(0)?;
            ScheduleOutcome::default()
        } else {
            store.schedule_embeddings(UnitScope::Auto, Trigger::Add)?
        };
        self.refresh_ai_blocks(project)?;
        Ok(out)
    }

    /// How many windows wait to be embedded and how long they would take, to show before a start.
    pub fn embedding_estimate(&self, project: &Project) -> Result<WindowEstimate, ProjectError> {
        Ok(WindowEstimate::of(
            project.store().count_embedding_windows(&UnitScope::All)?,
        ))
    }

    /// The user's start: everything not covered yet, cancelled and failed items too. The caller
    /// starts [`run_embeddings`](Self::run_embeddings).
    pub fn start_embeddings(&self, project: &Project) -> Result<ScheduleOutcome, ProjectError> {
        let out = project
            .store()
            .schedule_embeddings(UnitScope::All, Trigger::Add)?;
        self.refresh_ai_blocks(project)?;
        Ok(out)
    }

    /// "Re-run with Y": a pending run with the model now chosen over what the vectors covered. The
    /// old vectors stay until it is done and switches (ADR-0005). The caller starts
    /// [`run_embeddings`](Self::run_embeddings).
    pub fn start_embedding_rerun(&self, project: &Project) -> Result<WindowEstimate, ProjectError> {
        let target = self
            .ai_target(project, Role::Embeddings)
            .map_err(ProjectError::Invalid)?;
        let (_, windows) = project
            .store()
            .start_embedding_rerun(target)
            .map_err(|e| match e {
                StoreError::Invalid(m) => ProjectError::Invalid(m),
                e => ProjectError::Store(e),
            })?;
        self.refresh_ai_blocks(project)?;
        Ok(WindowEstimate::of(windows))
    }

    /// Cancels the re-run under way (the vectors stay as they are), or else the run's remaining
    /// work (what it made stays).
    pub fn cancel_embeddings(&self, project: &Project) -> Result<AiCancelled, ProjectError> {
        let out = project.store().cancel_embedding_run()?;
        self.refresh_ai_blocks(project)?;
        Ok(out)
    }

    /// Runs the lane until nothing may start, then makes the index if the vectors are complete. A
    /// second call while it runs makes it look for work once more and returns at once.
    pub fn run_embeddings(&self, project: &Project) -> Result<EmbeddingSummary, ProjectError> {
        let lane = project.embedding_lane();
        if lane.busy.swap(true, Ordering::SeqCst) {
            lane.again.store(true, Ordering::SeqCst);
            return Ok(EmbeddingSummary::default());
        }
        let done_before = project
            .store()
            .embedding_progress()
            .map_or(0, |p| p.done + p.failed);
        *lane.started.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((Instant::now(), done_before));
        let mut summary = EmbeddingSummary::default();
        let result = self.embedding_loop(project, &mut summary).and_then(|()| {
            let open = project.store().embedding_progress()?.open;
            if open == 0 && !lane.is_stopped() {
                project.prepare_meaning_index()?;
            }
            Ok(())
        });
        *lane.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        lane.busy.store(false, Ordering::SeqCst);
        result?;
        Ok(summary)
    }

    fn embedding_loop(
        &self,
        project: &Project,
        summary: &mut EmbeddingSummary,
    ) -> Result<(), ProjectError> {
        let lane = project.embedding_lane();
        let store = project.store();
        loop {
            lane.again.store(false, Ordering::SeqCst);
            if lane.is_stopped() {
                return Ok(());
            }
            self.refresh_ai_blocks(project)?;
            let target = match self.ai_target(project, Role::Embeddings) {
                Ok(t) => t,
                // The refresh Blocked what could run, with this reason.
                Err(reason) => {
                    if store.embedding_progress()?.open > 0 {
                        summary.blocked = Some(reason);
                    }
                    return Ok(());
                }
            };
            if !store.has_runnable_embedding_work()? {
                let p = store.embedding_progress()?;
                if p.job_state.as_deref() == Some("blocked") {
                    summary.blocked = p.blocked_reason;
                }
                if lane.again.load(Ordering::SeqCst) {
                    continue;
                }
                return Ok(());
            }
            let identity = target.identity.clone();
            store.bind_embedding_run(target)?;
            if self.embedding_slot(project, &identity, summary)? {
                return Ok(());
            }
        }
    }

    fn block_embeddings(
        &self,
        project: &Project,
        summary: &mut EmbeddingSummary,
        item: Option<&StartedEmbeddingItem>,
        reason: String,
    ) -> Result<bool, StoreError> {
        if let Some(item) = item {
            project.store().release_ai_item(item.item, item.attempt)?;
        }
        project
            .store()
            .block_ai_jobs(&[Stage::Embeddings], &reason)?;
        summary.blocked = Some(reason);
        Ok(true)
    }

    /// The lane's one request in flight. True when the lane should end.
    fn embedding_slot(
        &self,
        project: &Project,
        identity: &RunIdentity,
        summary: &mut EmbeddingSummary,
    ) -> Result<bool, StoreError> {
        let store = project.store();
        let lane = project.embedding_lane();
        let (_, prefix) = self.embedding_prefixes();
        let mut client: Option<AiClient<'_>> = None;
        let mut failures = 0u32;
        loop {
            if lane.is_stopped() {
                return Ok(true);
            }
            if client.is_none() {
                match self.ai_client(project, Role::Embeddings) {
                    Ok(c) => client = Some(c),
                    Err(e) if e.blocked => {
                        // `ai_client` Blocked the job with this reason (NFR-REL-12).
                        summary.blocked = Some(e.message);
                        return Ok(true);
                    }
                    Err(e) => {
                        failures += 1;
                        if failures >= MAX_NETWORK_FAILURES {
                            return self.block_embeddings(project, summary, None, e.message);
                        }
                        if !backoff_wait(failures, || lane.is_stopped()) {
                            return Ok(true);
                        }
                        continue;
                    }
                }
            }
            let Some(item) = store.start_next_embedding_item()? else {
                return Ok(false);
            };
            // Run identity on every item (ADR-0005): two models' vectors never mix (INV-SR-02).
            let now = self.ai_target(project, Role::Embeddings);
            if now.as_ref().map(|t| &t.identity) != Ok(&item.identity) || *identity != item.identity
            {
                store.release_ai_item(item.item, item.attempt)?;
                // The loop's refresh Blocks it, or a new look takes the new model's client.
                return Ok(false);
            }
            let c = client.as_ref().expect("made above");
            match embed_item(project, c, &item, &prefix) {
                Ok(Some(rows)) => {
                    failures = 0;
                    let windows = rows.len() as u64;
                    match store.commit_embedding_item(item.item, item.attempt, rows) {
                        Ok(true) => {
                            summary.done += 1;
                            summary.windows += windows;
                        }
                        Ok(false) => summary.dropped_replies += 1,
                        // The commit rolled back. Record why on the item, so it does not come back.
                        Err(StoreError::Invalid(reason)) => {
                            store.fail_ai_item(
                                item.item,
                                item.attempt,
                                "internal_error",
                                &reason,
                            )?;
                            summary.failed += 1;
                        }
                        Err(e) => return Err(e),
                    }
                }
                Ok(None) => summary.dropped_replies += 1,
                Err(CallFailure::Refused(reason)) => {
                    return self.block_embeddings(project, summary, Some(&item), reason);
                }
                Err(CallFailure::Item(code, reason)) => {
                    failures = 0;
                    if store.fail_ai_item(item.item, item.attempt, &code, &reason)? {
                        summary.failed += 1;
                    } else {
                        summary.dropped_replies += 1;
                    }
                }
                Err(CallFailure::Network(reason)) => {
                    failures += 1;
                    if failures >= MAX_NETWORK_FAILURES {
                        return self.block_embeddings(project, summary, Some(&item), reason);
                    }
                    store.release_ai_item(item.item, item.attempt)?;
                    // The local runtime may be restarting on a new port and key.
                    client = None;
                    if !backoff_wait(failures, || lane.is_stopped()) {
                        return Ok(true);
                    }
                }
            }
        }
    }

    /// The Meaning tab's status and the progress panel's row.
    pub fn meaning_view(&self, project: &Project) -> Result<MeaningView, ProjectError> {
        let p = project.store().embedding_progress()?;
        let target = self.ai_target(project, Role::Embeddings);
        let chosen = target.as_ref().ok();
        // A run only has something to search once it has a vector.
        let vectors = p.active.as_ref().filter(|r| r.dims.is_some());
        let changed = vectors.filter(|r| {
            chosen.is_some_and(|t| t.identity != r.identity)
                && p.pending
                    .as_ref()
                    .is_none_or(|r| chosen.is_some_and(|t| t.identity != r.identity))
        });
        let at = percent(&p);
        let (state, reason) = match (p.job_state.as_deref(), &target) {
            (Some("blocked"), _) => (MeaningState::Blocked, p.blocked_reason.clone()),
            (Some("paused"), _) => (
                MeaningState::Paused,
                Some(format!("Embeddings paused at {at}%")),
            ),
            (_, Err(reason)) => (MeaningState::Blocked, Some(reason.clone())),
            (Some(_), _) => (
                MeaningState::Running,
                Some(match &p.pending {
                    Some(r) => format!(
                        "Re-running embeddings with {}: {at}% done. Meaning search comes back when \
                         it is done.",
                        r.model_name
                    ),
                    None => format!("Embeddings: {at}% done"),
                }),
            ),
            (None, Ok(t)) => match vectors {
                Some(r) if changed.is_some() => (
                    MeaningState::ModelChanged,
                    Some(model_changed(&r.model_name, &t.model_name)),
                ),
                Some(_) if project.meaning_index_current()? => (MeaningState::Ready, None),
                Some(_) => (
                    MeaningState::Preparing,
                    Some("Preparing meaning search…".into()),
                ),
                None if p.waiting_units > 0 => (
                    MeaningState::WaitingForStart,
                    Some(format!(
                        "Embeddings have not started: about {} min of work",
                        embedding_estimate(p.waiting_units).as_secs().div_ceil(60)
                    )),
                ),
                None => (
                    MeaningState::Idle,
                    Some("Embeddings have not run yet".to_string()),
                ),
            },
        };
        let finished = p.done + p.failed;
        let lane = project.embedding_lane();
        let eta = match (
            state,
            *lane.started.lock().unwrap_or_else(|e| e.into_inner()),
        ) {
            (MeaningState::Running, Some((at, before))) => {
                let done = finished.saturating_sub(before);
                match mk_jobs::estimate(done, done, p.open, at.elapsed()) {
                    Some(left) => Eta::Left(left),
                    None => Eta::Estimating,
                }
            }
            _ => Eta::None,
        };
        let current = p.pending.as_ref().or(p.active.as_ref());
        let destination = current.map(|r| {
            let class =
                DestinationClass::parse(&r.destination_class).unwrap_or(DestinationClass::Internet);
            AiActivity::test(&r.destination_host, class).label()
        });
        Ok(MeaningView {
            state,
            reason,
            items_done: finished,
            items_total: finished + p.open,
            items_failed: p.failed,
            eta,
            destination,
            vectors_model: vectors.map(|r| r.model_name.clone()),
            rerun_model: p.pending.as_ref().map(|r| r.model_name.clone()),
            rerun_with: changed.and(chosen).map(|t| t.model_name.clone()),
            waiting: (p.waiting_units > 0).then(|| WindowEstimate::of(p.waiting_units)),
        })
    }
}
