//! The runtime supervisor (domain §7.8, ADR-0015, architecture §7): one `llama-server` per
//! loaded model, at most one per role (extraction and embeddings, so at most 2). Started on
//! demand by [`Supervisor::ensure_ready`], stopped after [`RuntimeConfig::idle`] without work
//! (NFR-RES-06) and at app quit, restarted after a crash at most [`MAX_RESTARTS`] times
//! (NFR-REL-12, INV-MM-06). Plain threads: the caller's thread starts the process, one monitor
//! thread watches for crashes and idle time.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::find::{self, BinaryInfo};
use super::process::{Failure, ModelSpec, Process, SpawnOptions, exit_text, free_port};
use super::state::{Event, MAX_RESTARTS, RuntimeState};
use crate::Role;

/// NFR-RES-06, US-09.5 AC2 (ASSUMPTION: 10 min).
pub const IDLE_STOP: Duration = Duration::from_secs(10 * 60);
/// A large model on a slow disk takes minutes to load.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(300);
/// SIGTERM, then a kill after this long.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// How often the monitor looks for crashes and idle time.
const MONITOR_EVERY: Duration = Duration::from_millis(250);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub idle: Duration,
    pub health_timeout: Duration,
    pub stop_grace: Duration,
    /// `PATH` to search; `None` reads the environment at each search.
    pub path_var: Option<OsString>,
    pub spawn: SpawnOptions,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        RuntimeConfig {
            idle: IDLE_STOP,
            health_timeout: HEALTH_TIMEOUT,
            stop_grace: STOP_GRACE,
            path_var: None,
            spawn: SpawnOptions::default(),
        }
    }
}

/// One model's runtime as the UI shows it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SlotStatus {
    pub model_id: String,
    pub role: Role,
    pub state: RuntimeState,
    pub failure: Option<Failure>,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    /// Auto-restarts used since the last successful call.
    pub restarts: u8,
    /// The output of the running process, or of the last one that ended.
    pub tail: String,
}

struct Slot {
    spec: ModelSpec,
    state: RuntimeState,
    failure: Option<Failure>,
    restarts: u8,
    proc: Option<Process>,
    /// Bumped at every start, so a lease or start of an older process changes nothing.
    generation: u64,
    leases: usize,
    last_used: Instant,
    last_tail: String,
}

impl Slot {
    fn new(spec: ModelSpec) -> Slot {
        Slot {
            spec,
            state: RuntimeState::Stopped,
            failure: None,
            restarts: 0,
            proc: None,
            generation: 0,
            leases: 0,
            last_used: Instant::now(),
            last_tail: String::new(),
        }
    }

    /// Moves by `event` (domain §7.8). A move the machine does not allow is a bug; it is
    /// ignored in release builds.
    fn apply(&mut self, event: Event) {
        match self.state.transition(event, self.restarts) {
            Some(s) => self.state = s,
            None => debug_assert!(false, "{:?} --{event:?}--> not allowed", self.state),
        }
    }
}

type Listener = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    cfg: RuntimeConfig,
    settings: Mutex<Option<PathBuf>>,
    binary: Mutex<Option<BinaryInfo>>,
    slots: Mutex<HashMap<Role, Slot>>,
    changed: Condvar,
    listener: Mutex<Option<Listener>>,
    quit: AtomicBool,
    /// Starts in progress; quit waits for them, so no process outlives the app.
    launches: AtomicUsize,
}

/// The app's local runtimes.
pub struct Supervisor {
    inner: Arc<Inner>,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

/// A model that is Ready, held for calls: Busy while any lease is held, Ready again when the
/// last one is dropped.
pub struct Lease {
    inner: Arc<Inner>,
    role: Role,
    generation: u64,
    port: u16,
    key: String,
    model_id: String,
}

impl Lease {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The per-start key, sent as Bearer (NFR-SEC-06).
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// A call through this lease succeeded: the restart count starts again (INV-MM-06).
    pub fn succeeded(&self) {
        let mut slots = lock(&self.inner.slots);
        if let Some(s) = slots.get_mut(&self.role)
            && s.generation == self.generation
        {
            s.restarts = 0;
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        {
            let mut slots = lock(&self.inner.slots);
            if let Some(s) = slots.get_mut(&self.role)
                && s.generation == self.generation
            {
                s.leases = s.leases.saturating_sub(1);
                s.last_used = Instant::now();
                if s.leases == 0 && s.state == RuntimeState::Busy {
                    s.apply(Event::Received);
                }
            }
            self.inner.changed.notify_all();
        }
        self.inner.notify();
    }
}

impl Inner {
    fn notify(&self) {
        let f = lock(&self.listener).clone();
        if let Some(f) = f {
            f();
        }
    }

