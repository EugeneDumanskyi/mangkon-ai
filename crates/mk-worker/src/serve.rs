//! The worker loop.

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mk_kernel::ContentHash;
use mk_parsers::{Engines, Format, OcrConfig};

use crate::extract::ChunkSink;
use crate::fault::Fault;
use crate::frame::{FrameReader, FrameWriter};
use crate::protocol::{
    FromWorker, PROTOCOL, PageImage, PagePoints, ProcessContent, RenderPage, Settings, ToWorker,
    WorkStage,
};

/// How often a busy worker sends a heartbeat. ASSUMPTION; the core's heartbeat timeout is a
/// multiple of it.
pub const HEARTBEAT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default)]
pub struct ServeOptions {
    /// Exit the process when the input ends, even in the middle of an item: the core is gone,
    /// so no worker is left behind. Off for in-process tests.
    pub exit_on_eof: bool,
}

/// Progress shared with the heartbeat thread.
#[derive(Default)]
struct Progress {
    busy: AtomicBool,
    stage: AtomicU8,
    units: AtomicU64,
}

impl Progress {
    fn set(&self, stage: WorkStage) {
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    fn stage(&self) -> WorkStage {
        match self.stage.load(Ordering::Relaxed) {
            0 => WorkStage::Reading,
            1 => WorkStage::Parsing,
            2 => WorkStage::Sending,
            3 => WorkStage::Ocr,
            _ => WorkStage::Rendering,
        }
    }
}

type Shared<W> = Arc<Mutex<FrameWriter<W>>>;

fn send<W: Write>(out: &Shared<W>, msg: &FromWorker) -> io::Result<()> {
    out.lock().unwrap_or_else(|e| e.into_inner()).send(msg)
}

/// Serves requests from `input` until `Shutdown` or the end of the input: says `Hello`, then for
/// each [`ToWorker::Process`] streams chunks and ends with `Done` or `Failed`, and for each
/// [`ToWorker::RenderPage`] answers `PageImage` or `Failed`. A parser panic fails only its item.
/// The engines (and a loaded OCR language set) are kept from one item to the next. Returns an
/// error if the output is gone.
pub fn serve<R, W>(input: R, output: W, opts: ServeOptions) -> io::Result<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let out: Shared<W> = Arc::new(Mutex::new(FrameWriter::new(output)));
    send(
        &out,
        &FromWorker::Hello {
            protocol: PROTOCOL,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
        },
    )?;

    // Requests arrive on their own thread, so the end of the input is seen even while an item is
    // in work.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut input = FrameReader::new(input);
        loop {
            match input.recv::<ToWorker>() {
                Ok(Some(msg)) => {
                    if tx.send(msg).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    if opts.exit_on_eof {
                        std::process::exit(0);
                    }
                    return;
                }
            }
        }
    });

    let progress = Arc::new(Progress::default());
    let (stop, stopped) = mpsc::channel::<()>();
    let heartbeat = {
        let (out, progress) = (out.clone(), progress.clone());
        std::thread::spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(HEARTBEAT) {
                if progress.busy.load(Ordering::Relaxed) {
                    let msg = FromWorker::Heartbeat {
                        stage: progress.stage(),
                        units: progress.units.load(Ordering::Relaxed),
                    };
                    if send(&out, &msg).is_err() {
                        return;
                    }
                }
            }
        })
    };

    let mut result = Ok(());
    let mut engines = Engines::default();
    for msg in rx {
        progress.units.store(0, Ordering::Relaxed);
        progress.set(WorkStage::Reading);
        progress.busy.store(true, Ordering::Relaxed);
        let outcome = match msg {
            ToWorker::Shutdown => break,
            ToWorker::Process(req) => process(&req, &progress, &out, &mut engines)
                .map(|r| r.map(|warnings| FromWorker::Done { warnings })),
            ToWorker::RenderPage(req) => Ok(render(&req, &progress, &engines)),
        };
        progress.busy.store(false, Ordering::Relaxed);
        let reply = match outcome {
            Ok(Ok(reply)) => reply,
            Ok(Err((code, reason))) => FromWorker::Failed {
                code: code.to_string(),
                plain_reason: reason,
            },
            Err(e) => {
                result = Err(e);
                break;
            }
        };
        if let Err(e) = send(&out, &reply) {
            result = Err(e);
            break;
        }
    }
    drop(stop);
    let _ = heartbeat.join();
    result
}

