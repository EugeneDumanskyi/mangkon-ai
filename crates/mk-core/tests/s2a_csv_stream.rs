//! Spike S2a (roadmap §3, R6, K3; NFR-PERF-22): does a 1M-row CSV stream worker → core → one
//! commit at ≥ 50,000 rows/s end to end? Worker start time? Kill-and-replace time? Core memory
//! while it buffers the result?
//!
//! 1. End to end through the real lane with one worker, for chunks of 1k, 4k and 16k segments:
//!    re-scan (read + hash), then Stage A (worker read + hash check, parse, extract, stream;
//!    core decode + assemble; commit with the derived rebuild). Peak RSS sampled every 100 ms.
//!    The same bytes once more as a `.txt` file (memory factor for plain text).
//! 2. The same steps in process, one at a time, to see where the time goes.
//! 3. 100 × worker start to `Hello`; 100 × kill + replace + `Hello`. The desktop binary too when
//!    `MK_S2A_DESKTOP` names it (it maps the WebKitGTK libraries on Linux).
//!
//! Pass mark: ≥ 50,000 rows/s including the commit. Machine-dependent, so `#[ignore]`; run on
//! RM-1 when it is idle:
//!
//! ```sh
//! cargo run -p corpus-gen --release -- csv-rows --rows 1000000
//! cargo test -p mk-core --release --test s2a_csv_stream -- --ignored --nocapture
//! ```
//!
//! Prints one JSON summary.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mk_core::{Project, WorkerConfig};
use mk_kernel::ContentHash;
use mk_parsers::Format;
use mk_store::NewContent;
use mk_worker::frame::{FrameReader, FrameWriter};
use mk_worker::{FromWorker, Limits};
use serde_json::{Value, json};

const ROWS: usize = 1_000_000;
const PASS_ROWS_PER_S: f64 = 50_000.0;
const STARTS: usize = 100;

fn worker() -> WorkerConfig {
    WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e5).round() / 100.0
}

fn mb(b: u64) -> f64 {
    (b as f64 / 1048576.0 * 10.0).round() / 10.0
}

/// One end-to-end run with `chunk` segments per chunk, on a fresh project. `name` sets the
/// format by its extension.
fn end_to_end(csv: &Path, name: &str, chunk: u32) -> Value {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join(name);
    if std::fs::hard_link(csv, &file).is_err() {
        std::fs::copy(csv, &file).unwrap();
    }
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() = WorkerConfig {
        limits: Limits {
            chunk_segments: chunk,
        },
        ..worker()
    };
    project.set_stage_a_workers(1);
    project.add_source_root(&src).unwrap();
    let t = Instant::now();
    project.rescan(false).unwrap();
    let rescan = t.elapsed();
    let t = Instant::now();
    let run = project.run_stage_a().unwrap();
    let stage_a = t.elapsed();
    assert_eq!(run.done, 1, "{run:?}");
    let rows = ROWS + 1;
    let rate = rows as f64 / stage_a.as_secs_f64();
    json!({
        "file": name,
        "chunk_segments": chunk,
        "rescan_ms": ms(rescan),
        "stage_a_ms": ms(stage_a),
        "commit_ms": ms(run.commit_latencies[0]),
        "rows_per_s": rate.round(),
        "rows_per_s_incl_rescan": (rows as f64 / (stage_a + rescan).as_secs_f64()).round(),
        "peak_core_and_worker_mb": mb(run.peak_memory),
        "peak_core_mb": mb(run.peak_core_memory),
        "pass": rate >= PASS_ROWS_PER_S,
    })
}

