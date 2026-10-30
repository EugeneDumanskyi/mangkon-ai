//! Worker supervision (ADR-0001, ADR-0007): spawn a worker, check its protocol, send it one
//! content at a time, and stop it on a deadline, a missing heartbeat or a memory limit. The
//! lane in `project.rs` decides what to run; this module runs one item on one worker.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mk_worker::frame::{FrameReader, FrameWriter};
use mk_worker::{
    FromWorker, Limits, PROTOCOL, PageImage, ProcessContent, RenderPage, Settings, ToWorker,
    WorkStage,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

use crate::control::RunControl;
use crate::stage_a::{ChunkOrderError, Failure, Staging};

const MB: u64 = 1 << 20;

/// How Stage A workers run. Defaults per ADR-0007 and architecture §7; values marked
/// ASSUMPTION there.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// The worker program; `None` is this executable. Started with `args`.
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    /// Per-file parse limit (NFR-REL-06): 120 s. For MBOX it is per message, for PDFs and
    /// images per page: it restarts each time the worker reports one more finished unit
    /// (ADR-0016, ADR-0017).
    pub deadline: Duration,
    /// While a page is in OCR it must finish within this: 30 s (NFR-REL-06, ADR-0017).
    pub ocr_page_deadline: Duration,
    /// A page render for the page view: 10 s (ASSUMPTION, ADR-0011).
    pub render_deadline: Duration,
    /// Per-file memory limit (NFR-REL-07): 1 GB of resident memory.
    pub max_memory: u64,
    /// How often the watchdog samples a busy worker's resident memory: 100 ms.
    pub sample_every: Duration,
    /// A busy worker silent this long is stopped as stuck: 15 s (ASSUMPTION; heartbeats come
    /// every 500 ms).
    pub heartbeat_timeout: Duration,
    /// Time allowed from start to `Hello`: 10 s.
    pub start_timeout: Duration,
    /// Replace a worker after this many files: 200.
    pub recycle_after: usize,
    /// Replace a worker whose idle resident memory is over this: 300 MB.
    pub recycle_idle_memory: u64,
    /// Total memory budget across workers (gap A2): 1.5 GB.
    pub budget: u64,
    pub limits: Limits,
    /// Honour the test fault markers (tests and the kill-test driver only).
    pub test_faults: bool,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            program: None,
            args: vec!["--worker".into()],
            deadline: Duration::from_secs(120),
            ocr_page_deadline: Duration::from_secs(30),
            render_deadline: Duration::from_secs(10),
            max_memory: 1024 * MB,
            sample_every: Duration::from_millis(100),
            heartbeat_timeout: Duration::from_secs(15),
            start_timeout: Duration::from_secs(10),
            recycle_after: 200,
            recycle_idle_memory: 300 * MB,
            budget: 1536 * MB,
            limits: Limits::default(),
            test_faults: false,
        }
    }
}

impl WorkerConfig {
    /// Workers from `program --worker` (tests use a small worker binary).
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        WorkerConfig {
            program: Some(program.into()),
            ..Self::default()
        }
    }
}

/// A worker could not be started or broke the protocol before any item. Ends the run: no item
/// is started without a worker (the items stay Pending).
#[derive(Debug)]
pub enum WorkerError {
    Spawn(PathBuf, io::Error),
    Handshake(String),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Spawn(p, e) => {
                write!(f, "could not start the worker {}: {e}", p.display())
            }
            WorkerError::Handshake(m) => write!(f, "the worker did not start properly: {m}"),
        }
    }
}

impl std::error::Error for WorkerError {}

/// The last bytes a worker wrote to stderr. Used only to classify how it stopped; never shown.
#[derive(Default)]
struct Tail(VecDeque<u8>);

const TAIL: usize = 4096;

impl Tail {
    fn text(&self) -> String {
        let (a, b) = self.0.as_slices();
        String::from_utf8_lossy(&[a, b].concat()).into_owned()
    }
}

/// One running worker process.
pub(crate) struct WorkerProcess {
    child: Child,
    to: FrameWriter<ChildStdin>,
    from: Receiver<io::Result<FromWorker>>,
    stderr: Arc<Mutex<Tail>>,
    pid: u32,
    /// Files finished (Done or Failed) by this worker.
    files: usize,
    sys: System,
}

