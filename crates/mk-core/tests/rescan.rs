//! Re-scan through the real pipeline on temp folders (US-01.3 AC2–AC4, US-01.4 AC1–AC4,
//! architecture §5.4), plus `ConnectionsChanged`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mk_core::Project;
use mk_corpus::IngestionState;
use mk_jobs::ItemState;

struct Fixture {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    project: Project,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() =
        mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    Fixture {
        _tmp: tmp,
        src,
        project,
    }
}

/// Writes `text` with a fixed modified time `t` (seconds), so tests never depend on clock
/// granularity.
fn write(path: &Path, text: &str, t: u64) {
    std::fs::write(path, text).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + t))
        .unwrap();
}

fn connections(p: &Project) -> Vec<(String, u64)> {
    p.store()
        .connections_page(
            &mk_core::ConnectionQuery {
                include_ignored: true,
                ..Default::default()
            },
            None,
            100,
        )
        .unwrap()
        .into_iter()
        .map(|c| (c.row.subject_key, c.row.source_count))
        .collect()
}

fn state_of(p: &Project, rel: &str) -> IngestionState {
    let r = p
        .store()
        .stored_refs()
        .unwrap()
        .into_iter()
        .find(|r| r.rel_path == rel)
        .unwrap();
    p.store().content(&r.hash).unwrap().unwrap().state
}

fn seed(f: &Fixture) {
    write(
        &f.src.join("a.txt"),
        "From ann@x.example\nto bob@y.example\n",
        1,
    );
    write(
        &f.src.join("sub/b.md"),
        "# Notes\nann@x.example called\n",
        2,
    );
    write(
        &f.src.join("c.csv"),
        "name,email\nAnn,ann@x.example\nBob,bob@y.example\n",
        3,
    );
    write(&f.src.join("sheet.xlsx"), "PK", 4);
    write(&f.src.join(".hidden.txt"), "eve@z.example", 5);
}

/// NFR-PERF-33, INV-PR-08, US-01.4 AC1: a re-scan with no changes hashes nothing and creates no
/// work.
#[test]
fn nfr_perf_33_rescan_unchanged_creates_no_work() {
    let f = fixture();
    seed(&f);
    let first = f.project.rescan(false).unwrap();
    assert_eq!(
        (first.walked, first.hashed, first.outcome.new_items),
        (3, 3, 3)
    );
    assert_eq!(
        first.skipped.len(),
        1,
        "sheet.xlsx is reported, .hidden.txt is not"
    );
    f.project.run_stage_a().unwrap();
    let items_before = f.project.store().progress().unwrap().items;

    let again = f.project.rescan(false).unwrap();
    assert_eq!((again.walked, again.hashed, again.unchanged), (3, 0, 3));
    assert_eq!(again.outcome, Default::default());
    assert_eq!(f.project.store().progress().unwrap().items, items_before);

    // A full re-scan reads everything but still finds no work.
    let full = f.project.rescan(true).unwrap();
    assert_eq!((full.hashed, full.outcome.new_items), (3, 0));
    assert_eq!(f.project.run_stage_a().unwrap().done, 0);
}

/// US-01.3 AC2, US-07.1 AC1: the first scan and Stage A give the expected connections.
#[test]
fn us_01_3_ac2_add_folder_builds_connections() {
    let f = fixture();
    seed(&f);
    f.project.rescan(false).unwrap();
    let run = f.project.run_stage_a().unwrap();
    assert_eq!((run.done, run.failed), (3, 0));
    assert_eq!(
        connections(&f.project),
        vec![
            ("domain:x.example".into(), 3),
            ("email:ann@x.example".into(), 3),
            ("domain:y.example".into(), 2),
            ("email:bob@y.example".into(), 2)
        ]
    );
}

/// US-01.4 AC3, INV-CO-06: a modified file points to a new content; the old content's data is
/// replaced in one step, and the new results show after Stage A.
#[test]
fn us_01_4_ac3_modified_file_is_reparsed() {
    let f = fixture();
    seed(&f);
    f.project.rescan(false).unwrap();
    f.project.run_stage_a().unwrap();
    write(&f.src.join("a.txt"), "Only carl@w.example now\n", 10);
    let scan = f.project.rescan(false).unwrap();
    assert_eq!((scan.hashed, scan.outcome.new_items), (1, 1));
    assert_eq!(scan.outcome.replaced_contents, 1);
    // Old evidence of a.txt is gone already; bob is now in one document only.
    assert_eq!(
        connections(&f.project),
        vec![
            ("domain:x.example".into(), 2),
            ("email:ann@x.example".into(), 2)
        ]
    );
    assert_eq!(state_of(&f.project, "a.txt"), IngestionState::Queued);
    f.project.run_stage_a().unwrap();
    assert_eq!(state_of(&f.project, "a.txt"), IngestionState::Done);
    let facts = f.project.store().facts().unwrap();
    assert!(
        facts
            .mentions
            .iter()
            .any(|m| m.key.value() == "carl@w.example")
    );
    assert_eq!(facts.contents.len(), 3, "the replaced content is deleted");
}

