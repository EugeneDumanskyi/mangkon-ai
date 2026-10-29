//! M3a/M3b criterion 4: every file of TD-3 (text formats, PDFs and images, with the memory bomb,
//! endless loop and crash fault files), mixed into TD-1, ends as `expected.json` says, with the
//! reason it quotes. A failure fails that file alone with
//! a plain reason; every other file finishes (NFR-REL-01/06/07, NFR-USE-05, INV-CO-09). Files
//! over the size limits fail before any worker reads them (NFR-CAP-07). Skipped without
//! `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use mk_bench::td3::Outcome;
use mk_core::{Project, WorkerConfig};
use mk_corpus::IngestionState;
use mk_parsers::Format;

fn testdata() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    if dir.join("td3/expected.json").exists() && dir.join("td1/ground-truth.json").exists() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
        "testdata/td1 or testdata/td3 is missing"
    );
    eprintln!("skipped: testdata is missing (see docs/07-test-corpus.md §9)");
    None
}

fn format(path: &str) -> Option<Format> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Format::from_extension)
}

fn is_text(path: &str) -> bool {
    format(path).is_some_and(|f| !f.is_paged())
}

fn is_paged(path: &str) -> bool {
    format(path).is_some_and(|f| f.is_paged())
}

/// TD-3 mixed into TD-1, ingested once for the tests below.
struct Fixture {
    _tmp: tempfile::TempDir,
    project: Project,
    td3: mk_bench::td3::Td3Manifest,
    gt: mk_bench::GroundTruth,
    td1_root: i64,
    td3_root: i64,
    failed: usize,
}

fn fixture() -> Option<&'static Fixture> {
    static F: OnceLock<Option<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        let data = testdata()?;
        let td3 = mk_bench::load_td3(&data.join("td3/expected.json")).unwrap();
        let gt = mk_bench::load(&data.join("td1/ground-truth.json")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
        *project.worker_config_mut() = WorkerConfig {
            test_faults: true,
            // Short, so the endless loop does not hold the test for 120 s; the slowest real
            // file (the DOCX bomb, read to 256 MB) takes ~5 s in a debug build.
            deadline: Duration::from_secs(20),
            ..WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
        };
        project.set_tessdata_dir(Some(tmp.path().join("tessdata")));
        let td1_root = project
            .add_source_root(&data.join("td1/synthetic"))
            .unwrap();
        let td3_root = project.add_source_root(&data.join("td3")).unwrap();
        project.rescan(false).unwrap();
        let run = project.run_stage_a().unwrap();
        Some(Fixture {
            _tmp: tmp,
            project,
            td3,
            gt,
            td1_root,
            td3_root,
            failed: run.failed,
        })
    })
    .as_ref()
}

fn content(f: &Fixture, root: i64, rel: &str) -> mk_store::ContentRow {
    let refs = f.project.store().stored_refs().unwrap();
    let r = refs
        .iter()
        .find(|r| r.root == root && r.rel_path == rel)
        .unwrap_or_else(|| panic!("{rel} was not stored"));
    f.project.store().content(&r.hash).unwrap().unwrap()
}

