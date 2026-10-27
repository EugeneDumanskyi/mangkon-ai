//! M2 acceptance criterion 2: every TXT/CSV file of TD-3, with the memory bomb, endless loop
//! and crash fault files, mixed into the TD-1 TXT/CSV subset, ends as `expected.json` says. A
//! failure fails that file alone with a plain reason; every other file finishes (NFR-REL-01/06/07,
//! NFR-USE-05, INV-CO-09). Skipped without `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::path::{Path, PathBuf};
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

fn is_text(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Format::from_extension)
        .is_some()
}

#[test]
fn nfr_rel_01_td3_txt_csv_fail_alone() {
    let Some(data) = testdata() else { return };
    let td3 = mk_bench::load_td3(&data.join("td3/expected.json")).unwrap();
    let gt = mk_bench::load(&data.join("td1/ground-truth.json")).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() = WorkerConfig {
        test_faults: true,
        // Short, so the endless loop does not hold the test for 120 s; the slowest real file
        // (16 MB on one line) takes well under a second.
        deadline: Duration::from_secs(5),
        ..WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
    };
    let td1_root = project
        .add_source_root(&data.join("td1/synthetic"))
        .unwrap();
    let td3_root = project.add_source_root(&data.join("td3")).unwrap();
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();

    let refs = project.store().stored_refs().unwrap();
    let state = |root: i64, rel: &str| {
        let r = refs
            .iter()
            .find(|r| r.root == root && r.rel_path == rel)
            .unwrap_or_else(|| panic!("{rel} was not stored"));
        project.store().content(&r.hash).unwrap().unwrap()
    };

    let mut want_failed = 0;
    let mut checked = 0;
    for f in td3.files.iter().filter(|f| is_text(&f.path)) {
        let c = state(td3_root, &f.path);
        match f.expected {
            Outcome::Success => assert_eq!(c.state, IngestionState::Done, "{}", f.path),
            Outcome::Fail => {
                want_failed += 1;
                assert_eq!(c.state, IngestionState::Failed, "{}", f.path);
                // NFR-USE-05: a plain sentence, not a code or a stack trace.
                let reason = c.failure_reason.unwrap_or_default();
                assert!(
                    reason.starts_with(char::is_uppercase) && !reason.contains("::"),
                    "{}: {reason:?}",
                    f.path
                );
            }
        }
        checked += 1;
    }
    assert!(checked >= 15, "only {checked} TD-3 text files");
    let reason = |rel| state(td3_root, rel).failure_reason.unwrap();
    assert_eq!(reason("fault-memory-bomb.txt"), "Needed too much memory");
    assert_eq!(reason("fault-endless-loop.txt"), "Took too long to read");
    assert_eq!(
        reason("fault-crash.txt"),
        "The file could not be read: the reader stopped unexpectedly"
    );
    assert_eq!(reason("binary-named.txt"), "Unknown text encoding");

    // Every TD-1 text file finishes, except those in an encoding read only from M3.
    for f in gt
        .files
        .iter()
        .filter(|f| f.path.starts_with("synthetic/") && is_text(&f.path))
    {
        let c = state(td1_root, f.path.strip_prefix("synthetic/").unwrap());
        if matches!(f.encoding.as_deref(), Some("utf-8" | "utf-8-bom")) {
            assert_eq!(c.state, IngestionState::Done, "{}", f.path);
        } else {
            want_failed += 1;
            assert_eq!(c.failure_reason.as_deref(), Some("Unknown text encoding"));
        }
    }
    assert_eq!(run.failed, want_failed);
    // The fault files added nothing; the project is consistent.
    let derived = project.store().derived_state().unwrap();
    assert!(
        !derived
            .identifiers
            .contains_key("email:hidden@fault.example")
    );
    // The huge files were read to the end.
    assert!(derived.identifiers.contains_key("email:long0@line.example"));
    assert!(derived.identifiers.contains_key("email:wide@cols.example"));
    assert!(project.check_health().unwrap().is_healthy());
}