    fn path_var(&self) -> Option<OsString> {
        self.cfg
            .path_var
            .clone()
            .or_else(|| std::env::var_os("PATH"))
    }

    /// Finds the binary, checks its build, starts it and waits for `/health`. A port that
    /// turned out to be taken is tried once more on a new port.
    fn launch(&self, role: Role, spec: &ModelSpec, generation: u64) -> Result<Process, Failure> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let r = self.launch_inner(role, spec, generation);
        if r.is_err() {
            self.launches.fetch_sub(1, Ordering::SeqCst);
        }
        r
    }

    fn launch_inner(
        &self,
        role: Role,
        spec: &ModelSpec,
        generation: u64,
    ) -> Result<Process, Failure> {
        let settings = lock(&self.settings).clone();
        let found = find::find(settings.as_deref(), self.path_var().as_deref())
            .map_err(|e| Failure::new(e.code(), e.message()))?;
        find::version(&found.path, &self.cfg.spawn.env)
            .map_err(|e| Failure::new(e.code(), e.message()))?;
        let cancelled = || {
            self.quit.load(Ordering::SeqCst)
                || lock(&self.slots)
                    .get(&role)
                    .is_none_or(|s| s.generation != generation || s.state != RuntimeState::Starting)
        };
        let mut tries = 0;
        loop {
            tries += 1;
            if cancelled() {
                return Err(Failure::new("stopped", "Stopped while starting"));
            }
            let port = free_port().map_err(|e| Failure::new("spawn", e.to_string()))?;
            let mut p = Process::spawn(&found.path, spec, port, &self.cfg.spawn).map_err(|e| {
                Failure::new("spawn", format!("llama-server could not be started: {e}"))
            })?;
            match p.wait_healthy(self.cfg.health_timeout, &cancelled) {
                Ok(()) => return Ok(p),
                Err(f) if f.code == "port_in_use" && tries == 1 => continue,
                Err(f) => {
                    let tail = p.tail();
                    let mut slots = lock(&self.slots);
                    if let Some(s) = slots.get_mut(&role)
                        && s.generation == generation
                    {
                        s.last_tail = tail;
                    }
                    return Err(f);
                }
            }
        }
    }

    /// The end of a start: Ready with the process, or Failed with the reason. A start that was
    /// overtaken (quit, model switched) kills what it started.
    fn finish_start(&self, role: Role, generation: u64, result: Result<Process, Failure>) {
        let started = result.is_ok();
        let mut leftover = None;
        {
            let mut slots = lock(&self.slots);
            match slots.get_mut(&role) {
                Some(s) if s.generation == generation && s.state == RuntimeState::Starting => {
                    match result {
                        Ok(p) => {
                            s.apply(Event::HealthOk);
                            s.proc = Some(p);
                            s.failure = None;
                            s.last_used = Instant::now();
                        }
                        Err(f) => {
                            s.apply(Event::StartFailed);
                            s.failure = Some(f);
                        }
                    }
                }
                _ => leftover = result.ok(),
            }
            self.changed.notify_all();
        }
        if let Some(mut p) = leftover {
            p.kill();
        }
        if started {
            self.launches.fetch_sub(1, Ordering::SeqCst);
        }
        self.changed.notify_all();
        self.notify();
    }

    fn start_in_background(self: &Arc<Self>, role: Role, generation: u64, spec: ModelSpec) {
        let inner = self.clone();
        let spawned = std::thread::Builder::new()
            .name("llama-server-start".into())
            .spawn(move || {
                let r = inner.launch(role, &spec, generation);
                inner.finish_start(role, generation, r);
            });
        if spawned.is_err() {
            self.finish_start(
                role,
                generation,
                Err(Failure::new("spawn", "No thread to start llama-server")),
            );
        }
    }