/// The Stage A steps in process, timed one by one.
fn breakdown(csv: &Path) -> Value {
    let t = Instant::now();
    let bytes = std::fs::read(csv).unwrap();
    let hash = ContentHash::of(&bytes);
    let read_hash = t.elapsed();

    let t = Instant::now();
    let parsed = mk_parsers::parse(Format::Csv, &bytes).unwrap();
    let parse = t.elapsed();

    let copy = parsed.clone();
    let t = Instant::now();
    let mut n = 0usize;
    mk_worker::extract::chunks::<()>(
        copy,
        4096,
        |_| {
            n += 1;
            Ok(())
        },
        |_| {},
    )
    .unwrap();
    let extract = t.elapsed();

    let t = Instant::now();
    let mut w = FrameWriter::new(Vec::new());
    mk_worker::extract::chunks(parsed, 4096, |c| w.send(&FromWorker::Chunk(c)), |_| {}).unwrap();
    let extract_encode = t.elapsed();
    let frames = w.into_inner();

    let t = Instant::now();
    let mut r = FrameReader::new(&frames[..]);
    let mut staging = mk_core::stage_a::Staging::in_memory();
    while let Some(FromWorker::Chunk(c)) = r.recv().unwrap() {
        staging.push(c).unwrap();
    }
    let output = staging.into_output(Vec::new()).unwrap();
    let decode_assemble = t.elapsed();
    let mentions = output.mentions.len();

    let tmp = tempfile::tempdir().unwrap();
    let (project, _) = Project::open(&tmp.path().join("project")).unwrap();
    let item = project
        .store()
        .enqueue_contents(vec![NewContent {
            hash,
            file_type: "csv".into(),
            size: bytes.len() as u64,
        }])
        .unwrap()[0];
    project.store().start_item(item).unwrap().unwrap();
    let t = Instant::now();
    project.store().commit_stage_a(item, output).unwrap();
    let commit = t.elapsed();

    json!({
        "bytes": bytes.len(),
        "frame_bytes": frames.len(),
        "chunks": n,
        "mentions": mentions,
        "read_hash_ms": ms(read_hash),
        "parse_ms": ms(parse),
        "extract_ms": ms(extract),
        "encode_ms": ms(extract_encode.saturating_sub(extract)),
        "decode_assemble_ms": ms(decode_assemble),
        "commit_ms": ms(commit),
    })
}

fn start(program: &Path) -> (Child, Duration) {
    let t = Instant::now();
    let mut child = Command::new(program)
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut from = FrameReader::new(child.stdout.take().unwrap());
    match from.recv::<FromWorker>().unwrap() {
        Some(FromWorker::Hello { .. }) => {}
        other => panic!("{other:?}"),
    }
    (child, t.elapsed())
}

fn stats(mut v: Vec<Duration>) -> Value {
    v.sort();
    let at = |q: f64| ms(v[((v.len() - 1) as f64 * q).round() as usize]);
    json!({ "n": v.len(), "p50_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0) })
}

/// Start to `Hello`, and kill + wait + start + `Hello`.
fn starts(program: &Path) -> Value {
    let mut first = Vec::new();
    let mut replace = Vec::new();
    for _ in 0..STARTS {
        let (mut child, d) = start(program);
        first.push(d);
        let t = Instant::now();
        child.kill().unwrap();
        child.wait().unwrap();
        let (mut next, _) = start(program);
        replace.push(t.elapsed());
        next.kill().unwrap();
        next.wait().unwrap();
    }
    json!({ "program": program.display().to_string(), "start": stats(first), "kill_and_replace": stats(replace) })
}

#[test]
#[ignore = "spike S2a: timing on RM-1 with testdata/s2a/rows-1m.csv"]
fn s2a_csv_1m_rows() {
    let csv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/s2a/rows-1m.csv");
    assert!(
        csv.exists(),
        "run `cargo run -p corpus-gen --release -- csv-rows --rows 1000000` first"
    );
    let runs: Vec<Value> = [1024, 4096, 16384]
        .into_iter()
        .map(|c| end_to_end(&csv, "rows.csv", c))
        .collect();
    // The same bytes as plain text, for the TXT memory factor of `Format::memory_estimate`.
    let txt = end_to_end(&csv, "rows.txt", 4096);
    let mut workers = vec![starts(Path::new(env!("CARGO_BIN_EXE_mk-test-worker")))];
    if let Some(desktop) = std::env::var_os("MK_S2A_DESKTOP") {
        workers.push(starts(Path::new(&desktop)));
    }
    let summary = json!({
        "spike": "S2a",
        "rows": ROWS,
        "pass_mark_rows_per_s": PASS_ROWS_PER_S,
        "end_to_end": runs,
        "same_bytes_as_txt": txt,
        "breakdown_in_process": breakdown(&csv),
        "workers": workers,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
