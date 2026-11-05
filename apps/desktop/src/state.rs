//! The app's state: the project list (`app.db`), at most one open project (gap A7, decided
//! 2026-10-28) and the UI's event channel. Ingestion runs on a plain background thread; reads run
//! on the store's reader pool next to it (US-03.1 AC3). Events go over a `tauri::ipc::Channel`
//! the UI passes to `subscribe`, so the UI needs no `core:event` permission (ADR-0003).
//!
//! Opening, closing or switching projects while files are read stops the run first: the files
//! in a worker go back to waiting and the queue keeps its state, so a running queue runs again
//! on the next open and a paused one stays paused (US-03.4 AC1).
//!
//! M5c: when a local model changes state, `llama-server` is checked again, a model is installed
//! or deleted, or the AI choice changes, the open project's AI jobs are Blocked or run again to
//! match (INV-PR-09). Quitting stops every `llama-server` (US-09.5 AC2).
//!
//! M6a: the AI lane has its own thread and holds no run lock, so pause and cancel never wait on a
//! model. It starts after ingestion, on open, when a job is unblocked and on the user's action.
//! Closing waits a moment for it; a reply after that is dropped by its token (NFR-REL-05).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use mk_core::{IngestSummary, Project, ProjectError, RegistryEntry, Workspace};
use tauri::ipc::Channel;

use crate::ipc::{IpcError, SkippedFile, UiEvent};

/// How often the progress panel is told about changes, at most (US-03.1 AC1).
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
/// Skipped files listed in the add summary, at most.
const MAX_SKIPPED: usize = 100;
/// How long closing waits for the stopped run's thread to let the project go.
const CLOSE_WAIT: Duration = Duration::from_secs(10);
/// How long closing waits for the AI lane: a call may take minutes, and its reply is dropped.
const CLOSE_WAIT_AI: Duration = Duration::from_secs(2);

/// One open project.
pub struct Open {
    pub project: Project,
    pub dir: PathBuf,
    pub entry: RegistryEntry,
    /// A re-scan or Stage A run is going on.
    ingesting: AtomicBool,
    /// A source was added during the run: run again when it ends.
    again: AtomicBool,
    /// The AI lane's thread is going on.
    ai_running: AtomicBool,
    /// A start came while the lane ran: it runs once more.
    ai_again: AtomicBool,
    /// The embeddings lane's thread is going on (M6b).
    embedding_running: AtomicBool,
    /// A start came while that lane ran: it runs once more.
    embedding_again: AtomicBool,
    /// The project is closing: no run starts again.
    closed: AtomicBool,
}

impl Open {
    pub fn is_ingesting(&self) -> bool {
        self.ingesting.load(Ordering::SeqCst)
    }
}

/// Which project to open.
pub enum Target<'a> {
    /// A folder that holds a project (Open…, a drop on the start screen).
    Folder(&'a Path),
    /// An entry of the recent list.
    Entry(i64),
}

#[derive(Default)]
pub struct AppState {
    workspace: Mutex<Option<Arc<Workspace>>>,
    open: Arc<Mutex<Option<Arc<Open>>>>,
    events: Arc<Mutex<Option<Channel<UiEvent>>>>,
    scripted: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    /// The project list in the app's data folder, opened on first use.
    pub fn workspace(&self) -> Result<Arc<Workspace>, IpcError> {
        let mut ws = lock(&self.workspace);
        if let Some(w) = ws.as_ref() {
            return Ok(w.clone());
        }
        let w = self.listen(Workspace::open_default()?);
        *ws = Some(w.clone());
        Ok(w)
    }

