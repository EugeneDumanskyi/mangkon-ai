//! TD-1 end to end through the real pipeline (re-scan → Stage A in worker processes → one staged
//! commit per file → derived rows): US-01.3 AC2, US-02.1–US-02.6, US-04.1 AC1, US-07.1 AC1,
//! US-08.1 AC1–AC2, NFR-COMP-06. One ingest is shared by the tests below.
//!
//! The `inv_co_04_roundtrip_<kind>` tests check, for every ground-truth mention of that kind,
//! that the stored segment at its place holds its raw text at its offsets (INV-CO-03/04); for
//! pages, that the page view's highlight lies on the ground-truth boxes. HTML
//! is matched by text: the ground truth counts offsets in the HTML source, the store in the
//! extracted text. So are mentions in an email's HTML part; one in the HTML alternative of a
//! message with a plain part is found in the plain part (US-02.6 AC3). Skipped without
//! `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mk_bench::{DocPart, GroundTruth, Location, Mention, Subject};
use mk_corpus::IngestionState;
use mk_kernel::{Column, Locator};
use mk_parsers::Format;
use mk_store::{MentionRow, SegmentRow};

/// Messages whose body has an HTML part, and which of them have no plain part.
const HTML_PARTS: &[&str] = &[
    "d-2024-01-29-irene-to-tomas-formation",
    "d-2025-01-20-daniel-wallets",
    "d-2025-03-27-priya-report",
];
const HTML_ONLY: &[&str] = &["d-2025-01-20-daniel-wallets"];

struct Fixture {
    _dir: tempfile::TempDir,
    project: mk_core::Project,
    gt: GroundTruth,
    /// Ground-truth document id → stored document id.
    docs: BTreeMap<String, i64>,
    /// The same for PDF and image files.
    pages: BTreeMap<String, i64>,
    segments: BTreeMap<i64, Vec<SegmentRow>>,
    mentions: Vec<MentionRow>,
    files: usize,
    done: usize,
}

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

fn format(path: &str) -> Option<Format> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Format::from_extension)
}

fn is_text(path: &str) -> bool {
    format(path).is_some_and(|f| !f.is_paged())
}

fn fixture() -> Option<&'static Fixture> {
    static F: OnceLock<Option<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        let td1 = td1()?;
        let gt = mk_bench::load(&td1.join("ground-truth.json")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (mut project, _) = mk_core::Project::open(&dir.path().join("project")).unwrap();
        *project.worker_config_mut() =
            mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
        project.set_tessdata_dir(Some(dir.path().join("tessdata")));
        let root = project.add_source_root(&td1.join("synthetic")).unwrap();
        let scan = project.rescan(false).unwrap();
        let files: Vec<mk_bench::FileEntry> = gt
            .files
            .iter()
            .filter(|f| f.path.starts_with("synthetic/") && format(&f.path).is_some())
            .cloned()
            .collect();
        assert_eq!(scan.walked, files.len());
        // US-01.3 AC3: TD-1 has no other kind of file.
        assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
        let run = project.run_stage_a().unwrap();

        let store = project.store();
        let refs = store.stored_refs().unwrap();
        let mut docs = BTreeMap::new();
        let mut pages = BTreeMap::new();
        let mut segments: BTreeMap<i64, Vec<SegmentRow>> = BTreeMap::new();
        let mut mentions = Vec::new();
        for f in &files {
            let rel = f.path.strip_prefix("synthetic/").unwrap();
            let stored = refs
                .iter()
                .find(|r| r.root == root && r.rel_path == rel)
                .unwrap();
            let content = store.content(&stored.hash).unwrap().unwrap();
            assert_eq!(
                content.state,
                IngestionState::Done,
                "{}: {:?}",
                f.path,
                content.failure_reason
            );
            for seg in store.segments(content.id).unwrap() {
                segments.entry(seg.document_id).or_default().push(seg);
            }
            let positions: BTreeMap<String, i64> = segments
                .keys()
                .filter_map(|d| store.document_info(*d).unwrap())
                .filter(|d| d.content_id == content.id && d.parent.is_none())
                .map(|d| (d.position, d.id))
                .collect();
            for d in gt.documents.iter().filter(|d| d.file == f.path) {
                let position = match &d.part {
                    DocPart::Whole => "whole".to_string(),
                    DocPart::MboxMessage { index } => format!("message {index}"),
                    DocPart::Attachment { .. } => continue,
                };
                let id = positions
                    .get(&position)
                    .unwrap_or_else(|| panic!("{}: no {position}", f.path));
                if is_text(&f.path) {
                    docs.insert(d.id.clone(), *id);
                } else {
                    pages.insert(d.id.clone(), *id);
                }
            }
            mentions.extend(store.mentions(content.id).unwrap());
        }
        Some(Fixture {
            _dir: dir,
            project,
            gt,
            docs,
            pages,
            segments,
            mentions,
            files: files.len(),
            done: run.done,
        })
    })
    .as_ref()
}

