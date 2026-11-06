//! BC-03 Jobs: persistent jobs, work items and the scheduler rules (domain §3, §7.2, ADR-0006).
//! No I/O: `mk-store` persists the states, `mk-core` runs the lanes.
//!
//! M2b: every state of domain §7.2 (so schema v1 needs no migration later), the Stage A lane
//! and the restart rule. M2c: the worker memory budget ([`pick_next`], gap A2). M3c: the move
//! Queued → Paused (a project-wide pause covers jobs not started yet) and the ETA rule
//! ([`estimate`]). M6a: the Stage B lane limit, its priority and start rule.

use std::fmt;
use std::time::Duration;

/// Job state (domain §7.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Blocked,
    Completed,
    Cancelled,
}

/// Work item state (domain §7.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ItemState {
    Pending,
    InProgress,
    Done,
    Failed,
    Skipped,
    Cancelled,
}

/// What made a job (domain §5.1 `JOB.trigger`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Trigger {
    Add,
    Rescan,
    Retry,
    Settings,
    ModelRerun,
}

/// Which stage a job runs (domain §5.1 `JOB.stage`). M2b runs Stage A only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    A,
    B,
    C,
    Embeddings,
    CustomPatterns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalMove {
    pub from: &'static str,
    pub to: &'static str,
}

impl fmt::Display for IllegalMove {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot move from {} to {}", self.from, self.to)
    }
}

impl std::error::Error for IllegalMove {}

/// Stored names and the allowed moves of a state enum.
macro_rules! states {
    ($ty:ident { $($v:ident => $s:literal),* $(,)? } moves [$(($a:ident, $b:ident)),* $(,)?]) => {
        impl $ty {
            pub const ALL: &'static [$ty] = &[$($ty::$v),*];

            pub fn as_str(&self) -> &'static str {
                match self { $($ty::$v => $s),* }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|s| s.as_str() == name)
            }

            pub fn can_move_to(self, to: $ty) -> bool {
                matches!((self, to), $(($ty::$a, $ty::$b))|*)
            }

            /// `to`, if domain §7.2 allows the move.
            pub fn move_to(self, to: $ty) -> Result<$ty, IllegalMove> {
                if self.can_move_to(to) {
                    Ok(to)
                } else {
                    Err(IllegalMove { from: self.as_str(), to: to.as_str() })
                }
            }
        }
    };
}

states!(JobState {
    Queued => "queued",
    Running => "running",
    Paused => "paused",
    Blocked => "blocked",
    Completed => "completed",
    Cancelled => "cancelled",
} moves [
    (Queued, Running),
    // ASSUMPTION (M3c): pause is project-wide (D11), so a job not started yet pauses too.
    (Queued, Paused),
    (Running, Paused),
    (Paused, Running),
    (Running, Blocked),
    (Blocked, Running),
    (Running, Completed),
    (Queued, Cancelled),
    (Running, Cancelled),
    (Paused, Cancelled),
    (Blocked, Cancelled),
]);

states!(ItemState {
    Pending => "pending",
    InProgress => "in_progress",
    Done => "done",
    Failed => "failed",
    Skipped => "skipped",
    Cancelled => "cancelled",
} moves [
    (Pending, InProgress),
    (InProgress, Done),
    (InProgress, Failed),
    (InProgress, Pending),
    (Pending, Skipped),
    (Pending, Cancelled),
    (InProgress, Cancelled),
    (Failed, Pending),
]);

impl Trigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Trigger::Add => "add",
            Trigger::Rescan => "rescan",
            Trigger::Retry => "retry",
            Trigger::Settings => "settings",
            Trigger::ModelRerun => "model_rerun",
        }
    }
}

impl Stage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage::A => "a",
            Stage::B => "b",
            Stage::C => "c",
            Stage::Embeddings => "embeddings",
            Stage::CustomPatterns => "custom_patterns",
        }
    }
}

impl JobState {
    /// The state after a restart: every job state is kept (INV-PR-07). A job that was running
    /// comes back running; a paused one stays paused.
    pub fn after_restart(self) -> JobState {
        self
    }

    /// True once no more work happens in this job.
    pub fn is_final(self) -> bool {
        matches!(self, JobState::Completed | JobState::Cancelled)
    }
}

