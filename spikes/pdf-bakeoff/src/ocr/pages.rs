//! Which TD-1 pages go to OCR, and their page images: image files at their own resolution,
//! scanned PDF pages and mixed-PDF pages with under 20 characters of text (US-02.2 AC1)
//! rendered by hayro at 300 DPI.

use std::collections::HashMap;

use mk_bench::{FileKind, PageText, ScanTier, Unit};

use crate::corpus::Corpus;
use crate::engine::{GrayImage, PdfEngine};
use crate::engines::hayro::Hayro;
use crate::layout::layout;
use crate::ocr::engine::OcrError;
use crate::ocr::image::{self, OCR_DPI};

/// US-02.2 AC1: a PDF page with fewer extractable characters than this is OCR'd. ASSUMPTION.
pub const MIN_TEXT_CHARS: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Image,
    Pdf,
}

#[derive(Clone, Debug)]
pub struct OcrPage {
    /// Path relative to the TD-1 root.
    pub file: String,
    pub document: String,
    /// 1-based.
    pub page: u32,
    pub source: Source,
    pub tier: ScanTier,
    /// Resolution of the ground-truth pixel boxes.
    pub gt_dpi: u32,
    /// Ground-truth page size in pixels at `gt_dpi`.
    pub gt_size: (f32, f32),
    pub reference: String,
}

impl OcrPage {
    /// Ground-truth pixels → pixels of the image that is OCR'd.
    pub fn gt_scale(&self) -> f32 {
        match self.source {
            Source::Image => 1.0,
            Source::Pdf => OCR_DPI / self.gt_dpi as f32,
        }
    }

    pub fn key(&self) -> (String, u32) {
        (self.file.clone(), self.page)
    }

    /// Clean tier and A4 (the NFR-PERF-21 page): 300 DPI, 2,480 × 3,508 px give or take one.
    pub fn clean_a4(&self) -> bool {
        self.tier == ScanTier::Clean && (self.gt_size.0 - 2479.0).abs() <= 2.0
    }
}

/// Every TD-1 page that goes to OCR, with its reference text.
pub fn ocr_pages(corpus: &Corpus) -> Vec<OcrPage> {
    let refs: HashMap<(&str, u32), &PageText> = corpus
        .gt
        .page_text
        .iter()
        .filter(|p| p.unit == Unit::Px)
        .map(|p| ((p.document.as_str(), p.page), p))
        .collect();
    let hayro = Hayro::new();
    let mut out = Vec::new();
    for f in &corpus.gt.files {
        let source = match f.kind {
            FileKind::Image => Source::Image,
            FileKind::PdfScan | FileKind::PdfMixed => Source::Pdf,
            _ => continue,
        };
        let Some(doc) = corpus
            .gt
            .documents
            .iter()
            .find(|d| d.file == f.path && d.parent.is_none())
        else {
            continue;
        };
        let pages: Vec<u32> = match f.kind {
            FileKind::PdfMixed => {
                let Ok(pdf) = hayro.open(&corpus.path(&f.path)) else {
                    continue;
                };
                (0..pdf.page_count())
                    .filter(|&i| {
                        let chars = pdf
                            .page_info(i)
                            .and_then(|info| Ok(layout(&pdf.chars(i)?, &info).text))
                            .map(|t| t.chars().filter(|c| !c.is_whitespace()).count())
                            .unwrap_or(0);
                        chars < MIN_TEXT_CHARS
                    })
                    .map(|i| i as u32 + 1)
                    .collect()
            }
            _ => (1..=f.pages.unwrap_or(1)).collect(),
        };
        for page in pages {
            let Some(r) = refs.get(&(doc.id.as_str(), page)) else {
                continue;
            };
            out.push(OcrPage {
                file: f.path.clone(),
                document: doc.id.clone(),
                page,
                source,
                tier: f.scan_tier.unwrap_or(ScanTier::Clean),
                gt_dpi: r.dpi.or(f.dpi).unwrap_or(300),
                gt_size: (r.width as f32, r.height as f32),
                reference: r.text.clone(),
            });
        }
    }
    out
}

/// The page image that is OCR'd: decode the image file, or render the PDF page.
pub fn load(corpus: &Corpus, page: &OcrPage) -> Result<GrayImage, OcrError> {
    image::load_any(&corpus.path(&page.file), page.page as usize)
}
