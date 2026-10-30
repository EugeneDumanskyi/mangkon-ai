//! M2d slice reads over the real pipeline (re-scan → Stage A in workers → commit), on generated
//! TXT and CSV files with a multi-byte character before the hit: US-07.1 AC1, US-07.3 AC2,
//! US-08.1 AC3, INV-CO-04; and the full rebuild (ADR-0005).

use std::path::Path;

use mk_core::{Locator, Marked, Project, SourceBody, WorkerConfig};

fn project(dir: &Path) -> Project {
    let (mut project, _) = Project::open(dir).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project
}

fn source(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let mut notes = String::from("first line\n\n");
    notes.push_str("Zoë → shared@x.example wrote\n");
    for i in 0..60 {
        notes.push_str(&format!("filler {i} only@notes.example\n"));
    }
    std::fs::write(dir.join("notes.txt"), notes).unwrap();
    std::fs::write(
        dir.join("people.csv"),
        "name,email,amount\nAnn,ann@x.example,1\nZoë,shared@x.example,2\n",
    )
    .unwrap();
}

#[test]
fn us_07_3_ac2_occurrence_opens_its_line_and_cell() {
    let tmp = tempfile::tempdir().unwrap();
    source(&tmp.path().join("src"));
    let project = project(&tmp.path().join("p"));
    project.add_source_root(&tmp.path().join("src")).unwrap();
    let run = project.ingest().unwrap();
    assert_eq!(
        (run.rescan.walked, run.stage_a.done, run.stage_a.failed),
        (2, 2, 0)
    );
    let counts = project.counts().unwrap();
    assert_eq!(
        (counts.total, counts.done, counts.failed, counts.queued),
        (2, 2, 0, 0)
    );

    // US-07.1 AC1: two connections, the shared address and its domain (US-04.3); keyset
    // paging gives each once and ends.
    assert_eq!(project.connection_count().unwrap(), 2);
    let mut rows = Vec::new();
    let mut next = None;
    loop {
        let page = project.connections_page(next.as_ref(), 1).unwrap();
        assert!(page.rows.len() <= 1);
        rows.extend(page.rows);
        match page.next {
            Some(n) => next = Some(n),
            None => break,
        }
    }
    let mut keys: Vec<_> = rows.iter().map(|r| r.subject_key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["domain:x.example", "email:shared@x.example"]);
    let row = rows
        .iter()
        .find(|r| r.subject_key == "email:shared@x.example")
        .unwrap();
    assert_eq!((row.source_count, row.file_type_count), (2, 2));

    let occurrences = project.occurrences(&row.subject_key).unwrap();
    assert_eq!(occurrences.len(), 2);
    for o in &occurrences {
        let path = o.path.as_deref().unwrap();
        let ctx = project.source_context(o.mention_id).unwrap().unwrap();
        match (&o.locator, &ctx.body) {
            (Locator::TextLine { line, .. }, SourceBody::Text { lines }) => {
                assert!(path.ends_with("notes.txt"));
                assert_eq!(line.get(), 3);
                // The empty line 2 is not stored; the window stops at the file start.
                assert_eq!(lines.first().map(|l| l.number), Some(1));
                let hits: Vec<_> = lines.iter().filter(|l| l.text.has_hit()).collect();
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].number, 3);
                assert_eq!(
                    hits[0].text,
                    Marked {
                        before: "Zoë → ".into(),
                        hit: "shared@x.example".into(),
                        after: " wrote".into(),
                        runs: Vec::new(),
                    }
                );
                // 20 segments after the hit, never the whole file.
                assert_eq!(lines.len(), 2 + mk_core::CONTEXT_SEGMENTS);
            }
            (Locator::CsvCell { row, .. }, SourceBody::Table { header, rows }) => {
                assert!(path.ends_with("people.csv"));
                assert_eq!(*row, 2);
                let header = header.as_ref().unwrap();
                assert_eq!(header, &["name", "email", "amount"]);
                assert_eq!(rows.iter().map(|r| r.row).collect::<Vec<_>>(), [1, 2]);
                let hit = &rows[1].cells[1];
                assert_eq!(
                    (hit.before.as_str(), hit.hit.as_str()),
                    ("", "shared@x.example")
                );
                assert_eq!(rows[1].cells[0], Marked::plain("Zoë"));
                assert!(rows[0].cells.iter().all(|c| !c.has_hit()));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(project.source_context(i64::MAX).unwrap(), None);
}

/// ADR-0005: a full rebuild gives the rows the incremental commits left, and ingesting again
/// with nothing changed creates no work (INV-PR-08).
#[test]
fn rebuild_equals_incremental_state() {
    let tmp = tempfile::tempdir().unwrap();
    source(&tmp.path().join("src"));
    let project = project(&tmp.path().join("p"));
    project.add_source_root(&tmp.path().join("src")).unwrap();
    project.ingest().unwrap();
    project.set_ignored("email:ann@x.example", true).unwrap();
    let r = project.rebuild().unwrap();
    assert!(r.same_as_incremental && r.consistent, "{r:?}");
    assert_eq!(r.connections, 2, "the shared address and its domain");
    let again = project.ingest().unwrap();
    assert_eq!((again.rescan.hashed, again.stage_a.done), (0, 0));
}
