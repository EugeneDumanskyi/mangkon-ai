//! Run control (US-03.2, US-03.3, US-03.4, ADR-0006): one signal the Stage A lanes, the
//! re-scan hashing and each busy worker look at. Pause gives the busy item [`PAUSE_GRACE`] to
//! finish, then its worker is killed and the item goes back to Pending; cancel and close stop at
//! once. Nothing of a stopped item was committed (INV-PR-04).

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a busy item may still finish after a pause (ADR-0006 "up to 5 s"): 4 s, so the
/// whole pause, kill and roll-back included, stays within 5 s (NFR-REL-05).
pub const PAUSE_GRACE: Duration = Duration::from_secs(4);

/// Why a run is asked to stop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    /// The user paused: jobs become Paused.
    Pause,
    /// The user cancelled: unfinished work becomes Not processed.
    Cancel,
    /// The project closes or the app quits: job states are kept (US-03.4 AC1).
    Close,
}

impl Stop {
    /// How long a busy item may still finish before its worker is killed.
    pub fn grace(self) -> Duration {
        match self {
            Stop::Pause => PAUSE_GRACE,
            Stop::Cancel | Stop::Close => Duration::ZERO,
        }
    }
}

/// The stop request of a project's run, if any, and when it came.
#[derive(Default)]
pub(crate) struct RunControl {
    request: Mutex<Option<(Stop, Instant)>>,
}

impl RunControl {
    pub(crate) fn request(&self, stop: Stop) {
        let mut r = self.request.lock().unwrap_or_else(|e| e.into_inner());
        // A later, stronger request (cancel, close) replaces a pause; its own clock starts now.
        if r.is_none_or(|(s, _)| s != stop) {
            *r = Some((stop, Instant::now()));
        }
    }

    pub(crate) fn clear(&self) {
        *self.request.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The pending request, if any.
    pub(crate) fn requested(&self) -> Option<Stop> {
        self.request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|(s, _)| s)
    }

    /// True once no new work may start.
    pub(crate) fn stopping(&self) -> bool {
        self.requested().is_some()
    }

    /// True once a busy item's grace is over: its worker must be killed now.
    pub(crate) fn must_interrupt(&self) -> Option<Stop> {
        self.request
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .filter(|(s, at)| at.elapsed() >= s.grace())
            .map(|(s, _)| s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NFR-REL-05: a pause lets the busy item run 4 s; cancel and close interrupt at once.
    #[test]
    fn nfr_rel_05_grace_per_request() {
        let c = RunControl::default();
        assert!(!c.stopping() && c.must_interrupt().is_none());
        c.request(Stop::Pause);
        assert!(c.stopping());
        assert_eq!(c.must_interrupt(), None, "the busy item may still finish");
        c.request(Stop::Cancel);
        assert_eq!(c.must_interrupt(), Some(Stop::Cancel));
        c.clear();
        assert!(!c.stopping());
        assert!(PAUSE_GRACE < Duration::from_secs(5));
    }
}
