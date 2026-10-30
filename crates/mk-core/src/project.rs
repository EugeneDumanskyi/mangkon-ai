//! One open project: its store, source roots, re-scan and the Stage A lane.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mk_corpus::fs::{FileTypes, Skipped};
use mk_corpus::{RescanPlan, Resolved, RootId};
use mk_graph::DecisionKind;
use mk_jobs::Trigger;
use mk_kernel::ContentHash;
use mk_parsers::Format;
use mk_store::{OpenReport, Progress, Recovery, RescanChange, RescanOutcome, Store, StoreError};
use mk_worker::Settings;
use mk_workspace::{ProjectLayout, RootKind};

use crate::control::{RunControl, Stop};
use crate::events::{ConnectionsChanged, DEBOUNCE, Events};
use crate::stage_a::{Failure, SPILL_AT, Staging};
use crate::workers::{ItemResult, WorkerConfig, WorkerError, WorkerProcess, WorkerStop, own_rss};

#[derive(Debug)]
pub enum ProjectError {
    Store(StoreError),
    Io(PathBuf, std::io::Error),
    /// No worker could be started; the remaining items stay Pending.
    Worker(WorkerError),
    /// A setting was refused; the plain reason.
    Invalid(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Store(e) => e.fmt(f),
            ProjectError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            ProjectError::Worker(e) => e.fmt(f),
            ProjectError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<StoreError> for ProjectError {
    fn from(e: StoreError) -> Self {
        ProjectError::Store(e)
    }
}

impl From<WorkerError> for ProjectError {
    fn from(e: WorkerError) -> Self {
        ProjectError::Worker(e)
    }
}

/// What opening did: the schema steps (domain §7.9) and the restart recovery (ADR-0006).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenSummary {
    pub schema: OpenReport,
    pub recovered: Recovery,
}

/// What a re-scan found and did (US-01.3, US-01.4).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RescanSummary {
    /// Supported files found.
    pub walked: usize,
    /// Files read and hashed (new or changed size/time, or all with `full`).
    pub hashed: usize,
    /// Paths with nothing to do.
    pub unchanged: usize,
    pub outcome: RescanOutcome,
    /// Unsupported files and names that cannot be stored (INV-CO-11), with their root.
    pub skipped: Vec<(RootId, Skipped)>,
    /// Folders or files that could not be read.
    pub errors: Vec<(RootId, String, String)>,
    /// Pause, cancel or close came while hashing: nothing was committed; the next re-scan
    /// starts over.
    pub interrupted: bool,
}

/// What a Stage A run did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StageASummary {
    pub done: usize,
    pub failed: usize,
    /// Commits that changed connection rows.
    pub changed_commits: usize,
    /// For each committed item: time from "parse finished" to "commit done", which includes
    /// waiting for the writer (spike S2c, NFR-PERF-34).
    pub commit_latencies: Vec<Duration>,
    /// Highest sampled resident memory of this process plus its workers, in bytes, sampled
    /// every [`WorkerConfig::sample_every`] (NFR-RES-02).
    pub peak_memory: u64,
    /// The same for this process alone (results are buffered here until commit).
    pub peak_core_memory: u64,
    /// Workers started: the first of each lane, replacements and recycled ones.
    pub workers_started: usize,
    /// Workers stopped in the middle of an item (deadline, memory, crash), in order.
    pub stops: Vec<WorkerStop>,
    /// Items stopped by pause, cancel or close and put back to Pending (ADR-0006).
    pub interrupted: usize,
}

/// What [`Project::ingest`] did: the re-scan, then the Stage A run.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct IngestSummary {
    pub rescan: RescanSummary,
    pub stage_a: StageASummary,
    /// The run stopped early for this request (US-03.2, US-03.3, US-03.4).
    pub interrupted: Option<Stop>,
}

/// What [`Project::rebuild`] did (ADR-0005 "full rebuild").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RebuildSummary {
    pub identifiers: usize,
    /// Connection rows, ignored ones included.
    pub connections: usize,
    /// The rebuilt rows equal the rows the incremental commits had left.
    pub same_as_incremental: bool,
    /// The rebuilt rows equal the reference model over the facts.
    pub consistent: bool,
}

