//! Spike S2b (roadmap §3, R5, gap A5; NFR-REL-07): is a memory bomb stopped within 200 ms after
//! it passes 1 GB, and failed with "Needed too much memory", while other files finish?
//!
//! 20 bomb files (`mk-test-fault: memory-bomb`: 64 MB blocks, every page touched) mixed with 20
//! plain files, one lane, the default limits: watchdog sample every 100 ms, 1 GB per file; on
//! Linux the worker's own `RLIMIT_AS` cap behind it. For each bomb: how it was stopped (watchdog,
//! or the hard cap aborting an allocation), the highest RSS sampled, and the time from the last
//! sample under 1 GB to the stop, an upper bound on how long it ran over the limit.
//!
//! Pass mark: every bomb stopped ≤ 200 ms after passing 1 GB; every bomb Failed with "Needed too
//! much memory"; every plain file Done. Machine-dependent, so `#[ignore]`; run on RM-1 when it is
//! idle:
//!
//! ```sh
//! cargo test -p mk-core --release --test s2b_memory_bomb -- --ignored --nocapture
//! ```
//!
//! Prints one JSON summary. The short form, `s2b_memory_bomb_short` in `workers.rs`, runs on
//! every OS in CI.

use std::time::{Duration, Instant};

use mk_core::{Project, Stopped, WorkerConfig};
use serde_json::json;

const RUNS: usize = 20;
const PASS: Duration = Duration::from_millis(200);

#[test]
#[ignore = "spike S2b: timing on RM-1"]
fn s2b_memory_bomb() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..RUNS {
        std::fs::write(
            src.join(format!("bomb-{i:02}.txt")),
            format!("mk-test-fault: memory-bomb\nrun {i}\n"),
        )
        .unwrap();
        std::fs::write(
            src.join(format!("plain-{i:02}.txt")),
            format!("p{i}@x.example\n"),
        )
        .unwrap();
    }
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() = WorkerConfig {
        test_faults: true,
        ..WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
    };
    let limit = project.worker_config_mut().max_memory;
    project.set_stage_a_workers(1);
    project.add_source_root(&src).unwrap();
    project.rescan(false).unwrap();
    let t = Instant::now();
    let run = project.run_stage_a().unwrap();
    let total = t.elapsed();

    let runs: Vec<_> = run
        .stops
        .iter()
        .map(|s| {
            json!({
                "why": format!("{:?}", s.why),
                "code": s.code,
                "stopped_by": if s.peak_rss > limit { "watchdog" } else { "hard cap" },
                "peak_rss_mb": s.peak_rss / (1 << 20),
                "over_limit_ms": s.over_limit_for.map(|d| d.as_secs_f64() * 1000.0),
            })
        })
        .collect();
    let delays: Vec<Duration> = run.stops.iter().filter_map(|s| s.over_limit_for).collect();
    let all_memory = run.stops.len() == RUNS && run.stops.iter().all(|s| s.why == Stopped::Memory);
    let max = delays.iter().max().copied().unwrap_or_default();
    let summary = json!({
        "spike": "S2b",
        "os": std::env::consts::OS,
        "runs": RUNS,
        "limit_mb": limit / (1 << 20),
        "done": run.done,
        "failed": run.failed,
        "all_needed_too_much_memory": all_memory,
        "max_over_limit_ms": max.as_secs_f64() * 1000.0,
        "total_s": total.as_secs_f64(),
        "pass": all_memory && run.done == RUNS && max <= PASS,
        "stops": runs,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert_eq!(run.done, RUNS);
    assert!(all_memory);
}