    /// The workspace, telling the UI and the open project's AI jobs what changes in it.
    fn listen(&self, w: Workspace) -> Arc<Workspace> {
        let w = Arc::new(w);
        // Weak: the workspace holds these callbacks.
        let (events, open, weak) = (self.events.clone(), self.open.clone(), Arc::downgrade(&w));
        w.on_models_changed(move || {
            emit(&events, UiEvent::ModelsChanged);
            refresh_blocks(&weak, &open, &events);
        });
        let (events, open, weak) = (self.events.clone(), self.open.clone(), Arc::downgrade(&w));
        w.on_runtime_changed(move || {
            emit(&events, UiEvent::RuntimeChanged);
            refresh_blocks(&weak, &open, &events);
        });
        let events = self.events.clone();
        w.on_ai_activity(move |a| {
            emit(
                &events,
                UiEvent::AiActivity {
                    activity: a.map(Into::into),
                },
            )
        });
        w
    }

    /// The open project's AI jobs follow what can run now (INV-PR-09).
    pub fn refresh_ai_blocks(&self) {
        let ws = lock(&self.workspace).as_ref().map(Arc::downgrade);
        if let Some(weak) = ws {
            refresh_blocks(&weak, &self.open, &self.events);
        }
        // What the AI row and the Meaning tab say can change with no job to block, and the project
        // screen stays mounted behind the AI models screen.
        self.emit(UiEvent::ProgressChanged);
    }

    /// App quit: the open project's run is stopped (US-03.4 AC1), a download is left
    /// Interrupted and every `llama-server` is stopped and reaped (US-09.5 AC2).
    pub fn shut_down(&self) {
        self.close();
        if let Some(w) = lock(&self.workspace).as_ref() {
            w.shut_down();
        }
    }

    /// "Open demo project" (US-14.4 AC2): opens or makes it with TD-1 from `corpus` as its
    /// sources, then reads them.
    pub fn open_demo(&self, corpus: &Path) -> Result<Arc<Open>, IpcError> {
        let ws = self.workspace()?;
        let mut open = lock(&self.open);
        close_open(open.take());
        let (project, _, entry) = ws.open_demo(corpus)?;
        let dir = PathBuf::from(&entry.path);
        let result = self.install(&mut open, project, dir, entry);
        drop(open);
        self.emit(UiEvent::ProjectChanged);
        let o = self.start_work(result?);
        self.start_ingest(&o);
        Ok(o)
    }

    /// The open project, or a plain error.
    pub fn current(&self) -> Result<Arc<Open>, IpcError> {
        lock(&self.open)
            .clone()
            .ok_or_else(|| IpcError::new("no_project", "No project is open."))
    }

    pub fn emit(&self, event: UiEvent) {
        emit(&self.events, event);
    }

    /// The UI's event channel; a new one replaces the old (a reload of the page).
    pub fn subscribe(&self, channel: Channel<UiEvent>) {
        *lock(&self.events) = Some(channel);
    }

    /// Timing runs only (`MK_UI_TIMING=1`, NFR-PERF-34): once the UI has subscribed, opens
    /// `MK_UI_TIMING_PROJECT` (made there if new) and adds `MK_UI_TIMING_SOURCE` if set, so the
    /// run needs no clicks. Without a source the project is only reopened (W3d-1, NFR-RES-01).
    pub fn scripted_run_once(&self) {
        let Some(project) = std::env::var_os("MK_UI_TIMING_PROJECT") else {
            return;
        };
        let source = std::env::var_os("MK_UI_TIMING_SOURCE");
        if self.scripted.swap(true, Ordering::SeqCst) {
            return;
        }
        let dir = Path::new(&project);
        let result = (|| {
            if !mk_core::project_exists(dir) {
                Project::open(dir)?;
            }
            Ok::<_, ProjectError>(())
        })()
        .map_err(IpcError::from)
        .and_then(|_| self.open(Target::Folder(dir)))
        .and_then(|_| match &source {
            Some(source) => self.add_source(Path::new(source)),
            None => Ok(()),
        });
        if let Err(e) = result {
            self.emit(UiEvent::Error { message: e.message });
        }
    }

