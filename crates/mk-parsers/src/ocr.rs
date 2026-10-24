use std::fmt;
use std::path::Path;

use mk_tesseract_sys::Tesseract;

/// An OCR word with its pixel box and a confidence in `[0, 1]` (INV-ID-03).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWordBox {
    pub text: String,
    /// Pixels of the OCR'd image, top-left origin, y down; `x1` and `y1` are exclusive.
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrPage {
    /// The engine's text. The shared layout step that orders words and builds spans comes in M3.
    pub text: String,
    pub words: Vec<OcrWordBox>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrError(String);

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OcrError {}

impl From<mk_tesseract_sys::Error> for OcrError {
    fn from(e: mk_tesseract_sys::Error) -> Self {
        Self(e.to_string())
    }
}

/// Tesseract, loaded once for one language (ADR-0014: one `TessBaseAPI` per worker).
pub struct TesseractOcr {
    api: Tesseract,
}

impl TesseractOcr {
    /// English from the data file pinned in the build (`tessdata_fast` 4.1.0).
    pub fn english() -> Result<Self, OcrError> {
        Self::new(mk_tesseract_sys::bundled_tessdata_dir(), "eng")
    }

    /// `tessdata` is the folder holding `<language>.traineddata`.
    pub fn new(tessdata: &Path, language: &str) -> Result<Self, OcrError> {
        Ok(Self {
            api: Tesseract::new(tessdata, language)?,
        })
    }

    pub fn engine_version() -> String {
        mk_tesseract_sys::version()
    }

    /// OCRs an 8-bit grey image of `width × height` bytes at `dpi`.
    pub fn recognize_grey(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        dpi: u32,
    ) -> Result<OcrPage, OcrError> {
        let page = self.api.recognize_grey(pixels, width, height, dpi)?;
        let words = page
            .words
            .into_iter()
            .map(|w| OcrWordBox {
                text: w.text,
                x0: w.left.max(0) as u32,
                y0: w.top.max(0) as u32,
                x1: w.right.max(0) as u32,
                y1: w.bottom.max(0) as u32,
                // Tesseract gives 0–100 (ADR-0014).
                confidence: (w.confidence / 100.0).clamp(0.0, 1.0),
            })
            .collect();
        Ok(OcrPage {
            text: page.text,
            words,
        })
    }
}