/// A project's health after a crash or a kill (NFR-REL-02 kill test, ADR-0006).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HealthReport {
    /// `PRAGMA integrity_check` lines other than `ok`.
    pub integrity_errors: Vec<String>,
    /// Rows reported by `PRAGMA foreign_key_check`.
    pub foreign_key_errors: usize,
    /// The derived tables equal a rebuild from the facts (ADR-0005).
    pub derived_consistent: bool,
    pub progress: Progress,
}

impl HealthReport {
    pub fn is_healthy(&self) -> bool {
        self.integrity_errors.is_empty() && self.foreign_key_errors == 0 && self.derived_consistent
    }
}

// The desktop reads from one thread while Stage A runs on another (US-03.1 AC3).
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Project>();
};

/// An open project (ADR-0002: the shell and the CLI reach the core only through services like
/// this one).
pub struct Project {
    layout: ProjectLayout,
    store: Store,
    events: Events,
    workers: usize,
    worker_config: WorkerConfig,
    /// The OCR language files (ADR-0018); `None` leaves OCR off.
    tessdata: Option<PathBuf>,
    /// The page view's render worker, started on the first page shown (ADR-0011).
    renderer: Mutex<Option<WorkerProcess>>,
    /// Pause, cancel and close requests (US-03.2–03.4).
    control: RunControl,
    /// Held while a re-scan and Stage A run: pause and cancel wait on it for the run to end.
    run: Mutex<()>,
    running: AtomicBool,
    /// When this run started: the monotonic clock for the ETA and UTC ms for the store.
    run_started: Mutex<Option<(Instant, i64)>>,
    /// Item → display path, for "current file" (US-03.1 AC1).
    in_flight: Mutex<BTreeMap<i64, String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Milliseconds since the Unix epoch, UTC.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Whether `dir` holds a project database.
pub fn project_exists(dir: &Path) -> bool {
    ProjectLayout::new(dir).database().is_file()
}

/// The file types with a parser (US-01.3 AC3).
pub fn file_types() -> FileTypes {
    FileTypes::new(Format::ALL.iter().flat_map(|f| f.extensions()))
}

/// "File too large (limit: X)" for a file over its format's size limit (NFR-CAP-05/06/07).
fn too_large(path: &str, file_type: &str) -> Option<Failure> {
    let limit = Format::from_name(file_type)?.size_limit()?;
    let size = std::fs::metadata(path).ok()?.len();
    (size > limit).then(|| {
        let label = if limit % (1 << 30) == 0 {
            format!("{} GB", limit >> 30)
        } else {
            format!("{} MB", limit >> 20)
        };
        Failure::new("file_too_large", format!("File too large (limit: {label})"))
    })
}

impl Project {
    /// Opens the project in `dir`, creating it if needed: schema check, backup and migrations
    /// (domain §7.9), then restart recovery (INV-PR-05/07), then the writer and readers run.
    pub fn open(dir: &Path) -> Result<(Project, OpenSummary), ProjectError> {
        let layout = ProjectLayout::new(dir);
        std::fs::create_dir_all(dir).map_err(|e| ProjectError::Io(dir.into(), e))?;
        let (store, schema) = Store::open(&layout.database(), &layout.backups())?;
        // The app that created or migrated the project, named if a too-new project is refused
        // later (US-01.2 AC3).
        if schema.from_version < schema.to_version {
            store.stamp(None, env!("CARGO_PKG_VERSION"))?;
        }
        let recovered = store.recover_after_restart()?;
        // Staged results of a killed run: their items run again (ADR-0006).
        crate::stage_a::clear_staging(&layout.cache())
            .map_err(|e| ProjectError::Io(layout.cache(), e))?;
        let workers =
            mk_jobs::stage_a_limit(std::thread::available_parallelism().map_or(1, |n| n.get()));
        Ok((
            Project {
                layout,
                store,
                events: Events::start(DEBOUNCE),
                workers,
                worker_config: WorkerConfig::default(),
                tessdata: None,
                renderer: Mutex::new(None),
                control: RunControl::default(),
                run: Mutex::new(()),
                running: AtomicBool::new(false),
                run_started: Mutex::new(None),
                in_flight: Mutex::new(BTreeMap::new()),
            },
            OpenSummary { schema, recovered },
        ))
    }

    pub fn layout(&self) -> &ProjectLayout {
        &self.layout
    }

    /// The store, for reads (and for tests and benchmarks that write through its API).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The Stage A lane width: at most this many items are in progress at once.
    pub fn stage_a_workers(&self) -> usize {
        self.workers
    }

