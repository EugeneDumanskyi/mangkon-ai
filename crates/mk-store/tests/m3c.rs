//! M3c store checks: `app.db` and its registry (INV-WS-01/03), the v3 → v4 migration, and the
//! queue rules that need no files (INV-PR-06, D10).

use mk_jobs::{ItemState, JobState};
use mk_kernel::ContentHash;
use mk_store::{APP_MIGRATIONS, AppStore, MIGRATIONS, Migration, NewContent, Store, StoreError};
use mk_workspace::ProjectName;
use rusqlite::Connection;

fn name(n: &str) -> ProjectName {
    ProjectName::parse(n).unwrap()
}

/// INV-WS-01, US-01.2 AC1: names are unique ignoring case (the database enforces it), a path
/// registers once, and the list is newest first.
#[test]
fn inv_ws_01_registry_names_unique_ignoring_case() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppStore::open(dir.path()).unwrap();
    let acme = app.register(&name("Acme"), "/p/Acme").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let other = app.register(&name("Other"), "/p/Other").unwrap();
    assert!(matches!(
        app.register(&name("ACME"), "/p/acme2"),
        Err(StoreError::NameTaken(_))
    ));
    assert!(app.name_taken(&name("acme")).unwrap());
    // Same path again: the same entry, touched.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let again = app.register(&name("Renamed"), "/p/Acme").unwrap();
    assert_eq!((again.id, again.name.as_str()), (acme.id, "Acme"));
    let ids: Vec<i64> = app.recent().unwrap().iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![acme.id, other.id]);

    app.relocate(other.id, "/moved/Other").unwrap();
    assert_eq!(app.entry(other.id).unwrap().unwrap().path, "/moved/Other");
    app.remove(acme.id).unwrap();
    assert_eq!(app.recent().unwrap().len(), 1);
    // The database itself refuses a clash, whatever the caller checks.
    let raw = Connection::open(app.path()).unwrap();
    let clash = raw.execute(
        "INSERT INTO registry_entry (name, name_key, path, last_opened_at)
         VALUES ('X', 'other', '/x', 0)",
        [],
    );
    assert!(clash.is_err());
}

/// INV-WS-03: a newer `app.db` is refused and not changed.
#[test]
fn inv_ws_03_newer_app_db_is_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let v2 = Migration {
        version: 2,
        name: "later",
        sql: "CREATE TABLE later (x);",
    };
    AppStore::open_with(dir.path(), &[APP_MIGRATIONS, &[v2]].concat()).unwrap();
    let before = std::fs::read(dir.path().join("app.db")).unwrap();
    let err = AppStore::open(dir.path()).err().expect("refused");
    assert!(
        matches!(
            err,
            StoreError::TooNew {
                found: 2,
                supported: 1,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(std::fs::read(dir.path().join("app.db")).unwrap(), before);
}

/// US-01.2 AC3: a too-new project names the app version it needs, when it says.
#[test]
fn us_01_2_ac3_too_new_names_app_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.db");
    let backups = dir.path().join("backups");
    {
        let (store, _) = Store::open(&db, &backups).unwrap();
        store.stamp(Some("Acme"), "9.1.0").unwrap();
        store
            .write_raw(|c| {
                c.pragma_update(None, "user_version", 99)?;
                Ok(())
            })
            .unwrap();
    }
    let err = Store::open(&db, &backups).err().expect("refused");
    let StoreError::TooNew { app_version, .. } = &err else {
        panic!("{err}")
    };
    assert_eq!(app_version.as_deref(), Some("9.1.0"));
    assert!(err.to_string().contains("needs Mangkon AI 9.1.0 or newer"));
}

/// NFR-REL-11, INV-WS-04: an M3b project (v3) migrates to v4 (and on to v6) with a backup; its rows stay and
/// the new columns and table exist.
#[test]
fn nfr_rel_11_migration_v3_to_v4() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.db");
    let backups = dir.path().join("backups");
    {
        let (store, _) = Store::open_with(&db, &backups, &MIGRATIONS[..3]).unwrap();
        store
            .enqueue_contents(vec![NewContent {
                hash: ContentHash::from_bytes([7; 32]),
                file_type: "txt".into(),
                size: 10,
            }])
            .unwrap();
    }
    let (store, report) = Store::open(&db, &backups).unwrap();
    assert_eq!((report.from_version, report.to_version), (3, 6));
    assert!(report.backup.unwrap().exists());
    let (pending, detail, excluded) = store
        .read_raw(|c| {
            let p: i64 = c.query_row(
                "SELECT COUNT(*) FROM work_item WHERE started_at IS NULL AND finished_at IS NULL",
                [],
                |r| r.get(0),
            )?;
            let d: Option<String> =
                c.query_row("SELECT failure_detail FROM content", [], |r| r.get(0))?;
            let e: i64 = c.query_row("SELECT COUNT(*) FROM root_exclusion", [], |r| r.get(0))?;
            Ok((p, d, e))
        })
        .unwrap();
    assert_eq!((pending, detail, excluded), (1, None, 0));
}