/// One item: `Ok(Ok(warnings))` when every chunk was sent, `Ok(Err((code, reason)))` when the
/// item fails, `Err` when the output is gone.
///
/// The file is read twice: once to check its hash, then as a stream by the parser, which sends
/// chunks as it goes. If its size or modification time changed by the end, the item fails with
/// `file_changed` and the core drops what it staged.
fn process<W: Write>(
    req: &ProcessContent,
    progress: &Arc<Progress>,
    out: &Shared<W>,
    engines: &mut Engines,
) -> io::Result<Result<Vec<String>, (&'static str, String)>> {
    let (mut file, before) = match open(&req.path) {
        Ok(f) => f,
        Err(e) => return Ok(Err(e)),
    };
    let mut hasher = ContentHash::hasher();
    let mut head = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Ok(Err(unreadable(&e))),
        };
        hasher.update(&buf[..n]);
        if head.len() < FAULT_HEAD {
            head.extend_from_slice(&buf[..n.min(FAULT_HEAD - head.len())]);
        }
    }
    drop(buf);
    if hasher.finish() != req.content_hash {
        return Ok(Err(changed()));
    }
    let fault = req
        .settings
        .test_faults
        .then(|| Fault::in_file(&head))
        .flatten();
    let Some(format) = Format::from_name(&req.file_type) else {
        return Ok(Err((
            "unsupported_type",
            "This file type is not supported yet".into(),
        )));
    };
    progress.set(WorkStage::Parsing);
    engines.set_ocr(ocr_config(&req.settings));
    let mut input = io::BufReader::with_capacity(1 << 16, file);
    let mut custom = Vec::with_capacity(req.settings.custom_patterns.len());
    for (name, expression) in &req.settings.custom_patterns {
        match mk_identifiers::CustomPattern::compile(name, expression) {
            Ok(p) => custom.push(p),
            Err(e) => {
                return Ok(Err((
                    "invalid_pattern",
                    format!("The custom pattern \"{name}\" cannot be used: {e}"),
                )));
            }
        }
    }
    let rules = mk_identifiers::Rules {
        phone_region: req
            .settings
            .phone_region
            .as_deref()
            .and_then(mk_identifiers::Region::parse),
        custom,
    };
    let mut sink = ChunkSink::new(
        req.limits.chunk_segments as usize,
        |c| send(out, &FromWorker::Chunk(c)),
        |n| progress.units.store(n, Ordering::Relaxed),
    )
    .with_rules(rules);
    if format == Format::Mbox || format.is_paged() {
        let p = progress.clone();
        sink = sink.in_units().on_ocr(move |active| p.ocr(active));
    }
    let parse = || {
        if let Some(fault) = fault {
            // Only the slow and stuck faults return: a parse with no output.
            fault.run(
                |n| progress.units.store(n, Ordering::Relaxed),
                |active| progress.ocr(active),
            );
            return Ok(());
        }
        mk_parsers::parse_stream(format, &mut input, &mut sink, engines)
    };
    let parsed = catch_unwind(AssertUnwindSafe(parse));
    if let Some(e) = sink.take_error() {
        return Err(e);
    }
    match parsed {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Ok(Err((e.code, e.message))),
        Err(_) => {
            return Ok(Err((
                "parser_crashed",
                "The file could not be read: the parser stopped".into(),
            )));
        }
    }
    progress.set(WorkStage::Sending);
    let warnings = sink.finish()?;
    match std::fs::metadata(&req.path) {
        Ok(after) if Stamp::of(&after) == before => Ok(Ok(warnings)),
        _ => Ok(Err(changed())),
    }
}

impl Progress {
    /// A page's OCR starts or ends (ADR-0017).
    fn ocr(&self, active: bool) {
        self.set(if active {
            WorkStage::Ocr
        } else {
            WorkStage::Parsing
        });
    }
}

/// The OCR engine settings of a request; `None` without a `tessdata` folder.
fn ocr_config(s: &Settings) -> Option<OcrConfig> {
    let languages = if s.ocr_languages.trim().is_empty() {
        mk_parsers::ocr::ENGLISH.to_string()
    } else {
        s.ocr_languages.clone()
    };
    s.tessdata.as_ref().map(|t| OcrConfig {
        tessdata: t.into(),
        languages,
    })
}