/// How a worker stopped in the middle of an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stopped {
    /// Over the per-file deadline (NFR-REL-06).
    Deadline,
    /// No message for [`WorkerConfig::heartbeat_timeout`].
    Silent,
    /// Over the memory limit: seen by the watchdog, or an allocation failed under the hard cap
    /// (NFR-REL-07).
    Memory,
    /// Exited on its own or broke the protocol.
    Crashed,
}

/// A worker stopped in the middle of an item. Kept in the run summary (spike S2b).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorkerStop {
    pub why: Stopped,
    /// The failure code the item got.
    pub code: String,
    /// The highest resident memory sampled during the item, in bytes.
    pub peak_rss: u64,
    /// Memory stops only: from the last sample at or under the limit to the stop. An upper
    /// bound on how long the worker ran over the limit.
    pub over_limit_for: Option<Duration>,
}

impl Stopped {
    pub(crate) fn failure(self) -> Failure {
        match self {
            Stopped::Deadline | Stopped::Silent => {
                Failure::new("took_too_long", "Took too long to read")
            }
            Stopped::Memory => Failure::new("needed_too_much_memory", "Needed too much memory"),
            Stopped::Crashed => Failure::new(
                "worker_crashed",
                "The file could not be read: the reader stopped unexpectedly",
            ),
        }
    }
}

/// What one item on one worker came to.
pub(crate) enum ItemResult {
    /// The worker sent `Done` (and is still usable).
    Done(Staging, Vec<String>),
    /// The worker sent `Failed` (and is still usable).
    Failed(Failure),
    /// The worker was stopped or stopped itself; it is gone.
    Stopped(WorkerStop),
    /// The core could not stage the result (its spill file failed); the worker was stopped.
    Lost(Failure),
    /// Pause, cancel or close: the worker was killed once the item's grace was over. The item
    /// committed nothing and goes back to Pending (ADR-0006).
    Interrupted,
}

impl WorkerProcess {
    /// Starts a worker and waits for its `Hello` with the right protocol version.
    pub(crate) fn spawn(cfg: &WorkerConfig) -> Result<WorkerProcess, WorkerError> {
        let program = match &cfg.program {
            Some(p) => p.clone(),
            None => std::env::current_exe().map_err(|e| WorkerError::Spawn("worker".into(), e))?,
        };
        let mut child = Command::new(&program);
        child
            .args(&cfg.args)
            .arg("--max-memory")
            .arg(cfg.max_memory.to_string());
        if let Some(t) = malloc_tunables() {
            child.env("GLIBC_TUNABLES", t);
        }
        let mut child = child
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| WorkerError::Spawn(program.clone(), e))?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");