/// US-01.4 AC4, INV-CO-07: a deleted file's content is Missing, keeps its data, and stops
/// counting; found again with the same bytes it is Done again with no new work.
#[test]
fn us_01_4_ac4_missing_then_found_again() {
    let f = fixture();
    seed(&f);
    f.project.rescan(false).unwrap();
    f.project.run_stage_a().unwrap();
    let hidden = f.src.join(".trash");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::rename(f.src.join("c.csv"), hidden.join("c.csv")).unwrap();
    let scan = f.project.rescan(false).unwrap();
    assert_eq!(scan.outcome.missing_contents, 1);
    assert_eq!(state_of(&f.project, "c.csv"), IngestionState::Missing);
    assert_eq!(
        connections(&f.project),
        vec![
            ("domain:x.example".into(), 2),
            ("email:ann@x.example".into(), 2)
        ]
    );
    let derived = f.project.store().derived_state().unwrap();
    assert_eq!(
        derived.identifiers["email:bob@y.example"].mention_count, 2,
        "data kept"
    );

    std::fs::rename(hidden.join("c.csv"), f.src.join("c.csv")).unwrap();
    let scan = f.project.rescan(false).unwrap();
    assert_eq!(
        (
            scan.hashed,
            scan.outcome.found_contents,
            scan.outcome.new_items
        ),
        (0, 1, 0)
    );
    assert_eq!(state_of(&f.project, "c.csv"), IngestionState::Done);
    assert_eq!(
        connections(&f.project).len(),
        4,
        "two emails and their domains"
    );
}

/// INV-CO-02, US-01.3 AC4: a copy of a file at a second path is one content, processed once.
#[test]
fn inv_co_02_duplicate_is_processed_once() {
    let f = fixture();
    seed(&f);
    f.project.rescan(false).unwrap();
    f.project.run_stage_a().unwrap();
    std::fs::copy(f.src.join("a.txt"), f.src.join("sub/a-copy.txt")).unwrap();
    let scan = f.project.rescan(false).unwrap();
    assert_eq!((scan.hashed, scan.outcome.new_items), (1, 0));
    assert_eq!(f.project.store().stored_refs().unwrap().len(), 4);
    assert_eq!(f.project.store().facts().unwrap().contents.len(), 3);
    // Deleting one of the two paths keeps the content present.
    std::fs::remove_file(f.src.join("a.txt")).unwrap();
    let scan = f.project.rescan(false).unwrap();
    assert_eq!(scan.outcome.missing_contents, 0);
    assert_eq!(state_of(&f.project, "sub/a-copy.txt"), IngestionState::Done);
}

/// US-02.7 AC1, INV-CO-09: a file changed between scan and parse fails alone, plainly; the
/// other files are processed.
#[test]
fn us_02_7_file_changed_during_processing_fails_alone() {
    let f = fixture();
    seed(&f);
    f.project.rescan(false).unwrap();
    write(&f.src.join("a.txt"), "changed under us\n", 20);
    let run = f.project.run_stage_a().unwrap();
    assert_eq!((run.done, run.failed), (2, 1));
    let r = f
        .project
        .store()
        .stored_refs()
        .unwrap()
        .into_iter()
        .find(|r| r.rel_path == "a.txt")
        .unwrap();
    let c = f.project.store().content(&r.hash).unwrap().unwrap();
    assert_eq!(c.state, IngestionState::Failed);
    assert_eq!(
        c.failure_reason.as_deref(),
        Some("File changed during processing")
    );
    let p = f.project.store().progress().unwrap();
    assert_eq!(
        (p.items_in(ItemState::Done), p.items_in(ItemState::Failed)),
        (2, 1)
    );
    // The next re-scan sees the new bytes as a new content.
    let scan = f.project.rescan(false).unwrap();
    assert_eq!(scan.outcome.new_items, 1);
}

/// US-02.4 AC3: a CSV with a malformed row is processed, with a warning.
#[test]
fn us_02_4_ac3_malformed_rows_warn() {
    let f = fixture();
    write(&f.src.join("m.csv"), "a,b\n1,2\n3\n4,5\n", 1);
    f.project.rescan(false).unwrap();
    assert_eq!(f.project.run_stage_a().unwrap().done, 1);
    let r = &f.project.store().stored_refs().unwrap()[0];
    let c = f.project.store().content(&r.hash).unwrap().unwrap();
    assert_eq!(c.warnings, vec!["1 row has the wrong number of columns"]);
    let segs = f.project.store().segments(c.id).unwrap();
    assert_eq!(segs.iter().filter(|s| s.malformed).count(), 1);
}

/// ADR-0005 step 5, NFR-PERF-34: a commit that changes connections raises one debounced
/// `ConnectionsChanged`; an ignore decision raises another.
#[test]
fn connections_changed_is_raised() {
    let f = fixture();
    let events = f.project.subscribe();
    seed(&f);
    f.project.rescan(false).unwrap();
    f.project.run_stage_a().unwrap();
    let e = events.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(e.changes >= 1);
    while events.recv_timeout(Duration::from_millis(700)).is_ok() {}
    f.project.set_ignored("email:bob@y.example", true).unwrap();
    assert!(events.recv_timeout(Duration::from_secs(2)).is_ok());
    let rows = f
        .project
        .store()
        .connections_page(&Default::default(), None, 10)
        .unwrap();
    assert_eq!(rows.len(), 3, "ann and the two domains");
}
