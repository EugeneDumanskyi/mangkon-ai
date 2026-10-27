//! Worker isolation through the real Stage A lane (ADR-0001, ADR-0007): an endless loop, a memory
//! bomb and a crash each fail only their file with a plain reason, and the lane carries on with a
//! fresh worker (NFR-REL-01/06/07, NFR-USE-05, INV-CO-09). Recycling and a worker that cannot
//! start. Uses the test fault markers of `mk_worker::fault`.

use std::path::Path;
use std::time::Duration;

use mk_core::{Project, ProjectError, Stopped, WorkerConfig};
use mk_corpus::IngestionState;
use mk_jobs::ItemState;

fn project(dir: &Path, src: &Path) -> Project {
    let (mut project, _) = Project::open(dir).unwrap();
    *project.worker_config_mut() = WorkerConfig {
        test_faults: true,
        ..WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
    };
    project.add_source_root(src).unwrap();
    project
}

fn write_good(src: &Path, n: usize) {
    for i in 0..n {
        std::fs::write(
            src.join(format!("good-{i}.txt")),
            format!("p{i}@x.example\nshared@x.example\n"),
        )
        .unwrap();
    }
}

/// `(state, failure reason)` of the content stored for `name`.
fn outcome(project: &Project, name: &str) -> (IngestionState, Option<String>) {
    let stored = project.store().stored_refs().unwrap();
    let r = stored.iter().find(|r| r.rel_path == name).unwrap();
    let c = project.store().content(&r.hash).unwrap().unwrap();
    (c.state, c.failure_reason)
}

/// M2 acceptance criterion 2 (short form), NFR-REL-01/06/07, NFR-USE-05: an endless loop, a
/// memory bomb and a crash fail alone with plain reasons; every other file finishes.
#[test]
fn nfr_rel_01_faults_fail_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_good(&src, 6);
    for fault in ["endless-loop", "memory-bomb", "crash"] {
        std::fs::write(
            src.join(format!("fault-{fault}.txt")),
            format!("mk-test-fault: {fault}\nhidden@x.example\n"),
        )
        .unwrap();
    }
    let mut project = project(&tmp.path().join("project"), &src);
    project.worker_config_mut().deadline = Duration::from_secs(2);
    project.set_stage_a_workers(2);
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();

    assert_eq!((run.done, run.failed), (6, 3));
    for i in 0..6 {
        assert_eq!(
            outcome(&project, &format!("good-{i}.txt")).0,
            IngestionState::Done
        );
    }
    let failed = |name| outcome(&project, name);
    assert_eq!(
        failed("fault-endless-loop.txt"),
        (IngestionState::Failed, Some("Took too long to read".into()))
    );
    assert_eq!(
        failed("fault-memory-bomb.txt"),
        (
            IngestionState::Failed,
            Some("Needed too much memory".into())
        )
    );
    assert_eq!(
        failed("fault-crash.txt"),
        (
            IngestionState::Failed,
            Some("The file could not be read: the reader stopped unexpectedly".into())
        )
    );
    let mut whys: Vec<Stopped> = run.stops.iter().map(|s| s.why).collect();
    whys.sort_by_key(|w| format!("{w:?}"));
    assert_eq!(
        whys,
        vec![Stopped::Crashed, Stopped::Deadline, Stopped::Memory]
    );
    // Each stopped worker was replaced (2 lanes + 3 replacements, at most).
    assert!(run.workers_started >= 3 && run.workers_started <= 5);
    // The connection from the good files is there; the faulty files added nothing.
    let derived = project.store().derived_state().unwrap();
    assert!(derived.connections.contains_key("email:shared@x.example"));
    assert!(!derived.connections.contains_key("email:hidden@x.example"));
    assert!(project.check_health().unwrap().is_healthy());
}

/// Spike S2b (short form, runs on every OS in CI): a memory bomb is stopped soon after it
/// passes the limit and fails with "Needed too much memory". The 20-run form is
/// `s2b_memory_bomb` (ignored).
#[test]
fn s2b_memory_bomb_short() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("bomb.txt"), "mk-test-fault: memory-bomb\n").unwrap();
    let project = project(&tmp.path().join("project"), &src);
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();
    assert_eq!(run.failed, 1);
    let stop = &run.stops[0];
    assert_eq!(stop.why, Stopped::Memory);
    assert_eq!(stop.code, "needed_too_much_memory");
    // Generous here (CI machines are slow and shared); the pass mark is checked in s2b_memory_bomb.
    assert!(
        stop.over_limit_for.unwrap() < Duration::from_secs(2),
        "{stop:?}"
    );
}

/// ADR-0007: a worker is replaced after `recycle_after` files.
#[test]
fn workers_are_recycled() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_good(&src, 5);
    let mut project = project(&tmp.path().join("project"), &src);
    project.set_stage_a_workers(1);
    project.worker_config_mut().recycle_after = 2;
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();
    assert_eq!(run.done, 5);
    assert_eq!(run.workers_started, 3);
    assert!(run.stops.is_empty());
    assert!(run.peak_memory > 0);
}

/// A worker that cannot start ends the run with an error and starts no item: everything stays
/// Pending for the next run.
#[test]
fn worker_that_cannot_start_leaves_items_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    write_good(&src, 3);
    let mut project = project(&tmp.path().join("project"), &src);
    project.worker_config_mut().program = Some(tmp.path().join("no-such-worker"));
    project.rescan(false).unwrap();
    assert!(matches!(
        project.run_stage_a(),
        Err(ProjectError::Worker(_))
    ));
    let p = project.store().progress().unwrap();
    assert_eq!(p.items_in(ItemState::Pending), 3);
    assert_eq!(p.items_in(ItemState::InProgress), 0);

    // A program that is not a worker fails the handshake the same way.
    #[cfg(unix)]
    {
        project.worker_config_mut().program = Some("/bin/echo".into());
        assert!(matches!(
            project.run_stage_a(),
            Err(ProjectError::Worker(_))
        ));
        assert_eq!(
            project
                .store()
                .progress()
                .unwrap()
                .items_in(ItemState::Pending),
            3
        );
    }
}

/// The kill test's process list finds a running worker and drops it once it exits; a worker
/// whose input closes exits by itself (no orphan when the core dies).
#[test]
fn worker_processes_lists_workers_and_workers_exit_on_eof() {
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_mk-test-worker")).unwrap();
    let mut child = std::process::Command::new(&program)
        .arg("--worker")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let listed = |pid| mk_core::worker_processes(&program).contains(&pid);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !listed(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "worker {pid} never listed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert!(!listed(pid));
}