fn job_states(store: &Store) -> Vec<String> {
    store
        .read_raw(|c| {
            let mut s = c.prepare("SELECT state FROM job ORDER BY id")?;
            let rows = s.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

/// INV-PR-06, D11: while paused no item is offered, and jobs made meanwhile start Paused;
/// resume offers them again.
#[test]
fn inv_pr_06_paused_queue_offers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(&dir.path().join("project.db"), &dir.path().join("b")).unwrap();
    let content = |n: u8| NewContent {
        hash: ContentHash::from_bytes([n; 32]),
        file_type: "txt".into(),
        size: 10,
    };
    store.enqueue_contents(vec![content(1)]).unwrap();
    assert_eq!(store.pending_items().unwrap().len(), 1);
    assert_eq!(store.set_queue_paused(true).unwrap(), 1);
    store.enqueue_contents(vec![content(2)]).unwrap();
    assert!(store.pending_items().unwrap().is_empty());
    assert!(!store.has_runnable_work().unwrap());
    let paused = JobState::Paused.as_str();
    assert_eq!(job_states(&store), vec![paused, paused]);
    let d = store.progress_detail(None).unwrap();
    assert!(d.paused);
    assert_eq!((d.open, d.open_bytes), (2, 20));

    store.set_queue_paused(false).unwrap();
    assert_eq!(store.pending_items().unwrap().len(), 2);
    assert!(!store.progress_detail(None).unwrap().paused);
}

/// US-03.3, D10: cancel turns every unfinished item Cancelled and its content Not processed;
/// retry queues it again under a Retry job.
#[test]
fn us_03_3_cancel_then_retry_store_rules() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(&dir.path().join("project.db"), &dir.path().join("b")).unwrap();
    let items = store
        .enqueue_contents(
            (1..=3)
                .map(|n| NewContent {
                    hash: ContentHash::from_bytes([n; 32]),
                    file_type: "txt".into(),
                    size: 10,
                })
                .collect(),
        )
        .unwrap();
    let started = store.start_item(items[0]).unwrap().unwrap();
    let out = store.cancel_queue().unwrap();
    assert_eq!((out.items, out.contents, out.jobs), (3, 3, 1));
    let p = store.progress().unwrap();
    assert_eq!(p.items_in(ItemState::Cancelled), 3);
    assert_eq!(p.contents_in(mk_corpus::IngestionState::NotProcessed), 3);
    assert_eq!(job_states(&store), vec![JobState::Cancelled.as_str()]);

    assert_eq!(store.retry_contents(vec![started.content]).unwrap(), 1);
    assert_eq!(store.pending_items().unwrap().len(), 1);
    // Done or queued contents are not retried.
    assert_eq!(store.retry_contents(vec![started.content]).unwrap(), 0);
}
