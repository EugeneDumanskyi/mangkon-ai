//! Golden checks of the PDF and OCR parsers on TD-1 (US-02.1 AC1, US-02.2 AC1–AC3, NFR-QA-07,
//! ADR-0014). Every file is parsed once, with OCR in English, and shared by the tests. A mention
//! is found when its raw text is on its page, whitespace ignored (a mention may wrap). Skipped
//! without `testdata/` unless `MK_REQUIRE_TESTDATA=1`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;

use mk_bench::{DocPart, FileKind, GroundTruth, Location, ScanTier, Unit};
use mk_kernel::Locator;
use mk_parsers::{Engines, Format, OcrConfig, ParseError, ParsedContent, ParsedDocument};

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

fn is_paged(kind: FileKind) -> bool {
    matches!(
        kind,
        FileKind::PdfText
            | FileKind::PdfScan
            | FileKind::PdfMixed
            | FileKind::PdfOcrLayer
            | FileKind::Image
    )
}

struct Corpus {
    gt: GroundTruth,
    /// Every PDF and image file by path, and the emails that carry PDF attachments.
    parsed: HashMap<String, Result<ParsedContent, ParseError>>,
}

/// TD-1 parsed once for all tests in this file. `None` without test data.
fn corpus() -> Option<&'static Corpus> {
    static CORPUS: OnceLock<Option<Corpus>> = OnceLock::new();
    CORPUS
        .get_or_init(|| {
            let td1 = td1()?;
            let gt = mk_bench::load(&td1.join("ground-truth.json")).unwrap();
            let tessdata = tempfile::tempdir().unwrap().keep();
            mk_parsers::ocr::install_english(&tessdata).unwrap();
            let mut engines = Engines::new(Some(OcrConfig {
                tessdata,
                languages: "eng".into(),
            }));
            let with_attachments: Vec<&str> = gt
                .documents
                .iter()
                .filter(|d| d.parent.is_some())
                .map(|d| d.file.as_str())
                .collect();
            let mut parsed = HashMap::new();
            for f in &gt.files {
                let format = if is_paged(f.kind) {
                    Format::from_extension(f.path.rsplit('.').next().unwrap()).unwrap()
                } else if with_attachments.contains(&f.path.as_str()) {
                    match f.kind {
                        FileKind::Eml => Format::Eml,
                        _ => Format::Mbox,
                    }
                } else {
                    continue;
                };
                let bytes = std::fs::read(td1.join(&f.path)).unwrap();
                parsed.insert(
                    f.path.clone(),
                    mk_parsers::parse_with(format, &bytes, &mut engines),
                );
            }
            Some(Corpus { gt, parsed })
        })
        .as_ref()
}