    /// One monitor pass: crashes (auto-restart or Failed) and idle stops.
    fn watch(self: &Arc<Self>) {
        let mut restarts = Vec::new();
        let mut idle = Vec::new();
        let mut changed = false;
        {
            let mut slots = lock(&self.slots);
            for (role, s) in slots.iter_mut() {
                if matches!(s.state, RuntimeState::Ready | RuntimeState::Busy)
                    && let Some(p) = s.proc.as_mut()
                    && let Some(status) = p.exited()
                {
                    std::thread::sleep(Duration::from_millis(20));
                    s.last_tail = p.tail();
                    s.proc = None;
                    s.leases = 0;
                    s.apply(Event::ProcessEnded);
                    let why = exit_text(Some(status));
                    s.apply(Event::AutoRestart);
                    changed = true;
                    if s.state == RuntimeState::Starting {
                        s.restarts += 1;
                        s.generation += 1;
                        s.failure = None;
                        restarts.push((*role, s.generation, s.spec.clone()));
                    } else {
                        s.failure = Some(Failure::new(
                            "crashed",
                            format!(
                                "llama-server stopped again after {MAX_RESTARTS} restarts ({why})"
                            ),
                        ));
                    }
                } else if s.state == RuntimeState::Ready
                    && s.leases == 0
                    && s.last_used.elapsed() >= self.cfg.idle
                {
                    s.apply(Event::Stop);
                    if let Some(p) = s.proc.take() {
                        idle.push((*role, s.generation, p));
                    }
                    changed = true;
                }
            }
            if changed {
                self.changed.notify_all();
            }
        }
        for (role, generation, spec) in restarts {
            self.start_in_background(role, generation, spec);
        }
        if !idle.is_empty() {
            self.notify();
            for (role, generation, mut p) in idle {
                p.stop(self.cfg.stop_grace);
                self.freed(role, generation);
            }
        }
        if changed {
            self.notify();
        }
    }

    fn freed(&self, role: Role, generation: u64) {
        let mut slots = lock(&self.slots);
        if let Some(s) = slots.get_mut(&role)
            && s.generation == generation
            && s.state == RuntimeState::Stopping
        {
            s.apply(Event::Freed);
        }
        self.changed.notify_all();
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Supervisor::new(RuntimeConfig::default())
    }
}

impl Supervisor {
    pub fn new(cfg: RuntimeConfig) -> Supervisor {
        Supervisor {
            inner: Arc::new(Inner {
                cfg,
                settings: Mutex::new(None),
                binary: Mutex::new(None),
                slots: Mutex::new(HashMap::new()),
                changed: Condvar::new(),
                listener: Mutex::new(None),
                quit: AtomicBool::new(false),
                launches: AtomicUsize::new(0),
            }),
            monitor: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.inner.cfg
    }

    fn ensure_monitor(&self) {
        let mut m = lock(&self.monitor);
        if m.is_some() {
            return;
        }
        let inner = self.inner.clone();
        *m = std::thread::Builder::new()
            .name("llama-server-monitor".into())
            .spawn(move || {
                while !inner.quit.load(Ordering::SeqCst) {
                    inner.watch();
                    let slots = lock(&inner.slots);
                    let _ = inner.changed.wait_timeout(slots, MONITOR_EVERY);
                }
            })
            .ok();
    }

    /// `f` runs (on any thread, no lock held) after every state change.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.inner.listener) = Some(Arc::new(f));
    }

    /// The path set in the app settings (ADR-0015: looked at before `PATH`).
    pub fn set_settings_path(&self, path: Option<PathBuf>) {
        *lock(&self.inner.settings) = path;
        *lock(&self.inner.binary) = None;
    }

    pub fn settings_path(&self) -> Option<PathBuf> {
        lock(&self.inner.settings).clone()
    }

    /// The binary as last checked; checked now if it never was.
    pub fn binary(&self) -> BinaryInfo {
        if let Some(b) = lock(&self.inner.binary).clone() {
            return b;
        }
        self.recheck()
    }

    /// The binary as last checked, without running it.
    pub fn binary_cached(&self) -> Option<BinaryInfo> {
        lock(&self.inner.binary).clone()
    }

