//! The Stage B lane (M6a, BC-06, ADR-0005, ADR-0006): people and organizations with checked
//! evidence.
//!
//! - [`Workspace::schedule_stage_b`]: the units not covered yet, then the start rule: at most
//!   [`mk_jobs::STAGE_B_AUTO_UNITS`] start by themselves, more wait for the user's pick (gap A3).
//! - [`Workspace::run_stage_b`]: 4 slots on the local model, 1 on an endpoint (NFR-PERF-25), each
//!   with its own client through the gate: next item → identity check → call → checks → commit.
//! - The lane holds no `Project.run`, so pause and cancel never wait on a model: they move the
//!   items out of In progress, and a reply that comes later fails its token in the store
//!   (NFR-REL-05).
//! - Failures (ADR-0006): a network error puts the item back and tries again with a backoff, three
//!   in a row Block the job; a refusal (permission, key, certificate) Blocks at once; an HTTP error
//!   fails that item alone; an unreadable reply is retried once, then counted (INV-AI-03). The
//!   local runtime is acquired again after a crash, and its Failed state Blocks through
//!   [`Workspace::ai_client`] (NFR-REL-12).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use mk_ai::{
    IdentifierSpan, PROMPT_VERSION, Piece, RunIdentity, check_reply, parse_reply, retry_message,
};
use mk_egress::{ChatMessage, ChatRequest, DestinationClass, ErrorCode, GateError, ReplySchema};
use mk_jobs::{Stage, Trigger, stage_b_estimate, stage_b_limit};
use mk_models::Role;
use mk_store::{AiItemResult, RunBinding, ScheduleOutcome, StartedAiItem, StoreError, UnitScope};

use crate::ai::{AiChoice, AiClient, AiReadiness};
use crate::egress::refusal_text;
use crate::manage::Eta;
use crate::project::{Project, ProjectError};
use crate::workspace::Workspace;

/// Network errors in a row before the job is Blocked (ADR-0006).
pub(crate) const MAX_NETWORK_FAILURES: u32 = 3;
/// The first wait after a network error; it doubles.
const BACKOFF: Duration = Duration::from_millis(500);
/// How often a backoff looks at the stop request.
const STOP_POLL: Duration = Duration::from_millis(50);

/// `AiLane::active_run` before the first look.
pub(crate) const UNKNOWN_RUN: i64 = -1;

/// A project's Stage B lane.
pub(crate) struct AiLane {
    busy: AtomicBool,
    /// A start came while the lane ran: it looks for work once more before it ends.
    again: AtomicBool,
    stopped: AtomicBool,
    /// The last commit of a re-run is on its way: the switch is in it.
    applying: AtomicBool,
    /// When this lane started, and the items finished before it.
    started: Mutex<Option<(Instant, u64)>>,
    /// The active run at the last look, to tell when a switch happened.
    pub(crate) active_run: AtomicI64,
}

impl Default for AiLane {
    fn default() -> Self {
        AiLane {
            busy: AtomicBool::new(false),
            again: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            applying: AtomicBool::new(false),
            started: Mutex::new(None),
            active_run: AtomicI64::new(UNKNOWN_RUN),
        }
    }
}

impl AiLane {
    /// The project is closing: no new item starts.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// What a lane run did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StageBSummary {
    pub done: u64,
    pub failed: u64,
    pub mentions: u64,
    pub discarded: u64,
    pub identifier_dropped: u64,
    pub invalid_responses: u64,
    /// Replies that came after their item left In progress (pause, cancel, close).
    pub dropped_replies: u64,
    /// The reason, when the lane stopped because the job is Blocked.
    pub blocked: Option<String>,
}

/// A number of units and how long they would take (gap A3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnitEstimate {
    pub units: u64,
    pub estimate: Duration,
}

impl UnitEstimate {
    fn of(units: u64) -> UnitEstimate {
        UnitEstimate {
            units,
            estimate: stage_b_estimate(units),
        }
    }
}

