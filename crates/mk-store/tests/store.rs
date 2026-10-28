//! ADR-0004 checks: decision log triggers, open/migrate rules, transactions, readers.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use mk_graph::{DecisionKind, DecisionStatus};
use mk_kernel::{ContentHash, IdentifierKey, IdentifierKind, LineNumber, Locator, Span};
use mk_store::{
    DocumentOut, MIGRATIONS, MentionOut, Migration, NewContent, SegmentOut, StageAOutput, Store,
    StoreError,
};
use rusqlite::Connection;

fn open(dir: &Path) -> Store {
    Store::open(&dir.join("project.db"), &dir.join("backups"))
        .unwrap()
        .0
}

fn count(store: &Store, table: &'static str) -> i64 {
    store
        .read_raw(move |c| {
            Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        })
        .unwrap()
}

fn key(v: &str) -> IdentifierKey {
    IdentifierKey::new(IdentifierKind::Email, v)
}

/// One TXT document with one line per email, each line one mention.
fn output(emails: &[&str]) -> StageAOutput {
    let mut doc = DocumentOut {
        kind: "txt".into(),
        position: "whole".into(),
        segments: Vec::new(),
    };
    let mut mentions = Vec::new();
    for (i, e) in emails.iter().enumerate() {
        let len = e.chars().count();
        let span = Span::new(0, len, len).unwrap();
        let line = LineNumber::new(i as u32 + 1).unwrap();
        doc.segments.push(SegmentOut {
            locator: Locator::TextLine { line, span },
            text: e.to_string(),
            malformed: false,
            cell_starts: Vec::new(),
        });
        mentions.push(MentionOut {
            document: 0,
            segment: i,
            locator: Locator::TextLine { line, span },
            raw: e.to_string(),
            key: key(e),
            confidence: 1.0,
        });
    }
    StageAOutput {
        documents: vec![doc],
        mentions,
        warnings: Vec::new(),
    }
}

/// Adds a content without a path and commits `emails` as its Stage A result.
fn ingest(store: &Store, n: u8, emails: &[&str]) {
    let items = store
        .enqueue_contents(vec![NewContent {
            hash: ContentHash::from_bytes([n; 32]),
            file_type: "txt".into(),
            size: 10,
        }])
        .unwrap();
    store.start_item(items[0]).unwrap().unwrap();
    store.commit_stage_a(items[0], output(emails)).unwrap();
}

/// INV-CU-01, ADR-0004: the decision log rejects UPDATE and DELETE, even in a raw write.
#[test]
fn inv_cu_01_decision_log_is_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .append_decision(
            DecisionKind::IgnoreIdentifier,
            "email:a@x.example",
            None,
            Some("ignored"),
        )
        .unwrap();
    for sql in [
        "UPDATE decision SET anchor = 'email:b@x.example'",
        "DELETE FROM decision",
    ] {
        let err = store
            .write_raw(move |c| Ok(c.execute(sql, [])?))
            .unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
    }
    assert_eq!(count(&store, "decision"), 1);
}

/// INV-WS-03: a newer schema is refused, and the file is not changed.
#[test]
fn inv_ws_03_newer_schema_is_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.db");
    {
        let c = Connection::open(&db).unwrap();
        c.execute_batch("CREATE TABLE x (y); PRAGMA user_version = 99;")
            .unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let err = Store::open(&db, &dir.path().join("backups"))
        .err()
        .expect("refused");
    assert!(
        matches!(
            err,
            StoreError::TooNew {
                found: 99,
                supported: 1
            }
        ),
        "{err}"
    );
    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert!(!dir.path().join("backups").exists());
    assert!(!dir.path().join("project.db-wal").exists());
}

const V2: Migration = Migration {
    version: 2,
    name: "note",
    sql: "ALTER TABLE content ADD COLUMN note TEXT;",
};

const V2_BROKEN: Migration = Migration {
    version: 2,
    name: "broken",
    sql: "ALTER TABLE content ADD COLUMN note TEXT; ALTER TABLE no_such_table ADD COLUMN x;",
};

