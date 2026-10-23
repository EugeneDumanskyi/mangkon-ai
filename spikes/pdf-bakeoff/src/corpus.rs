//! TD-1/TD-3 access for the bake-off: which files are text-layer PDFs, where each PDF mention
//! is, and a seeded sample.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mk_bench::{BBox, Document, FileKind, GroundTruth, Location, Mention, PageText, Subject, Unit};

pub struct Corpus {
    pub root: PathBuf,
    pub gt: GroundTruth,
    /// Document id → path of a PDF file on disk holding its bytes. Attachments resolve to the
    /// byte-identical file in `synthetic/pdf/` (same SHA-256).
    doc_file: HashMap<String, String>,
}

/// A text-layer PDF file and the 1-based pages that have a text layer.
pub struct TextPdf {
    pub path: String,
    pub group: &'static str,
    /// `None`: every page.
    pub pages: Option<Vec<u32>>,
}

/// One ground-truth PDF mention with boxes.
#[derive(Clone)]
pub struct Sample {
    pub mention: String,
    pub file: String,
    pub page: u32,
    pub raw: String,
    pub boxes: Vec<BBox>,
    pub identifier: bool,
    pub hard: bool,
}

impl Corpus {
    pub fn load(root: &Path) -> Result<Self, String> {
        let gt = mk_bench::load(&root.join("ground-truth.json")).map_err(|e| e.to_string())?;
        let by_sha: HashMap<&str, &str> = gt
            .files
            .iter()
            .map(|f| (f.sha256.as_str(), f.path.as_str()))
            .collect();
        let mut doc_file = HashMap::new();
        for d in &gt.documents {
            let path = match (&d.sha256, d.parent.is_some()) {
                (Some(sha), true) => by_sha.get(sha.as_str()).map(|p| p.to_string()),
                (_, false) => Some(d.file.clone()),
                _ => None,
            };
            if let Some(p) = path.filter(|p| p.ends_with(".pdf")) {
                doc_file.insert(d.id.clone(), p);
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            gt,
            doc_file,
        })
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn document(&self, id: &str) -> Option<&Document> {
        self.gt.documents.iter().find(|d| d.id == id)
    }

    pub fn file_of(&self, doc: &str) -> Option<&str> {
        self.doc_file.get(doc).map(String::as_str)
    }

    /// Reference page text, by (file, page), for text-layer pages (unit `pt`).
    pub fn page_texts(&self) -> HashMap<(String, u32), &PageText> {
        self.gt
            .page_text
            .iter()
            .filter(|p| p.unit == Unit::Pt)
            .filter_map(|p| Some(((self.file_of(&p.document)?.to_string(), p.page), p)))
            .collect()
    }

    /// Every TD-1 PDF with a text layer. Mixed PDFs count only their text pages.
    pub fn text_pdfs(&self) -> Vec<TextPdf> {
        let texts = self.page_texts();
        let mut out = Vec::new();
        for f in &self.gt.files {
            let group = match (f.kind, f.origin) {
                (FileKind::PdfText | FileKind::PdfOcrLayer, mk_bench::Origin::Public) => "public",
                (FileKind::PdfText, _) => "synthetic text",
                (FileKind::PdfOcrLayer, _) => "synthetic ocr-layer",
                (FileKind::PdfMixed, _) => "synthetic mixed (text pages)",
                _ => continue,
            };
            let pages = (f.kind == FileKind::PdfMixed).then(|| {
                let mut p: Vec<u32> = texts
                    .keys()
                    .filter(|(file, _)| *file == f.path)
                    .map(|(_, n)| *n)
                    .collect();
                p.sort();
                p
            });
            out.push(TextPdf {
                path: f.path.clone(),
                group,
                pages,
            });
        }
        out
    }

    /// Every non-derived mention with a `pdf_text` location in a file on disk.
    pub fn pdf_text_pool(&self) -> Vec<Sample> {
        self.gt
            .mentions
            .iter()
            .filter(|m| m.derived_from.is_none())
            .filter_map(|m| {
                let Location::PdfText { page, boxes } = &m.location else {
                    return None;
                };
                Some(Sample {
                    mention: m.id.clone(),
                    file: self.file_of(&m.document)?.to_string(),
                    page: *page,
                    raw: m.raw.clone(),
                    boxes: boxes.clone(),
                    identifier: matches!(m.subject, Subject::Identifier { .. }),
                    hard: m.difficulty == mk_bench::Difficulty::Hard,
                })
            })
            .collect()
    }

    /// Identifier mentions of public files: page only (`pdf_page`).
    pub fn pdf_page_identifiers(&self) -> Vec<(&Mention, String, u32)> {
        self.gt
            .mentions
            .iter()
            .filter(|m| m.derived_from.is_none() && matches!(m.subject, Subject::Identifier { .. }))
            .filter_map(|m| {
                let Location::PdfPage { page } = m.location else {
                    return None;
                };
                let d = self.document(&m.document)?;
                Some((m, d.file.clone(), page))
            })
            .collect()
    }
}

/// The M0b round-trip sample: every identifier mention, topped up with seeded-random name
/// mentions to `n`.
pub fn sample(pool: &[Sample], n: usize, seed: u64) -> Vec<Sample> {
    let mut out: Vec<Sample> = pool.iter().filter(|s| s.identifier).cloned().collect();
    let mut names: Vec<&Sample> = pool.iter().filter(|s| !s.identifier).collect();
    let mut rng = SplitMix(seed);
    for i in (1..names.len()).rev() {
        names.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    let room = n.saturating_sub(out.len());
    out.extend(names.into_iter().take(room).cloned());
    out.truncate(n);
    out
}

/// SplitMix64: small, seeded, good enough for picking samples.
pub struct SplitMix(pub u64);

impl SplitMix {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
