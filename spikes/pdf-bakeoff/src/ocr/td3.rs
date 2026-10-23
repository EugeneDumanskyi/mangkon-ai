//! TD-3 OCR rows: the image files, and the image-only PDF whose image claims 100,000 × 100,000
//! pixels, now scored because the OCR path decodes page images. Each runs alone in a child
//! process (`ocr-td3-one`) under the same deadline and RSS cap as the PDF rows.

use std::path::Path;

use mk_bench::td3::Outcome;
use mk_kernel::ImageSize;
use serde::{Deserialize, Serialize};

use crate::engine::PdfEngine;
use crate::engines::hayro::Hayro;
use crate::ocr::engine::{LANGS, OcrEngine, OcrError, OcrErrorKind};
use crate::ocr::image::{Format, decode, render_pdf_page, sniff};
use crate::ocr::layout::layout;
use crate::td3::{Ended, watch};

/// The TD-3 files that reach the OCR path: image magic bytes, an image extension without PDF
/// magic, or an image-only PDF.
pub const FILES: &[&str] = &[
    "bomb-image-dimensions.pdf",
    "empty.png",
    "jpeg-named.pdf",
    "png-decompression-bomb.png",
    "png-lying-ihdr.png",
    "truncated.jpg",
    "truncated.png",
];

#[derive(Serialize, Deserialize, Default)]
pub struct OcrChildReport {
    pub opened: bool,
    pub error_kind: Option<OcrErrorKind>,
    pub message: String,
    pub pages: usize,
    pub pages_failed: usize,
    pub words: usize,
}

/// Child side: sniff, decode under the guard (or open the PDF and render each page under the
/// guard), OCR, layout.
pub fn one(engine: &dyn OcrEngine, path: &Path) -> OcrChildReport {
    let mut r = OcrChildReport::default();
    let fail = |r: &mut OcrChildReport, e: OcrError| {
        if r.error_kind.is_none() {
            r.error_kind = Some(e.kind);
            r.message = e.message;
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            fail(&mut r, OcrError::new(OcrErrorKind::Other, e.to_string()));
            return r;
        }
    };
    let ocr = |img: crate::engine::GrayImage| -> Result<usize, OcrError> {
        let lines = engine.recognize(&img, LANGS)?;
        let size = ImageSize {
            width: img.width,
            height: img.height,
        };
        Ok(layout(&lines, size).geometry.words().len())
    };
    match sniff(&bytes, path) {
        Some(Format::Pdf) => {
            let hayro = Hayro::new();
            let doc = match hayro.open(path) {
                Ok(d) => d,
                Err(e) => {
                    fail(&mut r, OcrError::new(OcrErrorKind::Corrupt, e.to_string()));
                    return r;
                }
            };
            r.opened = true;
            r.pages = doc.page_count();
            for i in 0..r.pages {
                match render_pdf_page(&*doc, i).and_then(ocr) {
                    Ok(n) => r.words += n,
                    Err(e) => {
                        r.pages_failed += 1;
                        fail(&mut r, e);
                    }
                }
            }
        }
        Some(f) => {
            r.pages = 1;
            match decode(&bytes, f) {
                Ok(img) => {
                    r.opened = true;
                    match ocr(img) {
                        Ok(n) => r.words = n,
                        Err(e) => {
                            r.pages_failed = 1;
                            fail(&mut r, e);
                        }
                    }
                }
                Err(e) => {
                    r.pages_failed = 1;
                    fail(&mut r, e);
                }
            }
        }
        None => fail(
            &mut r,
            OcrError::new(OcrErrorKind::Corrupt, "not a known image format"),
        ),
    }
    r
}

#[derive(Serialize)]
pub struct OcrTd3Result {
    pub file: String,
    pub expected: Outcome,
    pub outcome: Outcome,
    pub ended: Ended,
    pub matches: bool,
    pub report: Option<OcrChildReport>,
    pub detail: String,
    pub secs: f64,
    pub peak_rss_mb: f64,
}

/// Parent side.
pub fn run(engine: &str, root: &Path, out_dir: &Path) -> Vec<OcrTd3Result> {
    let manifest = mk_bench::load_td3(&root.join("expected.json")).expect("TD-3 manifest");
    std::fs::create_dir_all(out_dir).ok();
    FILES
        .iter()
        .filter_map(|name| manifest.files.iter().find(|f| f.path == *name))
        .map(|f| {
            let log = out_dir.join(format!("td3-ocr-{engine}-{}.stderr", f.path));
            let w = watch(
                &[
                    "ocr-td3-one".into(),
                    engine.into(),
                    root.join(&f.path).into(),
                ],
                &log,
            );
            let (ended, report, detail) = match w.ended {
                Ended::Clean => match serde_json::from_str::<OcrChildReport>(w.stdout.trim()) {
                    Ok(r) => (Ended::Clean, Some(r), String::new()),
                    Err(e) => (Ended::Panic, None, format!("no report: {e}")),
                },
                e => (e, None, w.detail),
            };
            let outcome = match &report {
                Some(r) if r.opened && r.pages > 0 && r.pages_failed < r.pages => Outcome::Success,
                _ => Outcome::Fail,
            };
            OcrTd3Result {
                file: f.path.clone(),
                expected: f.expected,
                outcome,
                ended,
                matches: outcome == f.expected,
                report,
                detail,
                secs: w.secs,
                peak_rss_mb: w.peak_rss_mb,
            }
        })
        .collect()
}
