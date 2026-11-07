//! BC-07 and BC-09 services (M7a): the user's decisions on entities (US-11.4, US-11.5, US-12.2) and
//! merge proposals (US-11.2). Every decision is one transaction of `mk-store` with its derived rows.
//!
//! Proposals are made on a background thread, off the writer: it reads through a reader connection,
//! compares names, then replaces the table in one short transaction that filters them again against
//! the decisions as they are then (ADR-0005: proposals may lag, but never ahead of a decision). Its
//! next run comes no sooner than 20 times the last one took, so it stays cheap during a long run.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mk_graph::proposal::{Proposal, THRESHOLD, proposals};
use mk_kernel::EntityType;
use mk_store::{
    DecisionOutcome, DecisionRow, EntityDetail, EntityEdit, EntityNames, MergeEdit, OccurrenceRow,
    ProposalCursor, ProposalRow, SplitMade, Store,
};

use crate::events::Changes;
use crate::project::{Project, ProjectError};

/// How often the thread looks for changed entities.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// A run takes at most 1/20 of the time (ASSUMPTION, M7a).
const PACE: u32 = 20;
/// After a failed run.
const AFTER_FAILURE: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Shared {
    stop: AtomicBool,
    /// Held for a whole run: a run asked for by hand and the thread's never overlap.
    run: Mutex<()>,
    asleep: Mutex<()>,
    wake: Condvar,
}

/// The proposal thread of an open project. Dropping it stops the thread.
pub(crate) struct ProposalLane {
    pub(crate) shared: Arc<Shared>,
    /// A run that changed the stored rows says so, or the mark and the count stay stale (ADR-0003).
    pub(crate) changes: Changes,
    thread: Option<JoinHandle<()>>,
}

impl ProposalLane {
    pub(crate) fn start(store: Arc<Store>, changes: Changes) -> ProposalLane {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = shared.clone();
            let changes = changes.clone();
            std::thread::Builder::new()
                .name("mk-proposals".into())
                .spawn(move || lane(&store, &shared, &changes))
                .ok()
        };
        ProposalLane {
            shared,
            changes,
            thread,
        }
    }
}

impl Drop for ProposalLane {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn lane(store: &Store, shared: &Shared, changes: &Changes) {
    let mut next_run = Instant::now();
    loop {
        {
            let guard = shared.asleep.lock().unwrap_or_else(|e| e.into_inner());
            if shared.stop.load(Ordering::SeqCst) {
                return;
            }
            let _ = shared.wake.wait_timeout(guard, LOOK_EVERY);
        }
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        if Instant::now() < next_run {
            continue;
        }
        let Ok((entities, made_from)) = store.proposal_generations() else {
            continue;
        };
        if entities == made_from {
            continue;
        }
        let started = Instant::now();
        let finished = refresh(store, shared, changes);
        next_run = Instant::now()
            + match finished {
                Ok(_) => started.elapsed().saturating_mul(PACE),
                Err(_) => AFTER_FAILURE,
            };
    }
}

/// One proposal run: read, compare, replace. How many are stored after it.
fn refresh(store: &Store, shared: &Shared, changes: &Changes) -> Result<usize, ProjectError> {
    let _run = shared.run.lock().unwrap_or_else(|e| e.into_inner());
    // On a thread of its own: a large project allocates hundreds of MiB of small pieces here. When
    // nothing else has used the thread's heap, the allocator can give all of it back afterwards;
    // otherwise it keeps what it freed for as long as the app runs (NFR-RES-01).
    let stored = std::thread::scope(|s| {
        s.spawn(|| {
            let input = store.proposal_input()?;
            let made = proposals(input.candidates, &input.blocked, THRESHOLD, &shared.stop);
            if shared.stop.load(Ordering::SeqCst) {
                return Ok(Default::default());
            }
            Ok::<_, ProjectError>(
                store
                    .replace_proposals(input.generation, made)?
                    .unwrap_or_default(),
            )
        })
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })?;
    if stored.changed {
        changes.changed();
    }
    Ok(stored.stored)
}

impl Project {
    fn after_decision(&self, d: DecisionOutcome) -> i64 {
        if d.connections_changed {
            self.changed();
        }
        d.seq
    }

