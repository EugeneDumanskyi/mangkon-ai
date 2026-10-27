//! NFR-RES-02 (M2 acceptance criterion 7b): app memory during Stage A on the CSV part of TD-2
//! ≤ 2 GB, model not counted. The CSVs run through the default lane (cores − 1 workers, memory
//! budget); the resident memory of this process plus its workers is sampled every 100 ms.
//!
//! Machine-dependent, so `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! cargo test -p mk-core --release --test nfr_res_02 -- --ignored --nocapture
//! MK_WORKER_PROGRAM=target/release/mangkon cargo test ...   # workers from the desktop binary
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use mk_core::{Project, WorkerConfig};
use serde_json::json;

const LIMIT: u64 = 2 << 30;

fn csvs(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            csvs(&p, out);
        } else if p.extension().is_some_and(|e| e == "csv") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "NFR-RES-02: memory on RM-1 with testdata/td2"]
fn nfr_res_02_stage_a_memory_td2_csv() {
    let td2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td2");
    assert!(
        td2.exists(),
        "run `cargo run -p corpus-gen --release -- td2` first"
    );
    let mut files = Vec::new();
    csvs(&td2, &mut files);
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (i, f) in files.iter().enumerate() {
        let to = src.join(format!(
            "{i:04}-{}",
            f.file_name().unwrap().to_str().unwrap()
        ));
        if std::fs::hard_link(f, &to).is_err() {
            std::fs::copy(f, &to).unwrap();
        }
    }
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    // The desktop binary maps the WebKitGTK libraries, so its workers count more resident memory.
    let program = std::env::var_os("MK_WORKER_PROGRAM").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_mk-test-worker")),
        PathBuf::from,
    );
    *project.worker_config_mut() = WorkerConfig::with_program(&program);
    project.add_source_root(&src).unwrap();
    project.rescan(false).unwrap();
    let t = Instant::now();
    let run = project.run_stage_a().unwrap();
    let summary = json!({
        "worker_program": program.display().to_string(),
        "files": files.len(),
        "done": run.done,
        "failed": run.failed,
        "lanes": mk_jobs::stage_a_limit(std::thread::available_parallelism().map_or(1, |n| n.get())),
        "stage_a_s": t.elapsed().as_secs_f64(),
        "peak_core_and_workers_mb": run.peak_memory / (1 << 20),
        "peak_core_mb": run.peak_core_memory / (1 << 20),
        "workers_started": run.workers_started,
        "pass": run.peak_memory <= LIMIT,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    // The Windows-1252 file of TD-1 fails until M3 reads other encodings; nothing else fails.
    assert_eq!(run.done + run.failed, files.len());
    for r in project.store().stored_refs().unwrap() {
        let c = project.store().content(&r.hash).unwrap().unwrap();
        if let Some(reason) = c.failure_reason {
            assert_eq!(reason, "Unknown text encoding", "{}", r.rel_path);
        }
    }
    assert!(run.peak_memory <= LIMIT);
}