fn chars(text: &str, start: usize, end: usize) -> String {
    text.chars().skip(start).take(end - start).collect()
}

/// The stored text at a ground-truth location: `Some(raw)` when it is there.
fn at(f: &Fixture, m: &Mention) -> Option<String> {
    let doc = *f.docs.get(&m.document)?;
    let segs = &f.segments[&doc];
    let find = |pick: &dyn Fn(&Locator) -> bool| segs.iter().find(|s| pick(&s.locator));
    let cut = |s: Option<&SegmentRow>, start: &u32, end: &u32| {
        s.map(|s| chars(&s.text, *start as usize, *end as usize))
    };
    match &m.location {
        Location::TextLine { line, start, end } => cut(
            find(&|l| matches!(l, Locator::TextLine { line: n, .. } if n.get() == *line)),
            start,
            end,
        ),
        Location::CsvCell {
            row,
            column,
            start,
            end,
        } => {
            let s = find(&|l| {
                *l == Locator::CsvRow {
                    row: u64::from(*row),
                }
            })?;
            let cell = s.cell_starts[*column as usize];
            Some(chars(
                &s.text,
                (cell + start) as usize,
                (cell + end) as usize,
            ))
        }
        Location::DocxParagraph {
            paragraph,
            start,
            end,
        } => cut(
            find(&|l| matches!(l, Locator::DocxParagraph { paragraph: p, .. } if p == paragraph)),
            start,
            end,
        ),
        Location::JsonValue {
            pointer,
            start,
            end,
        } => cut(
            find(&|l| matches!(l, Locator::JsonValue { pointer: p, .. } if p == pointer)),
            start,
            end,
        ),
        Location::EmailHeader { header, start, end } => cut(
            find(
                &|l| matches!(l, Locator::EmailHeader { name, .. } if name.eq_ignore_ascii_case(header)),
            ),
            start,
            end,
        ),
        Location::MimePart { part, start, end } => {
            let html = HTML_PARTS.contains(&m.document.as_str());
            let plain_part = if html && !HTML_ONLY.contains(&m.document.as_str()) {
                "1"
            } else {
                part.as_str()
            };
            let s = find(&|l| matches!(l, Locator::MimePart { part: p, .. } if p == plain_part));
            if html {
                s.filter(|s| s.text.contains(&m.raw)).map(|_| m.raw.clone())
            } else {
                cut(s, start, end)
            }
        }
        Location::HtmlSource { .. } => segs
            .iter()
            .any(|s| s.text.contains(&m.raw))
            .then(|| m.raw.clone()),
        _ => None,
    }
}

