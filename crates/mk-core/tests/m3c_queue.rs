//! M3c criteria 1 and 8, E03 and US-02.7/02.8: progress (US-03.1), pause and resume (US-03.2,
//! NFR-REL-05), cancel (US-03.3), restart with paused jobs (US-03.4, NFR-REL-04), retry and the
//! documents list. A file with the `endless-loop` test fault keeps one item busy.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mk_core::{DocumentFilter, Eta, Project, Stop, WorkerConfig};
use mk_corpus::IngestionState;
use mk_jobs::{ItemState, JobState};
use mk_kernel::ContentHash;

const FAULT: &str = "busy.txt";

fn open(dir: &Path) -> Project {
    let (mut p, _) = Project::open(dir).unwrap();
    *p.worker_config_mut() = WorkerConfig {
        test_faults: true,
        ..WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
    };
    p
}

/// Three small files (read first), the busy fault file, then `big` larger files. With one lane
/// they run in that order (INV-PR-03: smaller first).
fn source(dir: &Path, big: usize) -> PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..3 {
        std::fs::write(src.join(format!("s{i}.txt")), format!("s{i}@x.example\n")).unwrap();
    }
    std::fs::write(
        src.join(FAULT),
        "mk-test-fault: endless-loop\nbusy@x.example\n",
    )
    .unwrap();
    for i in 0..big {
        std::fs::write(
            src.join(format!("big-{i}.txt")),
            format!("{}\nb{i}@x.example\nshared@x.example\n", "-".repeat(200)),
        )
        .unwrap();
    }
    src
}

fn busy_project(dir: &Path, big: usize) -> (Project, PathBuf) {
    let src = source(dir, big);
    let mut p = open(&dir.join("p"));
    p.set_stage_a_workers(1);
    p.add_source_root(&src).unwrap();
    (p, src)
}

/// Waits until `f` holds, at most `secs` seconds.
fn wait_for(secs: u64, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < until, "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn busy_now(p: &Project) -> bool {
    p.current_files().iter().any(|f| f.ends_with(FAULT))
}

