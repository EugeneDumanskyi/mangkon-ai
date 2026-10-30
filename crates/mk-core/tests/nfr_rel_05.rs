//! NFR-REL-05, US-03.2 AC1: pause takes effect within 5 s, p95 over 20 pauses during a TD-2
//! ingest. Each round lets Stage A run for 2–9 s (fixed seed), pauses and times `pause()` until
//! the lanes have stopped and the jobs are Paused, then resumes.
//!
//! Machine-dependent, so `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! cargo test -p mk-core --release --test nfr_rel_05 -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mk_core::{Project, Stop, WorkerConfig};
use serde_json::json;

const PAUSES: usize = 20;
const LIMIT: Duration = Duration::from_secs(5);

#[test]
#[ignore = "NFR-REL-05: pause latency on RM-1 with testdata/td2"]
fn nfr_rel_05_pause_p95_td2() {
    let td2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td2");
    assert!(
        td2.exists(),
        "run `cargo run -p corpus-gen --release -- td2` first"
    );
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    let program = std::env::var_os("MK_WORKER_PROGRAM").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_mk-test-worker")),
        PathBuf::from,
    );
    *project.worker_config_mut() = WorkerConfig::with_program(&program);
    project.set_tessdata_dir(Some(tmp.path().join("tessdata")));
    project.add_source_root(&td2).unwrap();
    // Hash everything first, so every pause lands in Stage A.
    project.rescan(false).unwrap();

    let mut seed: u64 = 1;
    let mut rounds = Vec::new();
    let started = Instant::now();
    for _ in 0..PAUSES {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let run_for = Duration::from_millis(2_000 + (seed >> 33) % 7_000);
        let (took, busy, done, interrupted) = std::thread::scope(|s| {
            let run = s.spawn(|| project.ingest().unwrap());
            std::thread::sleep(run_for);
            let v = project.progress_view().unwrap();
            let t = Instant::now();
            project.pause().unwrap();
            let took = t.elapsed();
            let summary = run.join().unwrap();
            (
                took,
                v.current_files.len(),
                v.files_done,
                summary.interrupted,
            )
        });
        assert!(project.is_paused().unwrap());
        if interrupted != Some(Stop::Pause) {
            // The run finished before the pause; nothing was left to stop.
            break;
        }
        eprintln!(
            "pause {}: after {run_for:?}, {busy} files in workers, {done} done: took {took:?}",
            rounds.len() + 1
        );
        rounds.push(json!({
            "run_for_ms": run_for.as_millis(),
            "in_workers": busy,
            "files_done": done,
            "took_ms": took.as_secs_f64() * 1e3,
        }));
        project.resume().unwrap();
    }
    let rest = project.ingest().unwrap();

    let mut took: Vec<f64> = rounds
        .iter()
        .map(|r| r["took_ms"].as_f64().unwrap())
        .collect();
    took.sort_by(f64::total_cmp);
    let pct = |p: f64| took[((p * took.len() as f64).ceil() as usize).clamp(1, took.len()) - 1];
    let summary = json!({
        "pauses": took.len(),
        "min_ms": took.first(),
        "median_ms": pct(0.5),
        "p95_ms": pct(0.95),
        "max_ms": took.last(),
        "over_4s": took.iter().filter(|t| **t >= 4_000.0).count(),
        "wall_s": started.elapsed().as_secs(),
        "last_run": { "done": rest.stage_a.done, "failed": rest.stage_a.failed },
        "rounds": rounds,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert_eq!(took.len(), PAUSES, "TD-2 finished before {PAUSES} pauses");
    assert!(
        pct(0.95) <= LIMIT.as_secs_f64() * 1e3,
        "p95 {} ms",
        pct(0.95)
    );
}
