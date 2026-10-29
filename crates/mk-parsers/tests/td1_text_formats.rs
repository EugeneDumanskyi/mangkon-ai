//! Golden checks of the text parsers on TD-1's non-PDF files (US-02.3 AC1/AC3, US-02.4
//! AC1–AC3, US-02.5 AC1, US-02.6 AC1–AC3, NFR-COMP-06). Every ground-truth mention is found at
//! its line, cell, paragraph, JSON pointer, header or MIME part, and the stored segment text
//! holds its raw text at its offsets (INV-CO-04). HTML mentions, and mentions in an email's
//! HTML part, are found by text: the ground truth counts offsets in the HTML source, the store
//! in the extracted text. A mention in the HTML alternative of a message that has a plain one is
//! found in the plain part (US-02.6 AC3 keeps only that). PDF attachments are child documents
//! since M3b (US-02.6 AC4); their mentions are checked in `td1_pdf_ocr.rs`. Skipped without
//! `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::path::PathBuf;

use mk_bench::{DocPart, FileKind, Location};
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

/// The text of the first segment whose locator `find` accepts.
fn segment(doc: &ParsedDocument, find: impl Fn(&Locator) -> bool) -> Option<&str> {
    doc.segments
        .iter()
        .find(|s| find(&s.locator))
        .map(|s| s.text.as_str())
}

fn format_of(kind: FileKind) -> Option<Format> {
    Some(match kind {
        FileKind::Txt => Format::Text,
        FileKind::Md => Format::Markdown,
        FileKind::Csv => Format::Csv,
        FileKind::Json => Format::Json,
        FileKind::Html => Format::Html,
        FileKind::Docx => Format::Docx,
        FileKind::Eml => Format::Eml,
        FileKind::Mbox => Format::Mbox,
        _ => return None,
    })
}

#[test]
fn us_02_3_to_us_02_6_td1_text_formats_golden() {
    let Some(td1) = td1() else { return };
    let gt = mk_bench::load(&td1.join("ground-truth.json")).unwrap();
    let mut files = 0;
    let mut mentions = std::collections::BTreeMap::<&str, u32>::new();
    // Messages with an HTML body part (the ground truth counts its offsets in the HTML source).
    let html_parts = [
        "d-2024-01-29-irene-to-tomas-formation",
        "d-2025-01-20-daniel-wallets",
        "d-2025-03-27-priya-report",
    ];
    for file in &gt.files {
        let Some(format) = format_of(file.kind) else {
            continue;
        };
        let bytes = std::fs::read(td1.join(&file.path)).unwrap();
        let parsed =
            mk_parsers::parse(format, &bytes).unwrap_or_else(|e| panic!("{}: {e}", file.path));
        files += 1;
        if format == Format::Csv {
            assert!(
                parsed.documents[0]
                    .segments
                    .iter()
                    .all(|s| s.row_state == RowState::Normal),
                "{}: TD-1 CSVs are well formed",
                file.path
            );
        }
        for d in gt.documents.iter().filter(|d| d.file == file.path) {
            let doc = match &d.part {
                DocPart::Whole => &parsed.documents[0],
                DocPart::MboxMessage { index } => {
                    let position = format!("message {index}");
                    parsed
                        .documents
                        .iter()
                        .find(|p| p.position == position && p.parent.is_none())
                        .unwrap_or_else(|| panic!("{}: no {position}", file.path))
                }
                DocPart::Attachment { filename, .. } => {
                    // TD-1 attachments are text PDFs: child documents (US-02.6 AC4).
                    assert!(
                        parsed.documents.iter().any(|p| p.kind == Format::Pdf
                            && p.parent.is_some()
                            && p.position.ends_with(filename.as_str())
                            && !p.segments.is_empty()),
                        "{}: no PDF document for {filename}: {:?}",
                        file.path,
                        parsed.warnings
                    );
                    continue;
                }
            };
            for m in gt.mentions.iter().filter(|m| m.document == d.id) {
                let got = match &m.location {
                    Location::TextLine { line, start, end } => {
                        let text = segment(doc, |l| {
                            matches!(l, Locator::TextLine { line: n, .. } if n.get() == *line)
                        });
                        text.map(|t| chars(t, *start, *end))
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
                        Some(chars(&seg.text, cell + start, cell + end))
                    }
                    Location::DocxParagraph {
                        paragraph,
                        start,
                        end,
                    } => segment(doc, |l| {
                        matches!(l, Locator::DocxParagraph { paragraph: p, .. } if p == paragraph)
                    })
                    .map(|t| chars(t, *start, *end)),
                    Location::JsonValue {
                        pointer,
                        start,
                        end,
                    } => segment(doc, |l| {
                        matches!(l, Locator::JsonValue { pointer: p, .. } if p == pointer)
                    })
                    .map(|t| chars(t, *start, *end)),
                    Location::EmailHeader { header, start, end } => segment(doc, |l| {
                        matches!(l, Locator::EmailHeader { name, .. } if name.eq_ignore_ascii_case(header))
                    })
                    .map(|t| chars(t, *start, *end)),
                    Location::MimePart { part, start, end } => {
                        let text = segment(doc, |l| {
                            matches!(l, Locator::MimePart { part: p, .. } if p == part)
                        });
                        match text {
                            Some(t) if chars(t, *start, *end) == m.raw => Some(m.raw.clone()),
                            // An HTML part: by text, in the part or in the plain alternative.
                            _ if html_parts.contains(&d.id.as_str()) => {
                                let t = text.or_else(|| {
                                    segment(doc, |l| {
                                        matches!(l, Locator::MimePart { part: p, .. } if p == "1")
                                    })
                                });
                                t.filter(|t| t.contains(&m.raw)).map(|_| m.raw.clone())
                            }
                            _ => text.map(|t| chars(t, *start, *end)),
                        }
                    }
                    Location::HtmlSource { .. } => doc
                        .segments
                        .iter()
                        .any(|s| s.text.contains(&m.raw))
                        .then(|| m.raw.clone()),
                    other => panic!("{}: unexpected location {other:?}", file.path),
                };
                assert_eq!(
                    got.as_deref(),
                    Some(m.raw.as_str()),
                    "{}: mention {} at {:?}",
                    file.path,
                    m.id,
                    m.location
                );
                *mentions.entry(m.location.kind_name()).or_default() += 1;
            }
        }
    }
    eprintln!("{files} files, mentions checked: {mentions:?}");
    assert_eq!(files, 37);
    for kind in [
        "text_line",
        "csv_cell",
        "docx_paragraph",
        "json_value",
        "email_header",
        "mime_part",
        "html_source",
    ] {
        assert!(mentions.get(kind).copied().unwrap_or(0) > 0, "no {kind}");
    }
}
