//! The worker loop.

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mk_kernel::ContentHash;
use mk_parsers::Format;

use crate::fault::Fault;
use crate::frame::{FrameReader, FrameWriter};
use crate::protocol::{FromWorker, PROTOCOL, ProcessContent, ToWorker, WorkStage};

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
            _ => WorkStage::Sending,
        }
    }
}

type Shared<W> = Arc<Mutex<FrameWriter<W>>>;

fn send<W: Write>(out: &Shared<W>, msg: &FromWorker) -> io::Result<()> {
    out.lock().unwrap_or_else(|e| e.into_inner()).send(msg)
}

/// Serves requests from `input` until `Shutdown` or the end of the input: says `Hello`, then for
/// each [`ToWorker::Process`] streams chunks and ends with `Done` or `Failed`. A parser panic
/// fails only its item. Returns an error if the output is gone.
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
    for msg in rx {
        let req = match msg {
            ToWorker::Shutdown => break,
            ToWorker::Process(req) => req,
        };
        progress.units.store(0, Ordering::Relaxed);
        progress.set(WorkStage::Reading);
        progress.busy.store(true, Ordering::Relaxed);
        let outcome = process(&req, &progress, &out);
        progress.busy.store(false, Ordering::Relaxed);
        let reply = match outcome {
            Ok(Ok(warnings)) => FromWorker::Done { warnings },
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
fn process<W: Write>(
    req: &ProcessContent,
    progress: &Progress,
    out: &Shared<W>,
) -> io::Result<Result<Vec<String>, (&'static str, String)>> {
    let bytes = match std::fs::read(&req.path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Err(("file_missing", "The file is no longer there".into())));
        }
        Err(e) => {
            return Ok(Err((
                "file_unreadable",
                format!("The file could not be read: {e}"),
            )));
        }
    };
    if ContentHash::of(&bytes) != req.content_hash {
        return Ok(Err((
            "file_changed",
            "File changed during processing".into(),
        )));
    }
    let fault = req
        .settings
        .test_faults
        .then(|| Fault::in_file(&bytes))
        .flatten();
    let Some(format) = Format::from_name(&req.file_type) else {
        return Ok(Err((
            "unsupported_type",
            "This file type is not supported yet".into(),
        )));
    };
    progress.set(WorkStage::Parsing);
    let parse = || {
        if let Some(fault) = fault {
            fault.run();
        }
        mk_parsers::parse(format, &bytes)
    };
    let parsed = match catch_unwind(AssertUnwindSafe(parse)) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return Ok(Err((e.code, e.message))),
        Err(_) => {
            return Ok(Err((
                "parser_crashed",
                "The file could not be read: the parser stopped".into(),
            )));
        }
    };
    drop(bytes);
    progress.set(WorkStage::Sending);
    let warnings = parsed.warnings.clone();
    crate::extract::chunks(
        parsed,
        req.limits.chunk_segments as usize,
        |c| send(out, &FromWorker::Chunk(c)),
        |n| progress.units.store(n, Ordering::Relaxed),
    )?;
    Ok(Ok(warnings))
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
