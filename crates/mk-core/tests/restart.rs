//! Restart recovery (US-03.4, ADR-0006, INV-PR-05/07, NFR-REL-04 partial). The project is
//! dropped with work in flight and reopened. Real process kills come with the M2c kill test.

use std::time::{Duration, SystemTime};

use mk_core::Project;
use mk_corpus::IngestionState;
use mk_jobs::{ItemState, JobState};

fn write(path: &std::path::Path, text: &str, t: u64) {
    std::fs::write(path, text).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + t))
        .unwrap();
}

fn job_states(p: &Project) -> Vec<(i64, String)> {
    p.store()
        .read_raw(|c| {
            let mut s = c.prepare("SELECT id, state FROM job ORDER BY id")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

/// INV-PR-05, INV-PR-07, US-03.4 AC2: items in progress at a crash are Pending after reopen and
/// are redone; Done items are never redone; job states are kept, a paused job stays paused.
#[test]
fn inv_pr_05_inv_pr_07_restart_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..4 {
        write(
            &src.join(format!("f{i}.txt")),
            &format!("p{i}@x.example\nall@x.example\n"),
            i,
        );
    }
    let dir = tmp.path().join("project");
    let (in_progress, done_item) = {
        let (project, _) = Project::open(&dir).unwrap();
        project.add_source_root(&src).unwrap();
        project.rescan(false).unwrap();
        let pending = project.store().pending_items().unwrap();
        assert_eq!(pending.len(), 4);
        // Item 0 finishes; item 1 is in progress when the app stops.
        let done = project.store().start_item(pending[0].id).unwrap().unwrap();
        let output = mk_core::stage_a::process(&done).unwrap();
        project.store().commit_stage_a(done.item, output).unwrap();
        let started = project.store().start_item(pending[1].id).unwrap().unwrap();
        // A paused job (pause itself comes in M3) to check that its state survives.
        project
            .store()
            .write_raw(|c| {
                c.execute(
                    "INSERT INTO job (trigger, stage, state, created_at)
                     VALUES ('settings', 'a', 'paused', 'then')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let p = project.store().progress().unwrap();
        assert_eq!(p.items_in(ItemState::InProgress), 1);
        assert_eq!(p.contents_in(IngestionState::Processing), 1);
        (started, done.item)
        // Dropped here with item 1 in progress and its content Processing.
    };

    let (project, summary) = Project::open(&dir).unwrap();
    assert_eq!(
        (summary.recovered.items, summary.recovered.contents),
        (1, 1)
    );
    let p = project.store().progress().unwrap();
    assert_eq!(p.items_in(ItemState::InProgress), 0);
    assert_eq!(p.items_in(ItemState::Pending), 3);
    assert_eq!(p.items_in(ItemState::Done), 1);
    assert_eq!(p.contents_in(IngestionState::Queued), 3);
    let jobs = job_states(&project);
    assert_eq!(
        jobs[0].1,
        JobState::Running.as_str(),
        "running comes back running"
    );
    assert_eq!(jobs[1].1, JobState::Paused.as_str(), "paused stays paused");

    let run = project.run_stage_a().unwrap();
    assert_eq!(run.done, 3, "only the three unfinished items run");
    let attempts: i64 = project
        .store()
        .read_raw(move |c| {
            Ok(c.query_row(
                "SELECT attempts FROM work_item WHERE id = ?1",
                [in_progress.item],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(attempts, 2, "the interrupted item ran again");
    let done_attempts: i64 = project
        .store()
        .read_raw(move |c| {
            Ok(c.query_row(
                "SELECT attempts FROM work_item WHERE id = ?1",
                [done_item],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(done_attempts, 1, "the finished item was not redone");
    assert_eq!(job_states(&project)[0].1, JobState::Completed.as_str());
    let all = &project.store().derived_state().unwrap().connections["email:all@x.example"];
    assert_eq!(all.source_count, 4);
}
