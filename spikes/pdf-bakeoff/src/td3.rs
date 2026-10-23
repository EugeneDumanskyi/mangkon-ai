//! TD-3 PDF rows: every file runs alone in a child process (this binary, `td3-one`), with a
//! wall-clock deadline and an RSS watchdog read from `/proc` — the ADR-0007 Linux path, as
//! harness code. The OCR rows (images, M0c) use the same watchdog (`ocr::td3`).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mk_bench::td3::{Outcome, Td3Category};
use serde::{Deserialize, Serialize};

use crate::engine::{PdfEngine, PdfErrorKind};
use crate::layout::layout;

/// ASSUMPTION (spike only): per-file limits until NFR-REL-06/07 workers exist (M2c).
pub const DEADLINE: Duration = Duration::from_secs(30);
pub const MAX_RSS_BYTES: u64 = 1 << 30;
const POLL: Duration = Duration::from_millis(100);

/// Files whose expected outcome is the app's own guard, not the engine's (TD-3 manifest
/// reason: "the expected outcome is the app's guard, not theirs"). Reported, not scored.
pub const APP_GUARD: &[&str] = &["bomb-image-dimensions.pdf"];

/// What the child reports on stdout as one JSON line when it ends normally.
#[derive(Serialize, Deserialize, Default)]
pub struct ChildReport {
    pub opened: bool,
    pub error_kind: Option<PdfErrorKind>,
    pub message: String,
    pub pages: usize,
    pub pages_failed: usize,
    pub chars: usize,
    pub render_ok: Option<bool>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// The child exited normally and reported a result.
    Clean,
    Panic,
    Signal,
    Timeout,
    OomKill,
}

#[derive(Serialize)]
pub struct Td3Result {
    pub file: String,
    pub category: Td3Category,
    pub expected: Outcome,
    pub outcome: Outcome,
    pub ended: Ended,
    pub matches: bool,
    pub report: Option<ChildReport>,
    pub detail: String,
    pub secs: f64,
    pub peak_rss_mb: f64,
}

/// Child side: open, chars + layout on every page (the parse path), render page 1 at scale 1
/// (the page-view path).
pub fn one(engine: &dyn PdfEngine, path: &Path) -> ChildReport {
    let mut r = ChildReport::default();
    let doc = match engine.open(path) {
        Ok(d) => d,
        Err(e) => {
            r.error_kind = Some(e.kind);
            r.message = e.message;
            return r;
        }
    };
    r.opened = true;
    r.pages = doc.page_count();
    if r.pages == 0 {
        r.error_kind = Some(PdfErrorKind::Corrupt);
        r.message = "no pages".into();
    }
    for i in 0..r.pages {
        match doc
            .page_info(i)
            .and_then(|info| Ok(layout(&doc.chars(i)?, &info)))
        {
            Ok(laid) => r.chars += laid.text.chars().filter(|c| !c.is_whitespace()).count(),
            Err(e) => {
                r.pages_failed += 1;
                if r.message.is_empty() {
                    r.error_kind = Some(e.kind);
                    r.message = format!("page {}: {}", i + 1, e.message);
                }
            }
        }
    }
    if r.pages > 0 {
        r.render_ok = Some(doc.render(0, 1.0).is_ok());
    }
    r
}

/// A processed file fails when it does not open, has no pages, or no page could be read.
pub fn outcome(r: &ChildReport) -> Outcome {
    if r.opened && r.pages > 0 && r.pages_failed < r.pages {
        Outcome::Success
    } else {
        Outcome::Fail
    }
}

