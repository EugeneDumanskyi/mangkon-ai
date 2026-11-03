//! Watch items W3a-1 and W3a-2 (M3a, `docs/11-ingestion.md`): do a 500 MB CSV and a 2 GB MBOX
//! stream through one worker and the staged commit (ADR-0016) with bounded memory, and how fast?
//!
//! Each run: a fresh project, one lane, one worker; re-scan (read + hash), then Stage A (worker:
//! read + hash check, streamed parse, extract, chunks; core: staging with the spill file, commit).
//! Peak resident memory of core + worker and of the core alone, sampled every 100 ms.
//!
//! Marks: one file ≤ 1 GB (NFR-REL-07), the app ≤ 2 GB (NFR-RES-02); the CSV done ≤ 120 s
//! (NFR-REL-06); ≥ 50,000 rows/s (NFR-PERF-22); ≥ 200 messages/s (NFR-PERF-23).
//! Machine-dependent, so `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! cargo run -p corpus-gen --release -- csv-rows --rows 1000000          # s2a/rows-1m.csv
//! cargo run -p corpus-gen --release -- csv-rows --rows 6700000          # s2a/rows-6700000.csv
//! cargo run -p corpus-gen --release -- mbox-messages --bytes 2G         # w3a/messages-2g.mbox
//! cargo test -p mk-core --release --test w3a_streaming -- --ignored --nocapture --test-threads 1
//! ```
//!
//! `MK_WORKER_PROGRAM` runs another worker binary (`target/release/mangkon` for the desktop app).
//! Each test prints one JSON line.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mk_core::{Project, WorkerConfig};
use serde_json::json;

const GB: u64 = 1 << 30;

fn testdata(rel: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(rel);
    assert!(
        p.exists(),
        "{} is missing; see the module docs",
        p.display()
    );
    p
}

fn worker() -> WorkerConfig {
    match std::env::var_os("MK_WORKER_PROGRAM") {
        Some(p) => WorkerConfig::with_program(p),
        None => WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker")),
    }
}

fn secs(d: Duration) -> f64 {
    (d.as_secs_f64() * 100.0).round() / 100.0
}

fn mb(b: u64) -> u64 {
    b >> 20
}

/// Lines starting with `prefix` (counted before the timed run).
fn count_lines(path: &Path, prefix: &[u8]) -> u64 {
    let mut r = BufReader::with_capacity(1 << 20, std::fs::File::open(path).unwrap());
    let (mut n, mut line) = (0, Vec::new());
    while r.read_until(b'\n', &mut line).unwrap() > 0 {
        n += u64::from(line.starts_with(prefix));
        line.clear();
    }
    n
}

/// One end-to-end run on `file`; `units` is its rows or messages. Prints and checks the marks.
fn run(file: &Path, units: u64, unit: &str, min_rate: f64, max_time: Option<Duration>) {
    // Under `target/`, on the same file system as `testdata/`, so the hard link works (a re-scan
    // skips symlinks, and copying 2 GB would time the disk).
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let link = src.join(file.file_name().unwrap());
    if std::fs::hard_link(file, &link).is_err() {
        std::fs::copy(file, &link).unwrap();
    }
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() = worker();
    project.set_stage_a_workers(1);
    // `MK_W3A_PATTERNS=3` runs Stage A with three custom patterns (M4c timing, NFR-CAP-05).
    if std::env::var("MK_W3A_PATTERNS").is_ok_and(|v| v == "rare") {
        // Three patterns that match nothing in the file: the scanning cost alone.
        project.add_pattern("order", r"\bORD-\d{6}\b").unwrap();
        project
            .add_pattern("invoice", r"\bINV[-/]\d{4}[-/]\d{4,6}\b")
            .unwrap();
        project
            .add_pattern("ticket", r"\bTKT-[A-Z]{3}\d{5}\b")
            .unwrap();
    }
    if std::env::var("MK_W3A_PATTERNS").is_ok_and(|v| v == "3") {
        project.add_pattern("order", r"\bORD-\d{6}\b").unwrap();
        project
            .add_pattern("invoice", r"\bINV[-/]\d{4}[-/]\d{4,6}\b")
            .unwrap();
        project
            .add_pattern("iban-like", r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b")
            .unwrap();
    }
    project.add_source_root(&src).unwrap();
    let t = Instant::now();
    project.rescan(false).unwrap();
    let rescan = t.elapsed();
    let t = Instant::now();
    let run = project.run_stage_a().unwrap();
    let stage_a = t.elapsed();
    let rate = units as f64 / stage_a.as_secs_f64();
    let bytes = std::fs::metadata(file).unwrap().len();
    let summary = json!({
        "file": file.file_name().unwrap().to_string_lossy(),
        "bytes": bytes,
        unit: units,
        "done": run.done,
        "failed": run.failed,
        "rescan_s": secs(rescan),
        "stage_a_s": secs(stage_a),
        "commit_s": run.commit_latencies.first().map(|d| secs(*d)),
        "per_s": rate.round(),
        "peak_core_and_worker_mb": mb(run.peak_memory),
        "peak_core_mb": mb(run.peak_core_memory),
        "db_mb": mb(std::fs::metadata(tmp.path().join("project/project.db")).unwrap().len()),
    });
    println!("{summary}");
    assert_eq!((run.done, run.failed), (1, 0), "{run:?}");
    assert!(rate >= min_rate, "{rate:.0} {unit}/s");
    // A worker over 1 GB is stopped by the watchdog (NFR-REL-07), so `done` covers that mark.
    assert!(run.peak_memory <= 2 * GB, "app over 2 GB (NFR-RES-02)");
    if let Some(max) = max_time {
        assert!(stage_a <= max, "{stage_a:?}");
    }
}

/// NFR-PERF-22 again, on the streaming path: the 1M-row CSV of S2a.
#[test]
#[ignore]
fn w3a_1_perf_22_rows_1m() {
    let file = testdata("s2a/rows-1m.csv");
    run(&file, count_lines(&file, b"") - 1, "rows", 50_000.0, None);
}

/// W3a-1 (NFR-CAP-05): a CSV just under 500 MB, done ≤ 120 s with bounded memory.
#[test]
#[ignore]
fn w3a_1_csv_500mb() {
    let file = testdata("s2a/rows-6700000.csv");
    let len = std::fs::metadata(&file).unwrap().len();
    assert!(len <= 500 << 20, "{len} bytes is over the CSV limit");
    run(
        &file,
        count_lines(&file, b"") - 1,
        "rows",
        50_000.0,
        Some(Duration::from_secs(120)),
    );
}

/// W3a-2 (NFR-CAP-06, NFR-PERF-23): a 2 GB MBOX with bounded memory, ≥ 200 messages/s.
#[test]
#[ignore]
fn w3a_2_mbox_2gb() {
    let file = testdata("w3a/messages-2g.mbox");
    run(&file, count_lines(&file, b"From "), "messages", 200.0, None);
}