    /// Overrides the Stage A lane width (default cores − 1, NFR-PERF-25).
    pub fn set_stage_a_workers(&mut self, n: usize) {
        self.workers = n.max(1);
    }

    /// How Stage A workers are started and limited (default: this executable with `--worker`).
    pub fn worker_config_mut(&mut self) -> &mut WorkerConfig {
        &mut self.worker_config
    }

    pub(crate) fn worker_config(&self) -> &WorkerConfig {
        &self.worker_config
    }

    pub(crate) fn renderer(&self) -> &Mutex<Option<WorkerProcess>> {
        &self.renderer
    }

    /// The folder of OCR language files (ADR-0018); `None` until set.
    pub fn tessdata_dir(&self) -> Option<&Path> {
        self.tessdata.as_deref()
    }

    /// Sets the folder of OCR language files: the apps pass [`crate::tessdata_dir`], tests a
    /// temp folder. `None` (the default) turns OCR off: files that need it fail with a plain
    /// reason.
    pub fn set_tessdata_dir(&mut self, dir: Option<PathBuf>) {
        self.tessdata = dir;
    }

    /// The project's OCR languages, `+`-joined; `eng` until set (US-02.2 AC4).
    pub fn ocr_languages(&self) -> Result<String, ProjectError> {
        Ok(self.store.ocr_languages()?)
    }

    /// Sets the project's OCR languages, `+`-joined codes such as `eng+deu` (US-02.2 AC4).
    /// They take effect for files read after this; a language whose file is not installed
    /// fails the files that need OCR with "OCR language not installed: <code>" (ADR-0018).
    pub fn set_ocr_languages(&self, languages: &str) -> Result<(), ProjectError> {
        let codes = mk_parsers::ocr::languages(languages);
        if codes.is_empty() {
            return Err(ProjectError::Invalid("No OCR language given".into()));
        }
        if let Some(bad) = codes.iter().find(|c| !mk_parsers::ocr::valid_language(c)) {
            return Err(ProjectError::Invalid(format!(
                "Not an OCR language code: {bad}"
            )));
        }
        Ok(self.store.set_ocr_languages(&codes.join("+"))?)
    }

    /// The OCR languages installed in [`Project::tessdata_dir`], after writing the built-in
    /// English file there if it is missing or differs (ADR-0018).
    pub fn installed_ocr_languages(&self) -> Result<Vec<String>, ProjectError> {
        let Some(dir) = &self.tessdata else {
            return Ok(Vec::new());
        };
        mk_parsers::ocr::install_english(dir).map_err(|e| ProjectError::Io(dir.clone(), e))?;
        Ok(mk_parsers::ocr::installed_languages(dir))
    }

    /// What the workers of this run need to OCR: the project's languages and the language
    /// folder, with English written there first. Without a folder, or if English cannot be
    /// written, OCR is off and files that need it fail with a plain reason.
    fn ocr_settings(&self) -> Result<Settings, ProjectError> {
        let tessdata = self.tessdata.as_ref().and_then(|dir| {
            mk_parsers::ocr::install_english(dir).ok()?;
            dir.to_str().map(str::to_string)
        });
        Ok(Settings {
            ocr_languages: self.store.ocr_languages()?,
            tessdata,
            ..Settings::default()
        })
    }

    /// `ConnectionsChanged` events, debounced 500 ms.
    pub fn subscribe(&self) -> Receiver<ConnectionsChanged> {
        self.events.subscribe()
    }

    /// A commit outside the lanes changed connection rows.
    pub(crate) fn changed(&self) {
        self.events.changed();
    }

    /// Adds a folder or a single file as a source root. The path is stored absolute. A path at
    /// or inside a folder root already in the project adds no root: its removed paths there are
    /// found again (US-01.5, the exclusions are cleared).
    pub fn add_source_root(&self, path: &Path) -> Result<i64, ProjectError> {
        let abs = std::fs::canonicalize(path).map_err(|e| ProjectError::Io(path.into(), e))?;
        for root in self.store.source_roots()? {
            if root.kind != RootKind::Folder.as_str() {
                continue;
            }
            let Ok(rel) = abs.strip_prefix(&root.path) else {
                continue;
            };
            let rel: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
            if let Some(rel) = rel {
                self.store.clear_exclusions(root.id, &rel.join("/"))?;
                return Ok(root.id);
            }
        }
        let kind = if abs.is_file() {
            RootKind::File
        } else {
            RootKind::Folder
        };
        let s = abs.to_str().ok_or_else(|| {
            ProjectError::Io(
                abs.clone(),
                std::io::Error::other("path is not valid Unicode"),
            )
        })?;
        Ok(self.store.add_source_root(s, kind.as_str())?)
    }