fn job_states(p: &Project) -> Vec<String> {
    p.store()
        .read_raw(|c| {
            let mut s = c.prepare("SELECT state FROM job ORDER BY id")?;
            let rows = s.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

fn state(p: &Project, src: &Path, name: &str) -> IngestionState {
    let h = ContentHash::of(&std::fs::read(src.join(name)).unwrap());
    p.store().content(&h).unwrap().unwrap().state
}

/// The most attempts any Done item took.
fn max_done_attempts(p: &Project) -> i64 {
    p.store()
        .read_raw(|c| {
            Ok(c.query_row(
                "SELECT COALESCE(MAX(attempts), 0) FROM work_item WHERE state = 'done'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap()
}

/// US-03.1 AC1–AC2: while a run is going the panel shows stage A, files done of total, the file
/// in a worker and "Estimating…" until 5 files are done.
#[test]
fn us_03_1_progress_view_current_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, _) = busy_project(tmp.path(), 3);
    std::thread::scope(|s| {
        let run = s.spawn(|| p.ingest().unwrap());
        wait_for(30, || busy_now(&p));
        let v = p.progress_view().unwrap();
        assert!(v.running && !v.paused);
        assert_eq!(v.stage, Some("A"));
        assert_eq!((v.files_done, v.files_total), (3, 7));
        assert_eq!(v.current_files.len(), 1);
        assert!(v.current_files[0].ends_with(FAULT));
        assert_eq!(v.eta, Eta::Estimating);
        p.cancel().unwrap();
        run.join().unwrap();
    });
    let v = p.progress_view().unwrap();
    assert!(!v.running && v.current_files.is_empty());
    assert_eq!((v.stage, v.eta), (None, Eta::None));
}

/// US-03.1 AC2: once 5 files are done the ETA is a time.
#[test]
fn us_03_1_ac2_eta_after_five_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, _) = busy_project(tmp.path(), 0);
    // Two more small files so 5 finish before the busy one.
    let src = tmp.path().join("src");
    for i in 3..5 {
        std::fs::write(src.join(format!("s{i}.txt")), format!("s{i}@x.example\n")).unwrap();
    }
    std::fs::write(
        src.join("zz.txt"),
        format!("{}\nzz@x.example\n", "-".repeat(300)),
    )
    .unwrap();
    std::thread::scope(|s| {
        let run = s.spawn(|| p.ingest().unwrap());
        wait_for(30, || busy_now(&p));
        let v = p.progress_view().unwrap();
        assert_eq!(v.files_done, 5);
        assert!(matches!(v.eta, Eta::Left(_)), "{:?}", v.eta);
        p.cancel().unwrap();
        run.join().unwrap();
    });
}

/// US-03.2 AC1, INV-PR-06, NFR-REL-05: pause with a busy item takes effect within 5 s: the
/// item gets its 4 s grace, is stopped and goes back to Pending; no new item starts; the jobs
/// are Paused. US-03.2 AC2, INV-PR-05: resume carries on and redoes no finished file.
#[test]
fn us_03_2_ac1_pause_stops_new_items_and_ac2_resume_no_redo() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, src) = busy_project(tmp.path(), 4);
    let mut took = Duration::ZERO;
    std::thread::scope(|s| {
        let run = s.spawn(|| p.ingest().unwrap());
        wait_for(30, || busy_now(&p));
        let t = Instant::now();
        p.pause().unwrap();
        took = t.elapsed();
        let summary = run.join().unwrap();
        assert_eq!(summary.interrupted, Some(Stop::Pause));
        assert_eq!(summary.stage_a.interrupted, 1);
        assert_eq!(summary.stage_a.done, 3);
    });
    assert!(took <= Duration::from_secs(5), "pause took {took:?}");
    assert!(
        took >= mk_core::PAUSE_GRACE,
        "the busy item had its grace: {took:?}"
    );
    eprintln!("NFR-REL-05 busy item: pause took {took:?}");

    let progress = p.store().progress().unwrap();
    assert_eq!(progress.items_in(ItemState::InProgress), 0);
    assert_eq!(progress.items_in(ItemState::Pending), 5);
    assert_eq!(state(&p, &src, FAULT), IngestionState::Queued);
    assert!(p.is_paused().unwrap());
    assert!(!p.has_runnable_work().unwrap());
    assert_eq!(job_states(&p), vec![JobState::Paused.as_str()]);
    // A run while paused starts nothing.
    assert_eq!(p.ingest().unwrap().stage_a.done, 0);
    // Files added while paused wait too.
    std::fs::write(src.join("new.txt"), "new@x.example\n").unwrap();
    let s = p.ingest().unwrap();
    assert_eq!((s.rescan.outcome.added, s.stage_a.done), (1, 0));

    // Resume: the rest runs; the busy file now hits a short deadline and fails alone.
    p.worker_config_mut().deadline = Duration::from_secs(1);
    p.resume().unwrap();
    assert!(!p.is_paused().unwrap());
    let s = p.ingest().unwrap();
    assert_eq!((s.stage_a.done, s.stage_a.failed), (5, 1));
    assert_eq!(max_done_attempts(&p), 1, "no finished file was redone");
    assert_eq!(state(&p, &src, FAULT), IngestionState::Failed);
}

/// NFR-REL-05, fast items: a pause between short files is near instant. The p95 over 20
/// pauses during a TD-2 ingest is in `docs/11-ingestion.md` §8.
#[test]
fn nfr_rel_05_pause_fast_items() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..60 {
        std::fs::write(
            src.join(format!("f{i:02}.txt")),
            format!("f{i}@x.example\nshared@x.example\n"),
        )
        .unwrap();
    }
    let mut p = open(&tmp.path().join("p"));
    p.set_stage_a_workers(2);
    p.add_source_root(&src).unwrap();
    let mut took = Duration::ZERO;
    std::thread::scope(|s| {
        let run = s.spawn(|| p.ingest().unwrap());
        wait_for(30, || p.progress_view().unwrap().files_done >= 3);
        let t = Instant::now();
        p.pause().unwrap();
        took = t.elapsed();
        run.join().unwrap();
    });
    assert!(took < Duration::from_secs(2), "pause took {took:?}");
    eprintln!("NFR-REL-05 fast items: pause took {took:?}");
    p.resume().unwrap();
    p.ingest().unwrap();
    assert_eq!(p.counts().unwrap().done, 60);
    assert_eq!(max_done_attempts(&p), 1);
    assert!(p.rebuild().unwrap().same_as_incremental);
}

