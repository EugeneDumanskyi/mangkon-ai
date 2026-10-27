//! M2 criterion 3 (NFR-PERF-32/33): a re-scan of TD-2 with no changes takes ≤ 60 s and creates
//! no work. Two numbers:
//!
//! 1. The app path: `Project::rescan` over TD-2. In M2b only TXT/MD/CSV files are stored
//!    (US-01.3 AC3), so the other files are walked and reported as skipped.
//! 2. The full corpus: the `mk-corpus` walk and diff with every TD-2 file as a stored
//!    reference, as it will be once every format is stored (M3).
//!
//! Each is run three times after a first scan, so the page cache is warm. Machine-dependent,
//! so `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! cargo test -p mk-core --release -- --ignored rescan_td2 --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mk_core::Project;
use mk_corpus::fs::FileTypes;
use mk_corpus::{Resolved, StoredRef};
use serde_json::json;

const RUNS: usize = 3;
const PASS: Duration = Duration::from_secs(60);

/// Every extension in TD-2.
const ALL_TYPES: &[&str] = &[
    "txt", "md", "csv", "eml", "mbox", "json", "html", "pdf", "png", "jpg", "jpeg", "tif", "tiff",
    "docx",
];

fn secs(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e3).round() / 1e3
}

#[test]
#[ignore = "NFR-PERF-32: timing on RM-1 with testdata/td2"]
fn rescan_td2() {
    let td2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td2");
    assert!(
        td2.join("td2-manifest.json").exists(),
        "testdata/td2 is missing: cargo run -p corpus-gen --release -- td2"
    );
    let tmp = tempfile::tempdir().unwrap();

    // 1. The app path.
    let (project, _) = Project::open(&tmp.path().join("project")).unwrap();
    project.add_source_root(&td2).unwrap();
    let t = Instant::now();
    let first = project.rescan(false).unwrap();
    let first_time = t.elapsed();
    let items = project.store().progress().unwrap().items;
    let mut app_runs = Vec::new();
    for _ in 0..RUNS {
        let t = Instant::now();
        let again = project.rescan(false).unwrap();
        let took = t.elapsed();
        // NFR-PERF-33, INV-PR-08: nothing hashed, no work created.
        assert_eq!((again.hashed, again.unchanged), (0, first.walked));
        assert_eq!(again.outcome, Default::default());
        assert_eq!(project.store().progress().unwrap().items, items);
        app_runs.push(took);
    }

    // 2. The full corpus: every file is a stored reference. The hashes are stand-ins: an
    // unchanged path is never hashed, so they are not read.
    let types = FileTypes::new(ALL_TYPES);
    let walk = mk_corpus::fs::walk(1, &td2, &types);
    assert!(walk.errors.is_empty(), "{:?}", walk.errors);
    let stored: Vec<StoredRef> = walk
        .entries
        .iter()
        .map(|e| StoredRef {
            root: e.root,
            rel_path: e.rel_path.clone(),
            meta: e.meta,
            hash: mk_kernel::ContentHash::of(e.rel_path.as_bytes()),
            present: true,
        })
        .collect();
    let mut full_runs = Vec::new();
    for _ in 0..RUNS {
        let t = Instant::now();
        let w = mk_corpus::fs::walk(1, &td2, &types);
        let walked = t.elapsed();
        let plan = mk_corpus::plan(&stored, &w.entries, &[1], false);
        assert_eq!(plan.to_hash().count(), 0);
        let Resolved { changes, unchanged } = plan.resolve(&Default::default());
        let total = t.elapsed();
        assert!(changes.is_empty());
        assert_eq!(unchanged, stored.len());
        full_runs.push((walked, total));
    }

    let app_worst = app_runs.iter().max().copied().unwrap_or_default();
    let full_worst = full_runs.iter().map(|r| r.1).max().unwrap_or_default();
    let pass = app_worst <= PASS && full_worst <= PASS;
    let summary = json!({
        "check": "rescan_td2",
        "app_path": {
            "walked": first.walked,
            "skipped": first.skipped.len(),
            "first_scan_s": secs(first_time),
            "first_scan_hashed": first.hashed,
            "rescan_s": app_runs.iter().map(|d| secs(*d)).collect::<Vec<_>>(),
            "new_items": 0,
        },
        "full_corpus": {
            "files": stored.len(),
            "walk_s": full_runs.iter().map(|r| secs(r.0)).collect::<Vec<_>>(),
            "walk_and_diff_s": full_runs.iter().map(|r| secs(r.1)).collect::<Vec<_>>(),
        },
        "page_cache": "warm",
        "pass_mark_s": PASS.as_secs(),
        "pass": pass,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert!(pass, "NFR-PERF-32 pass mark missed");
}