    /// Re-scans every source root (architecture §5.4): walk, compare size and time with the
    /// stored references, hash only new or changed files (all of them with `full`), then commit
    /// the outcome in one transaction. Unchanged files create no work (INV-PR-08).
    pub fn rescan(&self, full: bool) -> Result<RescanSummary, ProjectError> {
        let types = file_types();
        let roots = self.store.source_roots()?;
        let mut summary = RescanSummary::default();
        let mut walked = Vec::new();
        let mut paths: HashMap<RootId, PathBuf> = HashMap::new();
        for root in &roots {
            let path = PathBuf::from(&root.path);
            let excluded = self.store.exclusions(root.id)?;
            let w = mk_corpus::fs::walk_excluding(root.id, &path, &types, &excluded);
            walked.extend(w.entries);
            summary
                .skipped
                .extend(w.skipped.into_iter().map(|s| (root.id, s)));
            summary
                .errors
                .extend(w.errors.into_iter().map(|(p, e)| (root.id, p, e)));
            paths.insert(root.id, path);
        }
        summary.walked = walked.len();
        let stored = self.store.stored_refs()?;
        let trigger = if stored.is_empty() {
            Trigger::Add
        } else {
            Trigger::Rescan
        };
        let root_ids: Vec<RootId> = roots.iter().map(|r| r.id).collect();
        let plan = mk_corpus::plan(&stored, &walked, &root_ids, full);
        let hashes = self.hash_planned(&plan, &paths);
        summary.hashed = hashes.len();
        if self.control.stopping() {
            summary.interrupted = true;
            return Ok(summary);
        }
        let Resolved { changes, unchanged } = plan.resolve(&hashes);
        summary.unchanged = unchanged;
        if changes.is_empty() {
            return Ok(summary);
        }
        let changes = changes
            .into_iter()
            .map(|change| {
                let path = mk_corpus::fs::resolve(&paths[&change.root()], change.rel_path());
                let file_type = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(Format::from_extension)
                    .map_or_else(String::new, |f| f.as_str().to_string());
                RescanChange { change, file_type }
            })
            .collect();
        summary.outcome = self.store.apply_rescan(changes, trigger)?;
        if summary.outcome.connections_changed {
            self.events.changed();
        }
        Ok(summary)
    }

    /// Re-scan, then Stage A until nothing is pending: what both apps run after a folder is
    /// added (US-01.3) and on reopen (a killed run is finished, ADR-0006). A paused queue only
    /// re-scans. Pause, cancel and close end it early ([`IngestSummary::interrupted`]).
    pub fn ingest(&self) -> Result<IngestSummary, ProjectError> {
        let _run = self.begin_run();
        let rescan = self.rescan(false)?;
        let stage_a = if rescan.interrupted {
            StageASummary::default()
        } else {
            self.stage_a_locked()?
        };
        Ok(IngestSummary {
            rescan,
            stage_a,
            interrupted: self.control.requested(),
        })
    }

    /// Takes the run lock, waiting for a pause or cancel to finish first.
    fn begin_run(&self) -> RunGuard<'_> {
        let guard = lock(&self.run);
        self.running.store(true, Ordering::SeqCst);
        *lock(&self.run_started) = Some((Instant::now(), now_ms()));
        RunGuard {
            project: self,
            _guard: guard,
        }
    }

    /// Whether a re-scan or Stage A run is going on.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Asks the run to stop, waits until it has (≤ 5 s for a pause, NFR-REL-05), then runs
    /// `then` while no run can start.
    fn stop_run<R>(
        &self,
        stop: Stop,
        then: impl FnOnce() -> Result<R, ProjectError>,
    ) -> Result<R, ProjectError> {
        self.control.request(stop);
        let _run = lock(&self.run);
        let result = then();
        self.control.clear();
        result
    }