fn rss_bytes(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// TD-3 files a PDF engine should take: PDF magic, or a `.pdf` name without image magic.
pub fn pdf_files(
    manifest: &mk_bench::td3::Td3Manifest,
    root: &Path,
) -> Vec<mk_bench::td3::Td3File> {
    manifest
        .files
        .iter()
        .filter(|f| {
            let mut head = [0u8; 4];
            let n = std::fs::File::open(root.join(&f.path))
                .and_then(|mut h| h.read(&mut head))
                .unwrap_or(0);
            let pdf_magic = n == 4 && &head == b"%PDF";
            let image_magic = n >= 3 && (head[..3] == [0xFF, 0xD8, 0xFF] || &head == b"\x89PNG");
            pdf_magic || (f.path.ends_with(".pdf") && !image_magic)
        })
        .cloned()
        .collect()
}

/// How a watched child ended, what it printed, and what it cost.
pub struct Watched {
    /// `Clean` when it exited 0; the caller still has to parse `stdout`.
    pub ended: Ended,
    pub stdout: String,
    pub detail: String,
    pub secs: f64,
    pub peak_rss_mb: f64,
}

/// Runs this binary with `args` in a child process under the deadline and the RSS cap; stderr
/// goes to `log`.
pub fn watch(args: &[std::ffi::OsString], log: &Path) -> Watched {
    let exe = std::env::current_exe().expect("own path");
    let stderr = std::fs::File::create(log)
        .map(Stdio::from)
        .unwrap_or(Stdio::null());
    let start = Instant::now();
    let mut child = Command::new(&exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .expect("spawn child");
    let mut peak = 0u64;
    let mut killed = None;
    let status = loop {
        if let Some(s) = child.try_wait().expect("wait") {
            break s;
        }
        peak = peak.max(rss_bytes(child.id()).unwrap_or(0));
        if peak > MAX_RSS_BYTES {
            killed = Some(Ended::OomKill);
        } else if start.elapsed() > DEADLINE {
            killed = Some(Ended::Timeout);
        }
        if killed.is_some() {
            child.kill().ok();
            break child.wait().expect("wait");
        }
        std::thread::sleep(POLL);
    };
    let secs = start.elapsed().as_secs_f64();
    let mut stdout = String::new();
    if let Some(mut o) = child.stdout.take() {
        o.read_to_string(&mut stdout).ok();
    }
    let (ended, detail) = match killed {
        Some(k) => (
            k,
            format!("killed after {secs:.1} s, peak RSS {} MB", peak >> 20),
        ),
        None if status.success() => (Ended::Clean, String::new()),
        None => {
            use std::os::unix::process::ExitStatusExt;
            match status.signal() {
                Some(sig) => (Ended::Signal, format!("signal {sig}")),
                None => (
                    Ended::Panic,
                    format!("exit {status}; see {}", log.display()),
                ),
            }
        }
    };
    Watched {
        ended,
        stdout,
        detail,
        secs,
        peak_rss_mb: peak as f64 / (1 << 20) as f64,
    }
}

/// Parent side: runs every TD-3 PDF in its own child.
pub fn run(engine: &str, root: &Path, out_dir: &Path) -> Vec<Td3Result> {
    let manifest = mk_bench::load_td3(&root.join("expected.json")).expect("TD-3 manifest");
    std::fs::create_dir_all(out_dir).ok();
    pdf_files(&manifest, root)
        .into_iter()
        .map(|f| {
            let log = out_dir.join(format!("td3-{engine}-{}.stderr", f.path));
            let w = watch(
                &["td3-one".into(), engine.into(), root.join(&f.path).into()],
                &log,
            );
            let (ended, report, detail) = match w.ended {
                Ended::Clean => match serde_json::from_str::<ChildReport>(w.stdout.trim()) {
                    Ok(r) => (Ended::Clean, Some(r), String::new()),
                    Err(e) => (Ended::Panic, None, format!("no report: {e}")),
                },
                e => (e, None, w.detail),
            };
            let outcome = report.as_ref().map_or(Outcome::Fail, outcome);
            Td3Result {
                file: f.path.clone(),
                category: f.category,
                expected: f.expected,
                outcome,
                ended,
                matches: outcome == f.expected,
                report,
                detail,
                secs: w.secs,
                peak_rss_mb: w.peak_rss_mb,
            }
        })
        .collect()
}