    /// Accepts a pending proposal for the pair of name keys `a` and `b`, with an optional name and
    /// type (US-11.2 AC2, US-11.3 AC2 and AC4). Returns the merge decision's `seq`.
    pub fn accept_proposal(&self, a: &str, b: &str, edit: MergeEdit) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().accept_proposal(a, b, edit)?))
    }

    /// Rejects the pair: it is never proposed again, and nor is any pair between the two sides
    /// (US-11.3 AC3, INV-ER-04).
    pub fn reject_proposal(&self, a: &str, b: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().reject_merge(a, b)?))
    }

    /// Undoes an accepted merge: the two come apart, and the pair is stored as rejected (US-11.4
    /// AC1).
    pub fn undo_merge(&self, a: &str, b: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().undo_merge(a, b)?))
    }

    /// Moves some mentions of an entity to a new entity (US-11.1 AC2, US-11.4 AC2). `mentions` are
    /// the ids of its occurrence rows. Returns the decision and the new entity's key.
    pub fn split_mentions(&self, key: &str, mentions: &[i64]) -> Result<SplitMade, ProjectError> {
        let (outcome, key) = self.store().split_mentions(key, mentions.to_vec())?;
        Ok(SplitMade {
            seq: self.after_decision(outcome),
            key,
        })
    }

    /// Puts a split entity's mentions back.
    pub fn undo_split(&self, key: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().undo_split(key)?))
    }

    /// Sets an entity's name (US-12.2 AC1, INV-ER-07).
    pub fn rename_entity(&self, key: &str, name: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(
            self.store()
                .edit_entity(key, EntityEdit::Rename(name.to_string()))?,
        ))
    }

    /// Sets an entity's type (US-12.2 AC1, INV-ER-07).
    pub fn change_entity_type(&self, key: &str, t: EntityType) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().edit_entity(key, EntityEdit::ChangeType(t))?))
    }

    /// Sets an entity's note; an empty note clears it (US-12.2 AC2).
    pub fn set_entity_note(&self, key: &str, note: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(
            self.store()
                .edit_entity(key, EntityEdit::Note(note.to_string()))?,
        ))
    }

    /// Takes a name off an entity's aliases (US-12.2 AC1).
    pub fn remove_alias(&self, key: &str, alias: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().set_alias_removed(key, alias, true)?))
    }

    /// Brings a removed alias back.
    pub fn restore_alias(&self, key: &str, alias: &str) -> Result<i64, ProjectError> {
        Ok(self.after_decision(self.store().set_alias_removed(key, alias, false)?))
    }

    /// A page of the decision log, newest first, older than `before` (US-11.5 AC2).
    pub fn decisions(
        &self,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<DecisionRow>, ProjectError> {
        Ok(self.store().decisions_page(before, limit)?)
    }

    /// A page of pending proposals, highest score first (US-11.3 AC1).
    pub fn proposals(
        &self,
        after: Option<&ProposalCursor>,
        limit: usize,
    ) -> Result<Vec<ProposalRow>, ProjectError> {
        self.proposals_of(None, after, limit)
    }

    /// A page of pending proposals, only those of the entity holding `entity` when given (US-07.7
    /// AC4).
    pub fn proposals_of(
        &self,
        entity: Option<&str>,
        after: Option<&ProposalCursor>,
        limit: usize,
    ) -> Result<Vec<ProposalRow>, ProjectError> {
        Ok(self.store().proposals_page(entity, after, limit)?)
    }

    /// How many pending proposals there are, only those of the entity when given.
    pub fn proposal_count(&self, entity: Option<&str>) -> Result<u64, ProjectError> {
        Ok(self.store().proposal_count(entity)?)
    }

    /// An entity's page (US-12.1, US-11.4). `None` when no entity holds `key` now.
    pub fn entity_detail(
        &self,
        key: &str,
        include_low: bool,
    ) -> Result<Option<EntityDetail>, ProjectError> {
        Ok(self.store().entity_detail(key, include_low)?)
    }

    /// The counted mentions of the entity holding `key`, at most `limit` (US-12.1 AC1, US-11.3 AC1).
    pub fn entity_evidence(
        &self,
        key: &str,
        limit: usize,
        include_low: bool,
    ) -> Result<Vec<OccurrenceRow>, ProjectError> {
        Ok(self.store().entity_evidence(key, limit, include_low)?)
    }

    /// An entity's names: display name and aliases (INV-ER-07). `None` if it is gone.
    pub fn entity_names(&self, key: &str) -> Result<Option<EntityNames>, ProjectError> {
        Ok(self.store().entity_names(key)?)
    }

    /// Those of `keys` with a pending proposal: the "Unreviewed merge" mark (D6).
    pub fn unreviewed_keys(
        &self,
        keys: &[String],
    ) -> Result<std::collections::BTreeSet<String>, ProjectError> {
        Ok(self.store().unreviewed_keys(keys)?)
    }

    /// Makes the proposals now and waits for them, instead of for the thread. How many are stored.
    pub fn refresh_proposals(&self) -> Result<usize, ProjectError> {
        let lane = self.proposal_lane();
        refresh(self.store(), &lane.shared, &lane.changes)
    }

    /// Every proposal at or above `threshold`, stored or not: what spike S7 sweeps.
    pub fn score_proposals(&self, threshold: f32) -> Result<Vec<Proposal>, ProjectError> {
        let input = self.store().proposal_input()?;
        Ok(proposals(
            input.candidates,
            &input.blocked,
            threshold,
            &AtomicBool::new(false),
        ))
    }
}
