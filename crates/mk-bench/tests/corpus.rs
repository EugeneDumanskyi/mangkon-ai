//! Acceptance checks on the local test corpus (`testdata/`, see `docs/07-test-corpus.md` §9).
//! The corpus is not committed. Without it these tests print a notice and pass, unless
//! `MK_REQUIRE_TESTDATA=1` is set (CI sets it after running `corpus-gen fetch` and `generate`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use mk_bench::td3::{Outcome, Td3Category};
use mk_bench::{Annotation, FileKind, Origin};
use mk_kernel::ContentHash;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

/// False (after a notice) when `marker` is missing and the corpus is not required.
fn corpus_present(marker: &str) -> bool {
    if testdata().join(marker).exists() {
        return true;
    }
    assert!(
        std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
        "testdata/{marker} is missing; run `cargo run -p corpus-gen --release -- fetch` and `-- generate`"
    );
    eprintln!("skipped: testdata/{marker} is missing (see docs/07-test-corpus.md §9)");
    false
}

/// US-14.3 AC1, NFR-LEG-04: the synthetic and public TD-1 slices are present with a
/// ground truth that loads, validates (including SHA-256 of every file) and covers both.
#[test]
fn us_14_3_ac1_ground_truth_valid() {
    if !corpus_present("td1/ground-truth.json") {
        return;
    }
    let td1 = testdata().join("td1");
    let gt = mk_bench::load(&td1.join("ground-truth.json")).expect("ground truth loads");
    if let Err(errors) = gt.validate(Some(&td1)) {
        panic!("ground truth is invalid:\n{}", errors.join("\n"));
    }
    let synthetic: Vec<_> = gt
        .files
        .iter()
        .filter(|f| f.origin == Origin::Synthetic)
        .collect();
    let public: Vec<_> = gt
        .files
        .iter()
        .filter(|f| f.origin == Origin::Public)
        .collect();
    assert!(
        synthetic.len() >= 75,
        "{} synthetic files, want at least 75",
        synthetic.len()
    );
    assert!(
        public.len() >= 15,
        "{} public files, want at least 15",
        public.len()
    );
    // Synthetic files are fully annotated; public files are partial (NFR-LEG-04: no names).
    assert!(
        synthetic
            .iter()
            .all(|f| f.annotation == Annotation::Full && f.path.starts_with("synthetic/"))
    );
    assert!(
        public
            .iter()
            .all(|f| f.annotation == Annotation::Partial && f.path.starts_with("public/"))
    );
    // Every file kind the plan names is present in the synthetic slice.
    let kinds: BTreeSet<FileKind> = synthetic.iter().map(|f| f.kind).collect();
    for k in [
        FileKind::PdfText,
        FileKind::PdfScan,
        FileKind::PdfMixed,
        FileKind::PdfOcrLayer,
        FileKind::Image,
        FileKind::Eml,
        FileKind::Mbox,
        FileKind::Csv,
        FileKind::Json,
        FileKind::Docx,
        FileKind::Html,
        FileKind::Txt,
        FileKind::Md,
    ] {
        assert!(kinds.contains(&k), "no synthetic {k:?} file");
    }
    assert!(!gt.page_text.is_empty(), "page text is loaded");
    assert!(
        !gt.links.is_empty()
            && !gt.negatives.is_empty()
            && !gt.relations.is_empty()
            && !gt.events.is_empty()
    );
}

/// M0 AC2: TD-3 v1 is generated; every file is listed (and only those), hashes match,
/// and every category has at least one file.
#[test]
fn td3_expected_manifest_complete() {
    if !corpus_present("td3/expected.json") {
        return;
    }
    let td3 = testdata().join("td3");
    let m = mk_bench::load_td3(&td3.join("expected.json")).expect("expected.json loads");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&td3)
        .expect("td3 dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .into_string()
                .expect("utf-8 name")
        })
        .filter(|n| n != "expected.json")
        .collect();
    let listed: BTreeSet<String> = m.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        listed.len(),
        m.files.len(),
        "duplicate paths in expected.json"
    );
    assert_eq!(on_disk, listed, "files on disk and in expected.json differ");
    for f in &m.files {
        let bytes = std::fs::read(td3.join(&f.path)).expect("read td3 file");
        assert_eq!(bytes.len() as u64, f.bytes, "{}: size", f.path);
        assert_eq!(
            ContentHash::of(&bytes).to_hex(),
            f.sha256,
            "{}: sha256",
            f.path
        );
        assert!(!f.reason.trim().is_empty(), "{}: no reason", f.path);
    }
    for c in Td3Category::ALL {
        assert!(
            m.files.iter().any(|f| f.category == c),
            "no TD-3 file in category {c:?}"
        );
    }
    assert!(m.files.iter().any(|f| f.expected == Outcome::Fail));
    assert!(m.files.iter().any(|f| f.expected == Outcome::Success));
}