impl ItemState {
    /// The state after a restart: only items in progress at the crash go back to Pending and
    /// are redone; finished items are never redone (INV-PR-05, ADR-0006).
    pub fn after_restart(self) -> ItemState {
        if self == ItemState::InProgress {
            ItemState::Pending
        } else {
            self
        }
    }

    /// Done, Failed, Skipped or Cancelled: the item needs nothing more (a job with only such
    /// items is Completed, domain §7.2).
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            ItemState::Done | ItemState::Failed | ItemState::Skipped | ItemState::Cancelled
        )
    }
}

/// Stage A lane limit by cores: cores − 1, at least 1 (NFR-PERF-25). The lane runs
/// `min(this, what the memory budget allows)` workers at once: [`pick_next`] holds an item back
/// while its memory need does not fit (ADR-0007, architecture §7).
pub fn stage_a_limit(cores: usize) -> usize {
    cores.saturating_sub(1).max(1)
}

/// Stage B lane limit: 4 parallel requests on the local model, whose server has 4 slots
/// (NFR-PERF-25), 1 on an endpoint (ASSUMPTION: its limits are not known).
pub fn stage_b_limit(local: bool) -> usize {
    if local { 4 } else { 1 }
}

/// Stage B priority (gap A3): units with a Stage A identifier first, so the results that link
/// to something come early. Lower runs first.
pub fn stage_b_priority(has_identifiers: bool) -> i64 {
    i64::from(!has_identifiers)
}

/// The start rule (decided 2026-11-05): Stage B starts by itself when at most this many units
/// wait. About 32 min at [`STAGE_B_UNITS_PER_SECOND`]. Above it, the user picks.
pub const STAGE_B_AUTO_UNITS: u64 = 1600;
/// The measured rate with 4 slots on the minimum-tier model, TD-1 on RM-1
/// (`16-entities-acceptance.md` §3).
pub const STAGE_B_UNITS_PER_SECOND: f64 = 0.82;

/// How long `units` would take at [`STAGE_B_UNITS_PER_SECOND`].
pub fn stage_b_estimate(units: u64) -> Duration {
    Duration::from_secs_f64(units as f64 / STAGE_B_UNITS_PER_SECOND)
}

/// The start rule for embeddings (decided in M6b planning): the lane starts by itself when at most
/// this many windows wait, about 20 min at [`EMBEDDING_WINDOWS_PER_SECOND`]. Above it, on a click.
pub const EMBEDDING_AUTO_WINDOWS: u64 = 480_000;
/// S6's rate on TD-2 with bge-m3 on RM-1's GPU, rounded down: 421 windows/s over the whole run, and
/// falling as the database grew (`15-meaning-search.md`). ASSUMPTION: other machines differ.
pub const EMBEDDING_WINDOWS_PER_SECOND: f64 = 400.0;

/// How long `windows` would take at [`EMBEDDING_WINDOWS_PER_SECOND`].
pub fn embedding_estimate(windows: u64) -> Duration {
    Duration::from_secs_f64(windows as f64 / EMBEDDING_WINDOWS_PER_SECOND)
}

/// A pending Stage A item as the scheduler sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingItem {
    pub id: i64,
    pub priority: i64,
    /// Text-native content (no OCR needed). In M2b every content is text-native.
    pub text_native: bool,
    /// File size in bytes; with the file type it gives the item's memory need.
    pub size: u64,
}

/// Priority of a Stage A item: text-native first (INV-PR-03), then smaller files first, so
/// results show up early. Lower runs first.
pub fn stage_a_priority(text_native: bool, size: u64) -> i64 {
    let band = if text_native { 0 } else { 1i64 << 48 };
    band + size.min((1u64 << 48) - 1) as i64
}

/// The next items to start: up to `limit − in_flight`, by priority, then by id (oldest first).
/// Ordering is a preference, not a block (INV-PR-03).
pub fn next_batch(pending: &[PendingItem], in_flight: usize, limit: usize) -> Vec<i64> {
    let room = limit.saturating_sub(in_flight);
    let mut items: Vec<&PendingItem> = pending.iter().collect();
    items.sort_by_key(|i| (!i.text_native, i.priority, i.id));
    items.into_iter().take(room).map(|i| i.id).collect()
}