/// Every ground-truth mention of `kind` in a stored document is at its place (INV-CO-04).
fn round_trip(kind: &str) {
    let Some(f) = fixture() else { return };
    let mut n = 0;
    for m in
        f.gt.mentions
            .iter()
            .filter(|m| m.location.kind_name() == kind && f.docs.contains_key(&m.document))
    {
        assert_eq!(
            at(f, m).as_deref(),
            Some(m.raw.as_str()),
            "{} {} at {:?}",
            m.document,
            m.id,
            m.location
        );
        n += 1;
    }
    assert!(n > 0, "no {kind} mentions");
    eprintln!("{kind}: {n} mentions at their place");
}

#[test]
fn inv_co_04_roundtrip_line() {
    round_trip("text_line");
}

#[test]
fn inv_co_04_roundtrip_row_column() {
    round_trip("csv_cell");
}

#[test]
fn inv_co_04_roundtrip_json_path() {
    round_trip("json_value");
}

#[test]
fn inv_co_04_roundtrip_header() {
    round_trip("email_header");
}

#[test]
fn inv_co_04_roundtrip_mime_part() {
    round_trip("mime_part");
}

#[test]
fn inv_co_04_roundtrip_paragraph() {
    round_trip("docx_paragraph");
}

#[test]
fn inv_co_04_roundtrip_html_offset() {
    round_trip("html_source");
}

/// A stored mention's place in ground-truth terms; HTML (and HTML email parts) by key only.
fn place(f: &Fixture, doc: &str, m: &MentionRow) -> String {
    let span = m.locator.span().unwrap();
    let (s, e) = (span.start(), span.end());
    match &m.locator {
        Locator::TextLine { line, .. } => format!("line {line} {s}..{e}"),
        Locator::CsvCell { row, column, .. } => {
            let seg = f.segments[&m.document_id]
                .iter()
                .find(|x| x.locator == Locator::CsvRow { row: *row })
                .unwrap();
            let i = seg.cell_starts.partition_point(|c| *c as usize <= s) - 1;
            if let Column::Index(c) = column {
                assert_eq!(*c as usize, i);
            }
            let cell = seg.cell_starts[i] as usize;
            format!("row {row} col {i} {}..{}", s - cell, e - cell)
        }
        Locator::DocxParagraph { paragraph, .. } => format!("paragraph {paragraph} {s}..{e}"),
        Locator::JsonValue { pointer, .. } => format!("json {pointer} {s}..{e}"),
        Locator::EmailHeader { name, .. } => format!("header {} {s}..{e}", name.to_lowercase()),
        Locator::MimePart { .. } if HTML_ONLY.contains(&doc) => "html".into(),
        Locator::MimePart { part, .. } => format!("part {part} {s}..{e}"),
        Locator::HtmlText { .. } => "html".into(),
        other => panic!("{other:?}"),
    }
}

fn gt_place(m: &Mention) -> Option<String> {
    Some(match &m.location {
        Location::TextLine { line, start, end } => format!("line {line} {start}..{end}"),
        Location::CsvCell {
            row,
            column,
            start,
            end,
        } => format!("row {row} col {column} {start}..{end}"),
        Location::DocxParagraph {
            paragraph,
            start,
            end,
        } => format!("paragraph {paragraph} {start}..{end}"),
        Location::JsonValue {
            pointer,
            start,
            end,
        } => format!("json {pointer} {start}..{end}"),
        Location::EmailHeader { header, start, end } => {
            format!("header {} {start}..{end}", header.to_lowercase())
        }
        Location::MimePart { .. } if HTML_ONLY.contains(&m.document.as_str()) => "html".into(),
        // The HTML alternative is not stored when there is a plain part (US-02.6 AC3).
        Location::MimePart { part, .. }
            if HTML_PARTS.contains(&m.document.as_str()) && part != "1" =>
        {
            return None;
        }
        Location::MimePart { part, start, end } => format!("part {part} {start}..{end}"),
        Location::HtmlSource { .. } => "html".into(),
        _ => return None,
    })
}

