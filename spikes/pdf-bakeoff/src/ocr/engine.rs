//! The `OcrEngine` trait shape (ADR-0002, ADR-0013 rule 2). Written to move into `mk-parsers`
//! in M3 with the winning adapter; the bake-off only adds `by_name` and the feature switch.

use std::fmt;

use mk_kernel::Rect;

use crate::engine::GrayImage;

/// Why an image or page could not be OCR'd. Same pattern as `PdfError`: `kind` is what the
/// product shows a plain reason for (NFR-USE-05), `message` is for logs.
#[derive(Debug, Clone)]
pub struct OcrError {
    pub kind: OcrErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrErrorKind {
    /// The image is over the app's pixel or memory cap; refused before decoding.
    TooLarge,
    /// Not a readable image: no known format, bad header, truncated data.
    Corrupt,
    /// A readable image the engine cannot take.
    Unsupported,
    /// Model or language files missing or unreadable.
    Model,
    Other,
}

impl OcrError {
    pub fn new(kind: OcrErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

/// One recognised word: its text (no whitespace inside), its box in pixels of the input image
/// (top-left origin, y down), and the engine's confidence in `[0, 1]` if it gives one.
#[derive(Debug, Clone)]
pub struct OcrWord {
    pub text: String,
    pub rect: Rect,
    pub confidence: Option<f32>,
}

/// One text line as the engine found it, words left to right. Line order is not trusted: the
/// shared layout orders lines itself.
#[derive(Debug, Clone, Default)]
pub struct OcrLine {
    pub words: Vec<OcrWord>,
}

/// An OCR engine with its models loaded. Models load once in the constructor, since workers are
/// long-lived (ADR-0007), and `recognize` is called once per page image.
pub trait OcrEngine {
    /// `langs` are ISO 639-2 codes of the project's OCR languages (US-02.2 AC4). Engines that
    /// have one fixed model ignore them.
    fn recognize(&self, image: &GrayImage, langs: &[&str]) -> Result<Vec<OcrLine>, OcrError>;
}

/// TD-1 is English, so every run asks for English only.
pub const LANGS: &[&str] = &["eng"];

/// OCR engines compiled into this build.
pub fn available() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "ocrs") {
        v.push("ocrs");
    }
    if cfg!(feature = "tesseract") {
        v.push("tesseract");
    }
    if cfg!(feature = "onnx") {
        v.push("onnx");
    }
    v
}

pub fn by_name(name: &str) -> Result<Box<dyn OcrEngine>, String> {
    match name {
        #[cfg(feature = "ocrs")]
        "ocrs" => crate::ocr::engines::ocrs::Ocrs::new().map(|e| Box::new(e) as _),
        #[cfg(feature = "tesseract")]
        "tesseract" => crate::ocr::engines::tesseract::Tesseract::new().map(|e| Box::new(e) as _),
        #[cfg(feature = "onnx")]
        "onnx" => crate::ocr::engines::onnx::PpOcr::new().map(|e| Box::new(e) as _),
        _ => Err(format!(
            "OCR engine {name:?} not built; available: {:?} (enable it with --features)",
            available()
        )),
    }
}