    /// Finds the binary and reads its build and devices again ("Check again").
    pub fn recheck(&self) -> BinaryInfo {
        let settings = self.settings_path();
        let info = find::probe(
            settings.as_deref(),
            self.inner.path_var().as_deref(),
            &self.inner.cfg.spawn.env,
        );
        *lock(&self.inner.binary) = Some(info.clone());
        self.inner.notify();
        info
    }

    /// Blocks until `spec` is Ready and returns a lease on it (US-09.5 AC1). Starts it when
    /// Stopped; a different model of the same role is stopped first (model switched). Failed
    /// stays Failed until [`retry`](Self::retry).
    pub fn ensure_ready(&self, spec: &ModelSpec) -> Result<Lease, Failure> {
        self.ensure_monitor();
        let inner = &self.inner;
        let role = spec.role;
        let mut slots = lock(&inner.slots);
        loop {
            if inner.quit.load(Ordering::SeqCst) {
                return Err(Failure::new("stopped", "The app is closing"));
            }
            let switch = slots.get(&role).is_some_and(|s| s.spec != *spec);
            if switch {
                let s = slots.get_mut(&role).expect("slot");
                match s.state {
                    RuntimeState::Ready | RuntimeState::Busy => {
                        s.apply(Event::Stop);
                        let generation = s.generation;
                        let p = s.proc.take();
                        drop(slots);
                        inner.notify();
                        if let Some(mut p) = p {
                            p.stop(inner.cfg.stop_grace);
                        }
                        inner.freed(role, generation);
                        slots = lock(&inner.slots);
                    }
                    RuntimeState::Starting | RuntimeState::Stopping | RuntimeState::Crashed => {
                        slots = inner
                            .changed
                            .wait_timeout(slots, MONITOR_EVERY)
                            .map(|(g, _)| g)
                            .unwrap_or_else(|e| e.into_inner().0);
                    }
                    RuntimeState::Stopped | RuntimeState::Failed => {
                        slots.insert(role, Slot::new(spec.clone()));
                    }
                }
                continue;
            }
            let s = slots.entry(role).or_insert_with(|| Slot::new(spec.clone()));
            match s.state {
                RuntimeState::Ready | RuntimeState::Busy => {
                    s.apply(Event::Send);
                    s.leases += 1;
                    s.last_used = Instant::now();
                    let lease = Lease {
                        inner: inner.clone(),
                        role,
                        generation: s.generation,
                        port: s.proc.as_ref().map_or(0, |p| p.port),
                        key: s.proc.as_ref().map(|p| p.key.clone()).unwrap_or_default(),
                        model_id: s.spec.id.clone(),
                    };
                    drop(slots);
                    inner.notify();
                    return Ok(lease);
                }
                RuntimeState::Failed => {
                    return Err(s
                        .failure
                        .clone()
                        .unwrap_or_else(|| Failure::new("failed", "llama-server failed")));
                }
                RuntimeState::Stopped => {
                    s.apply(Event::Need);
                    s.generation += 1;
                    let generation = s.generation;
                    drop(slots);
                    inner.notify();
                    let r = inner.launch(role, spec, generation);
                    inner.finish_start(role, generation, r);
                    slots = lock(&inner.slots);
                }
                RuntimeState::Starting | RuntimeState::Stopping | RuntimeState::Crashed => {
                    slots = inner
                        .changed
                        .wait_timeout(slots, MONITOR_EVERY)
                        .map(|(g, _)| g)
                        .unwrap_or_else(|e| e.into_inner().0);
                }
            }
        }
    }

    /// "Retry" (domain §7.8 Failed → Starting): checks the binary again, so a new install
    /// needs no app restart (ADR-0015), and starts every Failed model again in the background
    /// with a fresh restart count.
    pub fn retry(&self) -> BinaryInfo {
        self.ensure_monitor();
        let info = self.recheck();
        let mut starts = Vec::new();
        {
            let mut slots = lock(&self.inner.slots);
            for (role, s) in slots.iter_mut() {
                if s.state == RuntimeState::Failed {
                    s.apply(Event::UserRetry);
                    s.restarts = 0;
                    s.failure = None;
                    s.generation += 1;
                    starts.push((*role, s.generation, s.spec.clone()));
                }
            }
            self.inner.changed.notify_all();
        }
        for (role, generation, spec) in starts {
            self.inner.start_in_background(role, generation, spec);
        }
        self.inner.notify();
        info
    }