/// US-03.3 AC1, D10, INV-PR-04: cancel keeps finished files, rolls back the file in progress
/// and turns all queued work into "Not processed". Retry queues them again.
#[test]
fn us_03_3_ac1_cancel_keeps_done_rolls_back_in_progress_not_processed() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, src) = busy_project(tmp.path(), 4);
    let mut took = Duration::ZERO;
    std::thread::scope(|s| {
        let run = s.spawn(|| p.ingest().unwrap());
        wait_for(30, || busy_now(&p));
        let t = Instant::now();
        let out = p.cancel().unwrap();
        took = t.elapsed();
        assert_eq!((out.contents, out.jobs), (5, 1));
        assert_eq!(run.join().unwrap().interrupted, Some(Stop::Cancel));
    });
    assert!(took < Duration::from_secs(2), "cancel took {took:?}");
    let c = p.counts().unwrap();
    assert_eq!((c.done, c.queued, c.processing), (3, 0, 0));
    assert_eq!(state(&p, &src, FAULT), IngestionState::NotProcessed);
    assert_eq!(state(&p, &src, "big-0.txt"), IngestionState::NotProcessed);
    let busy_docs: i64 = {
        let h = ContentHash::of(&std::fs::read(src.join(FAULT)).unwrap());
        let id = p.store().content(&h).unwrap().unwrap().id;
        p.store()
            .read_raw(move |c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM document WHERE content_id = ?1",
                    [id],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    };
    assert_eq!(busy_docs, 0, "nothing of the file in progress was kept");
    assert_eq!(job_states(&p), vec![JobState::Cancelled.as_str()]);
    assert!(!p.has_runnable_work().unwrap());
    // Not processed rows offer Retry (D10).
    let not_processed = DocumentFilter {
        status: Some("not_processed".into()),
        file_type: None,
    };
    assert_eq!(p.documents_count(&not_processed).unwrap(), 5);
    assert_eq!(p.retry_all().unwrap(), 5);
    assert!(p.has_runnable_work().unwrap());
}

/// US-03.4 AC1, NFR-REL-04: quit while running, paused or queued, and every job comes back in
/// the same state; a paused project starts nothing on open.
#[test]
fn us_03_4_ac1_quit_restores_running_paused_queued() {
    let tmp = tempfile::tempdir().unwrap();
    // Running: stopped for close with the busy item in progress.
    let (p, _) = busy_project(&tmp.path().join("running"), 2);
    std::thread::scope(|s| {
        let run = s.spawn(|| p.ingest().unwrap());
        wait_for(30, || busy_now(&p));
        p.stop_for_close();
        assert_eq!(run.join().unwrap().interrupted, Some(Stop::Close));
    });
    drop(p);
    let p = open(&tmp.path().join("running/p"));
    assert_eq!(job_states(&p), vec![JobState::Running.as_str()]);
    assert!(p.has_runnable_work().unwrap());
    assert_eq!(
        p.store().progress().unwrap().items_in(ItemState::Pending),
        3
    );
    drop(p);

    // Paused.
    let (p, _) = busy_project(&tmp.path().join("paused"), 2);
    p.rescan(false).unwrap();
    p.pause().unwrap();
    drop(p);
    let p = open(&tmp.path().join("paused/p"));
    assert_eq!(job_states(&p), vec![JobState::Paused.as_str()]);
    assert!(p.is_paused().unwrap());
    assert!(!p.has_runnable_work().unwrap());
    drop(p);

    // Queued: found but never started.
    let (p, _) = busy_project(&tmp.path().join("queued"), 2);
    p.rescan(false).unwrap();
    drop(p);
    let p = open(&tmp.path().join("queued/p"));
    assert_eq!(job_states(&p), vec![JobState::Queued.as_str()]);
    assert!(p.has_runnable_work().unwrap());
}

