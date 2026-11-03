//! M4d in the core, E07: the connections list with filters, find and sort (US-07.1, US-07.2),
//! occurrences with snippets (US-07.3), ignore and show ignored (US-07.4), the counts behind the
//! empty states (US-07.6), NFR-QA-10 (no link without two documents), and NFR-REL-09 (decisions
//! survive re-scan, re-process, rebuild and a derived-table migration). TD-1 cases are skipped
//! without `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::path::{Path, PathBuf};

use mk_core::{
    Assignments, ColumnType, ConnectionQuery, ListedConnection, Project, Reprocess, SortColumn,
    WorkerConfig,
};

fn open(dir: &Path) -> Project {
    let (mut p, _) = Project::open(dir).unwrap();
    *p.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    p.set_stage_a_workers(2);
    p
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// All rows of a query, read in pages of `size`.
fn all(p: &Project, query: &ConnectionQuery, size: usize) -> Vec<ListedConnection> {
    let mut rows = Vec::new();
    let mut after = None;
    loop {
        let page = p.connections_page(query, after.as_ref(), size).unwrap();
        rows.extend(page.rows);
        match page.next {
            Some(n) => after = Some(n),
            None => return rows,
        }
    }
}

fn keys(rows: &[ListedConnection]) -> Vec<&str> {
    rows.iter().map(|r| r.subject_key.as_str()).collect()
}

const PAYMENTS: &str = "Supplier,Contact\n\
    Brightwater Logistics Ltd,accounts@brightwater.example\n\
    Harbour Freight GmbH,billing@harbour.example\n";

const NOTES: &str = "call +44 20 7946 0958 about brightwater\n\
    mail accounts@brightwater.example and card 4111 1111 1111 1111 please\n\
    also billing@harbour.example and https://harbour.example/pay\n";

const MAIL: &str = "From: ann@brightwater.example\nTo: accounts@brightwater.example\n\
    Subject: invoice\nMessage-ID: <1@x>\n\nPlease call +44 20 7946 0958 today.\n";

fn fixture(tmp: &Path) -> Project {
    let src = tmp.join("src");
    write(
        &src,
        &[
            ("payments.csv", PAYMENTS),
            ("notes.txt", NOTES),
            ("mail.eml", MAIL),
            ("lonely.txt", "only@single.example\n"),
        ],
    );
    let p = open(&tmp.join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    p
}

/// US-07.1 AC2/AC3: each row lists its file types; filters by type, minimum sources and file
/// type narrow the list and its count; US-07.2: find is a case-insensitive contains.
#[test]
fn us_07_1_filters_and_find() {
    let tmp = tempfile::tempdir().unwrap();
    let p = fixture(tmp.path());
    let everything = all(&p, &ConnectionQuery::default(), 3);
    assert!(!everything.is_empty());
    assert!(
        everything.iter().all(|r| r.source_count >= 2),
        "no single-file link"
    );
    assert!(!keys(&everything).contains(&"email:only@single.example"));
    let bright = everything
        .iter()
        .find(|r| r.subject_key == "email:accounts@brightwater.example")
        .expect("in the CSV, the text file and the email");
    assert_eq!(bright.file_types, ["csv", "eml", "txt"]);
    assert_eq!(
        (bright.kind.as_str(), bright.value.as_str()),
        ("email", "accounts@brightwater.example")
    );

    let q = |f: &dyn Fn(&mut ConnectionQuery)| {
        let mut q = ConnectionQuery::default();
        f(&mut q);
        q
    };
    let emails = all(&p, &q(&|q| q.kinds = vec!["email".into()]), 2);
    assert!(!emails.is_empty() && emails.iter().all(|r| r.kind == "email"));
    assert_eq!(
        p.connection_count_for(&q(&|q| q.kinds = vec!["email".into()]))
            .unwrap(),
        emails.len() as u64
    );

    let three = all(&p, &q(&|q| q.min_sources = 3), 100);
    assert!(three.iter().all(|r| r.source_count >= 3));
    assert!(keys(&three).contains(&"email:accounts@brightwater.example"));
    assert!(three.len() < everything.len());

    let eml = all(&p, &q(&|q| q.file_types = vec!["eml".into()]), 100);
    assert!(
        eml.iter()
            .all(|r| r.file_types.contains(&"eml".to_string()))
    );
    assert!(
        !keys(&eml).contains(&"email:billing@harbour.example"),
        "not in the email"
    );

    let found = all(&p, &q(&|q| q.find = "BRIGHTWATER".into()), 100);
    assert!(
        found
            .iter()
            .all(|r| r.value.to_lowercase().contains("brightwater"))
    );
    assert!(found.len() >= 2);
    // `%` and `_` are plain characters in find.
    assert!(all(&p, &q(&|q| q.find = "%".into()), 100).is_empty());
    assert!(all(&p, &q(&|q| q.find = "_".into()), 100).is_empty());
    assert_eq!(
        all(&p, &q(&|q| q.find = "  ".into()), 100).len(),
        everything.len()
    );
}

/// US-07.2: every sort column in both directions pages without a gap or a repeat, equal to one
/// big page, with ties broken by the key.
#[test]
fn us_07_2_sort_pages_without_gaps() {
    let tmp = tempfile::tempdir().unwrap();
    let p = fixture(tmp.path());
    let n = all(&p, &ConnectionQuery::default(), 1000).len();
    assert!(n >= 5, "{n}");
    for sort in [
        SortColumn::Rank,
        SortColumn::Value,
        SortColumn::Kind,
        SortColumn::Sources,
        SortColumn::Mentions,
        SortColumn::FileTypes,
    ] {
        for descending in [false, true] {
            let query = ConnectionQuery {
                sort,
                descending,
                ..Default::default()
            };
            let whole = all(&p, &query, 1000);
            for size in [1, 2, 3] {
                let paged = all(&p, &query, size);
                assert_eq!(
                    keys(&paged),
                    keys(&whole),
                    "{sort:?} desc={descending} page={size}"
                );
            }
            assert_eq!(whole.len(), n);
            let nums: Vec<u64> = whole
                .iter()
                .map(|r| match sort {
                    SortColumn::Sources => r.source_count,
                    SortColumn::Mentions => r.mention_count,
                    SortColumn::FileTypes => r.file_type_count,
                    _ => 0,
                })
                .collect();
            let mut sorted = nums.clone();
            sorted.sort_unstable();
            if descending {
                sorted.reverse();
            }
            assert_eq!(nums, sorted, "{sort:?} desc={descending}");
            if sort == SortColumn::Value {
                let v: Vec<String> = whole.iter().map(|r| r.value.to_lowercase()).collect();
                let mut s = v.clone();
                s.sort();
                if descending {
                    s.reverse();
                }
                assert_eq!(v, s);
            }
        }
    }
}

/// US-07.3 AC1: an occurrence has a snippet with its value marked, and a card-like number next
/// to it is masked (NFR-SEC-14).
#[test]
fn us_07_3_occurrence_snippets() {
    let tmp = tempfile::tempdir().unwrap();
    let p = fixture(tmp.path());
    let occ = p.occurrences("email:accounts@brightwater.example").unwrap();
    assert!(occ.len() >= 3, "{}", occ.len());
    for o in &occ {
        let marked: String = o
            .snippet
            .iter()
            .filter(|r| r.hit)
            .map(|r| &*r.text)
            .collect();
        assert_eq!(marked, o.raw, "the value is marked in {:?}", o.snippet);
    }
    let in_notes = occ.iter().find(|o| o.file_type == "txt").unwrap();
    let text: String = in_notes.snippet.iter().map(|r| &*r.text).collect();
    assert!(!text.contains("4111 1111 1111 1111"), "{text}");
    assert!(text.contains("1111"), "last four digits stay: {text}");
}

/// US-07.4: an ignored connection leaves the list and returns with "Show ignored"; the summary
/// counts both; ignoring again is a no-op for the count.
#[test]
fn us_07_4_ignore_and_show_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let p = fixture(tmp.path());
    let key = "email:accounts@brightwater.example";
    let before = p.connections_summary().unwrap();
    assert_eq!(before.ignored, 0);
    let shown = ConnectionQuery {
        include_ignored: true,
        ..Default::default()
    };
    p.set_ignored(key, true).unwrap();
    let after = p.connections_summary().unwrap();
    assert_eq!((after.visible, after.ignored), (before.visible - 1, 1));
    assert!(!keys(&all(&p, &ConnectionQuery::default(), 100)).contains(&key));
    let with = all(&p, &shown, 100);
    let row = with.iter().find(|r| r.subject_key == key).unwrap();
    assert!(row.ignored);
    assert_eq!(p.connection_count_for(&shown).unwrap(), before.visible);
    p.set_ignored(key, false).unwrap();
    assert_eq!(p.connections_summary().unwrap().visible, before.visible);
}

/// US-07.6: the summary says how many files are read, for "No cross-source connections found in
/// N documents" and "N of M files done".
#[test]
fn us_07_6_summary_counts_documents() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[("a.txt", "one@x.example\n"), ("b.txt", "two@y.example\n")],
    );
    let p = open(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    let s = p.connections_summary().unwrap();
    assert_eq!(
        (s.visible, s.ignored, s.documents_done, s.documents_total),
        (0, 0, 2, 2)
    );
}

