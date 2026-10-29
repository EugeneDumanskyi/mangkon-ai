//! The Tesseract adapter (ADR-0014) over `mk-tesseract-sys`: words with pixel boxes and 0–1
//! confidences, grouped by Tesseract's text lines for the shared layout.

use std::path::Path;

use mk_kernel::Rect;
use mk_tesseract_sys::Tesseract;

use crate::image::GrayImage;
use crate::ocr::{OcrEngine, OcrError, OcrErrorKind, OcrLine, OcrWord, missing_language};

/// One `TessBaseAPI`, loaded for one set of languages (ADR-0014: one per worker).
pub struct TesseractOcr {
    api: Tesseract,
}

impl TesseractOcr {
    /// `tessdata` is the folder holding `<language>.traineddata` for every language in
    /// `languages` (`eng+deu`). A missing file is an [`OcrErrorKind::Language`] error naming it.
    pub fn new(tessdata: &Path, languages: &str) -> Result<Self, OcrError> {
        if let Some(l) = missing_language(tessdata, languages) {
            return Err(OcrError::new(OcrErrorKind::Language, l));
        }
        let api = Tesseract::new(tessdata, languages).map_err(|e| match e {
            mk_tesseract_sys::Error::Init { language } => {
                OcrError::new(OcrErrorKind::Language, language)
            }
            e => OcrError::new(OcrErrorKind::Other, e.to_string()),
        })?;
        Ok(Self { api })
    }

    /// The linked engine version, for example `5.5.2`.
    pub fn engine_version() -> String {
        mk_tesseract_sys::version()
    }
}

impl OcrEngine for TesseractOcr {
    fn recognize(&mut self, image: &GrayImage, dpi: u32) -> Result<Vec<OcrLine>, OcrError> {
        let page = self
            .api
            .recognize_grey(&image.pixels, image.width, image.height, dpi)
            .map_err(|e| OcrError::new(OcrErrorKind::Other, e.to_string()))?;
        let mut lines: Vec<OcrLine> = Vec::new();
        let mut current = None;
        for w in page.words {
            if w.text.trim().is_empty() {
                continue;
            }
            let Some(rect) = Rect::new(
                w.left.max(0) as f32,
                w.top.max(0) as f32,
                w.right.max(0) as f32,
                w.bottom.max(0) as f32,
            ) else {
                continue;
            };
            if current != Some(w.line) {
                lines.push(OcrLine::default());
                current = Some(w.line);
            }
            lines.last_mut().expect("pushed").words.push(OcrWord {
                text: w.text,
                rect,
                // Tesseract gives 0–100 (ADR-0014).
                confidence: Some((w.confidence / 100.0).clamp(0.0, 1.0)),
            });
        }
        Ok(lines)
    }
}
