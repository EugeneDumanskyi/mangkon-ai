//! The local runtime state machine (domain §7.8) and its restart rule (INV-MM-06). Pure: no
//! I/O. One machine per loaded model; never saved, so every app start begins at Stopped.

use serde::Serialize;

/// Auto-restarts after a crash, at most (NFR-REL-12, INV-MM-06).
pub const MAX_RESTARTS: u8 = 2;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Stopped,
    Starting,
    Ready,
    Busy,
    Stopping,
    Crashed,
    /// With a reason, kept next to the state ([`super::Failure`]).
    Failed,
}

/// What happens to a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// An AI stage (or Try) needs the model.
    Need,
    HealthOk,
    /// It could not start: not found, too old, out of memory, bad model file, no health.
    StartFailed,
    /// A request goes out.
    Send,
    /// The last request in flight came back.
    Received,
    /// Idle 10 min, app quit, or model switched.
    Stop,
    /// The process is gone and its memory freed.
    Freed,
    /// The process ended without being asked to.
    ProcessEnded,
    /// After a crash: start again if restarts are left, else Failed.
    AutoRestart,
    /// "Retry" in the UI.
    UserRetry,
}

impl RuntimeState {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeState::Stopped => "stopped",
            RuntimeState::Starting => "starting",
            RuntimeState::Ready => "ready",
            RuntimeState::Busy => "busy",
            RuntimeState::Stopping => "stopping",
            RuntimeState::Crashed => "crashed",
            RuntimeState::Failed => "failed",
        }
    }

    /// The next state, or `None` if `event` is not allowed now (domain §7.8). `restarts` is the
    /// number of auto-restarts used since the last successful call.
    pub fn transition(self, event: Event, restarts: u8) -> Option<RuntimeState> {
        use Event::*;
        use RuntimeState::*;
        Some(match (self, event) {
            (Stopped, Need) => Starting,
            (Starting, HealthOk) => Ready,
            (Starting, StartFailed) => Failed,
            (Ready, Send) => Busy,
            // Another call while one is in flight stays Busy.
            (Busy, Send) => Busy,
            (Busy, Received) => Ready,
            (Ready, Stop) => Stopping,
            // Not in §7.8: the app quits while a call runs or the model loads. The call fails;
            // the process must not outlive the app (US-09.5 AC2).
            (Busy | Starting, Stop) => Stopping,
            (Stopping, Freed) => Stopped,
            (Ready | Busy, ProcessEnded) => Crashed,
            (Crashed, AutoRestart) if restarts < MAX_RESTARTS => Starting,
            (Crashed, AutoRestart) => Failed,
            (Failed, UserRetry) => Starting,
            _ => return None,
        })
    }

    /// A process is (or may be) running in this state.
    pub fn has_process(self) -> bool {
        matches!(
            self,
            RuntimeState::Starting
                | RuntimeState::Ready
                | RuntimeState::Busy
                | RuntimeState::Stopping
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Event::*;
    use super::RuntimeState::*;

    #[test]
    fn domain_7_8_transitions() {
        let path = [
            (Stopped, Need, Starting),
            (Starting, HealthOk, Ready),
            (Starting, StartFailed, Failed),
            (Ready, Send, Busy),
            (Busy, Received, Ready),
            (Ready, Stop, Stopping),
            (Stopping, Freed, Stopped),
            (Busy, ProcessEnded, Crashed),
            (Ready, ProcessEnded, Crashed),
            (Failed, UserRetry, Starting),
        ];
        for (from, ev, to) in path {
            assert_eq!(from.transition(ev, 0), Some(to), "{from:?} --{ev:?}-->");
        }
        assert_eq!(Stopped.transition(Send, 0), None);
        assert_eq!(Failed.transition(Need, 0), None, "only the user retries");
        assert_eq!(Stopped.transition(ProcessEnded, 0), None);
    }

    /// INV-MM-06, NFR-REL-12: two auto-restarts, then Failed.
    #[test]
    fn inv_mm_06_at_most_two_restarts() {
        assert_eq!(Crashed.transition(AutoRestart, 0), Some(Starting));
        assert_eq!(Crashed.transition(AutoRestart, 1), Some(Starting));
        assert_eq!(Crashed.transition(AutoRestart, 2), Some(Failed));
    }

    #[test]
    fn every_app_start_is_stopped() {
        assert!(!Stopped.has_process());
        assert!(!Failed.has_process());
        assert!(Ready.has_process());
    }
}