/// NFR-QA-10: no connection without two counted documents; the health check says so, and
/// every connection's occurrences (each with a locator) span two documents.
fn assert_qa_10(p: &Project) {
    let health = p.check_health().unwrap();
    assert!(
        health.weak_connections.is_empty(),
        "{:?}",
        health.weak_connections
    );
    assert!(health.is_healthy());
    for c in all(
        p,
        &ConnectionQuery {
            include_ignored: true,
            ..Default::default()
        },
        500,
    ) {
        let occ = p.occurrences(&c.subject_key).unwrap();
        let docs: std::collections::BTreeSet<_> = occ.iter().map(|o| o.document_id).collect();
        assert!(
            occ.len() >= 2 && docs.len() >= 2,
            "{}: {} mentions, {} documents",
            c.subject_key,
            occ.len(),
            docs.len()
        );
        assert!(occ.len() >= c.mention_count.min(1000) as usize || occ.len() == 1000);
    }
}

#[test]
fn nfr_qa_10_synthetic() {
    let tmp = tempfile::tempdir().unwrap();
    let p = fixture(tmp.path());
    assert_qa_10(&p);
    // A removed file takes its links with it.
    std::fs::remove_file(tmp.path().join("src/mail.eml")).unwrap();
    p.ingest().unwrap();
    assert_qa_10(&p);
}