/// What Stage B is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiLaneState {
    /// Nothing has run.
    Idle,
    /// More units than the start rule allows: the user picks (gap A3).
    WaitingForPick,
    Running,
    Paused,
    Blocked,
    /// "Applying new results…" (ADR-0005).
    Applying,
    Done,
}

impl AiLaneState {
    pub fn as_str(self) -> &'static str {
        match self {
            AiLaneState::Idle => "idle",
            AiLaneState::WaitingForPick => "waiting_for_pick",
            AiLaneState::Running => "running",
            AiLaneState::Paused => "paused",
            AiLaneState::Blocked => "blocked",
            AiLaneState::Applying => "applying",
            AiLaneState::Done => "done",
        }
    }
}

/// The panel's AI row and the Settings section (US-03.1, US-10.2 AC2, US-10.3, US-10.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AiExtractionView {
    /// The "Experimental" label (US-10.3).
    pub experimental: bool,
    pub state: AiLaneState,
    /// Why it is Blocked.
    pub reason: Option<String>,
    /// Units finished and all units of the run the numbers below are about.
    pub units_done: u64,
    pub units_total: u64,
    pub units_failed: u64,
    pub eta: Eta,
    /// Where its data goes while it runs (US-09.8 AC1).
    pub destination: Option<String>,
    /// The model behind the results on screen (US-10.4 AC1).
    pub results_model: Option<String>,
    /// The model of a re-run under way.
    pub rerun_model: Option<String>,
    /// "Re-run with Y": the model now chosen, when it is not the one behind the results.
    pub rerun_with: Option<String>,
    /// Over the run the numbers are about.
    pub mentions: u64,
    pub discarded: u64,
    pub invalid_responses: u64,
    pub identifier_dropped: u64,
    /// Units over the start rule's limit, waiting for the user's pick.
    pub waiting: Option<UnitEstimate>,
}

/// How a call failed, for what happens next.
pub(crate) enum CallFailure {
    /// The destination refuses this project now: the job is Blocked at once.
    Refused(String),
    /// Only this item: it fails, the rest goes on.
    Item(String, String),
    /// May pass: back to Pending, a backoff, then Blocked.
    Network(String),
}

pub(crate) fn classify(e: GateError) -> CallFailure {
    match &e {
        GateError::NotAllowed { .. } | GateError::NeedsReconfirmation { .. } => {
            CallFailure::Refused(refusal_text(&e))
        }
        GateError::Failed(f) => match f.code {
            ErrorCode::KeyRefused
            | ErrorCode::KeyMissing
            | ErrorCode::BadCertificate
            | ErrorCode::SecretStore => CallFailure::Refused(f.message.clone()),
            ErrorCode::HttpStatus => {
                CallFailure::Item(f.code.as_str().to_string(), f.message.clone())
            }
            _ => CallFailure::Network(f.message.clone()),
        },
    }
}

fn request(messages: Vec<ChatMessage>) -> ChatRequest {
    ChatRequest {
        messages,
        schema: Some(ReplySchema {
            name: mk_ai::SCHEMA_NAME.into(),
            schema: mk_ai::schema(),
        }),
        max_tokens: Some(mk_ai::MAX_TOKENS),
        temperature: Some(0.0),
        seed: Some(mk_ai::SEED),
    }
}

