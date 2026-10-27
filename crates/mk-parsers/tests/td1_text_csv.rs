//! Golden checks of the TXT/MD and CSV parsers on TD-1 `notes/` and `csv/` (US-02.3 AC1,
//! US-02.4 AC1–AC3). Every ground-truth mention in a UTF-8 file is found at its line or cell,
//! and the stored segment text holds its raw text at its offsets (INV-CO-04). Files in other
//! encodings fail with "Unknown text encoding" until M3. Skipped without `testdata/` unless
//! `MK_REQUIRE_TESTDATA=1`.

use std::path::PathBuf;

use mk_bench::{FileKind, Location};
use mk_kernel::Locator;
use mk_parsers::{Format, ParsedDocument, RowState};

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

fn chars(text: &str, start: u32, end: u32) -> String {
    text.chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect()
}

fn line_text(doc: &ParsedDocument, line: u32) -> Option<&str> {
    doc.segments.iter().find_map(|s| match &s.locator {
        Locator::TextLine { line: l, .. } if l.get() == line => Some(s.text.as_str()),
        _ => None,
    })
}

#[test]
fn us_02_3_us_02_4_td1_notes_and_csv_golden() {
    let Some(td1) = td1() else { return };
    let gt = mk_bench::load(&td1.join("ground-truth.json")).unwrap();
    let (mut parsed_files, mut failed_files, mut mentions) = (0, 0, 0);
    for file in &gt.files {
        let format = match file.kind {
            FileKind::Txt => Format::Text,
            FileKind::Md => Format::Markdown,
            FileKind::Csv => Format::Csv,
            _ => continue,
        };
        let bytes = std::fs::read(td1.join(&file.path)).unwrap();
        let utf8 = matches!(file.encoding.as_deref(), Some("utf-8" | "utf-8-bom"));
        let parsed = mk_parsers::parse(format, &bytes);
        if !utf8 {
            let e = parsed.expect_err(&file.path);
            assert_eq!(e.code, "unknown_text_encoding", "{}", file.path);
            failed_files += 1;
            continue;
        }
        let parsed = parsed.unwrap_or_else(|e| panic!("{}: {e}", file.path));
        parsed_files += 1;
        assert_eq!(parsed.documents.len(), 1);
        let doc = &parsed.documents[0];
        if format == Format::Csv {
            assert!(
                doc.segments.iter().all(|s| s.row_state == RowState::Normal),
                "{}: TD-1 CSVs are well formed",
                file.path
            );
            assert!(parsed.warnings.is_empty());
        }
        let doc_id = &gt
            .documents
            .iter()
            .find(|d| d.file == file.path)
            .unwrap()
            .id;
        for m in gt.mentions.iter().filter(|m| &m.document == doc_id) {
            let got = match &m.location {
                Location::TextLine { line, start, end } => {
                    let text = line_text(doc, *line)
                        .unwrap_or_else(|| panic!("{}: no segment for line {line}", file.path));
                    chars(text, *start, *end)
                }
                Location::CsvCell {
                    row,
                    column,
                    start,
                    end,
                } => {
                    let seg = &doc.segments[*row as usize];
                    assert_eq!(seg.locator, Locator::CsvRow { row: *row as u64 });
                    let cell = seg.cell_starts[*column as usize];
                    chars(&seg.text, cell + start, cell + end)
                }
                other => panic!("{}: unexpected location {other:?}", file.path),
            };
            assert_eq!(got, m.raw, "{}: mention {}", file.path, m.id);
            mentions += 1;
        }
    }
    assert!(parsed_files >= 7, "{parsed_files} files parsed");
    assert!(failed_files >= 3, "{failed_files} files failed");
    assert!(mentions > 20, "{mentions} mentions checked");
}