/// One page for the page view: the file is read whole, its hash checked, then the page rendered
/// (PDF) or decoded (image) by the engines that read it at Stage A (ADR-0011).
fn render(
    req: &RenderPage,
    progress: &Progress,
    engines: &Engines,
) -> Result<FromWorker, (&'static str, String)> {
    let (mut file, _) = open(&req.path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| unreadable(&e))?;
    if ContentHash::of(&bytes) != req.content_hash {
        return Err(changed());
    }
    let format = Format::from_name(&req.file_type)
        .filter(Format::is_paged)
        .ok_or(("unsupported_type", "This file has no pages to show".into()))?;
    progress.set(WorkStage::Rendering);
    let render = || mk_parsers::render_page(format, bytes, req.page, req.scale, engines);
    let page = match catch_unwind(AssertUnwindSafe(render)) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return Err((e.code, e.message)),
        Err(_) => {
            return Err((
                "parser_crashed",
                "The page could not be shown: the reader stopped".into(),
            ));
        }
    };
    let png = page
        .image
        .to_png()
        .map_err(|e| ("render_failed", format!("The page could not be shown: {e}")))?;
    Ok(FromWorker::PageImage(PageImage {
        png,
        width: page.image.width,
        height: page.image.height,
        pages: page.pages,
        points: page.points.map(|p| PagePoints {
            width: p.width,
            height: p.height,
            rotate: p.rotate,
        }),
        geometry: page.geometry,
    }))
}

/// How much of a file's start [`Fault::in_file`] looks at (its first line).
const FAULT_HEAD: usize = 4096;

/// Size and modification time: a file that keeps both while it is read is taken as unchanged.
#[derive(PartialEq, Eq)]
struct Stamp(u64, Option<std::time::SystemTime>);

impl Stamp {
    fn of(m: &std::fs::Metadata) -> Self {
        Stamp(m.len(), m.modified().ok())
    }
}

fn open(path: &str) -> Result<(std::fs::File, Stamp), (&'static str, String)> {
    let file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            ("file_missing", "The file is no longer there".to_string())
        } else {
            unreadable(&e)
        }
    })?;
    let meta = file.metadata().map_err(|e| unreadable(&e))?;
    Ok((file, Stamp::of(&meta)))
}

fn unreadable(e: &io::Error) -> (&'static str, String) {
    (
        "file_unreadable",
        format!("The file could not be read: {e}"),
    )
}

fn changed() -> (&'static str, String) {
    ("file_changed", "File changed during processing".into())
}

/// Entry for `--worker [--max-memory BYTES]` on stdin/stdout. On Linux it first caps its own
/// address space (`RLIMIT_AS`) at what it has mapped now plus `--max-memory` plus headroom for
/// its threads: a coarse hard cap behind the core's RSS watchdog (ADR-0007). An allocation over
/// the cap aborts with "memory allocation of N bytes failed" on stderr, which the core reads as
/// "Needed too much memory".
pub fn run_stdio(args: &[String]) -> ExitCode {
    let mut max_memory = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--max-memory" => match it.next().and_then(|n| n.parse::<u64>().ok()) {
                Some(n) => max_memory = Some(n),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    if let Some(max) = max_memory
        && let Err(e) = cap_address_space(max)
    {
        eprintln!("mangkon worker: could not set the memory cap: {e}");
        return ExitCode::FAILURE;
    }
    let output = match stdout_file() {
        Ok(f) => io::BufWriter::with_capacity(1 << 16, f),
        Err(e) => {
            eprintln!("mangkon worker: {e}");
            return ExitCode::FAILURE;
        }
    };
    match serve(io::stdin(), output, ServeOptions { exit_on_eof: true }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: --worker [--max-memory BYTES]");
    ExitCode::from(2)
}

/// Standard output as a plain file, so binary frames skip the line buffer of `io::Stdout`.
fn stdout_file() -> io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        Ok(io::stdout().as_fd().try_clone_to_owned()?.into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        Ok(io::stdout().as_handle().try_clone_to_owned()?.into())
    }
}

/// Address space a worker may map beyond what it has at start plus its file budget: thread
/// stacks and allocator arenas. ASSUMPTION, checked in spike S2b.
#[cfg(target_os = "linux")]
const AS_HEADROOM: u64 = 512 << 20;

#[cfg(target_os = "linux")]
fn cap_address_space(max: u64) -> io::Result<()> {
    use rustix::process::{Resource, Rlimit, setrlimit};
    let statm = std::fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = statm
        .split_whitespace()
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| io::Error::other("unreadable /proc/self/statm"))?;
    let cap = (pages * 4096)
        .saturating_add(max)
        .saturating_add(AS_HEADROOM);
    setrlimit(
        Resource::As,
        Rlimit {
            current: Some(cap),
            maximum: Some(cap),
        },
    )
    .map_err(io::Error::from)
}

/// macOS does not enforce `RLIMIT_AS`, and Windows uses a Job Object set by the core: the cap is
/// the core's job there (gap A5).
#[cfg(not(target_os = "linux"))]
fn cap_address_space(_max: u64) -> io::Result<()> {
    Ok(())
}