        let (tx, from) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut r = FrameReader::new(io::BufReader::with_capacity(1 << 16, stdout));
            loop {
                match r.recv() {
                    Ok(Some(msg)) => {
                        if tx.send(Ok(msg)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                }
            }
        });
        let tail = Arc::new(Mutex::new(Tail::default()));
        {
            let tail = tail.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n @ 1..) = stderr.read(&mut buf) {
                    let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                    t.0.extend(&buf[..n]);
                    let over = t.0.len().saturating_sub(TAIL);
                    t.0.drain(..over);
                }
            });
        }

        let mut w = WorkerProcess {
            child,
            to: FrameWriter::new(stdin),
            from,
            stderr: tail,
            pid,
            files: 0,
            sys: System::new(),
        };
        match w.from.recv_timeout(cfg.start_timeout) {
            Ok(Ok(FromWorker::Hello { protocol, .. })) if protocol == PROTOCOL => Ok(w),
            Ok(Ok(FromWorker::Hello { protocol, .. })) => {
                w.kill();
                Err(WorkerError::Handshake(format!(
                    "protocol {protocol}, expected {PROTOCOL}"
                )))
            }
            Ok(Ok(other)) => {
                w.kill();
                Err(WorkerError::Handshake(format!(
                    "expected Hello, got {other:?}"
                )))
            }
            Ok(Err(e)) => {
                w.kill();
                Err(WorkerError::Handshake(e.to_string()))
            }
            Err(_) => {
                w.kill();
                let tail = w.stderr_tail();
                Err(WorkerError::Handshake(if tail.is_empty() {
                    "no Hello".into()
                } else {
                    format!("no Hello; stderr: {}", tail.trim())
                }))
            }
        }
    }

    pub(crate) fn files(&self) -> usize {
        self.files
    }

    fn stderr_tail(&self) -> String {
        self.stderr.lock().unwrap_or_else(|e| e.into_inner()).text()
    }

    /// Resident memory in bytes, or `None` if the process is gone.
    pub(crate) fn rss(&mut self) -> Option<u64> {
        let pid = Pid::from_u32(self.pid);
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.sys.process(pid).map(|p| p.memory())
    }

    /// Kills the process and waits for it.
    pub(crate) fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Asks the worker to exit; kills it if it has not after a second.
    pub(crate) fn shutdown(mut self) {
        let _ = self.to.send(&ToWorker::Shutdown);
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.kill();
    }

    /// Why the process ended on its own: an allocation that failed under the hard cap reads as
    /// memory (Rust prints "memory allocation of N bytes failed" before it aborts).
    fn classify_exit(&mut self) -> Stopped {
        let _ = self.child.wait();
        // The stderr thread may still be draining the last bytes.
        std::thread::sleep(Duration::from_millis(20));
        if self.stderr_tail().contains("memory allocation of") {
            Stopped::Memory
        } else {
            Stopped::Crashed
        }
    }

    /// Runs one content on this worker under the limits of `cfg`, staging its chunks in
    /// `staging`. `settings` carries the project's OCR languages; `rss` gets each sample of the
    /// worker's resident memory (for the run's peak); `control` interrupts it on pause, cancel
    /// or close. On `Stopped`, `Lost` and `Interrupted` the process has been killed and reaped.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(
        &mut self,
        cfg: &WorkerConfig,
        settings: &Settings,
        path: String,
        content_hash: mk_kernel::ContentHash,
        file_type: String,
        mut staging: Staging,
        rss: &AtomicU64,
        control: &RunControl,
    ) -> ItemResult {
        // ADR-0016/0017: an MBOX's deadline runs from its last finished message, a PDF's or an
        // image's from its last finished page.
        let per_unit = mk_parsers::Format::from_name(&file_type)
            .is_some_and(|f| f == mk_parsers::Format::Mbox || f.is_paged());
        let req = ToWorker::Process(ProcessContent {
            path,
            content_hash,
            file_type,
            limits: cfg.limits,
            settings: Settings {
                test_faults: cfg.test_faults,
                ..settings.clone()
            },
        });
        let mut started = Instant::now();
        let mut units = 0u64;
        // When the page now in OCR started it (ADR-0017).
        let mut ocr_since: Option<Instant> = None;
        let mut heard = started;
        let mut sampled = started;
        // The last sample at or under the memory limit, and the highest sample.
        let mut under_limit = started;
        let mut peak = 0u64;
        let stopped = |w: &mut WorkerProcess, why: Stopped, kill: bool, under: Instant, peak| {
            if kill {
                w.kill();
            }
            rss.store(0, Ordering::Relaxed);
            ItemResult::Stopped(WorkerStop {
                code: why.failure().code,
                peak_rss: peak,
                over_limit_for: (why == Stopped::Memory).then(|| under.elapsed()),
                why,
            })
        };
        if self.to.send(&req).is_err() {
            let why = self.classify_exit();
            return stopped(self, why, false, under_limit, peak);
        }
        loop {
            match self.from.recv_timeout(cfg.sample_every) {
                Ok(Ok(msg)) => {
                    heard = Instant::now();
                    match msg {
                        FromWorker::Heartbeat { units: u, stage } => {
                            if per_unit && u > units {
                                units = u;
                                started = heard;
                            }
                            ocr_since = match stage {
                                WorkStage::Ocr => ocr_since.or(Some(heard)),
                                _ => None,
                            };
                        }
                        FromWorker::Chunk(c) => match staging.push(c) {
                            Ok(()) => {}
                            Err(Ok(ChunkOrderError)) => {
                                return stopped(self, Stopped::Crashed, true, under_limit, peak);
                            }
                            Err(Err(e)) => {
                                self.kill();
                                rss.store(0, Ordering::Relaxed);
                                return ItemResult::Lost(Failure::new(
                                    "internal_error",
                                    format!("Could not store the results: {e}"),
                                ));
                            }
                        },
                        FromWorker::Done { warnings } => {
                            self.files += 1;
                            return ItemResult::Done(staging, warnings);
                        }
                        FromWorker::Failed { code, plain_reason } => {
                            self.files += 1;
                            return ItemResult::Failed(Failure::from_worker(code, plain_reason));
                        }
                        FromWorker::Hello { .. } | FromWorker::PageImage(_) => {
                            return stopped(self, Stopped::Crashed, true, under_limit, peak);
                        }
                    }
                }
                Ok(Err(_)) => return stopped(self, Stopped::Crashed, true, under_limit, peak),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let why = self.classify_exit();
                    return stopped(self, why, false, under_limit, peak);
                }
            }
            let now = Instant::now();
            if now.duration_since(sampled) >= cfg.sample_every {
                sampled = now;
                let r = self.rss().unwrap_or(0);
                rss.store(r, Ordering::Relaxed);
                peak = peak.max(r);
                if r > cfg.max_memory {
                    return stopped(self, Stopped::Memory, true, under_limit, peak);
                }
                under_limit = now;
            }
            if now.duration_since(started) > cfg.deadline
                || ocr_since.is_some_and(|t| now.duration_since(t) > cfg.ocr_page_deadline)
            {
                return stopped(self, Stopped::Deadline, true, under_limit, peak);
            }
            if now.duration_since(heard) > cfg.heartbeat_timeout {
                return stopped(self, Stopped::Silent, true, under_limit, peak);
            }
            if control.must_interrupt().is_some() {
                self.kill();
                rss.store(0, Ordering::Relaxed);
                return ItemResult::Interrupted;
            }
        }
    }
}