impl Corpus {
    fn file(&self, path: &str) -> &ParsedContent {
        self.parsed[path]
            .as_ref()
            .unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// The parsed document a ground-truth document id is.
    fn document(&self, id: &str) -> &ParsedDocument {
        let d = self.gt.documents.iter().find(|d| d.id == id).unwrap();
        let parsed = self.file(&d.file);
        match &d.part {
            DocPart::Attachment { filename, .. } => parsed
                .documents
                .iter()
                .find(|p| p.parent.is_some() && p.position.ends_with(filename.as_str()))
                .unwrap_or_else(|| panic!("{}: no attachment {filename}", d.file)),
            _ => &parsed.documents[0],
        }
    }
}

/// The text of page `page` and whether it was OCR'd.
fn page(doc: &ParsedDocument, page: u32) -> Option<(&str, bool)> {
    doc.segments.iter().find_map(|s| match &s.locator {
        Locator::PdfText { page: p, .. } if p.get() == page => Some((s.text.as_str(), false)),
        Locator::OcrText { page: p, .. } if p.get() == page => Some((s.text.as_str(), true)),
        _ => None,
    })
}

/// Whether `raw` is in `text`, whitespace ignored on both sides. The stored text keeps a hyphen
/// a line break added (`sr-\nphlx`), as the PDF draws it; the ground truth's raw text drops it,
/// so a hyphen before a line break may be skipped too.
fn contains(text: &str, raw: &str) -> bool {
    let squeeze = |t: &str| -> String { t.chars().filter(|c| !c.is_whitespace()).collect() };
    let needle = squeeze(raw);
    !needle.is_empty()
        && (squeeze(text).contains(&needle) || squeeze(&text.replace("-\n", "")).contains(&needle))
}

/// Pages where the shared layout interleaves two columns, so a wrapped URL is split by a line
/// of the other column. Known since M3b (`11-ingestion.md` §6); a layout change is measured
/// against them.
const INTERLEAVED: &[(&str, u32)] = &[("p-fr-2025-03561", 5)];

/// US-02.1 AC1: each page of a text PDF is a `PdfText` segment, and every ground-truth
/// `pdf_text` and `pdf_page` mention is on its page; attachments included (US-02.6 AC4).
#[test]
fn us_02_1_ac1_pdf_page_segments() {
    let Some(c) = corpus() else { return };
    let mut known = 0;
    let mut checked = BTreeMap::<&str, usize>::new();
    let mut missed = Vec::new();
    for m in &c.gt.mentions {
        let (n, ocr_ok) = match &m.location {
            Location::PdfText { page, .. } => (*page, false),
            Location::PdfPage { page } => (*page, true),
            _ => continue,
        };
        let doc = c.document(&m.document);
        assert_eq!(doc.kind, Format::Pdf, "{}", m.document);
        let Some((text, ocr)) = page(doc, n) else {
            missed.push(format!("{} p{n}: no page segment", m.document));
            continue;
        };
        assert!(ocr_ok || !ocr, "{} p{n}: a text page was OCR'd", m.document);
        if !contains(text, &m.raw) {
            if INTERLEAVED.contains(&(m.document.as_str(), n)) {
                known += 1;
            } else {
                missed.push(format!("{} p{n}: {:?}", m.document, m.raw));
            }
        }
        *checked.entry(m.location.kind_name()).or_default() += 1;
    }
    eprintln!("checked {checked:?}, {known} on interleaved pages");
    assert!(missed.is_empty(), "{} missed: {missed:#?}", missed.len());
    assert!(known <= 4, "{known} missed on interleaved pages");
    assert!(checked["pdf_text"] >= 300 && checked["pdf_page"] >= 150);
    // One segment per page, numbered in order, for every text PDF.
    for f in c.gt.files.iter().filter(|f| f.kind == FileKind::PdfText) {
        let doc = &c.file(&f.path).documents[0];
        let pages: Vec<u32> = doc
            .segments
            .iter()
            .map(|s| s.locator.page().unwrap().get())
            .collect();
        assert!(pages.windows(2).all(|w| w[0] < w[1]), "{}", f.path);
        assert!(
            pages.len() as u32 <= f.pages.unwrap() && pages.len() as u32 + 3 >= f.pages.unwrap(),
            "{}: {} segments for {:?} pages",
            f.path,
            pages.len(),
            f.pages
        );
    }
}

/// US-02.2 AC1: in mixed and scanned PDFs, the pages with under 20 characters of text, and only
/// those, are OCR'd; identifier mentions on clean pages are all found (ADR-0014: 100% clean-tier
/// identifier recall in M0c).
#[test]
fn us_02_2_ac1_mixed_pdf_pages() {
    let Some(c) = corpus() else { return };
    let mut ocr_pages = 0;
    for f in
        c.gt.files
            .iter()
            .filter(|f| matches!(f.kind, FileKind::PdfMixed | FileKind::PdfScan))
    {
        let doc = &c.file(&f.path).documents[0];
        let scanned: Vec<u32> =
            c.gt.mentions
                .iter()
                .filter(|m| {
                    c.gt.documents
                        .iter()
                        .any(|d| d.id == m.document && d.file == f.path)
                })
                .filter_map(|m| match m.location {
                    Location::Scan { page, .. } => Some(page),
                    _ => None,
                })
                .collect();
        for s in &doc.segments {
            let n = s.locator.page().unwrap().get();
            let ocr = matches!(s.locator, Locator::OcrText { .. });
            ocr_pages += usize::from(ocr);
            if f.kind == FileKind::PdfScan || scanned.contains(&n) {
                assert!(ocr, "{} p{n}: an image page was not OCR'd", f.path);
            } else if f.kind == FileKind::PdfMixed && !scanned.contains(&n) {
                assert!(!ocr, "{} p{n}: a text page was OCR'd", f.path);
            }
        }
    }
    assert!(ocr_pages >= 7, "{ocr_pages} OCR pages");
    let (found, total) = scan_recall(c, ScanTier::Clean);
    assert_eq!(found, total, "clean-tier scan mentions found");
}

/// Identifier mentions on scans of one tier found on their page: (found, total).
fn scan_recall(c: &Corpus, tier: ScanTier) -> (usize, usize) {
    let (mut found, mut total) = (0, 0);
    for m in &c.gt.mentions {
        let Location::Scan { page: n, .. } = m.location else {
            continue;
        };
        if !matches!(m.subject, mk_bench::Subject::Identifier { .. }) {
            continue;
        }
        let d = c.gt.documents.iter().find(|d| d.id == m.document).unwrap();
        let f = c.gt.files.iter().find(|f| f.path == d.file).unwrap();
        if f.scan_tier != Some(tier) {
            continue;
        }
        total += 1;
        let doc = c.document(&m.document);
        if page(doc, n).is_some_and(|(t, ocr)| ocr && contains(t, &m.raw)) {
            found += 1;
        } else {
            eprintln!("{tier:?} not found: {} p{n} {:?}", d.file, m.raw);
        }
    }
    (found, total)
}

/// US-02.2 AC2: PNG, JPEG and TIFF files are OCR'd, one segment per page (the TD-3 v4
/// multi-page TIFF and WEBP are in `nfr_rel_01_td3_pdf_images_fail_alone`).
#[test]
fn us_02_2_ac2_images() {
    let Some(c) = corpus() else { return };
    let mut by_ext = BTreeMap::<String, usize>::new();
    for f in c.gt.files.iter().filter(|f| f.kind == FileKind::Image) {
        let doc = &c.file(&f.path).documents[0];
        assert_eq!(doc.kind, Format::Image, "{}", f.path);
        assert_eq!(doc.segments.len() as u32, f.pages.unwrap(), "{}", f.path);
        assert!(
            doc.segments
                .iter()
                .all(|s| matches!(s.locator, Locator::OcrText { .. }))
        );
        *by_ext
            .entry(f.path.rsplit('.').next().unwrap().to_string())
            .or_default() += 1;
    }
    assert_eq!(
        by_ext.keys().cloned().collect::<Vec<_>>(),
        ["jpg", "png", "tif"]
    );
    for tier in [ScanTier::Office, ScanTier::Poor] {
        let (found, total) = scan_recall(c, tier);
        eprintln!("{tier:?}: {found}/{total} scan identifier mentions found");
    }
}

/// US-02.2 AC3: every OCR'd word has a box inside its image and a confidence; the poor-tier
/// invoice is marked "Low OCR quality" and no clean-tier page is.
#[test]
fn us_02_2_ac3_word_boxes_confidence() {
    let Some(c) = corpus() else { return };
    let mut words = 0;
    for (path, parsed) in &c.parsed {
        let Ok(parsed) = parsed else { continue };
        for s in parsed.documents.iter().flat_map(|d| &d.segments) {
            let Locator::OcrText { image, .. } = &s.locator else {
                continue;
            };
            let geo = s
                .ocr
                .as_ref()
                .unwrap_or_else(|| panic!("{path}: no geometry"));
            assert_eq!(geo.image(), *image);
            for w in geo.words() {
                assert!(w.confidence.is_some_and(|c| (0.0..=1.0).contains(&c)));
                assert!(w.rect.x1() <= image.width as f32 && w.rect.y1() <= image.height as f32);
                words += 1;
            }
        }
    }
    assert!(words > 1000, "{words} words");
    let low = |path: &str| {
        c.file(path)
            .warnings
            .iter()
            .any(|w| w.ends_with("Low OCR quality"))
    };
    assert!(low("synthetic/scan/invoice-bwl-005-poor.tif"));
    for f in
        c.gt.files
            .iter()
            .filter(|f| f.scan_tier == Some(ScanTier::Clean))
    {
        assert!(!low(&f.path), "{}: clean page marked low", f.path);
    }
}

/// NFR-QA-07: OCR character accuracy on the clean tier ≥ 95% (1 − CER, whitespace-normalised,
/// reading-order neutral; `mk_bench::ocr`).
#[test]
fn nfr_qa_07_ocr_accuracy_clean_tier() {
    let Some(c) = corpus() else { return };
    let mut pages = Vec::new();
    for r in c.gt.page_text.iter().filter(|p| p.unit == Unit::Px) {
        let d = c.gt.documents.iter().find(|d| d.id == r.document).unwrap();
        let f = c.gt.files.iter().find(|f| f.path == d.file).unwrap();
        if f.scan_tier != Some(ScanTier::Clean) {
            continue;
        }
        let got = page(c.document(&r.document), r.page)
            .filter(|(_, ocr)| *ocr)
            .map(|(t, _)| t);
        pages.push(mk_bench::ocr::page_errors(got, &r.text));
    }
    let accuracy = mk_bench::ocr::accuracy(pages.iter().copied());
    eprintln!(
        "NFR-QA-07: {} clean pages, accuracy {:.2}%",
        pages.len(),
        accuracy * 100.0
    );
    assert!(pages.len() >= 6);
    assert!(accuracy >= 0.95, "{accuracy}");
}
