//! The app's state: at most one open project (gap A7, decided 2026-10-28) and the UI's event
//! channel. Ingestion runs on a plain background thread; reads run on the store's reader pool
//! next to it (US-03.1 AC3). Events go over a `tauri::ipc::Channel` the UI passes to
//! `subscribe`, so the UI needs no `core:event` permission (ADR-0003).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mk_core::Project;
use tauri::ipc::Channel;

use crate::ipc::{IpcError, UiEvent};

/// One open project.
pub struct Open {
    pub project: Project,
    pub dir: PathBuf,
    /// A re-scan or Stage A run is going on.
    ingesting: AtomicBool,
    /// A source was added during the run: run again when it ends.
    again: AtomicBool,
}

impl Open {
    pub fn is_ingesting(&self) -> bool {
        self.ingesting.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct AppState {
    open: Mutex<Option<Arc<Open>>>,
    events: Arc<Mutex<Option<Channel<UiEvent>>>>,
    scripted: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn busy() -> IpcError {
    IpcError::new(
        "busy",
        "Files are still being read. Wait until that finishes, then try again.",
    )
}

impl AppState {
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
    /// `MK_UI_TIMING_PROJECT` and adds `MK_UI_TIMING_SOURCE`, so the run needs no clicks.
    pub fn scripted_run_once(&self) {
        let (Some(project), Some(source)) = (
            std::env::var_os("MK_UI_TIMING_PROJECT"),
            std::env::var_os("MK_UI_TIMING_SOURCE"),
        ) else {
            return;
        };
        if self.scripted.swap(true, Ordering::SeqCst) {
            return;
        }
        let result = self
            .open(Path::new(&project))
            .and_then(|_| self.add_source(Path::new(&source)));
        if let Err(e) = result {
            self.emit(UiEvent::Error { message: e.message });
        }
    }

    /// Opens the project in `dir`, creating it when the folder is new or empty. Refused while
    /// the open project is being ingested (cancel and pause come in M3). Items a killed run left
    /// are finished in the background (ADR-0006).
    pub fn open(&self, dir: &Path) -> Result<Arc<Open>, IpcError> {
        let mut open = lock(&self.open);
        if open.as_ref().is_some_and(|o| o.is_ingesting()) {
            return Err(busy());
        }
        if !mk_core::project_exists(dir) && !is_empty_dir(dir) {
            return Err(IpcError::new(
                "not_a_project",
                format!(
                    "{} is not a Mangkon project and is not empty. Pick an empty folder for a \
                     new project.",
                    dir.display()
                ),
            ));
        }
        // Close the old project first: its writer and readers go before the new one opens.
        *open = None;
        let (project, _) = Project::open(dir)?;
        let changes = project.subscribe();
        let pending = project.counts()?.queued > 0;
        let o = Arc::new(Open {
            project,
            dir: dir.to_path_buf(),
            ingesting: AtomicBool::new(false),
            again: AtomicBool::new(false),
        });
        *open = Some(o.clone());
        drop(open);
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
        self.emit(UiEvent::ProjectChanged);
        if pending {
            self.start_ingest(&o);
        }
        Ok(o)
    }

    /// Closes the open project. Refused while it is being ingested.
    pub fn close(&self) -> Result<(), IpcError> {
        let mut open = lock(&self.open);
        if open.as_ref().is_some_and(|o| o.is_ingesting()) {
            return Err(busy());
        }
        *open = None;
        drop(open);
        self.emit(UiEvent::ProjectChanged);
        Ok(())
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

    /// Runs `ingest` on a background thread, unless one runs already; then that run goes again
    /// when it ends, so the new source is picked up.
    fn start_ingest(&self, open: &Arc<Open>) {
        open.again.store(true, Ordering::SeqCst);
        if open.ingesting.swap(true, Ordering::SeqCst) {
            return;
        }
        let (run, events) = (open.clone(), self.events.clone());
        let spawned = std::thread::Builder::new()
            .name("mk-desktop-ingest".into())
            .spawn(move || {
                let open = run;
                loop {
                    open.again.store(false, Ordering::SeqCst);
                    emit(&events, UiEvent::IngestStarted);
                    let finished = match open.project.ingest() {
                        Ok(s) => UiEvent::IngestFinished {
                            done: s.stage_a.done as u32,
                            failed: s.stage_a.failed as u32,
                            error: None,
                        },
                        Err(e) => UiEvent::IngestFinished {
                            done: 0,
                            failed: 0,
                            error: Some(e.to_string()),
                        },
                    };
                    open.ingesting.store(false, Ordering::SeqCst);
                    emit(&events, finished);
                    // Go again if a source came in meanwhile and no other run took it.
                    if !open.again.load(Ordering::SeqCst)
                        || open.ingesting.swap(true, Ordering::SeqCst)
                    {
                        return;
                    }
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

fn emit(events: &Mutex<Option<Channel<UiEvent>>>, event: UiEvent) {
    if let Some(c) = lock(events).as_ref() {
        // The page may be reloading; the next `subscribe` brings a new channel.
        let _ = c.send(event);
    }
}

fn is_empty_dir(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}