/// One unit through the model: the call, the strict parse with one retry that tells the error
/// (INV-AI-03), the evidence rules. Still unreadable after the retry: Done with no mentions and a
/// count.
fn extract(
    client: &AiClient<'_>,
    pieces: &[Piece],
    identifiers: &[IdentifierSpan],
) -> Result<AiItemResult, GateError> {
    let mut messages = vec![
        ChatMessage::system(mk_ai::SYSTEM_PROMPT),
        ChatMessage::user(mk_ai::user_message(&mk_ai::check::unit_text(pieces))),
    ];
    for _ in 0..2 {
        let (reply, error) = match client.client.chat(&request(messages.clone())) {
            Ok(reply) => match parse_reply(&reply.content) {
                Ok(predictions) => {
                    client.succeeded();
                    let out = check_reply(pieces, identifiers, &predictions);
                    return Ok(AiItemResult {
                        mentions: out.kept,
                        discarded: out.discarded,
                        identifier_dropped: out.identifier_dropped,
                        invalid_responses: 0,
                    });
                }
                Err(e) => (reply.content, e),
            },
            Err(GateError::Failed(f)) if f.code == ErrorCode::BadReply => {
                (String::new(), f.message)
            }
            Err(e) => return Err(e),
        };
        client.succeeded();
        messages.push(ChatMessage::assistant(reply));
        messages.push(ChatMessage::user(retry_message(&error)));
    }
    Ok(AiItemResult {
        invalid_responses: 1,
        ..AiItemResult::default()
    })
}

/// What the lane's slots share.
#[derive(Default)]
struct LaneRun {
    summary: Mutex<StageBSummary>,
    /// Set by a slot that Blocked the job: the others stop too.
    blocked: AtomicBool,
    fatal: Mutex<Option<StoreError>>,
}

impl LaneRun {
    fn summary(&self) -> std::sync::MutexGuard<'_, StageBSummary> {
        self.summary.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Waits out the backoff after `failures` network errors; false when `stop` says to stop instead.
pub(crate) fn backoff_wait(failures: u32, stop: impl Fn() -> bool) -> bool {
    let until = Instant::now() + BACKOFF * (1 << failures.saturating_sub(1).min(4));
    while Instant::now() < until {
        if stop() {
            return false;
        }
        std::thread::sleep(STOP_POLL);
    }
    true
}

fn block_text(reason: &str, model: &str) -> String {
    format!("These results came from {model}. {reason}")
}

impl Workspace {
    /// The run a new item of `role` would belong to now: the chosen model and where its data would
    /// go. `Err` is why it cannot run.
    pub(crate) fn ai_target(&self, project: &Project, role: Role) -> Result<RunBinding, String> {
        let (class, name) = match self.ai_readiness(project, role) {
            AiReadiness::Ready { class, name, .. } => (class, name),
            AiReadiness::Blocked { reason, .. } => return Err(reason),
        };
        let choice = self.ai_choice(role).map_err(|e| e.message)?.choice;
        let (identity, host) = match choice {
            Some(AiChoice::Local(id)) => (RunIdentity::local(&id), "127.0.0.1".to_string()),
            Some(AiChoice::Endpoint(id)) => {
                let list = self.project_endpoints(project).map_err(|e| e.message)?;
                let e = list
                    .into_iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| crate::ai::NO_MODEL_ALLOWED.to_string())?;
                let base = mk_egress::BaseUrl::parse(&e.base_url).map_err(|b| b.to_string())?;
                let origin = base.origin();
                (
                    RunIdentity::endpoint(&id, &e.model, &origin.key()),
                    origin.host.clone(),
                )
            }
            None => return Err(crate::ai::NO_MODEL_ALLOWED.to_string()),
        };
        Ok(RunBinding {
            identity,
            model_name: name,
            destination_host: host,
            destination_class: class.as_str().to_string(),
            prompt_version: match role {
                Role::Extraction => PROMPT_VERSION,
                Role::Embeddings => 0,
            },
        })
    }

    fn extraction_target(&self, project: &Project) -> Result<RunBinding, String> {
        self.ai_target(project, Role::Extraction)
    }