/// US-02.7 AC2: a failed file shows a plain reason, and the technical detail behind "Show
/// details". AC3: Retry reads only that file again.
#[test]
fn us_02_7_ac2_reason_and_detail_ac3_retry_only_that_file() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("bad.txt"),
        b"\x89PNG\r\n\x1a\n\x00 not text \xc3\x28",
    )
    .unwrap();
    std::fs::write(src.join("crash.txt"), "mk-test-fault: crash\nc@x.example\n").unwrap();
    for i in 0..3 {
        std::fs::write(src.join(format!("ok{i}.txt")), format!("o{i}@x.example\n")).unwrap();
    }
    let mut p = open(&tmp.path().join("p"));
    p.set_stage_a_workers(2);
    p.add_source_root(&src).unwrap();
    let s = p.ingest().unwrap();
    assert_eq!((s.stage_a.done, s.stage_a.failed), (3, 2));

    let failed = DocumentFilter {
        status: Some("failed".into()),
        file_type: None,
    };
    let rows = p.documents_page(&failed, None, 10).unwrap().rows;
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(
            !r.failure_reason.as_deref().unwrap_or("").is_empty(),
            "{r:?}"
        );
        let detail = r.failure_detail.as_deref().unwrap_or("");
        assert!(
            detail.contains(r.failure_code.as_deref().unwrap()),
            "{detail}"
        );
    }
    let crash = rows.iter().find(|r| r.rel_path == "crash.txt").unwrap();
    assert!(crash.failure_detail.as_deref().unwrap().contains("Crashed"));

    let bad = rows.iter().find(|r| r.rel_path == "bad.txt").unwrap();
    assert_eq!(p.retry(&[bad.content_id]).unwrap(), 1);
    let again = p.ingest().unwrap();
    assert_eq!(
        again.stage_a.done + again.stage_a.failed,
        1,
        "only that file ran"
    );
    assert_eq!(max_done_attempts(&p), 1);
}

/// US-02.8 AC1–AC2: one row per path with name, type, size, units and status; filters by
/// status and type.
#[test]
fn us_02_8_ac2_filter_status_type() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.csv"),
        "name,email\nAnn,a@x.example\nBo,b@x.example\n",
    )
    .unwrap();
    std::fs::write(src.join("b.txt"), "a@x.example\n").unwrap();
    std::fs::write(src.join("gone.txt"), "g@x.example\n").unwrap();
    std::fs::write(src.join("bad.json"), "{ not json").unwrap();
    let p = open(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    std::fs::remove_file(src.join("gone.txt")).unwrap();
    p.ingest().unwrap();

    let all = p
        .documents_page(&DocumentFilter::default(), None, 100)
        .unwrap()
        .rows;
    let names: Vec<(&str, &str)> = all
        .iter()
        .map(|r| (r.rel_path.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("a.csv", "done"),
            ("b.txt", "done"),
            ("bad.json", "failed"),
            ("gone.txt", "missing")
        ]
    );
    let csv = &all[0];
    assert_eq!(
        (csv.file_type.as_str(), csv.units),
        ("csv", Some((3, "rows")))
    );
    assert!(csv.size > 0 && csv.root.ends_with("src"));

    let by = |status: Option<&str>, file_type: Option<&str>| {
        let f = DocumentFilter {
            status: status.map(Into::into),
            file_type: file_type.map(Into::into),
        };
        let rows = p.documents_page(&f, None, 100).unwrap().rows;
        assert_eq!(rows.len() as u64, p.documents_count(&f).unwrap());
        rows.into_iter().map(|r| r.rel_path).collect::<Vec<_>>()
    };
    assert_eq!(by(Some("missing"), None), vec!["gone.txt"]);
    assert_eq!(by(Some("done"), Some("txt")), vec!["b.txt"]);
    assert_eq!(by(None, Some("json")), vec!["bad.json"]);
    assert_eq!(p.document_types().unwrap()["txt"], 2);

    // Keyset paging gives every row once, in order.
    let mut after = None;
    let mut paged = Vec::new();
    loop {
        let page = p
            .documents_page(&DocumentFilter::default(), after.as_ref(), 1)
            .unwrap();
        paged.extend(page.rows.into_iter().map(|r| r.rel_path));
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    assert_eq!(paged.len(), 4);
    assert!(paged.windows(2).all(|w| w[0] < w[1]));
}
