//! The app's state: the project list (`app.db`), at most one open project (gap A7, decided
//! 2026-10-28) and the UI's event channel. Ingestion runs on a plain background thread; reads run
//! on the store's reader pool next to it (US-03.1 AC3). Events go over a `tauri::ipc::Channel`
//! the UI passes to `subscribe`, so the UI needs no `core:event` permission (ADR-0003).
//!
//! Opening, closing or switching projects while files are read stops the run first: the files
//! in a worker go back to waiting and the queue keeps its state, so a running queue runs again
//! on the next open and a paused one stays paused (US-03.4 AC1).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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

/// One open project.
pub struct Open {
    pub project: Project,
    pub dir: PathBuf,
    pub entry: RegistryEntry,
    /// A re-scan or Stage A run is going on.
    ingesting: AtomicBool,
    /// A source was added during the run: run again when it ends.
    again: AtomicBool,
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
    open: Mutex<Option<Arc<Open>>>,
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
        let w = Arc::new(Workspace::open_default()?);
        *ws = Some(w.clone());
        Ok(w)
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
        result
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
        result
    }

    fn install(
        &self,
        slot: &mut Option<Arc<Open>>,
        mut project: Project,
        dir: PathBuf,
        entry: RegistryEntry,
    ) -> Result<Arc<Open>, IpcError> {
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
        if runnable {
            self.start_ingest(&o);
        }
        Ok(o)
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
        let spawned = std::thread::Builder::new()
            .name("mk-desktop-ingest".into())
            .spawn(move || {
                let open = run;
                let ticker = spawn_ticker(&open, &events);
                loop {
                    open.again.store(false, Ordering::SeqCst);
                    emit(&events, UiEvent::IngestStarted);
                    let result = open.project.ingest();
                    let stopped = matches!(&result, Ok(s) if s.interrupted.is_some());
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
            });
        if let Err(e) = spawned {
            open.ingesting.store(false, Ordering::SeqCst);
            self.emit(UiEvent::Error {
                message: format!("Could not start reading the files: {e}"),
            });
        }
    }
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

fn emit(events: &Mutex<Option<Channel<UiEvent>>>, event: UiEvent) {
    if let Some(c) = lock(events).as_ref() {
        // The page may be reloading; the next `subscribe` brings a new channel.
        let _ = c.send(event);
    }
}
