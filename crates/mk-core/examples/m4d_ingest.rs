//! M4d ingest timings: NFR-PERF-10 (drop to the first connection row) and the commit side of
//! NFR-PERF-34. Usage: `MK_WORKER_PROGRAM=target/release/mangkon-cli cargo run --release -p
//! mk-core --example m4d_ingest -- SOURCE`. Makes a fresh project in a temp folder.
//!
//! "First row" is the first moment a poll of the store sees a connection; the app shows it
//! after the next debounced event (500 ms) plus one render, so that is added to the figure.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mk_core::{ConnectionQuery, Project, WorkerConfig};

fn pct(v: &[Duration], p: f64) -> f64 {
    let i = ((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1;
    v[i].as_secs_f64() * 1000.0
}

fn main() {
    let source = PathBuf::from(std::env::args().nth(1).expect("source folder"));
    let tmp = tempfile::tempdir().unwrap();
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    let program = std::env::var_os("MK_WORKER_PROGRAM").expect("MK_WORKER_PROGRAM");
    *project.worker_config_mut() = WorkerConfig::with_program(program);
    project.set_tessdata_dir(Some(tmp.path().join("tessdata")));
    project.add_source_root(&source).unwrap();

    let project = Arc::new(project);
    let done = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let poller = {
        let (p, d) = (project.clone(), done.clone());
        std::thread::spawn(move || {
            let q = ConnectionQuery::default();
            while !d.load(Ordering::Relaxed) {
                if p.connection_count_for(&q).unwrap_or(0) > 0 {
                    return Some(start.elapsed());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            None
        })
    };
    let s = project.ingest().unwrap();
    let total = start.elapsed();
    done.store(true, Ordering::Relaxed);
    let first = poller.join().unwrap();
    let mut c = s.stage_a.commit_latencies.clone();
    c.sort();
    println!(
        "ingest {:.1} s, {} done, {} failed, {} changed commits",
        total.as_secs_f64(),
        s.stage_a.done,
        s.stage_a.failed,
        s.stage_a.changed_commits
    );
    match first {
        Some(t) => println!(
            "first connection in the store at {:.2} s (+0.5 s debounce = {:.2} s on screen, before render)",
            t.as_secs_f64(),
            t.as_secs_f64() + 0.5
        ),
        None => println!("no connection appeared"),
    }
    if !c.is_empty() {
        println!(
            "commit latency  n {}  p50 {:.1} ms  p95 {:.1} ms  max {:.1} ms",
            c.len(),
            pct(&c, 0.5),
            pct(&c, 0.95),
            pct(&c, 1.0)
        );
    }
}