/// US-04.1 AC1, US-07.1 AC1: every text file is read; the stored email mentions are exactly the
/// ground truth's, at the same places; the connections are the emails in two or more of these
/// documents and equal a rebuild from the facts (INV-LK-01, ADR-0005).
#[test]
fn slice_td1_text_formats_end_to_end() {
    let Some(f) = fixture() else { return };
    assert_eq!(f.done, f.files);
    let by_doc: BTreeMap<i64, &str> = f.docs.iter().map(|(k, v)| (*v, k.as_str())).collect();
    let mut want_links: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for (gt_doc, doc) in &f.docs {
        let got: BTreeSet<(String, String)> = f
            .mentions
            .iter()
            .filter(|m| m.document_id == *doc)
            .map(|m| {
                // INV-CO-04: the span lies in the stored segment text.
                let span = m.locator.span().unwrap();
                assert_eq!(chars(&m.segment_text, span.start(), span.end()), m.raw);
                (place(f, by_doc[doc], m), m.key.to_string())
            })
            .collect();
        let want: BTreeSet<(String, String)> =
            f.gt.mentions
                .iter()
                .filter(|m| &m.document == gt_doc && m.derived_from.is_none())
                .filter_map(|m| match &m.subject {
                    Subject::Identifier { key } if key.id_type == mk_bench::IdType::Email => {
                        Some((gt_place(m)?, format!("email:{}", key.value)))
                    }
                    _ => None,
                })
                .collect();
        assert_eq!(got, want, "{gt_doc}");
        for (_, k) in want {
            want_links.entry(k).or_default().insert(gt_doc);
        }
    }
    let derived = f.project.store().derived_state().unwrap();
    let want: BTreeSet<&String> = want_links
        .iter()
        .filter(|(_, docs)| docs.len() >= 2)
        .map(|(k, _)| k)
        .collect();
    let got: BTreeSet<&String> = derived.connections.keys().collect();
    assert_eq!(got, want);
    assert_eq!(
        mk_graph::model::reference(&f.project.store().facts().unwrap()),
        derived
    );
}

/// A ground-truth box as a fraction of the shown page (`width` × `height` in its units).
fn fraction(b: &mk_bench::BBox, width: f32, height: f32) -> [f32; 4] {
    [
        b.x0() as f32 / width,
        b.y0() as f32 / height,
        b.x1() as f32 / width,
        b.y1() as f32 / height,
    ]
}

