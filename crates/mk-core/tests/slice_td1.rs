//! TD-1 TXT/MD/CSV end to end through the real pipeline (re-scan → Stage A in process → one
//! commit per file → derived rows): US-01.3 AC2, US-02.3 AC1, US-02.4, US-02.7 AC1, US-04.1 AC1,
//! US-07.1 AC1. Skipped without `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mk_bench::{IdType, Location, Subject};
use mk_corpus::IngestionState;
use mk_kernel::{Column, Locator};
use mk_parsers::Format;

fn td1() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1");
    if dir.join("ground-truth.json").exists() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
        "testdata/td1 is missing"
    );
    eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
    None
}

fn chars(text: &str, start: usize, end: usize) -> String {
    text.chars().skip(start).take(end - start).collect()
}

#[test]
fn slice_td1_text_and_csv_end_to_end() {
    let Some(td1) = td1() else { return };
    let gt = mk_bench::load(&td1.join("ground-truth.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (project, _) = mk_core::Project::open(dir.path()).unwrap();
    let root = project.add_source_root(&td1.join("synthetic")).unwrap();

    let scan = project.rescan(false).unwrap();
    let text_files: Vec<_> = gt
        .files
        .iter()
        .filter(|f| f.path.starts_with("synthetic/"))
        .filter(|f| {
            std::path::Path::new(&f.path)
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Format::from_extension)
                .is_some()
        })
        .collect();
    assert_eq!(scan.walked, text_files.len());
    assert_eq!(scan.outcome.new_items, text_files.len());
    // US-01.3 AC3: everything else is reported as skipped, not stored.
    let synthetic = gt
        .files
        .iter()
        .filter(|f| f.path.starts_with("synthetic/"))
        .count();
    assert_eq!(scan.skipped.len(), synthetic - text_files.len());

    let run = project.run_stage_a().unwrap();
    let utf8 =
        |f: &mk_bench::FileEntry| matches!(f.encoding.as_deref(), Some("utf-8" | "utf-8-bom"));
    let good: Vec<_> = text_files.iter().filter(|f| utf8(f)).collect();
    assert_eq!(run.done, good.len());
    assert_eq!(run.failed, text_files.len() - good.len());

    let refs = project.store().stored_refs().unwrap();
    let mut by_doc: BTreeMap<String, i64> = BTreeMap::new();
    let mut want_links: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut checked = 0;
    for f in &text_files {
        let rel = f.path.strip_prefix("synthetic/").unwrap();
        let stored = refs
            .iter()
            .find(|r| r.root == root && r.rel_path == rel)
            .unwrap();
        let content = project.store().content(&stored.hash).unwrap().unwrap();
        if !utf8(f) {
            // US-02.7 AC1, NFR-USE-05: fails alone, with a plain reason.
            assert_eq!(content.state, IngestionState::Failed, "{}", f.path);
            assert_eq!(
                content.failure_reason.as_deref(),
                Some("Unknown text encoding")
            );
            continue;
        }
        assert_eq!(content.state, IngestionState::Done, "{}", f.path);
        let doc = gt.documents.iter().find(|d| d.file == f.path).unwrap();
        by_doc.insert(doc.id.clone(), content.id);
        let got: BTreeSet<(String, String)> = project
            .store()
            .mentions(content.id)
            .unwrap()
            .into_iter()
            .map(|m| {
                // INV-CO-04: the span lies in the stored segment text.
                let span = m.locator.span().unwrap();
                assert_eq!(chars(&m.segment_text, span.start(), span.end()), m.raw);
                let at = match &m.locator {
                    Locator::TextLine { line, .. } => {
                        format!("line {line} {}..{}", span.start(), span.end())
                    }
                    Locator::CsvCell { row, column, .. } => {
                        let seg = project
                            .store()
                            .segments(content.id)
                            .unwrap()
                            .into_iter()
                            .find(|s| s.locator == Locator::CsvRow { row: *row })
                            .unwrap();
                        let i = seg
                            .cell_starts
                            .partition_point(|c| *c as usize <= span.start())
                            - 1;
                        if let Column::Index(c) = column {
                            assert_eq!(*c as usize, i);
                        }
                        let cell = seg.cell_starts[i] as usize;
                        format!(
                            "row {row} col {i} {}..{}",
                            span.start() - cell,
                            span.end() - cell
                        )
                    }
                    other => panic!("{other:?}"),
                };
                (at, m.key.to_string())
            })
            .collect();
        let want: BTreeSet<(String, String)> = gt
            .mentions
            .iter()
            .filter(|m| m.document == doc.id && m.derived_from.is_none())
            .filter_map(|m| match (&m.subject, &m.location) {
                (Subject::Identifier { key }, loc) if key.id_type == IdType::Email => {
                    let at = match loc {
                        Location::TextLine { line, start, end } => {
                            format!("line {line} {start}..{end}")
                        }
                        Location::CsvCell {
                            row,
                            column,
                            start,
                            end,
                        } => {
                            format!("row {row} col {column} {start}..{end}")
                        }
                        other => panic!("{other:?}"),
                    };
                    Some((at, format!("email:{}", key.value)))
                }
                _ => None,
            })
            .collect();
        assert_eq!(got, want, "{}", f.path);
        for (_, k) in want {
            want_links.entry(k).or_default().insert(doc.id.clone());
        }
        checked += 1;
    }
    assert_eq!(checked, good.len());

    // US-07.1 AC1, INV-LK-01: connections are the emails in two or more of these documents.
    let derived = project.store().derived_state().unwrap();
    let want: BTreeSet<&String> = want_links
        .iter()
        .filter(|(_, docs)| docs.len() >= 2)
        .map(|(k, _)| k)
        .collect();
    let got: BTreeSet<&String> = derived.connections.keys().collect();
    assert_eq!(got, want);
    assert_eq!(
        mk_graph::model::reference(&project.store().facts().unwrap()),
        derived
    );
}