    /// Creates the project `name` in a new folder in `location` and opens it (US-01.1).
    pub fn create(&self, name: &str, location: &Path) -> Result<Arc<Open>, IpcError> {
        let ws = self.workspace()?;
        // The new folder is not the open project's, so the old one can stay open until then.
        let (project, entry) = ws.create(name, location)?;
        let mut open = lock(&self.open);
        close_open(open.take());
        let dir = PathBuf::from(&entry.path);
        let result = self.install(&mut open, project, dir, entry);
        drop(open);
        self.emit(UiEvent::ProjectChanged);
        Ok(self.start_work(result?))
    }

    /// Opens a project (US-01.2). The open one is closed first; its run, if any, is stopped and
    /// picks up again on its next open. Files left waiting are read in the background unless the
    /// queue is paused (US-03.4 AC1).
    pub fn open(&self, target: Target<'_>) -> Result<Arc<Open>, IpcError> {
        let ws = self.workspace()?;
        let mut open = lock(&self.open);
        // Close the old project first: its writer and readers go before the new one opens.
        close_open(open.take());
        let (project, _, entry) = match target {
            Target::Folder(dir) => ws.open_project(dir)?,
            Target::Entry(id) => ws.open_entry(id)?,
        };
        let dir = PathBuf::from(&entry.path);
        let result = self.install(&mut open, project, dir, entry);
        drop(open);
        self.emit(UiEvent::ProjectChanged);
        Ok(self.start_work(result?))
    }

    /// Makes `project` the open one; returns it and whether it has files waiting. Its work starts
    /// in [`start_work`](Self::start_work), once the caller has let go of the `open` lock.
    fn install(
        &self,
        slot: &mut Option<Arc<Open>>,
        mut project: Project,
        dir: PathBuf,
        entry: RegistryEntry,
    ) -> Result<(Arc<Open>, bool), IpcError> {
        // OCR language files: English is written there on first use (ADR-0018).
        project.set_tessdata_dir(mk_core::tessdata_dir());
        let changes = project.subscribe();
        let runnable = project.has_runnable_work()?;
        let o = Arc::new(Open {
            project,
            dir,
            entry,
            ingesting: AtomicBool::new(false),
            again: AtomicBool::new(false),
            ai_running: AtomicBool::new(false),
            ai_again: AtomicBool::new(false),
            embedding_running: AtomicBool::new(false),
            embedding_again: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        });
        *slot = Some(o.clone());
        // Forward ConnectionsChanged until the project closes (its sender goes with it).
        let events = self.events.clone();
        std::thread::Builder::new()
            .name("mk-desktop-events".into())
            .spawn(move || {
                for _ in changes {
                    emit(&events, UiEvent::ConnectionsChanged);
                }
            })
            .map_err(|e| IpcError::new("internal", e.to_string()))?;
        Ok((o, runnable))
    }

    /// A project made or opened now: its AI jobs follow what can run (INV-PR-09) and its work
    /// goes on. Never under the `open` lock: the first check of `llama-server` tells the
    /// runtime's listeners on this thread, and they take that lock.
    fn start_work(&self, (o, runnable): (Arc<Open>, bool)) -> Arc<Open> {
        let ws = lock(&self.workspace).clone();
        if let Some(ws) = &ws {
            let _ = ws.refresh_ai_blocks(&o.project);
        }
        if runnable {
            self.start_ingest(&o);
        } else if let Some(ws) = ws {
            // Ingestion would start the lane when it ends; with none, it goes on from here.
            schedule_and_start_lane(&ws, &o, &self.events);
        }
        o
    }

    /// Closes the open project, stopping its run first (US-03.4 AC1).
    pub fn close(&self) {
        let mut open = lock(&self.open);
        close_open(open.take());
        drop(open);
        self.emit(UiEvent::ProjectChanged);
    }