/// Why a page render gave no image.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum RenderError {
    /// The worker refused (a changed or missing file, no such page); it is still usable.
    Failed(Failure),
    /// The worker was stopped or stopped itself; it is gone.
    Stopped(Stopped),
}

impl WorkerProcess {
    /// Renders one page under [`WorkerConfig::render_deadline`] and the memory limit (ADR-0011).
    pub(crate) fn render(
        &mut self,
        cfg: &WorkerConfig,
        req: RenderPage,
    ) -> Result<PageImage, RenderError> {
        if self.to.send(&ToWorker::RenderPage(req)).is_err() {
            return Err(RenderError::Stopped(self.classify_exit()));
        }
        let started = Instant::now();
        let stop = |w: &mut WorkerProcess, why| {
            w.kill();
            Err(RenderError::Stopped(why))
        };
        loop {
            match self.from.recv_timeout(cfg.sample_every) {
                Ok(Ok(FromWorker::PageImage(page))) => return Ok(page),
                Ok(Ok(FromWorker::Failed { code, plain_reason })) => {
                    return Err(RenderError::Failed(Failure::from_worker(
                        code,
                        plain_reason,
                    )));
                }
                Ok(Ok(FromWorker::Heartbeat { .. })) => {}
                Ok(Ok(_)) | Ok(Err(_)) => return stop(self, Stopped::Crashed),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RenderError::Stopped(self.classify_exit()));
                }
            }
            if self.rss().unwrap_or(0) > cfg.max_memory {
                return stop(self, Stopped::Memory);
            }
            if started.elapsed() > cfg.render_deadline {
                return stop(self, Stopped::Deadline);
            }
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            self.kill();
        }
    }
}

/// Resident memory of this process, in bytes.
/// Memory a worker frees goes back to the OS (NFR-RES-02). glibc raises its mmap threshold
/// after the first large buffer is freed (up to 32 MB), so later page buffers (8.7 MB for a
/// 300 DPI A4 page) come from the heap and an idle worker keeps tens of MB of them. A fixed
/// 1 MB threshold stops that: on TD-2, 23 workers peaked ≈ 530–560 MB lower (`docs/11-ingestion.md`
/// §7). Other C libraries ignore the variable. Kept in front of tunables set by the user.
fn malloc_tunables() -> Option<String> {
    if !cfg!(all(target_os = "linux", target_env = "gnu")) {
        return None;
    }
    const OURS: &str = "glibc.malloc.mmap_threshold=1048576";
    Some(match std::env::var("GLIBC_TUNABLES") {
        Ok(t) if !t.is_empty() => format!("{OURS}:{t}"),
        _ => OURS.to_string(),
    })
}

pub(crate) fn own_rss(sys: &mut System) -> u64 {
    let pid = Pid::from_u32(std::process::id());
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_memory(),
    );
    sys.process(pid).map_or(0, |p| p.memory())
}

/// Running processes of `program --worker`. The kill test lists them before it starts a core
/// and again after it kills that core: the new ones must exit by themselves (no orphans).
pub fn worker_processes(program: &Path) -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    sys.processes()
        .values()
        .filter(|p| {
            p.status() != ProcessStatus::Zombie
                && p.exe() == Some(program)
                && p.cmd().get(1).is_some_and(|a| a == "--worker")
        })
        .map(|p| p.pid().as_u32())
        .collect()
}
