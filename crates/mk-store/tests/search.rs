//! M4a store checks: the FTS5 index over segment text (schema v5, ADR-0010) follows commits,
//! re-commits and migration; search pages, filters and hit spans (US-06.1, INV-SR-01).

use std::path::Path;

use mk_kernel::{ContentHash, LineNumber, Locator, Span};
use mk_store::{
    DocumentOut, NewContent, SearchCursor, SearchFilter, SegmentOut, StageAOutput, Store,
};
use proptest::prelude::*;

fn open(dir: &Path) -> Store {
    Store::open(&dir.join("project.db"), &dir.join("backups"))
        .unwrap()
        .0
}

/// One document of `kind` with one line per text.
fn output(kind: &str, lines: &[&str]) -> StageAOutput {
    let segments = lines
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let len = t.chars().count();
            SegmentOut {
                locator: Locator::TextLine {
                    line: LineNumber::new(i as u32 + 1).unwrap(),
                    span: Span::new(0, len, len).unwrap(),
                },
                text: t.to_string(),
                malformed: false,
                cell_starts: Vec::new(),
                ocr: None,
            }
        })
        .collect();
    StageAOutput {
        documents: vec![DocumentOut {
            kind: kind.into(),
            position: "whole".into(),
            parent: None,
            segments,
        }],
        mentions: Vec::new(),
        card_flags: Vec::new(),
        warnings: Vec::new(),
    }
}

/// Adds content `n` of `file_type` (no path) and commits `lines` as its Stage A result.
fn ingest(store: &Store, n: u8, file_type: &str, lines: &[&str]) {
    let items = store
        .enqueue_contents(vec![NewContent {
            hash: ContentHash::from_bytes([n; 32]),
            file_type: file_type.into(),
            size: 10,
        }])
        .unwrap();
    store.start_item(items[0]).unwrap().unwrap();
    store
        .commit_stage_a(items[0], output(file_type, lines))
        .unwrap();
}

fn segments(store: &Store) -> u64 {
    store
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM segment", [], |r| r.get::<_, i64>(0))?))
        .unwrap() as u64
}

fn fts(q: &str) -> String {
    mk_search::parse(q).unwrap().to_fts5()
}

fn texts(store: &Store, q: &str, filter: &SearchFilter) -> Vec<String> {
    store
        .search_page(&fts(q), |_| fts(q), filter, None, 50)
        .unwrap()
        .into_iter()
        .map(|r| r.text)
        .collect()
}

/// INV-SR-01, US-06.1 AC1: every result is a segment with its locator, document and the
/// character spans of its hits in the stored text (INV-CO-04).
#[test]
fn inv_sr_01_every_result_has_segment_and_locator() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    ingest(
        &store,
        1,
        "txt",
        &[
            "Zoë signed the lease",
            "nothing here",
            "LEASE renewal: lease",
        ],
    );
    let rows = store
        .search_page(
            &fts("lease"),
            |_| fts("lease"),
            &SearchFilter::default(),
            None,
            50,
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(matches!(r.locator, Locator::TextLine { .. }));
        assert_eq!(r.file_type, "txt");
        assert!(!r.hits.is_empty());
        for &(s, e) in &r.hits {
            let hit: String = r.text.chars().skip(s).take(e - s).collect();
            assert_eq!(hit.to_lowercase(), "lease", "{r:?}");
        }
    }
    // More hits in a shorter text ranks first (bm25).
    assert_eq!(rows[0].text, "LEASE renewal: lease");
    assert_eq!(rows[0].hits, vec![(0, 5), (15, 20)]);
    // Case and diacritics fold.
    assert_eq!(
        texts(&store, "zoe", &SearchFilter::default()),
        ["Zoë signed the lease"]
    );
    // A prefix marked through the page's words (S4) gives the spans FTS5 gives.
    let query = mk_search::parse("lea* renew*").unwrap();
    let marks = |via: &dyn Fn(&[&str]) -> String| {
        store
            .search_page(&query.to_fts5(), via, &SearchFilter::default(), None, 50)
            .unwrap()
            .into_iter()
            .map(|r| r.hits)
            .collect::<Vec<_>>()
    };
    let expanded = marks(&|t| query.highlight_fts5(t));
    assert_eq!(expanded, vec![vec![(0, 5), (6, 13), (15, 20)]]);
    assert_eq!(expanded, marks(&|_| query.to_fts5()));
}

/// US-06.1 AC2 on a real index: phrase, prefix, OR.
#[test]
fn us_06_1_ac2_syntax_on_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    ingest(
        &store,
        1,
        "txt",
        &["harbour view lease", "view of the harbour", "invoice 42"],
    );
    let all = SearchFilter::default();
    assert_eq!(
        texts(&store, "\"harbour view\"", &all),
        ["harbour view lease"]
    );
    assert_eq!(texts(&store, "harbour view", &all).len(), 2);
    assert_eq!(texts(&store, "invo*", &all), ["invoice 42"]);
    assert_eq!(texts(&store, "invoice OR lease", &all).len(), 2);
    assert!(texts(&store, "nowhere", &all).is_empty());
}