/// INV-WS-04, NFR-REL-11, INV-CU-06: an older project is backed up before migrating; the
/// migration carries the decision log forward unchanged and the derived tables are rebuilt.
#[test]
fn inv_ws_04_backup_before_migration() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.db");
    let backups = dir.path().join("backups");
    {
        let (store, report) = Store::open(&db, &backups).unwrap();
        assert_eq!(
            (report.from_version, report.to_version, report.backup),
            (0, 1, None)
        );
        ingest(&store, 1, &["a@x.example"]);
        ingest(&store, 2, &["a@x.example"]);
        store
            .append_decision(
                DecisionKind::IgnoreIdentifier,
                "email:a@x.example",
                None,
                None,
            )
            .unwrap();
    }
    let (store, report) = Store::open_with(&db, &backups, &[MIGRATIONS[0], V2]).unwrap();
    assert_eq!((report.from_version, report.to_version), (1, 2));
    assert!(report.derived_rebuilt);
    let backup = report.backup.expect("backup written");
    let name = backup.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("project-v1-") && name.ends_with("Z.db"),
        "{name}"
    );
    let b = Connection::open(&backup).unwrap();
    let v: u32 = b
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let decisions: i64 = b
        .query_row("SELECT COUNT(*) FROM decision", [], |r| r.get(0))
        .unwrap();
    assert_eq!((v, decisions), (1, 1));
    assert_eq!(count(&store, "decision"), 1);
    let derived = store.derived_state().unwrap();
    assert!(derived.connections["email:a@x.example"].ignored);
    assert_eq!(derived.decision_status[&1], DecisionStatus::Active);
    // The new column exists.
    store
        .read_raw(|c| {
            Ok(c.query_row("SELECT COUNT(note) FROM content", [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .unwrap();
}

/// INV-WS-04, NFR-REL-11: a failed migration rolls back, keeps the backup, and leaves the
/// project usable at the old version.
#[test]
fn inv_ws_04_failed_migration_keeps_backup_and_old_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.db");
    let backups = dir.path().join("backups");
    {
        let (store, _) = Store::open(&db, &backups).unwrap();
        ingest(&store, 1, &["a@x.example"]);
    }
    let err = Store::open_with(&db, &backups, &[MIGRATIONS[0], V2_BROKEN])
        .err()
        .expect("migration fails");
    let StoreError::MigrationFailed {
        from: 1,
        to: 2,
        backup: Some(backup),
        reason,
    } = &err
    else {
        panic!("{err}");
    };
    assert!(backup.exists(), "{reason}");
    let (store, report) = Store::open(&db, &backups).unwrap();
    assert_eq!((report.from_version, report.to_version), (1, 1));
    assert_eq!(count(&store, "identifier_mention"), 1);
    let has_note = store
        .read_raw(|c| Ok(c.prepare("SELECT note FROM content").is_ok()))
        .unwrap();
    assert!(!has_note, "half-applied migration");
}

/// INV-PR-04: a write job that fails or panics leaves no partial rows, and the writer keeps
/// working.
#[test]
fn inv_pr_04_failed_write_leaves_no_partial_rows() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let insert = "INSERT INTO source_root (path, kind, added_at) VALUES ('/a', 'folder', 'now')";
    let err = store
        .write_raw(move |c| {
            c.execute(insert, [])?;
            Err::<(), _>(StoreError::Invalid("stop".into()))
        })
        .unwrap_err();
    assert_eq!(err.to_string(), "stop");
    let err = store
        .write_raw(move |c| -> Result<(), StoreError> {
            c.execute(insert, [])?;
            panic!("boom")
        })
        .unwrap_err();
    assert!(err.to_string().contains("boom"));
    assert_eq!(count(&store, "source_root"), 0);
    store.add_source_root("/b", "folder").unwrap();
    assert_eq!(count(&store, "source_root"), 1);
}

/// INV-PR-04, NFR-REL-02: a connection closed before COMMIT (as in a crash) leaves nothing.
#[test]
fn inv_pr_04_uncommitted_write_is_gone_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    drop(open(dir.path()));
    {
        let c = Connection::open(dir.path().join("project.db")).unwrap();
        c.execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO source_root (path, kind, added_at) VALUES ('/a', 'folder', 'now');",
        )
        .unwrap();
        // Dropped without COMMIT.
    }
    let store = open(dir.path());
    assert_eq!(count(&store, "source_root"), 0);
}

/// ADR-0004: readers run while a write transaction is open, and see the last commit only.
#[test]
fn readers_run_during_an_open_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(open(dir.path()));
    let (inserted, go_on) = (mpsc::channel(), mpsc::channel::<()>());
    let (tx_inserted, rx_inserted) = inserted;
    let (tx_go, rx_go) = go_on;
    let writer = {
        let store = store.clone();
        std::thread::spawn(move || {
            store
                .write_raw(move |c| {
                    c.execute(
                        "INSERT INTO source_root (path, kind, added_at) VALUES ('/a', 'folder', 'now')",
                        [],
                    )?;
                    tx_inserted.send(()).unwrap();
                    rx_go.recv_timeout(Duration::from_secs(10)).unwrap();
                    Ok(())
                })
                .unwrap()
        })
    };
    rx_inserted.recv().unwrap();
    // The write is open and holds its row; every reader still answers, with the old state.
    for _ in 0..mk_store::READERS * 2 {
        assert_eq!(count(&store, "source_root"), 0);
    }
    tx_go.send(()).unwrap();
    writer.join().unwrap();
    assert_eq!(count(&store, "source_root"), 1);
}