    /// Adds a folder or file to the open project and ingests it (US-01.3). Adding a source that
    /// is already there only re-scans.
    pub fn add_source(&self, path: &Path) -> Result<(), IpcError> {
        let open = self.current()?;
        open.project.add_source_root(path)?;
        self.emit(UiEvent::ProjectChanged);
        self.start_ingest(&open);
        Ok(())
    }

    /// Re-scans the sources and reads what is new or changed (US-01.4).
    pub fn rescan(&self) -> Result<(), IpcError> {
        self.start_ingest(&self.current()?);
        Ok(())
    }

    /// Pauses the queue (US-03.2 AC1): returns once no file is being read, within 5 s
    /// (NFR-REL-05).
    pub fn pause(&self) -> Result<(), IpcError> {
        let open = self.current()?;
        open.project.pause()?;
        self.emit(UiEvent::ProgressChanged);
        Ok(())
    }

    /// Resumes the queue and reads on (US-03.2 AC2).
    pub fn resume(&self) -> Result<(), IpcError> {
        let open = self.current()?;
        open.project.resume()?;
        self.emit(UiEvent::ProgressChanged);
        self.start_ingest(&open);
        Ok(())
    }

    /// Cancels the queue (US-03.3): files not finished become Not processed. Returns how many.
    pub fn cancel(&self) -> Result<u32, IpcError> {
        let open = self.current()?;
        let out = open.project.cancel()?;
        self.emit(UiEvent::ProgressChanged);
        Ok(u32::try_from(out.contents).unwrap_or(u32::MAX))
    }

    /// Runs `ingest` on a background thread, unless one runs already; then that run goes again
    /// when it ends, so new work is picked up. While it runs, `ProgressChanged` goes out when
    /// the counts or the current files change, at most every 500 ms.
    pub fn start_ingest(&self, open: &Arc<Open>) {
        if open.closed.load(Ordering::SeqCst) {
            return;
        }
        open.again.store(true, Ordering::SeqCst);
        if open.ingesting.swap(true, Ordering::SeqCst) {
            return;
        }
        let (run, events) = (open.clone(), self.events.clone());
        let ws = lock(&self.workspace).clone();
        let spawned = std::thread::Builder::new()
            .name("mk-desktop-ingest".into())
            .spawn(move || {
                let open = run;
                let ticker = spawn_ticker(&open, &events);
                let mut stopped;
                loop {
                    open.again.store(false, Ordering::SeqCst);
                    emit(&events, UiEvent::IngestStarted);
                    let result = open.project.ingest();
                    stopped = matches!(&result, Ok(s) if s.interrupted.is_some());
                    let finished = finished_event(result);
                    open.ingesting.store(false, Ordering::SeqCst);
                    emit(&events, finished);
                    // Go again if new work came in meanwhile and no other run took it; never
                    // after a pause, cancel or close.
                    if stopped
                        || open.closed.load(Ordering::SeqCst)
                        || !open.again.load(Ordering::SeqCst)
                        || open.ingesting.swap(true, Ordering::SeqCst)
                    {
                        break;
                    }
                }
                if let Some(t) = ticker {
                    let _ = t.join();
                }
                // Stage A is done with what it could read: Stage B takes over, unless the run
                // was stopped or another one has started and will do this when it ends.
                if let Some(ws) = ws
                    && !stopped
                    && !open.is_ingesting()
                {
                    schedule_and_start_lane(&ws, &open, &events);
                }
            });
        if let Err(e) = spawned {
            open.ingesting.store(false, Ordering::SeqCst);
            self.emit(UiEvent::Error {
                message: format!("Could not start reading the files: {e}"),
            });
        }
    }

    /// Starts the AI lane on its own thread, unless it runs; then it looks for work once more.
    pub fn start_ai_lane(&self, open: &Arc<Open>) {
        if let Ok(ws) = self.workspace() {
            start_lane(&ws, open, &self.events);
        }
    }