/// The index in `needs` (memory needs in bytes, in scheduler order) of the next item to start,
/// given `in_use` bytes and `in_flight` items already running under a `total` budget (gap A2,
/// ADR-0007):
///
/// - the first item whose need fits the room left runs, so small files keep flowing past a big
///   one that waits;
/// - if none fits and nothing runs, the first item runs alone, even over the budget (it is still
///   held to the per-file limit, NFR-REL-07), so the lane never stalls;
/// - otherwise `None`: wait until a running item finishes.
pub fn pick_next(needs: &[u64], in_use: u64, in_flight: usize, total: u64) -> Option<usize> {
    let room = total.saturating_sub(in_use);
    needs
        .iter()
        .position(|n| *n <= room)
        .or_else(|| (in_flight == 0 && !needs.is_empty()).then_some(0))
}

/// Finished items before an ETA is shown; until then the panel says "Estimating…" (US-03.1
/// AC2).
pub const ETA_MIN_DONE: u64 = 5;

/// Time left for `remaining` units at the rate of this run: `done` units in `elapsed`
/// (ASSUMPTION M3c: units are bytes, so a big file counts for more than a small one). `None`
/// while fewer than [`ETA_MIN_DONE`] items are finished or nothing was done yet.
pub fn estimate(
    finished_items: u64,
    done: u64,
    remaining: u64,
    elapsed: Duration,
) -> Option<Duration> {
    if finished_items < ETA_MIN_DONE || done == 0 || elapsed.is_zero() {
        return None;
    }
    let secs = elapsed.as_secs_f64() * remaining as f64 / done as f64;
    Some(Duration::from_secs_f64(secs.min(1e9)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-03.1 AC2: no ETA before 5 finished files; then remaining ÷ rate.
    #[test]
    fn us_03_1_ac2_eta_none_until_5() {
        let s = Duration::from_secs;
        assert_eq!(estimate(4, 400, 100, s(4)), None);
        assert_eq!(estimate(5, 0, 100, s(4)), None);
        assert_eq!(estimate(5, 500, 1000, s(10)), Some(s(20)));
        assert_eq!(estimate(9, 500, 0, s(10)), Some(s(0)));
    }

    /// D11: a project-wide pause pauses jobs that have not started.
    #[test]
    fn d11_queued_job_can_pause() {
        assert!(JobState::Queued.can_move_to(JobState::Paused));
        assert!(JobState::Paused.can_move_to(JobState::Running));
        assert!(!JobState::Paused.can_move_to(JobState::Queued));
    }

    #[test]
    fn names_round_trip() {
        for s in JobState::ALL {
            assert_eq!(JobState::from_name(s.as_str()), Some(*s));
        }
        for s in ItemState::ALL {
            assert_eq!(ItemState::from_name(s.as_str()), Some(*s));
        }
    }

    /// INV-PR-04: an item reaches Done only from InProgress, i.e. in the commit of its results;
    /// a rolled-back item goes back to Pending.
    #[test]
    fn inv_pr_04_done_only_from_in_progress() {
        use ItemState::*;
        for s in ItemState::ALL {
            assert_eq!(s.can_move_to(Done), *s == InProgress, "{s:?}");
        }
        assert_eq!(InProgress.move_to(Pending), Ok(Pending));
        assert!(Done.move_to(Pending).is_err());
        assert!(Skipped.move_to(InProgress).is_err());
    }

    /// INV-PR-05: restart redoes only items in progress; finished items stay finished.
    #[test]
    fn inv_pr_05_restart_never_redoes_finished_items() {
        use ItemState::*;
        assert_eq!(InProgress.after_restart(), Pending);
        for s in [Pending, Done, Failed, Skipped, Cancelled] {
            assert_eq!(s.after_restart(), s);
        }
        for s in ItemState::ALL {
            assert_eq!(s.is_finished(), !matches!(s, Pending | InProgress));
        }
    }

    /// INV-PR-07: job states survive a restart: running stays running, paused stays paused.
    #[test]
    fn inv_pr_07_job_states_survive_restart() {
        for s in JobState::ALL {
            assert_eq!(s.after_restart(), *s);
        }
        assert!(JobState::Completed.move_to(JobState::Running).is_err());
        assert!(JobState::Paused.move_to(JobState::Completed).is_err());
        assert!(JobState::Completed.is_final() && !JobState::Paused.is_final());
    }

    /// NFR-PERF-25: Stage A uses cores − 1 workers, at least one.
    #[test]
    fn nfr_perf_25_stage_a_limit() {
        assert_eq!(stage_a_limit(24), 23);
        assert_eq!(stage_a_limit(2), 1);
        assert_eq!(stage_a_limit(1), 1);
        assert_eq!(stage_a_limit(0), 1);
    }

    /// NFR-PERF-25: 4 slots on the local model, 1 on an endpoint.
    #[test]
    fn nfr_perf_25_stage_b_limit() {
        assert_eq!(stage_b_limit(true), 4);
        assert_eq!(stage_b_limit(false), 1);
    }

    /// Gap A3: identifiers first; 1,600 units is about 32 min.
    #[test]
    fn a3_stage_b_order_and_estimate() {
        assert!(stage_b_priority(true) < stage_b_priority(false));
        let minutes = stage_b_estimate(STAGE_B_AUTO_UNITS).as_secs() / 60;
        assert_eq!(minutes, 32);
    }

    /// The embeddings start rule's limit is about 20 min of work.
    #[test]
    fn m6b_embedding_limit_is_20_minutes() {
        assert_eq!(
            embedding_estimate(EMBEDDING_AUTO_WINDOWS).as_secs(),
            20 * 60
        );
    }

    /// INV-PR-03: text-native first, then by priority and age; never more than the lane limit.
    #[test]
    fn inv_pr_03_text_native_first() {
        let item = |id, priority, text_native| PendingItem {
            id,
            priority,
            text_native,
            size: 0,
        };
        let pending = [
            item(1, stage_a_priority(false, 10), false),
            item(2, stage_a_priority(true, 500), true),
            item(3, stage_a_priority(true, 20), true),
            item(4, stage_a_priority(true, 20), true),
        ];
        assert_eq!(next_batch(&pending, 0, 3), vec![3, 4, 2]);
        assert_eq!(next_batch(&pending, 2, 3), vec![3]);
        assert_eq!(next_batch(&pending, 3, 3), Vec::<i64>::new());
        assert_eq!(next_batch(&pending, 0, 10), vec![3, 4, 2, 1]);
    }

    /// Gap A2: small files pass a big one that does not fit; the big one runs once the lane is
    /// idle; nothing starts while the budget is full.
    #[test]
    fn a2_budget_lets_small_files_pass() {
        const MB: u64 = 1 << 20;
        let needs = [900 * MB, 50 * MB, 50 * MB];
        assert_eq!(pick_next(&needs, 0, 0, 1500 * MB), Some(0));
        assert_eq!(pick_next(&needs, 700 * MB, 1, 1500 * MB), Some(1));
        assert_eq!(pick_next(&needs[..1], 700 * MB, 1, 1500 * MB), None);
        assert_eq!(pick_next(&needs[..1], 0, 0, 500 * MB), Some(0));
        assert_eq!(pick_next(&[], 0, 0, 500 * MB), None);
        assert_eq!(pick_next(&needs, 1500 * MB, 3, 1500 * MB), None);
    }

    proptest::proptest! {
        /// Gap A2: a simulated lane never goes over the budget unless one item runs alone, and
        /// always finishes every item.
        #[test]
        fn a2_budget_never_over_unless_alone_and_never_stalls(
            needs in proptest::collection::vec(1u64..2_000, 0..40),
            total in 1u64..3_000,
            lanes in 1usize..8,
            finish_order in proptest::collection::vec(0usize..8, 0..200),
        ) {
            let mut queue = needs.clone();
            let mut running: Vec<u64> = Vec::new();
            let mut finished = 0;
            let mut steps = finish_order.into_iter();
            while !queue.is_empty() || !running.is_empty() {
                // Start what the budget allows, up to the lane width.
                while running.len() < lanes {
                    let in_use = running.iter().sum();
                    let Some(i) = pick_next(&queue, in_use, running.len(), total) else {
                        break;
                    };
                    running.push(queue.remove(i));
                    let sum: u64 = running.iter().sum();
                    proptest::prop_assert!(sum <= total || running.len() == 1);
                }
                // Idle lane with work left would be a stall.
                proptest::prop_assert!(!running.is_empty());
                let k = steps.next().unwrap_or(0) % running.len();
                running.remove(k);
                finished += 1;
            }
            proptest::prop_assert_eq!(finished, needs.len());
        }
    }
}