    /// Every model the supervisor knows, by role.
    pub fn status(&self) -> Vec<SlotStatus> {
        let slots = lock(&self.inner.slots);
        let mut out: Vec<SlotStatus> = slots
            .values()
            .map(|s| SlotStatus {
                model_id: s.spec.id.clone(),
                role: s.spec.role,
                state: s.state,
                failure: s.failure.clone(),
                port: s.proc.as_ref().map(|p| p.port),
                pid: s.proc.as_ref().map(Process::pid),
                restarts: s.restarts,
                tail: s
                    .proc
                    .as_ref()
                    .map_or_else(|| s.last_tail.clone(), Process::tail),
            })
            .collect();
        out.sort_by_key(|s| s.role);
        out
    }

    /// The state of the model of `role`, if it was ever needed.
    pub fn state_of(&self, role: Role) -> Option<(String, RuntimeState, Option<Failure>)> {
        lock(&self.inner.slots)
            .get(&role)
            .map(|s| (s.spec.id.clone(), s.state, s.failure.clone()))
    }

    /// Stops the model of `role` now, if it runs and no call holds it (a model was deleted or
    /// switched away from).
    pub fn stop(&self, role: Role) {
        let taken = {
            let mut slots = lock(&self.inner.slots);
            match slots.get_mut(&role) {
                Some(s) if s.state == RuntimeState::Ready && s.leases == 0 => {
                    s.apply(Event::Stop);
                    s.proc.take().map(|p| (s.generation, p))
                }
                Some(s) if s.state == RuntimeState::Failed => {
                    slots.remove(&role);
                    None
                }
                _ => None,
            }
        };
        if let Some((generation, mut p)) = taken {
            self.inner.notify();
            p.stop(self.inner.cfg.stop_grace);
            self.inner.freed(role, generation);
        }
        self.inner.notify();
    }

    /// App quit (US-09.5 AC2): every process is stopped and reaped, starts in progress are
    /// cancelled, and nothing starts again.
    pub fn shut_down(&self) {
        let inner = &self.inner;
        inner.quit.store(true, Ordering::SeqCst);
        let taken: Vec<(Role, u64, Process)> = {
            let mut slots = lock(&inner.slots);
            let mut out = Vec::new();
            for (role, s) in slots.iter_mut() {
                if matches!(
                    s.state,
                    RuntimeState::Ready | RuntimeState::Busy | RuntimeState::Starting
                ) {
                    s.apply(Event::Stop);
                    if let Some(p) = s.proc.take() {
                        out.push((*role, s.generation, p));
                    }
                }
            }
            inner.changed.notify_all();
            out
        };
        let handles: Vec<_> = taken
            .into_iter()
            .map(|(role, generation, mut p)| {
                let inner = inner.clone();
                std::thread::spawn(move || {
                    p.stop(inner.cfg.stop_grace);
                    inner.freed(role, generation);
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
        // A start in progress sees `quit` within one health poll (or after `--version`) and
        // kills its process.
        let deadline = Instant::now() + inner.cfg.stop_grace;
        let mut slots = lock(&inner.slots);
        while inner.launches.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            slots = inner
                .changed
                .wait_timeout(slots, Duration::from_millis(50))
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        for s in slots.values_mut() {
            if s.state == RuntimeState::Stopping && s.proc.is_none() {
                s.apply(Event::Freed);
            }
        }
        drop(slots);
        if let Some(m) = lock(&self.monitor).take() {
            inner.changed.notify_all();
            let _ = m.join();
        }
        inner.notify();
    }

    /// Waits until `f` holds for the status, or `limit` passes (tests, the CLI).
    pub fn wait_for(&self, limit: Duration, f: impl Fn(&[SlotStatus]) -> bool) -> bool {
        let end = Instant::now() + limit;
        loop {
            if f(&self.status()) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            let slots = lock(&self.inner.slots);
            let _ = self
                .inner
                .changed
                .wait_timeout(slots, Duration::from_millis(50));
        }
    }

    /// Whether the settings path names an executable file.
    pub fn path_is_executable(path: &Path) -> bool {
        find::is_executable(path)
    }
}

impl Drop for Supervisor {
    /// No `llama-server` outlives the app.
    fn drop(&mut self) {
        if !self.inner.quit.load(Ordering::SeqCst) {
            self.shut_down();
        }
    }
}