fn iou(a: [f32; 4], b: &mk_core::PageBox) -> f32 {
    let b = [b.x, b.y, b.x + b.width, b.y + b.height];
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = w * h;
    let area = |r: [f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    inter / (area(a) + area(b) - inter)
}

/// Each stored email mention on a page of `kind` with its ground-truth twin (same document,
/// page and text), and the shown page's size in the ground truth's units.
fn page_pairs(
    f: &Fixture,
    ocr: bool,
) -> Vec<(&MentionRow, &Mention, mk_core::PageView, (f32, f32))> {
    let td1 = td1().unwrap();
    let mut out = Vec::new();
    for (gt_doc, doc) in &f.pages {
        let file = &f
            .gt
            .documents
            .iter()
            .find(|d| &d.id == gt_doc)
            .unwrap()
            .file;
        let bytes = std::fs::read(td1.join(file)).unwrap();
        for m in f.mentions.iter().filter(|m| m.document_id == *doc) {
            let page = m.locator.page().unwrap().get();
            if matches!(m.locator, Locator::OcrText { .. }) != ocr {
                continue;
            }
            let Some(g) = f.gt.mentions.iter().find(|g| {
                &g.document == gt_doc
                    && g.raw == m.raw
                    && match &g.location {
                        Location::PdfText { page: p, .. } => !ocr && *p == page,
                        Location::Scan { page: p, .. } => ocr && *p == page,
                        _ => false,
                    }
            }) else {
                continue;
            };
            let id = f
                .project
                .occurrences(&m.key.to_string())
                .unwrap()
                .into_iter()
                .find(|o| o.document_id == *doc && o.locator == m.locator)
                .unwrap()
                .mention_id;
            let view = f.project.page_view(*doc, page, Some(id)).unwrap().unwrap();
            assert_eq!(view.status, mk_core::PageStatus::Shown, "{gt_doc} p{page}");
            let size = match &g.location {
                Location::Scan { dpi, .. } if file.ends_with(".pdf") => {
                    let (w, h) = display_points(&bytes, page);
                    (w * *dpi as f32 / 72.0, h * *dpi as f32 / 72.0)
                }
                Location::Scan { .. } => (view.width as f32, view.height as f32),
                _ => display_points(&bytes, page),
            };
            out.push((m, g, view, size));
        }
    }
    out
}

/// A PDF page's displayed size in points.
fn display_points(bytes: &[u8], page: u32) -> (f32, f32) {
    use mk_parsers::pdf::engine::PdfEngine;
    let engine = mk_parsers::pdf::hayro::Hayro::default();
    let doc = engine.open(bytes.to_vec()).unwrap();
    doc.page_info(page as usize - 1).unwrap().display_size()
}

/// The ground-truth PDF boxes of a mention, shown: `/Rotate` applied.
fn shown_pdf_boxes(bytes: &[u8], page: u32, g: &Mention) -> Vec<mk_bench::BBox> {
    use mk_parsers::pdf::engine::PdfEngine;
    let Location::PdfText { boxes, .. } = &g.location else {
        unreachable!()
    };
    let engine = mk_parsers::pdf::hayro::Hayro::default();
    let doc = engine.open(bytes.to_vec()).unwrap();
    let info = doc.page_info(page as usize - 1).unwrap();
    boxes
        .iter()
        .map(|b| {
            let r =
                mk_kernel::Rect::new(b.x0() as f32, b.y0() as f32, b.x1() as f32, b.y1() as f32)
                    .unwrap();
            let d = mk_parsers::layout::to_display(&r, &info);
            mk_bench::BBox([d.x0().into(), d.y0().into(), d.x1().into(), d.y1().into()])
        })
        .collect()
}

/// INV-CO-04, US-08.1 AC1: every email on a PDF text page is highlighted, through the page view,
/// on its ground-truth boxes (one box per line, IoU ≥ 0.5).
#[test]
fn inv_co_04_roundtrip_page() {
    let Some(f) = fixture() else { return };
    let td1 = td1().unwrap();
    let pairs = page_pairs(f, false);
    assert!(!pairs.is_empty(), "no PDF text mentions");
    for (m, g, view, (w, h)) in &pairs {
        let file = &f
            .gt
            .documents
            .iter()
            .find(|d| d.id == g.document)
            .unwrap()
            .file;
        let bytes = std::fs::read(td1.join(file)).unwrap();
        let want = shown_pdf_boxes(&bytes, view.page, g);
        assert_eq!(view.boxes.len(), want.len(), "{} {}", g.document, m.raw);
        for (b, got) in want.iter().zip(&view.boxes) {
            let score = iou(fraction(b, *w, *h), got);
            assert!(score >= 0.5, "{} {}: IoU {score}", g.document, m.raw);
        }
    }
    eprintln!("pdf_text: {} highlights on their boxes", pairs.len());
}

/// INV-CO-04, US-08.1 AC2, US-02.2 AC3: every email read by OCR is highlighted on its words'
/// boxes; their union lies on the ground-truth box (IoU ≥ 0.5).
#[test]
fn inv_co_04_roundtrip_ocr_box() {
    let Some(f) = fixture() else { return };
    let pairs = page_pairs(f, true);
    assert!(!pairs.is_empty(), "no OCR mentions");
    let mut low = Vec::new();
    for (m, g, view, (w, h)) in &pairs {
        let Location::Scan { boxes, .. } = &g.location else {
            unreachable!()
        };
        assert!(!view.boxes.is_empty(), "{} {}", g.document, m.raw);
        let union = view.boxes.iter().skip(1).fold(view.boxes[0], |a, b| {
            let (x1, y1) = (
                (a.x + a.width).max(b.x + b.width),
                (a.y + a.height).max(b.y + b.height),
            );
            let (x, y) = (a.x.min(b.x), a.y.min(b.y));
            mk_core::PageBox {
                x,
                y,
                width: x1 - x,
                height: y1 - y,
            }
        });
        let gt = boxes.iter().skip(1).fold(boxes[0], |a, b| {
            mk_bench::BBox([
                a.x0().min(b.x0()),
                a.y0().min(b.y0()),
                a.x1().max(b.x1()),
                a.y1().max(b.y1()),
            ])
        });
        let score = iou(fraction(&gt, *w, *h), &union);
        if score < 0.5 {
            low.push(format!("{} {}: IoU {score:.2}", g.document, m.raw));
        }
    }
    assert!(low.is_empty(), "{low:#?}");
    eprintln!("ocr: {} highlights on their boxes", pairs.len());
}

/// A small seeded generator (xorshift64*), so the sample is the same on every run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// The highlighted text of a source view, every hit part joined.
fn highlighted(view: &mk_core::SourceContext) -> String {
    use mk_core::SourceBody as B;
    let marked: Vec<&mk_core::Marked> = match &view.body {
        B::Text { lines } => lines.iter().map(|l| &l.text).collect(),
        B::Table { rows, .. } => rows.iter().flat_map(|r| &r.cells).collect(),
        B::Paragraphs { paragraphs } => paragraphs.iter().collect(),
        B::Json { rows } => rows.iter().filter_map(|r| r.value.as_ref()).collect(),
        B::Email { headers, parts, .. } => headers
            .iter()
            .map(|h| &h.value)
            .chain(parts.iter().flat_map(|p| &p.lines))
            .collect(),
        B::Page { lines, .. } => lines.iter().collect(),
    };
    // A hit across lines is split per line; the line break is not shown.
    marked
        .iter()
        .map(|m| m.hit.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the page view of a mention highlights its ground-truth boxes (IoU ≥ 0.5: each line
/// box for PDF text, the union of word boxes for OCR). `Ok(false)` when the ground truth has no
/// twin for it (checked by its text then).
fn page_highlight_ok(f: &Fixture, m: &MentionRow, id: i64) -> Result<bool, String> {
    let td1 = td1().unwrap();
    let (gt_doc, _) = f.pages.iter().find(|(_, d)| **d == m.document_id).unwrap();
    let file = &f
        .gt
        .documents
        .iter()
        .find(|d| &d.id == gt_doc)
        .unwrap()
        .file;
    let page = m.locator.page().unwrap().get();
    let ocr = matches!(m.locator, Locator::OcrText { .. });
    let Some(g) = f.gt.mentions.iter().find(|g| {
        &g.document == gt_doc
            && g.raw == m.raw
            && match &g.location {
                Location::PdfText { page: p, .. } => !ocr && *p == page,
                Location::Scan { page: p, .. } => ocr && *p == page,
                _ => false,
            }
    }) else {
        return Ok(false);
    };
    let view = f
        .project
        .page_view(m.document_id, page, Some(id))
        .unwrap()
        .unwrap();
    if view.status != mk_core::PageStatus::Shown || view.boxes.is_empty() {
        return Err(format!(
            "{gt_doc} p{page} {}: {:?} {:?}",
            m.raw, view.status, view.note
        ));
    }
    let bytes = std::fs::read(td1.join(file)).unwrap();
    let (w, h) = match &g.location {
        Location::Scan { dpi, .. } if file.ends_with(".pdf") => {
            let (w, h) = display_points(&bytes, page);
            (w * *dpi as f32 / 72.0, h * *dpi as f32 / 72.0)
        }
        Location::Scan { .. } => (view.width as f32, view.height as f32),
        _ => display_points(&bytes, page),
    };
    let ok = match &g.location {
        Location::Scan { boxes, .. } => {
            let union = view.boxes.iter().skip(1).fold(view.boxes[0], |a, b| {
                let (x, y) = (a.x.min(b.x), a.y.min(b.y));
                mk_core::PageBox {
                    x,
                    y,
                    width: (a.x + a.width).max(b.x + b.width) - x,
                    height: (a.y + a.height).max(b.y + b.height) - y,
                }
            });
            let gt = boxes.iter().skip(1).fold(boxes[0], |a, b| {
                mk_bench::BBox([
                    a.x0().min(b.x0()),
                    a.y0().min(b.y0()),
                    a.x1().max(b.x1()),
                    a.y1().max(b.y1()),
                ])
            });
            iou(fraction(&gt, w, h), &union) >= 0.5
        }
        _ => {
            let want = shown_pdf_boxes(&bytes, page, g);
            want.len() == view.boxes.len()
                && want
                    .iter()
                    .zip(&view.boxes)
                    .all(|(b, got)| iou(fraction(b, w, h), got) >= 0.5)
        }
    };
    if ok {
        Ok(true)
    } else {
        Err(format!("{gt_doc} p{page} {}: boxes off", m.raw))
    }
}

/// NFR-QA-12 (M3 criterion 5b): of 200 seeded random clicks on TD-1's mentions, at least 198 open on the
/// right text: the view's highlight is the mention's text, and on a page its boxes lie on the
/// ground truth's (PDF text and OCR). Release build: `cargo test --release -p mk-core --test
/// slice_td1 -- --ignored nfr_qa_12`.
#[test]
#[ignore = "M3 acceptance run, release build"]
fn nfr_qa_12_highlight_lands_on_right_text() {
    let Some(f) = fixture() else { return };
    let mut pool: Vec<&MentionRow> = f.mentions.iter().collect();
    assert!(!pool.is_empty());
    pool.sort_by_key(|m| (m.document_id, m.key.to_string(), format!("{:?}", m.locator)));
    let mut rng = Seeded(0x4D33_4421);
    // 200 clicks drawn with replacement: TD-1 has fewer stored mentions than that while only
    // emails are found (M4 adds the other types).
    let sample: Vec<&MentionRow> = (0..200)
        .map(|_| pool[(rng.next() % pool.len() as u64) as usize])
        .collect();
    let distinct: std::collections::BTreeSet<*const MentionRow> =
        sample.iter().map(|m| *m as *const _).collect();
    let mut wrong = Vec::new();
    let (mut on_boxes, mut by_text) = (0, 0);
    for m in &sample {
        let id = f
            .project
            .occurrences(&m.key.to_string())
            .unwrap()
            .into_iter()
            .find(|o| o.document_id == m.document_id && o.locator == m.locator)
            .unwrap()
            .mention_id;
        let view = f.project.source_context(id).unwrap().unwrap();
        // The stored text's highlight is the mention, on every kind of view.
        let hit = highlighted(&view).replace('\n', "");
        let raw = m.raw.replace('\n', "");
        if hit != raw {
            wrong.push(format!(
                "doc {} {:?}: highlighted {hit:?}, want {raw:?}",
                m.document_id, m.locator
            ));
            continue;
        }
        if m.locator.page().is_some() && f.pages.values().any(|d| *d == m.document_id) {
            match page_highlight_ok(f, m, id) {
                Ok(true) => on_boxes += 1,
                Ok(false) => by_text += 1,
                Err(e) => wrong.push(e),
            }
        } else {
            by_text += 1;
        }
    }
    let right = sample.len() - wrong.len();
    eprintln!(
        "NFR-QA-12: {right} of {} right ({} distinct of {} mentions; {on_boxes} on ground-truth \
         boxes, {by_text} by text); wrong: {wrong:#?}",
        sample.len(),
        distinct.len(),
        f.mentions.len()
    );
    assert!(right >= 198, "{wrong:#?}");
}