/// ADR-0005: a Stage A commit rebuilds the touched keys; a decision rebuilds its anchor; a full
/// rebuild gives the same rows.
#[test]
fn commit_rebuilds_touched_keys() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    ingest(&store, 1, &["a@x.example", "b@x.example"]);
    assert!(store.derived_state().unwrap().connections.is_empty());
    ingest(&store, 2, &["a@x.example"]);
    let d = store.derived_state().unwrap();
    assert_eq!(d.connections.len(), 1);
    assert_eq!(d.connections["email:a@x.example"].source_count, 2);
    assert_eq!(d.identifiers.len(), 2);
    let (seq, changed) = store
        .append_decision(
            DecisionKind::IgnoreIdentifier,
            "email:a@x.example",
            None,
            None,
        )
        .unwrap();
    assert!(changed);
    let d = store.derived_state().unwrap();
    assert!(d.connections["email:a@x.example"].ignored);
    assert_eq!(d.decision_status[&seq], DecisionStatus::Active);
    assert!(store.connections_page(None, 10, false).unwrap().is_empty());
    assert_eq!(store.connections_page(None, 10, true).unwrap().len(), 1);
    store.rebuild_all().unwrap();
    assert_eq!(store.derived_state().unwrap(), d);
    assert_eq!(mk_graph::model::reference(&store.facts().unwrap()), d);
}

/// US-07.3 AC2, INV-CO-03/04: every mention of a key comes with its locator, and its span points
/// at the raw text in its segment; ordered by content; one mention by id gives the same row.
#[test]
fn us_07_3_ac2_mentions_of_key_have_locators() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    ingest(&store, 1, &["a@x.example", "b@x.example"]);
    ingest(&store, 2, &["c@x.example", "a@x.example"]);
    assert_eq!(store.connection_count(false).unwrap(), 1);

    let key = key("a@x.example").to_string();
    let rows = store.mentions_of(&key, 100).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].content_id < rows[1].content_id);
    let lines: Vec<_> = rows
        .iter()
        .map(|r| match &r.locator {
            Locator::TextLine { line, span } => (line.get(), span.start(), span.end()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(lines, [(1, 0, 11), (2, 0, 11)]);
    for r in &rows {
        assert_eq!(
            (r.raw.as_str(), r.file_type.as_str()),
            ("a@x.example", "txt")
        );
        assert_eq!(r.path, None, "contents added without a file have no path");
        assert_eq!(store.mention(r.mention_id).unwrap().as_ref(), Some(r));
        let window = store.segment_window(r.segment_id, 0, 0).unwrap().unwrap();
        assert_eq!(window.segments.len(), 1);
        assert_eq!(window.segments[0].text, "a@x.example");
    }
    assert_eq!(store.mentions_of(&key, 1).unwrap().len(), 1);
    assert!(
        store
            .mentions_of("email:none@x.example", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.mention(i64::MAX).unwrap(), None);
}

/// US-08.1: the window around a segment stops at the start and end of its document and always
/// carries the document's first segment.
#[test]
fn segment_window_bounds_at_file_start_and_end() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let emails: Vec<String> = (0..10).map(|i| format!("p{i}@x.example")).collect();
    let refs: Vec<&str> = emails.iter().map(String::as_str).collect();
    ingest(&store, 1, &refs);
    let seg = |e: &str| store.mentions_of(&key(e).to_string(), 1).unwrap()[0].segment_id;
    let lines = |w: &mk_store::SegmentWindow| -> Vec<i64> {
        w.segments.iter().map(|s| s.ordinal).collect()
    };

    let start = store
        .segment_window(seg("p0@x.example"), 3, 2)
        .unwrap()
        .unwrap();
    assert_eq!(lines(&start), [0, 1, 2]);
    let end = store
        .segment_window(seg("p9@x.example"), 3, 2)
        .unwrap()
        .unwrap();
    assert_eq!(lines(&end), [6, 7, 8, 9]);
    let mid = store
        .segment_window(seg("p5@x.example"), 2, 2)
        .unwrap()
        .unwrap();
    assert_eq!(lines(&mid), [3, 4, 5, 6, 7]);
    for w in [&start, &end, &mid] {
        assert_eq!(w.first.ordinal, 0);
        assert_eq!(w.first.text, "p0@x.example");
    }
    assert_eq!(store.segment_window(i64::MAX, 1, 1).unwrap(), None);
}