    /// A role's jobs wait while the chosen model is not the one of their run, so two models'
    /// output is never mixed: choosing it again runs them, a re-run replaces it (decided
    /// 2026-11-05). Returns the jobs changed.
    pub(crate) fn refresh_identity_blocks(
        &self,
        project: &Project,
        role: Role,
    ) -> Result<usize, StoreError> {
        let Ok(target) = self.ai_target(project, role) else {
            return Ok(0);
        };
        let store = project.store();
        let (pending, active, jobs) = match role {
            Role::Extraction => (
                store.pending_ai_run()?,
                store.active_ai_run()?,
                store.open_ai_jobs()?,
            ),
            Role::Embeddings => (
                store.pending_embedding_run()?,
                store.active_embedding_run()?,
                store.open_embedding_jobs()?,
            ),
        };
        let mut changed = 0;
        for (job, identity, _) in jobs {
            let reason =
                identity
                    .filter(|i| *i != target.identity)
                    .map(|i| match (&pending, &active) {
                        (Some(p), _) if p.identity == i => format!(
                            "This re-run uses {0}. Choose {0} again, or cancel the re-run.",
                            p.model_name
                        ),
                        (_, Some(a)) if role == Role::Embeddings => {
                            crate::embeddings::model_changed(&a.model_name, &target.model_name)
                        }
                        (_, Some(a)) => block_text(
                            &format!(
                                "New work waits so two models are never mixed: re-run with {}, or \
                             choose {} again.",
                                target.model_name, a.model_name
                            ),
                            &a.model_name,
                        ),
                        _ => "The model chosen for AI changed".to_string(),
                    });
            changed += usize::from(store.set_ai_job_block(job, reason.as_deref())?);
        }
        Ok(changed)
    }

    /// The units not covered yet, then the start rule (decided 2026-11-05): at most
    /// [`mk_jobs::STAGE_B_AUTO_UNITS`] get a job, more wait for the user's pick and nothing
    /// starts. Stage A's documents only (INV-PR-01). Returns the units scheduled. A model's
    /// absence or change Blocks the new job with a reason; nothing large starts by itself.
    pub fn schedule_stage_b(&self, project: &Project) -> Result<ScheduleOutcome, ProjectError> {
        let store = project.store();
        let units = store.count_ai_units(&UnitScope::Auto)?;
        if units > mk_jobs::STAGE_B_AUTO_UNITS {
            store.set_ai_waiting_units(units)?;
            return Ok(ScheduleOutcome::default());
        }
        // Nothing to plan: no second scan on the writer (see `schedule_embeddings`, NFR-RES-01).
        let out = if units == 0 {
            store.set_ai_waiting_units(0)?;
            ScheduleOutcome::default()
        } else {
            store.schedule_ai_units(UnitScope::Auto, Trigger::Add)?
        };
        self.refresh_ai_blocks(project)?;
        Ok(out)
    }

    /// How many units `scope` would make and how long they would take, to show before "all".
    pub fn stage_b_estimate(
        &self,
        project: &Project,
        scope: &UnitScope,
    ) -> Result<UnitEstimate, ProjectError> {
        Ok(UnitEstimate::of(project.store().count_ai_units(scope)?))
    }

    /// The user picked documents, or all (gap A3): a job over those units. Cancelled and failed
    /// ones run again. The caller starts [`run_stage_b`](Self::run_stage_b).
    pub fn run_documents(
        &self,
        project: &Project,
        scope: UnitScope,
    ) -> Result<ScheduleOutcome, ProjectError> {
        let store = project.store();
        let out = store.schedule_ai_units(scope, Trigger::Add)?;
        // What is left over the limit still waits for a pick.
        let left = store.count_ai_units(&UnitScope::Auto)?;
        if left > 0 {
            store.set_ai_waiting_units(left)?;
        }
        self.refresh_ai_blocks(project)?;
        Ok(out)
    }

