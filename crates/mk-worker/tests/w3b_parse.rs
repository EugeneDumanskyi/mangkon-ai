//! M3b speed marks for the worker's parse + identifier step (`docs/11-ingestion.md` §6), on
//! TD-1, one thread, no store:
//!
//! - NFR-PERF-20: text-native PDF pages ≥ 20 pages/s (`synthetic/pdf/`, incl. the 500-page
//!   ledger).
//! - NFR-PERF-21: OCR ≤ 5 s per page (render or decode + OCR + layout; `synthetic/scan/`,
//!   `scan-pdf/`, `mixed/`), English `tessdata_fast`, the engine loaded before timing.
//!
//! Machine-dependent, so `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! cargo test -p mk-worker --release --test w3b_parse -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Tesseract is built without OpenMP, so OCR runs on one thread. Each test prints one JSON line.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mk_parsers::{Engines, Format, OcrConfig};
use mk_worker::extract::ChunkSink;

fn td1(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/td1/synthetic")
        .join(dir);
    assert!(dir.exists(), "{} is missing", dir.display());
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
}

fn format(path: &Path) -> Format {
    Format::from_extension(path.extension().unwrap().to_str().unwrap()).unwrap()
}

/// Parses one file through the worker's sink (segments, identifiers, chunks dropped). Returns
/// its pages.
fn parse(path: &Path, engines: &mut Engines, on_ocr: impl FnMut(bool) + 'static) -> u64 {
    let pages = Rc::new(RefCell::new(0));
    let seen = pages.clone();
    let mut sink = ChunkSink::new(
        4096,
        |_| Ok::<(), ()>(()),
        move |units| *seen.borrow_mut() = units,
    )
    .in_units()
    .on_ocr(on_ocr);
    let mut file = std::fs::File::open(path).unwrap();
    mk_parsers::parse_stream(format(path), &mut file, &mut sink, engines)
        .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
    sink.finish().unwrap();
    *pages.borrow()
}

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    (sorted[i].as_secs_f64() * 1000.0).round() / 1000.0
}

/// NFR-PERF-20: text-native PDF pages per second, parse + identifiers.
#[test]
#[ignore = "timing; run on an idle RM-1"]
fn w3b_pdf_throughput() {
    let mut engines = Engines::default();
    let (mut pages, mut total) = (0, Duration::ZERO);
    let mut ledger = None;
    for path in td1("pdf") {
        let start = Instant::now();
        let n = parse(&path, &mut engines, |_| {
            panic!("a text PDF page needed OCR")
        });
        let took = start.elapsed();
        if path.ends_with("vendor-ledger-2024-2025.pdf") {
            ledger = Some((n, took));
        }
        pages += n;
        total += took;
    }
    let (ledger_pages, ledger_took) = ledger.expect("the 500-page ledger");
    let rate = pages as f64 / total.as_secs_f64();
    println!(
        r#"{{"test":"w3b_pdf_throughput","pages":{pages},"secs":{:.2},"pages_per_sec":{rate:.0},"ledger_pages":{ledger_pages},"ledger_pages_per_sec":{:.0}}}"#,
        total.as_secs_f64(),
        ledger_pages as f64 / ledger_took.as_secs_f64(),
    );
    assert!(rate >= 20.0, "NFR-PERF-20: {rate:.1} pages/s < 20");
}

/// NFR-PERF-21: seconds per OCR page, one thread.
#[test]
#[ignore = "timing; run on an idle RM-1"]
fn w3b_ocr_page_time() {
    let tessdata = tempfile::tempdir().unwrap();
    mk_parsers::ocr::install_english(tessdata.path()).unwrap();
    let mut engines = Engines::new(Some(OcrConfig {
        tessdata: tessdata.path().to_path_buf(),
        languages: "eng".into(),
    }));
    // Load the engine before timing.
    parse(&td1("scan")[0], &mut engines, |_| {});

    let times = Rc::new(RefCell::new(Vec::new()));
    for path in [td1("scan"), td1("scan-pdf"), td1("mixed")].concat() {
        let (times, started) = (times.clone(), Rc::new(RefCell::new(None)));
        parse(&path, &mut engines, move |active| {
            if active {
                *started.borrow_mut() = Some(Instant::now());
            } else if let Some(s) = started.borrow_mut().take() {
                times.borrow_mut().push(s.elapsed());
            }
        });
    }
    let mut times = times.take();
    times.sort();
    let (p50, p95, max) = (
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        percentile(&times, 1.0),
    );
    println!(
        r#"{{"test":"w3b_ocr_page_time","pages":{},"p50_secs":{p50},"p95_secs":{p95},"max_secs":{max}}}"#,
        times.len(),
    );
    assert!(max <= 5.0, "NFR-PERF-21: slowest OCR page {max} s > 5 s");
}
