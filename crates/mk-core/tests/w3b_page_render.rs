//! Watch item W3b-1 (M3b, `docs/11-ingestion.md` §6): a page view of TD-1's 500-page ledger
//! through the real path (core → render worker → PNG → page cache), what the desktop app does on
//! a click (`page_view` + `page_image`).
//!
//! Twenty pages picked at random (fixed seed). Three cases: cold (project just opened, cache
//! empty: the render worker starts), warm (worker running, page not cached), cached. Mark:
//! p95 ≤ 1 s, worker start included (NFR-PERF-14 share). Machine-dependent, so `#[ignore]`; run
//! on RM-1 when it is idle:
//!
//! ```sh
//! cargo build -p mangkon-desktop --release
//! MK_WORKER_PROGRAM=$PWD/target/release/mangkon \
//!   cargo test -p mk-core --release --test w3b_page_render -- --ignored --nocapture
//! ```
//!
//! Prints one JSON line.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mk_core::{PageStatus, Project, WorkerConfig};

const PAGES: usize = 20;

fn ledger() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/td1/synthetic/pdf/vendor-ledger-2024-2025.pdf");
    assert!(p.exists(), "{} is missing", p.display());
    p
}

fn open(dir: &Path) -> Project {
    let (mut project, _) = Project::open(&dir.join("project")).unwrap();
    *project.worker_config_mut() = match std::env::var_os("MK_WORKER_PROGRAM") {
        Some(p) => WorkerConfig::with_program(p),
        None => WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker")),
    };
    project.set_tessdata_dir(Some(dir.join("tessdata")));
    project
}

/// One click: the page view, then its image.
fn view(project: &Project, document: i64, page: u32) -> Duration {
    let start = Instant::now();
    let v = project.page_view(document, page, None).unwrap().unwrap();
    assert_eq!(v.status, PageStatus::Shown, "page {page}: {:?}", v.note);
    let png = project.page_image(document, page).unwrap().unwrap();
    assert!(png.len() > 8);
    start.elapsed()
}

/// Distinct pages 1..=500 from a fixed-seed LCG.
fn pick(pages: u32) -> Vec<u32> {
    let mut x: u64 = 0x4d33_6231;
    let mut out = Vec::new();
    while out.len() < PAGES {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let p = (x >> 33) as u32 % pages + 1;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn stats(mut times: Vec<Duration>) -> (f64, f64) {
    times.sort();
    let at = |p: f64| {
        let i = ((times.len() as f64 * p).ceil() as usize).clamp(1, times.len()) - 1;
        (times[i].as_secs_f64() * 1000.0).round() / 1000.0
    };
    (at(0.5), at(0.95))
}

#[test]
#[ignore = "timing; run on an idle RM-1"]
fn w3b_page_render() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::copy(ledger(), src.join("ledger.pdf")).unwrap();
    let document = {
        let project = open(tmp.path());
        project.add_source_root(&src).unwrap();
        project.rescan(false).unwrap();
        assert_eq!(project.run_stage_a().unwrap().done, 1);
        project
            .store()
            .read_raw(|conn| {
                Ok(
                    conn.query_row("SELECT id FROM document WHERE parent_id IS NULL", [], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap()
    };
    let cache = mk_core::page_cache_dir(&open(tmp.path()).layout().cache());
    let pages = pick(500);

    // Cold: a fresh project and an empty cache for each page.
    let mut cold = Vec::new();
    for &page in &pages {
        let _ = std::fs::remove_dir_all(&cache);
        let project = open(tmp.path());
        cold.push(view(&project, document, page));
    }
    // Warm: one project, the worker started on a page not in the sample.
    let _ = std::fs::remove_dir_all(&cache);
    let project = open(tmp.path());
    let other = (1..=500).find(|p| !pages.contains(p)).unwrap();
    view(&project, document, other);
    let warm: Vec<Duration> = pages.iter().map(|&p| view(&project, document, p)).collect();
    let cached: Vec<Duration> = pages.iter().map(|&p| view(&project, document, p)).collect();

    let (cold, warm, cached) = (stats(cold), stats(warm), stats(cached));
    println!(
        r#"{{"test":"w3b_page_render","pages":{PAGES},"cold_p50":{},"cold_p95":{},"warm_p50":{},"warm_p95":{},"cached_p50":{},"cached_p95":{}}}"#,
        cold.0, cold.1, warm.0, warm.1, cached.0, cached.1
    );
    assert!(cold.1 <= 1.0, "W3b-1: cold p95 {} s > 1 s", cold.1);
    assert!(warm.1 <= 1.0, "W3b-1: warm p95 {} s > 1 s", warm.1);
}