    /// "Re-run with Y" (US-10.4): a pending run with the model now chosen over what the current
    /// results covered. Nothing visible changes until it is done and switches (ADR-0005). The
    /// caller starts [`run_stage_b`](Self::run_stage_b).
    pub fn start_rerun(&self, project: &Project) -> Result<UnitEstimate, ProjectError> {
        let target = self
            .extraction_target(project)
            .map_err(ProjectError::Invalid)?;
        let out = project
            .store()
            .start_ai_rerun(target)
            .map_err(|e| match e {
                StoreError::Invalid(m) => ProjectError::Invalid(m),
                e => ProjectError::Store(e),
            })?;
        self.refresh_ai_blocks(project)?;
        Ok(UnitEstimate::of(out.units))
    }

    /// Cancels the re-run under way (nothing visible changes), or else the run's remaining work
    /// (what it found stays).
    pub fn cancel_extraction(
        &self,
        project: &Project,
    ) -> Result<mk_store::AiCancelled, ProjectError> {
        let out = project.store().cancel_ai_run()?;
        // A job Blocked by a model change may have been the only thing keeping the choice's
        // blocks alive.
        self.refresh_ai_blocks(project)?;
        Ok(out)
    }

    /// Runs Stage B until nothing may start: pause, cancel, a Blocked job or the end of the work
    /// end it. A second call while it runs makes it look for work once more and returns at once.
    /// Layer 1 work goes on beside it (INV-PR-09).
    pub fn run_stage_b(&self, project: &Project) -> Result<StageBSummary, ProjectError> {
        let lane = project.ai_lane();
        if lane.busy.swap(true, Ordering::SeqCst) {
            lane.again.store(true, Ordering::SeqCst);
            return Ok(StageBSummary::default());
        }
        let done_before = project
            .store()
            .ai_progress()
            .map_or(0, |p| p.done + p.failed);
        *lane.started.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((Instant::now(), done_before));
        let run = LaneRun::default();
        let result = self.lane_loop(project, &run);
        *lane.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        lane.busy.store(false, Ordering::SeqCst);
        project.note_ai_switch();
        result?;
        Ok(run.summary.into_inner().unwrap_or_else(|e| e.into_inner()))
    }

    fn lane_loop(&self, project: &Project, run: &LaneRun) -> Result<(), ProjectError> {
        let lane = project.ai_lane();
        let store = project.store();
        loop {
            lane.again.store(false, Ordering::SeqCst);
            if lane.is_stopped() {
                return Ok(());
            }
            self.refresh_ai_blocks(project)?;
            let target = match self.extraction_target(project) {
                Ok(t) => t,
                // The refresh Blocked what could run, with this reason.
                Err(reason) => {
                    if store.ai_progress()?.open > 0 {
                        run.summary().blocked = Some(reason);
                    }
                    return Ok(());
                }
            };
            if !store.has_runnable_ai_work()? {
                if store.ai_progress()?.job_state.as_deref() == Some("blocked") {
                    run.summary().blocked = store.ai_progress()?.blocked_reason;
                }
                if lane.again.load(Ordering::SeqCst) {
                    continue;
                }
                return Ok(());
            }
            let slots = stage_b_limit(target.identity.is_local());
            let identity = target.identity.clone();
            store.bind_ai_run(target)?;
            std::thread::scope(|s| {
                for _ in 0..slots {
                    s.spawn(|| {
                        if let Err(e) = self.lane_slot(project, &identity, run) {
                            run.fatal
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .get_or_insert(e);
                            run.blocked.store(true, Ordering::SeqCst);
                        }
                    });
                }
            });
            if let Some(e) = run.fatal.lock().unwrap_or_else(|e| e.into_inner()).take() {
                return Err(e.into());
            }
            if run.blocked.load(Ordering::SeqCst) {
                return Ok(());
            }
        }
    }