    /// Pauses the queue (US-03.2 AC1, INV-PR-06): no new file starts; the busy ones finish
    /// within [`crate::PAUSE_GRACE`] or are stopped and go back to Pending. Returns once the
    /// lanes have stopped and the jobs are Paused.
    pub fn pause(&self) -> Result<(), ProjectError> {
        self.stop_run(Stop::Pause, || {
            Ok(self.store.set_queue_paused(true).map(|_| ())?)
        })
    }

    /// Resumes a paused queue (US-03.2 AC2). The caller starts [`Project::ingest`] again; it
    /// continues where it stopped and redoes no finished file (INV-PR-05).
    pub fn resume(&self) -> Result<(), ProjectError> {
        self.store.set_queue_paused(false)?;
        Ok(())
    }

    /// Cancels the queue (US-03.3): the lanes stop at once, finished files are kept, the files
    /// in progress roll back, and all unfinished work becomes Not processed (D10).
    pub fn cancel(&self) -> Result<mk_store::CancelOutcome, ProjectError> {
        self.stop_run(Stop::Cancel, || Ok(self.store.cancel_queue()?))
    }

    /// Stops the run before the project closes or the app quits: items in progress go back to
    /// Pending and job states are kept, so running jobs run again on the next open
    /// (US-03.4 AC1).
    pub fn stop_for_close(&self) {
        let _ = self.stop_run(Stop::Close, || Ok(()));
    }

    /// Whether the queue is paused (persisted, US-03.4 AC1).
    pub fn is_paused(&self) -> Result<bool, ProjectError> {
        Ok(self.store.progress_detail(None)?.paused)
    }

    /// Whether a run would start a file now: what the desktop checks on open, so a paused
    /// project stays paused (US-03.4 AC1, NFR-REL-04).
    pub fn has_runnable_work(&self) -> Result<bool, ProjectError> {
        Ok(self.store.has_runnable_work()?)
    }

    /// The files in a worker now, by display path (US-03.1 AC1).
    pub fn current_files(&self) -> Vec<String> {
        lock(&self.in_flight).values().cloned().collect()
    }

    /// When the current run started (monotonic, UTC ms); `None` when nothing runs.
    pub(crate) fn run_started(&self) -> Option<(Instant, i64)> {
        *lock(&self.run_started)
    }

    /// Drops and rebuilds every derived row from machine output and decisions (ADR-0005), then
    /// checks the result against the rows before and against the reference model.
    pub fn rebuild(&self) -> Result<RebuildSummary, ProjectError> {
        let before = self.store.derived_state()?;
        self.store.rebuild_all()?;
        let after = self.store.derived_state()?;
        let reference = mk_graph::model::reference(&self.store.facts()?);
        if after != before {
            self.events.changed();
        }
        Ok(RebuildSummary {
            identifiers: after.identifiers.len(),
            connections: after.connections.len(),
            same_as_incremental: after == before,
            consistent: after == reference,
        })
    }