/// Every TD-3 file `pick` takes ends as `expected.json` says, with the reason it quotes.
/// Returns how many were checked and how many fail.
fn check_td3(f: &Fixture, pick: fn(&str) -> bool) -> (usize, usize) {
    let mut want_failed = 0;
    let mut checked = 0;
    let mut wrong = Vec::new();
    for t in f.td3.files.iter().filter(|t| pick(&t.path)) {
        let c = content(f, f.td3_root, &t.path);
        match t.expected {
            Outcome::Success => {
                if c.state != IngestionState::Done {
                    wrong.push(format!("{}: {:?}", t.path, c.failure_reason));
                }
            }
            Outcome::Fail => {
                want_failed += 1;
                // NFR-USE-05: a plain sentence, not a code or a stack trace; and the one
                // `expected.json` quotes.
                let reason = c.failure_reason.clone().unwrap_or_default();
                if c.state != IngestionState::Failed
                    || !reason.starts_with(char::is_uppercase)
                    || reason.contains("::")
                    || !t.reason.contains(&format!("\"{reason}\""))
                {
                    wrong.push(format!(
                        "{}: {:?} {reason:?}, expected.json says {:?}",
                        t.path, c.state, t.reason
                    ));
                }
            }
        }
        checked += 1;
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    (checked, want_failed)
}

/// M3a criterion 4: every text-format file of TD-3 (TXT, CSV, EML, MBOX, DOCX, JSON, HTML),
/// with the memory bomb, endless loop and crash fault files, mixed into TD-1, ends as
/// `expected.json` says.
#[test]
fn nfr_rel_01_td3_text_formats_fail_alone() {
    let Some(f) = fixture() else { return };
    let (checked, _) = check_td3(f, is_text);
    assert!(checked >= 34, "only {checked} TD-3 text files");

    // Every TD-1 file finishes (all encodings are read from M3a, PDFs and images from M3b).
    for g in
        f.gt.files
            .iter()
            .filter(|g| g.path.starts_with("synthetic/") && format(&g.path).is_some())
    {
        let c = content(f, f.td1_root, g.path.strip_prefix("synthetic/").unwrap());
        assert_eq!(
            c.state,
            IngestionState::Done,
            "{}: {:?}",
            g.path,
            c.failure_reason
        );
    }
    // The fault files added nothing; the project is consistent.
    let derived = f.project.store().derived_state().unwrap();
    assert!(
        !derived
            .identifiers
            .contains_key("email:hidden@fault.example")
    );
    let found = |e: &str| derived.identifiers.contains_key(&format!("email:{e}"));
    // The huge files were read to the end.
    assert!(found("long0@line.example"));
    assert!(found("wide@cols.example"));
    // HTML: visible text only, never script text (US-02.3 AC2); open tags are fine. (`expected.json`
    // is read too, as JSON: its reasons must not quote an address that must stay unfound.)
    assert!(found("visible@html.example") && found("help@html.example"));
    assert!(!found("hidden@script.example"));
    assert!(found("open@tags.example") && found("cell@tags.example"));
    // Email: 8 levels of attached emails, not 9 (MAX_DEPTH); a cut mailbox keeps its last
    // message; an unknown charset is still read.
    assert!((0..=8).all(|l| found(&format!("level{l}@nested.example"))));
    assert!(!found("level9@nested.example"));
    assert!(found("eve.desk@mbox.example"));
    assert!(found("rates@charset.example"));
    assert!(found("anna.mueller@example.de"));
    assert!(f.project.check_health().unwrap().is_healthy());
}

/// M3b criterion 4, US-02.1 AC2, NFR-CAP-04: every PDF and image file of TD-3 (bombs, loops,
/// encrypted, truncated, wrong names, 2,000 and 2,001 pages) ends as `expected.json` says; each
/// failure fails that file alone.
#[test]
fn nfr_rel_01_td3_pdf_images_fail_alone() {
    let Some(f) = fixture() else { return };
    let (checked, paged_failed) = check_td3(f, is_paged);
    assert!(checked >= 24, "only {checked} TD-3 PDF and image files");
    // Nothing else failed: every TD-1 file and every other TD-3 file is checked above.
    let (_, text_failed) = check_td3(f, is_text);
    assert_eq!(f.failed, text_failed + paged_failed);
    // TD-3 v4: both TIFF pages, the WEBP and the scan attached to an email are read by OCR.
    let derived = f.project.store().derived_state().unwrap();
    for e in [
        "first@tiff.example",
        "second@tiff.example",
        "webp@scan.example",
        "signed@attach.example",
    ] {
        assert!(
            derived.identifiers.contains_key(&format!("email:{e}")),
            "{e} not found; have {:?}",
            derived
                .identifiers
                .keys()
                .filter(|k| k.contains("scan") || k.contains("tiff") || k.contains("attach"))
                .collect::<Vec<_>>()
        );
    }
    let email = content(f, f.td3_root, "email-with-scan.eml");
    let kinds: Vec<String> = f
        .project
        .store()
        .read_raw(move |c| {
            let mut stmt =
                c.prepare("SELECT kind FROM document WHERE content_id = ?1 ORDER BY id")?;
            let rows = stmt.query_map([email.id], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(kinds, ["eml", "image"]);
}

/// US-02.1 AC2: a PDF that needs a password to open fails with "PDF is password-protected"; one
/// with only an owner password (printing or copying limits) is read.
#[test]
fn us_02_1_ac2_encrypted_pdf_fails() {
    let Some(f) = fixture() else { return };
    let user = content(f, f.td3_root, "encrypted-user-password.pdf");
    assert_eq!(
        (user.state, user.failure_reason.as_deref()),
        (IngestionState::Failed, Some("PDF is password-protected"))
    );
    let owner = content(f, f.td3_root, "encrypted-owner-only.pdf");
    assert_eq!(owner.state, IngestionState::Done);
}

/// NFR-CAP-05/06/07: a CSV over 500 MB and an MBOX over 2 GB fail with "File too large" before a
/// worker reads them, and the batch goes on. The files are sparse, so they take no disk space.
#[test]
fn nfr_cap_07_over_limit_csv_mbox() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir(&src).unwrap();
    let sparse = |name: &str, head: &[u8], len: u64| {
        let f = std::fs::File::create(src.join(name)).unwrap();
        std::io::Write::write_all(&mut &f, head).unwrap();
        f.set_len(len).unwrap();
    };
    sparse("big.csv", b"a,b\n1,2\n", (500 << 20) + 1);
    sparse(
        "big.mbox",
        b"From a@b.example Mon Mar 03 10:00:00 2025\n",
        (2 << 30) + 1,
    );
    sparse("limit.csv", b"a,b\n1,ok@limit.example\n", 4096);
    std::fs::write(src.join("small.txt"), "fine@small.example\n").unwrap();

    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    let root = project.add_source_root(&src).unwrap();
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();
    assert_eq!(run.failed, 2);
    let refs = project.store().stored_refs().unwrap();
    let state = |rel: &str| {
        let r = refs
            .iter()
            .find(|r| r.root == root && r.rel_path == rel)
            .unwrap();
        project.store().content(&r.hash).unwrap().unwrap()
    };
    assert_eq!(
        state("big.csv").failure_reason.as_deref(),
        Some("File too large (limit: 500 MB)")
    );
    assert_eq!(
        state("big.mbox").failure_reason.as_deref(),
        Some("File too large (limit: 2 GB)")
    );
    for ok in ["limit.csv", "small.txt"] {
        assert_eq!(state(ok).state, IngestionState::Done, "{ok}");
    }
}