    fn block_lane(
        &self,
        project: &Project,
        run: &LaneRun,
        item: Option<&StartedAiItem>,
        reason: String,
    ) -> Result<(), StoreError> {
        if let Some(item) = item {
            project.store().release_ai_item(item.item, item.attempt)?;
        }
        project.store().block_ai_jobs(&[Stage::B], &reason)?;
        run.summary().blocked = Some(reason);
        run.blocked.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Waits out a backoff; false when the lane should stop instead.
    fn back_off(&self, project: &Project, run: &LaneRun, failures: u32) -> bool {
        backoff_wait(failures, || {
            project.ai_lane().is_stopped() || run.blocked.load(Ordering::SeqCst)
        })
    }

    /// One slot of the lane, with its own client (and lease on the local model).
    fn lane_slot(
        &self,
        project: &Project,
        identity: &RunIdentity,
        run: &LaneRun,
    ) -> Result<(), StoreError> {
        let store = project.store();
        let mut client: Option<AiClient<'_>> = None;
        let mut failures = 0u32;
        loop {
            if project.ai_lane().is_stopped() || run.blocked.load(Ordering::SeqCst) {
                return Ok(());
            }
            if client.is_none() {
                match self.ai_client(project, Role::Extraction) {
                    Ok(c) => client = Some(c),
                    Err(e) if e.blocked => {
                        // `ai_client` Blocked the job with this reason (NFR-REL-12).
                        run.summary().blocked = Some(e.message);
                        run.blocked.store(true, Ordering::SeqCst);
                        return Ok(());
                    }
                    Err(e) => {
                        failures += 1;
                        if failures >= MAX_NETWORK_FAILURES {
                            return self.block_lane(project, run, None, e.message);
                        }
                        if !self.back_off(project, run, failures) {
                            return Ok(());
                        }
                        continue;
                    }
                }
            }
            let Some(item) = store.start_next_ai_item()? else {
                return Ok(());
            };
            // Run identity on every item (ADR-0005): the gate reads an endpoint's config again on
            // each call, so a model edited mid-run would otherwise mix silently.
            let now = self.extraction_target(project);
            if now.as_ref().map(|t| &t.identity) != Ok(&item.identity) || *identity != item.identity
            {
                store.release_ai_item(item.item, item.attempt)?;
                // The lane's refresh Blocks it, or a new lane loop takes the new model's client.
                return Ok(());
            }
            let c = client.as_ref().expect("made above");
            let result = extract(c, &item.pieces, &item.identifiers);
            match result {
                Ok(result) => {
                    failures = 0;
                    self.commit_item(project, run, &item, result)?;
                }
                Err(e) => match classify(e) {
                    CallFailure::Refused(reason) => {
                        return self.block_lane(project, run, Some(&item), reason);
                    }
                    CallFailure::Item(code, reason) => {
                        failures = 0;
                        if store.fail_ai_item(item.item, item.attempt, &code, &reason)? {
                            run.summary().failed += 1;
                        } else {
                            run.summary().dropped_replies += 1;
                        }
                        project.note_ai_switch();
                    }
                    CallFailure::Network(reason) => {
                        failures += 1;
                        if failures >= MAX_NETWORK_FAILURES {
                            return self.block_lane(project, run, Some(&item), reason);
                        }
                        store.release_ai_item(item.item, item.attempt)?;
                        // The local runtime may be restarting on a new port and key: the new
                        // client waits for it, and its Failed state Blocks (NFR-REL-12).
                        client = None;
                        if !self.back_off(project, run, failures) {
                            return Ok(());
                        }
                    }
                },
            }
        }
    }

    fn commit_item(
        &self,
        project: &Project,
        run: &LaneRun,
        item: &StartedAiItem,
        result: AiItemResult,
    ) -> Result<(), StoreError> {
        let store = project.store();
        let lane = project.ai_lane();
        // Is this the last item of a re-run? Then the switch is in this commit.
        let applying = store.pending_ai_run()?.is_some_and(|p| p.id == item.run)
            && store.ai_progress()?.open <= 1;
        lane.applying.store(applying, Ordering::SeqCst);
        let (discarded, dropped, invalid) = (
            result.discarded,
            result.identifier_dropped,
            result.invalid_responses,
        );
        let outcome = match store.commit_ai_item(item.item, item.attempt, result) {
            Ok(o) => o,
            // The commit rolled back. Record why on the item, so it does not come back.
            Err(StoreError::Invalid(reason)) => {
                lane.applying.store(false, Ordering::SeqCst);
                store.fail_ai_item(item.item, item.attempt, "internal_error", &reason)?;
                run.summary().failed += 1;
                return Ok(());
            }
            Err(e) => {
                lane.applying.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };
        lane.applying.store(false, Ordering::SeqCst);
        {
            let mut s = run.summary();
            if outcome.committed {
                s.done += 1;
                s.mentions += outcome.mentions as u64;
                s.discarded += discarded;
                s.identifier_dropped += dropped;
                s.invalid_responses += invalid;
            } else {
                s.dropped_replies += 1;
            }
        }
        if outcome.connections_changed {
            project.changed();
        }
        project.note_ai_switch();
        Ok(())
    }

    /// The panel's AI row and the Settings section.
    pub fn ai_extraction_view(&self, project: &Project) -> Result<AiExtractionView, ProjectError> {
        let p = project.store().ai_progress()?;
        let lane = project.ai_lane();
        let state = if lane.applying.load(Ordering::SeqCst) {
            AiLaneState::Applying
        } else {
            match p.job_state.as_deref() {
                Some("blocked") => AiLaneState::Blocked,
                Some("paused") => AiLaneState::Paused,
                Some(_) => AiLaneState::Running,
                None if p.waiting_units > 0 => AiLaneState::WaitingForPick,
                None if p.active.is_some() => AiLaneState::Done,
                None => AiLaneState::Idle,
            }
        };
        let finished = p.done + p.failed;
        let eta = match (
            state,
            *lane.started.lock().unwrap_or_else(|e| e.into_inner()),
        ) {
            (AiLaneState::Running, Some((at, before))) => {
                let done = finished.saturating_sub(before);
                match mk_jobs::estimate(done, done, p.open, at.elapsed()) {
                    Some(left) => Eta::Left(left),
                    None => Eta::Estimating,
                }
            }
            _ => Eta::None,
        };
        let target = self.extraction_target(project).ok();
        let current = p.pending.as_ref().or(p.active.as_ref());
        // The counts go with the results they are shown beside: a re-run's come in at its switch.
        let results = p.active.as_ref().or(p.pending.as_ref());
        let rerun_with = target.as_ref().and_then(|t| {
            let results = p.active.as_ref()?;
            let is_new = results.identity != t.identity
                && p.pending.as_ref().is_none_or(|r| r.identity != t.identity);
            is_new.then(|| t.model_name.clone())
        });
        let destination = current.map(|r| {
            let class =
                DestinationClass::parse(&r.destination_class).unwrap_or(DestinationClass::Internet);
            crate::ai::AiActivity::test(&r.destination_host, class).label()
        });
        Ok(AiExtractionView {
            experimental: mk_ai::experimental(),
            state,
            reason: (state == AiLaneState::Blocked)
                .then(|| p.blocked_reason.clone())
                .flatten(),
            units_done: finished,
            units_total: finished + p.open,
            units_failed: p.failed,
            eta,
            destination,
            results_model: p.active.as_ref().map(|r| r.model_name.clone()),
            rerun_model: p.pending.as_ref().map(|r| r.model_name.clone()),
            rerun_with,
            mentions: results.map_or(0, |r| r.mentions),
            discarded: results.map_or(0, |r| r.discarded),
            invalid_responses: results.map_or(0, |r| r.invalid_responses),
            identifier_dropped: results.map_or(0, |r| r.identifier_dropped),
            waiting: (p.waiting_units > 0).then(|| UnitEstimate::of(p.waiting_units)),
        })
    }
}