    /// Starts the embeddings lane on its own thread, unless it runs; then it looks once more.
    pub fn start_embedding_lane(&self, open: &Arc<Open>) {
        if let Ok(ws) = self.workspace() {
            start_embeddings(&ws, open, &self.events);
        }
    }

    /// Makes the meaning index on its own thread, off the writer, unless that is going on already
    /// or the lane will do it when it ends. `ProgressChanged` tells the tab it is ready.
    pub fn prepare_meaning(&self, open: &Arc<Open>) {
        if open.closed.load(Ordering::SeqCst)
            || open.project.meaning_index_building()
            || open.embedding_running.load(Ordering::SeqCst)
        {
            return;
        }
        let (open, events) = (open.clone(), self.events.clone());
        let spawned = std::thread::Builder::new()
            .name("mk-desktop-meaning-index".into())
            .spawn(move || match open.project.prepare_meaning_index() {
                Ok(true) => emit(&events, UiEvent::ProgressChanged),
                Ok(false) => {}
                Err(e) => emit(
                    &events,
                    UiEvent::Error {
                        message: format!("Could not prepare meaning search: {e}"),
                    },
                ),
            });
        if let Err(e) = spawned {
            self.emit(UiEvent::Error {
                message: format!("Could not prepare meaning search: {e}"),
            });
        }
    }
}

/// The units not covered yet go through the start rule (gap A3), then the lane runs what may run.
fn schedule_and_start_lane(
    ws: &Arc<Workspace>,
    open: &Arc<Open>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) {
    if open.closed.load(Ordering::SeqCst) {
        return;
    }
    if let Err(e) = ws.schedule_stage_b(&open.project) {
        emit(
            events,
            UiEvent::Error {
                message: format!("Could not plan AI extraction: {e}"),
            },
        );
        return;
    }
    // The embeddings lane has its own start rule and its own model (ADR-0006).
    if let Err(e) = ws.schedule_embeddings(&open.project) {
        emit(
            events,
            UiEvent::Error {
                message: format!("Could not plan embeddings: {e}"),
            },
        );
    }
    emit(events, UiEvent::ProgressChanged);
    start_lane(ws, open, events);
    start_embeddings(ws, open, events);
}

fn start_embeddings(
    ws: &Arc<Workspace>,
    open: &Arc<Open>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) {
    if open.closed.load(Ordering::SeqCst) {
        return;
    }
    open.embedding_again.store(true, Ordering::SeqCst);
    if open.embedding_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let (lane_ws, run, lane_events) = (ws.clone(), open.clone(), events.clone());
    let spawned = std::thread::Builder::new()
        .name("mk-desktop-embeddings".into())
        .spawn(move || {
            let (ws, open, events) = (lane_ws, run, lane_events);
            let ticker = spawn_embedding_ticker(&ws, &open, &events);
            loop {
                open.embedding_again.store(false, Ordering::SeqCst);
                let result = ws.run_embeddings(&open.project);
                open.embedding_running.store(false, Ordering::SeqCst);
                if let Err(e) = result {
                    emit(
                        &events,
                        UiEvent::Error {
                            message: format!("Embeddings stopped: {e}"),
                        },
                    );
                }
                emit(&events, UiEvent::ProgressChanged);
                if open.closed.load(Ordering::SeqCst)
                    || !open.embedding_again.load(Ordering::SeqCst)
                    || open.embedding_running.swap(true, Ordering::SeqCst)
                {
                    break;
                }
            }
            if let Some(t) = ticker {
                let _ = t.join();
            }
        });
    if let Err(e) = spawned {
        open.embedding_running.store(false, Ordering::SeqCst);
        emit(
            events,
            UiEvent::Error {
                message: format!("Could not start embeddings: {e}"),
            },
        );
    }
}