fn td1() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1/synthetic");
    if dir.is_dir() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
        "testdata/td1 is missing"
    );
    eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
    None
}

/// NFR-QA-10 on TD-1, and a rebuild equal to the incremental rows with the new columns.
#[test]
fn nfr_qa_10_td1() {
    let Some(dir) = td1() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let p = open(&tmp.path().join("p"));
    p.add_source_root(&dir).unwrap();
    p.ingest().unwrap();
    assert!(p.connection_count().unwrap() > 0);
    assert_qa_10(&p);
    let r = p.rebuild().unwrap();
    assert!(r.same_as_incremental && r.consistent);
}

/// NFR-REL-09: ignore and un-ignore decisions on an identifier and on a mapped entity survive a
/// re-scan with a changed file, "Re-process" of identifiers, a full rebuild, a re-open, and a
/// derived-table migration (the derived version of an older app).
#[test]
fn nfr_rel_09_decisions_survive() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("payments.csv", PAYMENTS),
            (
                "ledger.csv",
                "Supplier,Contact\nBrightwater Logistics Ltd,ops@brightwater.example\n",
            ),
            ("notes.txt", NOTES),
            ("mail.eml", MAIL),
        ],
    );
    let dir = tmp.path().join("p");
    let p = open(&dir);
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    for name in ["payments.csv", "ledger.csv"] {
        let rows = p
            .documents_page(&Default::default(), None, 100)
            .unwrap()
            .rows;
        let id = rows.iter().find(|r| r.rel_path == name).unwrap().ref_id;
        p.confirm_mapping(
            id,
            Assignments([("Supplier".to_string(), ColumnType::OrgName)].into()),
        )
        .unwrap();
    }
    let ident = "email:accounts@brightwater.example";
    let entity = "org:brightwater logistics";
    let gone = "email:billing@harbour.example"; // ignored, then un-ignored
    let visible = |p: &Project| -> Vec<String> {
        all(p, &ConnectionQuery::default(), 500)
            .into_iter()
            .map(|r| r.row.subject_key)
            .collect()
    };
    assert!(visible(&p).iter().any(|k| k == ident));
    assert!(visible(&p).iter().any(|k| k == entity));
    p.set_ignored(ident, true).unwrap();
    p.set_ignored(entity, true).unwrap();
    p.set_ignored(gone, true).unwrap();
    p.set_ignored(gone, false).unwrap();
    let check = |p: &Project, what: &str| {
        let v = visible(p);
        assert!(
            !v.iter().any(|k| k == ident),
            "{what}: identifier shows again"
        );
        assert!(!v.iter().any(|k| k == entity), "{what}: entity shows again");
        assert!(v.iter().any(|k| k == gone), "{what}: un-ignored hidden");
    };
    check(&p, "after the decisions");

    write(&src, &[("notes.txt", &format!("{NOTES}extra line\n"))]);
    p.ingest().unwrap();
    check(&p, "re-scan");

    p.reprocess(Reprocess {
        identifiers: true,
        ..Default::default()
    })
    .unwrap();
    p.ingest().unwrap();
    check(&p, "re-process");

    let r = p.rebuild().unwrap();
    assert!(r.same_as_incremental && r.consistent);
    check(&p, "rebuild");

    // A project last opened by an app with an older derived layout is rebuilt on open.
    p.store()
        .write_raw(|c| {
            c.execute(
                "UPDATE project_meta SET value = '3' WHERE key = 'derived_version'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    drop(p);
    let (p, summary) = Project::open(&dir).unwrap();
    assert!(summary.schema.derived_rebuilt, "rebuilt on open");
    check(&p, "migration");
    let again = p.rebuild().unwrap();
    assert!(again.same_as_incremental && again.consistent);
}
