//! `ConnectionsChanged` (domain §8, ADR-0005 step 5): "something changed", debounced 500 ms,
//! the UI re-queries (architecture §7).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Debounce window (architecture §7).
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// One or more commits changed connection rows. Carries no rows: the UI re-queries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConnectionsChanged {
    /// Commits folded into this event.
    pub changes: u32,
}

/// The debounce rule, without clocks or threads.
///
/// The first change not yet reported opens a window; every change in it is folded in; the
/// event goes out when the window ends (its trailing edge). The window is **not** extended by
/// later changes, so during a long ingestion events still go out every 500 ms and the screen is
/// never more than 500 ms behind a commit (NFR-PERF-34).
#[derive(Clone, Debug)]
pub struct Debouncer {
    window: Duration,
    opened: Option<Instant>,
    changes: u32,
}

impl Debouncer {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            opened: None,
            changes: 0,
        }
    }

    pub fn on_change(&mut self, now: Instant) {
        self.opened.get_or_insert(now);
        self.changes = self.changes.saturating_add(1);
    }

    /// When the open window ends, if one is open.
    pub fn deadline(&self) -> Option<Instant> {
        self.opened.map(|t| t + self.window)
    }

    /// The event, once the window has ended.
    pub fn poll(&mut self, now: Instant) -> Option<ConnectionsChanged> {
        let due = self.deadline().is_some_and(|d| now >= d);
        due.then(|| {
            self.opened = None;
            ConnectionsChanged {
                changes: std::mem::take(&mut self.changes),
            }
        })
    }
}

struct State {
    debouncer: Debouncer,
    subscribers: Vec<Sender<ConnectionsChanged>>,
    stop: bool,
}

/// Runs a [`Debouncer`] on a small thread and fans events out to subscribers.
pub(crate) struct Events {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Events {
    pub(crate) fn start(window: Duration) -> Events {
        let shared = Arc::new((
            Mutex::new(State {
                debouncer: Debouncer::new(window),
                subscribers: Vec::new(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("mk-core-events".into())
                .spawn(move || drive(&shared))
                .expect("events thread starts")
        };
        Events {
            shared,
            thread: Some(thread),
        }
    }

    pub(crate) fn changed(&self) {
        let (lock, cv) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .debouncer
            .on_change(Instant::now());
        cv.notify_all();
    }

    pub(crate) fn subscribe(&self) -> Receiver<ConnectionsChanged> {
        let (tx, rx) = channel();
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribers
            .push(tx);
        rx
    }
}

fn drive(shared: &(Mutex<State>, Condvar)) {
    let (lock, cv) = shared;
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if state.stop {
            return;
        }
        let now = Instant::now();
        if let Some(event) = state.debouncer.poll(now) {
            state.subscribers.retain(|s| s.send(event).is_ok());
            continue;
        }
        state = match state.debouncer.deadline() {
            Some(d) => {
                cv.wait_timeout(state, d - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
            None => cv.wait(state).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// Architecture §7, NFR-PERF-34: changes in one window fold into one event at its end.
    #[test]
    fn nfr_perf_34_one_event_per_window() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(DEBOUNCE);
        assert_eq!(d.poll(t0), None);
        d.on_change(t0);
        d.on_change(t0 + 100 * MS);
        d.on_change(t0 + 499 * MS);
        assert_eq!(d.poll(t0 + 499 * MS), None);
        assert_eq!(
            d.poll(t0 + 500 * MS),
            Some(ConnectionsChanged { changes: 3 })
        );
        assert_eq!(d.poll(t0 + 900 * MS), None);
    }

    /// NFR-PERF-34: steady commits do not hold the event back; one goes out per window.
    #[test]
    fn nfr_perf_34_steady_changes_still_emit() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(DEBOUNCE);
        let mut events = 0;
        for i in 0..20u32 {
            let now = t0 + 100 * i * MS;
            events += d.poll(now).map_or(0, |_| 1);
            d.on_change(now);
        }
        assert_eq!(events, 3, "windows ending at 500, 1000 and 1500 ms");
        assert_eq!(d.deadline(), Some(t0 + 2000 * MS));
    }

    #[test]
    fn subscribers_get_the_event() {
        let events = Events::start(10 * MS);
        let rx = events.subscribe();
        events.changed();
        events.changed();
        let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(got.changes >= 1);
    }
}
