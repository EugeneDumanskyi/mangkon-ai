use std::fmt;

/// Ingestion state of a source content (domain §7.1). "Done" means Stage A is done.
///
/// M2b reaches Queued, Processing, Done, Failed, Missing and Replaced. NotProcessed (cancel) and
/// Removed (remove from project) exist so the stored states need no migration later; nothing
/// moves into them yet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IngestionState {
    Queued,
    Processing,
    Done,
    Failed,
    NotProcessed,
    Missing,
    Replaced,
    Removed,
}

use IngestionState::*;

/// Every move of domain §7.1, plus ASSUMPTION (M2b): a Queued, Failed or Missing content whose
/// last path now has a new hash is Replaced too, not only a Done one. Its pending work is
/// skipped and its data, if any, deleted with it.
const MOVES: &[(IngestionState, IngestionState)] = &[
    (Queued, Processing),
    (Processing, Done),
    (Processing, Failed),
    (Processing, Queued),
    (Queued, NotProcessed),
    (NotProcessed, Queued),
    (Failed, Queued),
    (Done, Queued),
    (Queued, Missing),
    (Done, Missing),
    (Failed, Missing),
    (Missing, Done),
    (Missing, Queued),
    (Done, Replaced),
    (Queued, Replaced),
    (Failed, Replaced),
    (Missing, Replaced),
    (Done, Removed),
    (Failed, Removed),
    (Missing, Removed),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalMove {
    pub from: IngestionState,
    pub to: IngestionState,
}

impl fmt::Display for IllegalMove {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "content cannot move from {:?} to {:?}",
            self.from, self.to
        )
    }
}

impl std::error::Error for IllegalMove {}

impl IngestionState {
    pub const ALL: [IngestionState; 8] = [
        Queued,
        Processing,
        Done,
        Failed,
        NotProcessed,
        Missing,
        Replaced,
        Removed,
    ];

    /// Stable stored name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Queued => "queued",
            Processing => "processing",
            Done => "done",
            Failed => "failed",
            NotProcessed => "not_processed",
            Missing => "missing",
            Replaced => "replaced",
            Removed => "removed",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }

    pub fn can_move_to(self, to: IngestionState) -> bool {
        MOVES.contains(&(self, to))
    }

    /// `to`, if domain §7.1 allows the move.
    pub fn move_to(self, to: IngestionState) -> Result<IngestionState, IllegalMove> {
        if self.can_move_to(to) {
            Ok(to)
        } else {
            Err(IllegalMove { from: self, to })
        }
    }

    /// Where a Missing content goes when a path is found again with the same hash: back to Done
    /// if it was Done before, otherwise to Queued (domain §7.1).
    pub fn found_again(before_missing: IngestionState) -> IngestionState {
        if before_missing == Done { Done } else { Queued }
    }

    /// The state after a restart: Processing goes back to Queued, as its work item goes back to
    /// Pending (domain §7.1 "crash, quit, or forced stop"). Others are kept.
    pub fn after_restart(self) -> IngestionState {
        if self == Processing { Queued } else { self }
    }

    /// Evidence of a Missing content is not counted for connections (INV-CO-07).
    pub fn counts_for_connections(self) -> bool {
        self != Missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in IngestionState::ALL {
            assert_eq!(IngestionState::from_name(s.as_str()), Some(s));
        }
    }

    /// Domain §7.1: only the drawn moves are allowed.
    #[test]
    fn only_drawn_moves() {
        assert_eq!(Queued.move_to(Processing), Ok(Processing));
        assert!(Done.move_to(Processing).is_err());
        assert!(Replaced.move_to(Queued).is_err());
        assert!(Processing.move_to(Missing).is_err());
        assert!(Removed.move_to(Done).is_err());
    }

    /// INV-CO-07, domain §7.1: Missing returns to Done only if it was Done before.
    #[test]
    fn inv_co_07_found_again() {
        assert_eq!(IngestionState::found_again(Done), Done);
        assert_eq!(IngestionState::found_again(Queued), Queued);
        assert_eq!(IngestionState::found_again(Failed), Queued);
        assert!(!Missing.counts_for_connections());
        assert!(Done.counts_for_connections());
    }

    /// INV-PR-04: after a crash, Processing content is queued again; nothing else changes.
    #[test]
    fn inv_pr_04_restart_requeues_processing() {
        for s in IngestionState::ALL {
            let want = if s == Processing { Queued } else { s };
            assert_eq!(s.after_restart(), want);
        }
    }
}