/// US-06.1 AC3: file type and document filters.
#[test]
fn us_06_1_ac3_filters() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    ingest(&store, 1, "txt", &["alpha one"]);
    ingest(&store, 2, "csv", &["alpha two"]);
    ingest(&store, 3, "pdf", &["alpha three"]);
    let only = |types: &[&str]| SearchFilter {
        file_types: types.iter().map(|t| t.to_string()).collect(),
        document: None,
    };
    assert_eq!(texts(&store, "alpha", &only(&["csv"])), ["alpha two"]);
    let mut two = texts(&store, "alpha", &only(&["txt", "pdf"]));
    two.sort();
    assert_eq!(two, ["alpha one", "alpha three"]);
    let doc = store
        .search_page(
            &fts("three"),
            |_| fts("three"),
            &SearchFilter::default(),
            None,
            1,
        )
        .unwrap()[0]
        .document_id;
    let in_doc = SearchFilter {
        file_types: Vec::new(),
        document: Some(doc),
    };
    assert_eq!(texts(&store, "alpha", &in_doc), ["alpha three"]);
}

/// Keyset paging: pages join up to the full list, with no row twice.
#[test]
fn paging_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let lines: Vec<String> = (0..120)
        .map(|i| format!("word {}", "pad ".repeat(i % 7)))
        .collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    ingest(&store, 1, "txt", &refs);
    ingest(&store, 2, "csv", &refs[..30]);
    for filter in [
        SearchFilter::default(),
        SearchFilter {
            file_types: vec!["txt".into(), "csv".into()],
            document: None,
        },
    ] {
        let whole = store
            .search_page(&fts("word"), |_| fts("word"), &filter, None, 500)
            .unwrap();
        assert_eq!(whole.len(), 150);
        let mut paged = Vec::new();
        let mut after: Option<SearchCursor> = None;
        loop {
            let page = store
                .search_page(&fts("word"), |_| fts("word"), &filter, after.as_ref(), 50)
                .unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.cursor());
            paged.extend(page.into_iter().map(|r| r.segment_id));
        }
        let ids: Vec<i64> = whole.iter().map(|r| r.segment_id).collect();
        assert_eq!(paged, ids);
    }
}

/// ADR-0010: the index follows re-commits (old segments out, new in) and stays consistent.
#[test]
fn index_follows_recommits() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    ingest(&store, 1, "txt", &["old words", "more old words"]);
    assert_eq!(store.search_index_rows().unwrap(), 2);
    // The same content read again replaces its segments (INV-CO-06).
    let item = store
        .write_raw(|c| {
            c.execute("UPDATE content SET state = 'queued' WHERE id = 1", [])?;
            c.execute(
                "INSERT INTO job (trigger, stage, state, created_at)
                 VALUES ('rescan', 'a', 'running', '2026-10-30T00:00:00Z')",
                [],
            )?;
            c.execute(
                "INSERT INTO work_item (job_id, content_id, priority, state)
                 VALUES (last_insert_rowid(), 1, 0, 'pending')",
                [],
            )?;
            Ok(c.last_insert_rowid())
        })
        .unwrap();
    store.start_item(item).unwrap().unwrap();
    store
        .commit_stage_a(item, output("txt", &["new text"]))
        .unwrap();
    assert_eq!(store.search_index_rows().unwrap(), segments(&store));
    assert!(texts(&store, "old", &SearchFilter::default()).is_empty());
    assert_eq!(texts(&store, "new", &SearchFilter::default()), ["new text"]);
    store.check_search_index().unwrap();
    store.rebuild_search_index().unwrap();
    store.check_search_index().unwrap();
    assert_eq!(store.search_index_rows().unwrap(), 1);
}

/// NFR-REL-11: a v4 project's segments are indexed by the v5 migration.
#[test]
fn migration_v4_to_v5_fills_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("project.db");
    let backups = dir.path().join("backups");
    {
        // A v4 project with segments: a commit writes the index, so build at the latest
        // version and take the index (and the v6 objects) back out.
        let store = open(dir.path());
        ingest(&store, 1, "txt", &["before search existed", "second line"]);
        store
            .write_raw(|c| {
                c.execute_batch(
                    "DROP TRIGGER segment_fts_delete; DROP TRIGGER segment_fts_update;
                     DROP TABLE segment_fts; DROP TABLE sensitive_flag;
                     DROP INDEX identifier_mention_content; PRAGMA user_version = 4;",
                )?;
                Ok(())
            })
            .unwrap();
    }
    let (store, report) = Store::open(&db, &backups).unwrap();
    assert_eq!((report.from_version, report.to_version), (4, 6));
    assert!(report.backup.unwrap().exists());
    assert_eq!(store.search_index_rows().unwrap(), 2);
    assert_eq!(
        texts(&store, "existed", &SearchFilter::default()),
        ["before search existed"]
    );
    store.check_search_index().unwrap();
}

fn shared_store() -> &'static (tempfile::TempDir, Store) {
    static STORE: std::sync::OnceLock<(tempfile::TempDir, Store)> = std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        ingest(
            &store,
            1,
            "txt",
            &["NEAR quote \" col: text ^ * OR AND NOT"],
        );
        (dir, store)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// US-06.1 AC2: whatever a person types, the FTS5 query `mk-search` makes from it is
    /// accepted by a real FTS5 table (no syntax error).
    #[test]
    fn us_06_1_ac2_any_query_is_valid_fts5(q in "[ -~éß\u{301}\u{E000}\t]{0,40}") {
        let Ok(query) = mk_search::parse(&q) else { return Ok(()) };
        let (_, store) = shared_store();
        let got = store.search_page(&query.to_fts5(), |t| query.highlight_fts5(t), &SearchFilter::default(), None, 5);
        prop_assert!(got.is_ok(), "{q:?} -> {}: {got:?}", query.to_fts5());
    }
}