    /// Hashes the plan's new and changed paths on all cores. A file that cannot be read is left
    /// out (it stays as it was).
    fn hash_planned(
        &self,
        plan: &RescanPlan,
        roots: &HashMap<RootId, PathBuf>,
    ) -> HashMap<usize, ContentHash> {
        let work: Vec<(usize, PathBuf)> = plan
            .to_hash()
            .map(|(i, item)| {
                (
                    i,
                    mk_corpus::fs::resolve(&roots[&item.root], &item.rel_path),
                )
            })
            .collect();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let next = Mutex::new(work.into_iter());
        let out = Mutex::new(HashMap::new());
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        if self.control.stopping() {
                            return;
                        }
                        let Some((i, path)) = next.lock().unwrap_or_else(|e| e.into_inner()).next()
                        else {
                            return;
                        };
                        if let Ok(h) = mk_corpus::fs::hash_file(&path) {
                            out.lock().unwrap_or_else(|e| e.into_inner()).insert(i, h);
                        }
                    }
                });
            }
        });
        out.into_inner().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs the Stage A lane until no item is pending: up to `cores − 1` lanes, each with its
    /// own worker process, take items in scheduler order (INV-PR-03) as the memory budget allows
    /// (gap A2); each result goes to the one writer, which commits it with its derived rebuild
    /// (ADR-0005). A failure, a deadline, a memory limit or a worker crash fails only its item
    /// (INV-CO-09, NFR-REL-01/06/07); the lane then starts a fresh worker. Workers are shut down
    /// when the run ends, so none stays idle (NFR-RES-01).
    ///
    /// Pause, cancel and close stop it early: no new item starts, busy items finish within
    /// their grace or go back to Pending ([`crate::PAUSE_GRACE`], ADR-0006).
    pub fn run_stage_a(&self) -> Result<StageASummary, ProjectError> {
        let _run = self.begin_run();
        self.stage_a_locked()
    }

    fn stage_a_locked(&self) -> Result<StageASummary, ProjectError> {
        let summary = Mutex::new(StageASummary::default());
        let peak = AtomicU64::new(0);
        let peak_core = AtomicU64::new(0);
        loop {
            if self.control.stopping() {
                break;
            }
            let pending = self.store.pending_items()?;
            if pending.is_empty() {
                break;
            }
            let needs: HashMap<i64, u64> = pending
                .iter()
                .map(|p| {
                    let size = p.item.size;
                    let need =
                        Format::from_name(&p.file_type).map_or(size, |f| f.memory_estimate(size));
                    (p.item.id, need)
                })
                .collect();
            // Language files are only touched when a run may OCR: a PDF, an image, or an email
            // that may carry one.
            let settings = if pending.iter().any(|p| {
                Format::from_name(&p.file_type)
                    .is_some_and(|f| f.is_paged() || matches!(f, Format::Eml | Format::Mbox))
            }) {
                self.ocr_settings()?
            } else {
                Settings::default()
            };
            let items: Vec<_> = pending.iter().map(|p| p.item).collect();
            let order = mk_jobs::next_batch(&items, 0, items.len());
            let queue = Budget::new(
                order.iter().map(|id| (*id, needs[id])).collect(),
                self.worker_config.budget,
                &self.control,
            );
            let lanes = self.workers.min(pending.len());
            let slots: Vec<AtomicU64> = (0..lanes).map(|_| AtomicU64::new(0)).collect();
            let fatal: Mutex<Option<ProjectError>> = Mutex::new(None);
            std::thread::scope(|s| {
                let (stop, stopped) = std::sync::mpsc::channel::<()>();
                let (slots, peak, peak_core) = (&slots, &peak, &peak_core);
                let sampler = s.spawn(move || {
                    let mut sys = sysinfo::System::new();
                    loop {
                        let workers: u64 = slots.iter().map(|r| r.load(Ordering::Relaxed)).sum();
                        let core = own_rss(&mut sys);
                        peak_core.fetch_max(core, Ordering::Relaxed);
                        peak.fetch_max(core + workers, Ordering::Relaxed);
                        if let Err(RecvTimeoutError::Disconnected) =
                            stopped.recv_timeout(self.worker_config.sample_every)
                        {
                            return;
                        }
                    }
                });
                let handles: Vec<_> = slots
                    .iter()
                    .map(|slot| {
                        let (queue, fatal, summary, settings) =
                            (&queue, &fatal, &summary, &settings);
                        s.spawn(move || {
                            let mut worker = None;
                            while let Some((id, need)) = queue.take() {
                                let r = self.run_item(id, &mut worker, settings, slot, summary);
                                queue.release(need);
                                if let Err(e) = r {
                                    fatal
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .get_or_insert(e);
                                    queue.stop();
                                }
                            }
                            if let Some(w) = worker.take() {
                                w.shutdown();
                            }
                            slot.store(0, Ordering::Relaxed);
                        })
                    })
                    .collect();
                for h in handles {
                    let _ = h.join();
                }
                drop(stop);
                let _ = sampler.join();
            });
            if let Some(e) = fatal.into_inner().unwrap_or_else(|e| e.into_inner()) {
                return Err(e);
            }
        }
        let mut summary = summary.into_inner().unwrap_or_else(|e| e.into_inner());
        summary.peak_memory = peak.into_inner();
        summary.peak_core_memory = peak_core.into_inner();
        Ok(summary)
    }

    /// One item on this lane's worker, started first if needed (so no item is started without
    /// one). The worker is dropped when it was stopped and recycled after
    /// [`WorkerConfig::recycle_after`] files or when idle over
    /// [`WorkerConfig::recycle_idle_memory`].
    fn run_item(
        &self,
        id: i64,
        worker: &mut Option<WorkerProcess>,
        settings: &Settings,
        rss: &AtomicU64,
        summary: &Mutex<StageASummary>,
    ) -> Result<(), ProjectError> {
        let cfg = &self.worker_config;
        let lock = || summary.lock().unwrap_or_else(|e| e.into_inner());
        if self.control.stopping() {
            return Ok(());
        }
        if worker.is_none() {
            *worker = Some(WorkerProcess::spawn(cfg)?);
            lock().workers_started += 1;
        }
        let Some(started) = self.store.start_item(id)? else {
            return Ok(());
        };
        let shown = started
            .path
            .as_ref()
            .map_or_else(|| format!("item {id}"), |p| p.display().to_string());
        self::lock(&self.in_flight).insert(id, shown);
        let result = self.run_started_item(id, &started, worker, settings, rss, summary);
        self::lock(&self.in_flight).remove(&id);
        result
    }

    fn run_started_item(
        &self,
        id: i64,
        started: &mk_store::StartedItem,
        worker: &mut Option<WorkerProcess>,
        settings: &Settings,
        rss: &AtomicU64,
        summary: &Mutex<StageASummary>,
    ) -> Result<(), ProjectError> {
        let cfg = &self.worker_config;
        let lock = || summary.lock().unwrap_or_else(|e| e.into_inner());
        let Some(path) = started.path.as_ref().and_then(|p| p.to_str()) else {
            self.fail(
                id,
                &Failure::new("file_missing", "The file is no longer there"),
                summary,
            )?;
            return Ok(());
        };
        // NFR-CAP-05/06/07: over-limit files fail before a worker reads them.
        if let Some(f) = too_large(path, &started.file_type) {
            self.fail(id, &f, summary)?;
            return Ok(());
        }
        let w = worker.as_mut().expect("started above");
        let staging = Staging::new(
            Some(&crate::stage_a::staging_dir(&self.layout.cache())),
            id,
            SPILL_AT,
        );
        let result = w.run(
            cfg,
            settings,
            path.to_string(),
            started.hash,
            started.file_type.clone(),
            staging,
            rss,
            &self.control,
        );
        match result {
            ItemResult::Done(staging, warnings) => self.commit(id, staging, warnings, summary)?,
            ItemResult::Failed(f) => self.fail(id, &f, summary)?,
            ItemResult::Lost(f) => {
                *worker = None;
                self.fail(id, &f, summary)?;
            }
            ItemResult::Stopped(stop) => {
                *worker = None;
                let failure = stop.why.failure().with_detail(format!(
                    "Error code: {}. The reader process was stopped ({:?}); its memory peaked at \
                     {} MB.",
                    stop.code,
                    stop.why,
                    stop.peak_rss >> 20
                ));
                self.fail(id, &failure, summary)?;
                lock().stops.push(stop);
            }
            ItemResult::Interrupted => {
                *worker = None;
                self.store.release_item(id)?;
                lock().interrupted += 1;
            }
        }
        if let Some(w) = worker.as_mut() {
            let idle = w.rss().unwrap_or(0);
            rss.store(idle, Ordering::Relaxed);
            if w.files() >= cfg.recycle_after || idle > cfg.recycle_idle_memory {
                worker.take().expect("checked").shutdown();
                rss.store(0, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    fn commit(
        &self,
        id: i64,
        staging: Staging,
        warnings: Vec<String>,
        summary: &Mutex<StageASummary>,
    ) -> Result<(), StoreError> {
        let parsed_at = Instant::now();
        let parts = match staging.into_parts() {
            Ok(p) => p,
            Err(e) => {
                return self.fail(
                    id,
                    &Failure::new(
                        "internal_error",
                        format!("Could not store the results: {e}"),
                    ),
                    summary,
                );
            }
        };
        match self.store.commit_stage_a_staged(id, parts, warnings) {
            Ok(outcome) => {
                let latency = parsed_at.elapsed();
                if outcome.connections_changed {
                    self.events.changed();
                }
                let mut s = summary.lock().unwrap_or_else(|e| e.into_inner());
                s.done += 1;
                s.changed_commits += usize::from(outcome.connections_changed);
                s.commit_latencies.push(latency);
                Ok(())
            }
            // The commit rolled back. Record why on the item, so it does not come back.
            Err(e @ StoreError::Invalid(_)) => self.fail(
                id,
                &Failure::new(
                    "internal_error",
                    format!("Could not store the results: {e}"),
                ),
                summary,
            ),
            Err(e) => Err(e),
        }
    }

    fn fail(&self, id: i64, f: &Failure, summary: &Mutex<StageASummary>) -> Result<(), StoreError> {
        let detail = f
            .detail
            .clone()
            .unwrap_or_else(|| format!("Error code: {}", f.code));
        self.store.fail_item(id, &f.code, &f.reason, &detail)?;
        summary.lock().unwrap_or_else(|e| e.into_inner()).failed += 1;
        Ok(())
    }

    /// Checks the project after a crash or kill: SQLite integrity and foreign keys, derived
    /// rows against a rebuild from the facts (ADR-0005), and the item and job counts.
    pub fn check_health(&self) -> Result<HealthReport, ProjectError> {
        let (integrity_errors, foreign_key_errors) = self.store.read_raw(|c| {
            let mut stmt = c.prepare("PRAGMA integrity_check")?;
            let lines: Vec<String> = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let mut stmt = c.prepare("PRAGMA foreign_key_check")?;
            let fk = stmt.query_map([], |_| Ok(()))?.count();
            Ok((lines.into_iter().filter(|l| l != "ok").collect(), fk))
        })?;
        let derived = self.store.derived_state()?;
        let reference = mk_graph::model::reference(&self.store.facts()?);
        Ok(HealthReport {
            integrity_errors,
            foreign_key_errors,
            derived_consistent: derived == reference,
            progress: self.store.progress()?,
        })
    }

    /// Ignores (or un-ignores) an identifier connection: appends the decision and rebuilds its
    /// anchor in one transaction (US-07.4, INV-CU-01). Returns the decision's `seq`.
    pub fn set_ignored(&self, key: &str, ignored: bool) -> Result<i64, ProjectError> {
        let kind = if ignored {
            DecisionKind::IgnoreIdentifier
        } else {
            DecisionKind::UnignoreIdentifier
        };
        let (seq, changed) = self.store.append_decision(
            kind,
            key,
            Some(if ignored { "shown" } else { "ignored" }),
            Some(if ignored { "ignored" } else { "shown" }),
        )?;
        if changed {
            self.events.changed();
        }
        Ok(seq)
    }
}

/// Ends a run: clears the running flag and the run's start, then releases the run lock.
struct RunGuard<'a> {
    project: &'a Project,
    _guard: MutexGuard<'a, ()>,
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        self.project.running.store(false, Ordering::SeqCst);
        *lock(&self.project.run_started) = None;
        // The run's maps and the store's page caches go back to the allocator (NFR-RES-01).
        *lock(&self.project.in_flight) = BTreeMap::new();
        let _ = self.project.store.release_memory();
    }
}

/// The Stage A queue in scheduler order, handed out as the memory budget allows (gap A2).
/// Hands out nothing once the run is asked to stop.
struct Budget<'a> {
    state: Mutex<BudgetState>,
    changed: Condvar,
    total: u64,
    control: &'a RunControl,
}