/// `ProgressChanged` while the embeddings lane runs, when its row would change.
fn spawn_embedding_ticker(
    ws: &Arc<Workspace>,
    open: &Arc<Open>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) -> Option<std::thread::JoinHandle<()>> {
    let (ws, open, events) = (ws.clone(), open.clone(), events.clone());
    std::thread::Builder::new()
        .name("mk-desktop-embeddings-progress".into())
        .spawn(move || {
            let mut last = None;
            while open.embedding_running.load(Ordering::SeqCst) {
                std::thread::sleep(PROGRESS_EVERY);
                let Ok(v) = ws.meaning_view(&open.project) else {
                    continue;
                };
                let now = Some((v.items_done, v.items_total, v.state));
                if now != last {
                    last = now;
                    emit(&events, UiEvent::ProgressChanged);
                }
            }
        })
        .ok()
}

fn start_lane(
    ws: &Arc<Workspace>,
    open: &Arc<Open>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) {
    if open.closed.load(Ordering::SeqCst) {
        return;
    }
    open.ai_again.store(true, Ordering::SeqCst);
    if open.ai_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let (lane_ws, run, lane_events) = (ws.clone(), open.clone(), events.clone());
    let spawned = std::thread::Builder::new()
        .name("mk-desktop-ai".into())
        .spawn(move || {
            let (ws, open, events) = (lane_ws, run, lane_events);
            let ticker = spawn_ai_ticker(&ws, &open, &events);
            loop {
                open.ai_again.store(false, Ordering::SeqCst);
                let result = ws.run_stage_b(&open.project);
                open.ai_running.store(false, Ordering::SeqCst);
                if let Err(e) = result {
                    emit(
                        &events,
                        UiEvent::Error {
                            message: format!("AI extraction stopped: {e}"),
                        },
                    );
                }
                emit(&events, UiEvent::ProgressChanged);
                if open.closed.load(Ordering::SeqCst)
                    || !open.ai_again.load(Ordering::SeqCst)
                    || open.ai_running.swap(true, Ordering::SeqCst)
                {
                    break;
                }
            }
            if let Some(t) = ticker {
                let _ = t.join();
            }
        });
    if let Err(e) = spawned {
        open.ai_running.store(false, Ordering::SeqCst);
        emit(
            events,
            UiEvent::Error {
                message: format!("Could not start AI extraction: {e}"),
            },
        );
    }
}

/// `ProgressChanged` while the lane runs, when its row would change.
fn spawn_ai_ticker(
    ws: &Arc<Workspace>,
    open: &Arc<Open>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) -> Option<std::thread::JoinHandle<()>> {
    let (ws, open, events) = (ws.clone(), open.clone(), events.clone());
    std::thread::Builder::new()
        .name("mk-desktop-ai-progress".into())
        .spawn(move || {
            let mut last = None;
            while open.ai_running.load(Ordering::SeqCst) {
                std::thread::sleep(PROGRESS_EVERY);
                let Ok(v) = ws.ai_extraction_view(&open.project) else {
                    continue;
                };
                let now = Some((v.units_done, v.units_total, v.state));
                if now != last {
                    last = now;
                    emit(&events, UiEvent::ProgressChanged);
                }
            }
        })
        .ok()
}