struct BudgetState {
    ids: Vec<i64>,
    needs: Vec<u64>,
    in_use: u64,
    in_flight: usize,
    stopped: bool,
}

/// How often a lane waiting for budget looks at the stop request.
const STOP_POLL: Duration = Duration::from_millis(50);

impl<'a> Budget<'a> {
    fn new(items: Vec<(i64, u64)>, total: u64, control: &'a RunControl) -> Self {
        let (ids, needs) = items.into_iter().unzip();
        Budget {
            state: Mutex::new(BudgetState {
                ids,
                needs,
                in_use: 0,
                in_flight: 0,
                stopped: false,
            }),
            changed: Condvar::new(),
            total,
            control,
        }
    }

    /// The next item and its need; waits while nothing fits. `None` when the queue is empty or
    /// the run stopped.
    fn take(&self) -> Option<(i64, u64)> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if s.stopped || s.ids.is_empty() || self.control.stopping() {
                return None;
            }
            if let Some(i) = mk_jobs::pick_next(&s.needs, s.in_use, s.in_flight, self.total) {
                let (id, need) = (s.ids.remove(i), s.needs.remove(i));
                s.in_use += need;
                s.in_flight += 1;
                return Some((id, need));
            }
            s = self
                .changed
                .wait_timeout(s, STOP_POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn release(&self, need: u64) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.in_use -= need;
        s.in_flight -= 1;
        self.changed.notify_all();
    }

    fn stop(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        self.changed.notify_all();
    }
}