/// Stops the run of a project that is closing and waits until its threads let it go, so its
/// writer is gone before another project opens.
fn close_open(open: Option<Arc<Open>>) {
    let Some(open) = open else {
        return;
    };
    open.closed.store(true, Ordering::SeqCst);
    open.project.stop_for_close();
    let start = Instant::now();
    while open.is_ingesting() && start.elapsed() < CLOSE_WAIT {
        std::thread::sleep(Duration::from_millis(10));
    }
    // A call in flight may take minutes: its reply fails its token and is dropped (NFR-REL-05).
    let start = Instant::now();
    while (open.ai_running.load(Ordering::SeqCst) || open.embedding_running.load(Ordering::SeqCst))
        && start.elapsed() < CLOSE_WAIT_AI
    {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `ProgressChanged` while `open` is ingesting, when the progress panel would change.
fn spawn_ticker(
    open: &Arc<Open>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) -> Option<std::thread::JoinHandle<()>> {
    let (open, events) = (open.clone(), events.clone());
    std::thread::Builder::new()
        .name("mk-desktop-progress".into())
        .spawn(move || {
            let mut last = None;
            while open.is_ingesting() {
                std::thread::sleep(PROGRESS_EVERY);
                let Ok(v) = open.project.progress_view() else {
                    continue;
                };
                let now = Some((v.files_done, v.files_total, v.current_files, v.paused));
                if now != last {
                    last = now;
                    emit(&events, UiEvent::ProgressChanged);
                }
            }
        })
        .ok()
}

fn finished_event(result: Result<IngestSummary, ProjectError>) -> UiEvent {
    let n = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
    match result {
        Ok(s) => UiEvent::IngestFinished {
            done: n(s.stage_a.done),
            failed: n(s.stage_a.failed),
            error: None,
            interrupted: s.interrupted.map(|i| format!("{i:?}").to_lowercase()),
            added: n(s.rescan.outcome.added),
            duplicates: n(s.rescan.outcome.duplicates),
            skipped: n(s.rescan.skipped.len()),
            skipped_files: s
                .rescan
                .skipped
                .iter()
                .take(MAX_SKIPPED)
                .map(|(_, sk)| SkippedFile {
                    path: sk.rel_path.clone(),
                    reason: sk.reason.to_string(),
                })
                .collect(),
        },
        Err(e) => UiEvent::IngestFinished {
            done: 0,
            failed: 0,
            error: Some(e.to_string()),
            interrupted: None,
            added: 0,
            duplicates: 0,
            skipped: 0,
            skipped_files: Vec::new(),
        },
    }
}

/// Blocks or unblocks the open project's AI jobs; `ProgressChanged` when one changed.
fn refresh_blocks(
    ws: &Weak<Workspace>,
    open: &Mutex<Option<Arc<Open>>>,
    events: &Arc<Mutex<Option<Channel<UiEvent>>>>,
) {
    let (Some(ws), Some(o)) = (ws.upgrade(), lock(open).clone()) else {
        return;
    };
    if ws.refresh_ai_blocks(&o.project).unwrap_or(false) {
        emit(events, UiEvent::ProgressChanged);
        // A job was unblocked, or the lane finds out that it was blocked.
        start_lane(&ws, &o, events);
        start_embeddings(&ws, &o, events);
    }
}

fn emit(events: &Mutex<Option<Channel<UiEvent>>>, event: UiEvent) {
    if let Some(c) = lock(events).as_ref() {
        // The page may be reloading; the next `subscribe` brings a new channel.
        let _ = c.send(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use mk_core::{
        AiChoice, DownloadState, EgressConfig, FixedProbe, Gpu, GpuVendor, HardwareProfile,
        ModelRole, ModelsConfig, RuntimeConfig, TlsRoots,
    };
    use mk_egress::testkit::{FakeKeyring, ScriptedResolver};
    use mk_models::GB;
    use mk_models::testkit::{
        FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
    };

    use super::*;

    const WAIT: Duration = Duration::from_secs(30);

    /// An app whose `llama-server` was not checked yet, as just after launch, with a model
    /// installed and chosen.
    fn just_launched(tmp: &Path) -> (AppState, Arc<Workspace>, FakeHf) {
        let bytes = test_bytes(64 * 1024, 1);
        let model = test_model("x", ModelRole::Extraction, std::slice::from_ref(&bytes));
        let hf = FakeHf::serving(&[(&model, std::slice::from_ref(&bytes))]);
        let ws = Workspace::open_with_runtime(
            &tmp.join("app"),
            EgressConfig {
                resolver: Arc::new(ScriptedResolver::new()),
                secrets: Some(Arc::new(FakeKeyring::new())),
                roots: TlsRoots::Platform,
                call_timeout: Duration::from_secs(5),
            },
            ModelsConfig {
                models_dir: Some(tmp.join("models")),
                catalog: test_catalog(vec![model]),
                probe: Arc::new(FixedProbe(HardwareProfile {
                    os_family: "linux".into(),
                    os: "Linux test".into(),
                    cpu: "Test CPU (8 threads)".into(),
                    ram_bytes: 64 * GB,
                    gpus: vec![Gpu {
                        vendor: GpuVendor::Nvidia,
                        name: "NVIDIA Test".into(),
                        vram_bytes: Some(8_550_000_000),
                        driver: Some("580.10".into()),
                    }],
                    unified_bytes: None,
                    apple_silicon: false,
                    free_disk_bytes: Some(100 * GB),
                    disk_path: "/models".into(),
                })),
                download: hf.config(NamedResolver::new(&[ORIGIN_HOST])),
            },
            RuntimeConfig {
                path_var: Some(Default::default()),
                ..RuntimeConfig::default()
            },
        )
        .unwrap();
        ws.start_download("x").unwrap();
        let end = Instant::now() + WAIT;
        while !ws
            .model_catalog()
            .unwrap()
            .iter()
            .any(|m| m.state == DownloadState::Installed)
        {
            assert!(Instant::now() < end, "the model did not install");
            std::thread::sleep(Duration::from_millis(5));
        }
        ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("x".into())))
            .unwrap();
        let state = AppState::default();
        let ws = state.listen(ws);
        *lock(&state.workspace) = Some(ws.clone());
        (state, ws, hf)
    }

    /// `f` on its own thread: a hang fails the test instead of stopping the whole run.
    fn must_return(what: &str, f: impl FnOnce() -> Result<(), IpcError> + Send + 'static) {
        let (done, ended) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(f());
        });
        match ended.recv_timeout(WAIT) {
            Ok(result) => result.unwrap(),
            Err(_) => panic!("{what} never returned"),
        }
    }

    /// US-06.2 AC2: another AI chosen changes what the Meaning tab and the AI row say even with no
    /// job to block, and the project screen stays mounted behind the AI models screen, so it has
    /// to be told.
    #[test]
    fn us_06_2_ac2_a_new_choice_tells_the_open_project() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, _ws, _hf) = just_launched(tmp.path());
        state.create("Case", tmp.path()).unwrap();
        let (sent, seen) = mpsc::channel();
        state.subscribe(Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                let _ = sent.send(json);
            }
            Ok(())
        }));
        // Making the project and a first look send their own; the second look changes no job.
        state.refresh_ai_blocks();
        while seen.recv_timeout(Duration::from_millis(500)).is_ok() {}
        state.refresh_ai_blocks();
        let event = seen.recv_timeout(Duration::from_secs(2));
        state.close();
        assert!(event.expect("no event came").contains("progressChanged"));
    }

    /// Checking `llama-server` for the first time tells the runtime's listeners, and they take
    /// the `open` lock: making a project must not hold it then (US-01.1, INV-PR-09).
    #[test]
    fn us_01_1_creating_a_project_just_after_launch_does_not_hang() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, _ws, _hf) = just_launched(tmp.path());
        let location = tmp.path().to_path_buf();
        must_return("creating the project", move || {
            state.create("Case", &location)?;
            state.close();
            Ok(())
        });
    }

    /// The same for a project opened from its folder (US-01.2, INV-PR-09).
    #[test]
    fn us_01_2_opening_a_project_just_after_launch_does_not_hang() {
        let tmp = tempfile::tempdir().unwrap();
        let (state, ws, _hf) = just_launched(tmp.path());
        let (project, entry) = ws.create("Case", tmp.path()).unwrap();
        drop(project);
        must_return("opening the project", move || {
            state.open(Target::Folder(Path::new(&entry.path)))?;
            state.close();
            Ok(())
        });
    }
}
