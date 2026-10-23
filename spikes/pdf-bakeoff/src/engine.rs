//! The `PdfEngine` trait shape (ADR-0002, ADR-0013 rule 2). Written to move into `mk-parsers`
//! in M2a/M3 with the winning adapter; the bake-off only adds `name` and the feature switch.

use std::fmt;
use std::path::Path;

use mk_kernel::Rect;

/// Why a document or page could not be read. `kind` is what the product shows a plain
/// reason for (NFR-USE-05); `message` is the engine's own text, for logs.
#[derive(Debug, Clone)]
pub struct PdfError {
    pub kind: PdfErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfErrorKind {
    /// Needs a password (US-02.1 AC2).
    Encrypted,
    /// Not a readable PDF: bad header, no objects, no pages.
    Corrupt,
    /// Valid PDF using something the engine cannot do.
    Unsupported,
    Other,
}

impl PdfError {
    pub fn new(kind: PdfErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

/// Unrotated page box size in points, and `/Rotate` (0, 90, 180, 270).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageInfo {
    pub width: f32,
    pub height: f32,
    pub rotate: u32,
}

/// One drawn character, as the engine sees it. `rect` is in PDF points in the **unrotated**
/// MediaBox, top-left origin, y down (the ground-truth convention).
#[derive(Debug, Clone, Copy)]
pub struct RawChar {
    pub ch: char,
    pub rect: Rect,
}

/// 8-bit gray page image in display orientation (`/Rotate` applied), row-major, top-left origin.
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl GrayImage {
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Luma of premultiplied RGBA over white.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8], bgr: bool) -> Self {
        let pixels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let (r, g, b) = if bgr {
                    (p[2], p[1], p[0])
                } else {
                    (p[0], p[1], p[2])
                };
                let a = p[3] as u32;
                let over = |c: u8| c as u32 + (255 - a);
                ((over(r) * 299 + over(g) * 587 + over(b) * 114) / 1000).min(255) as u8
            })
            .collect();
        Self {
            width,
            height,
            pixels,
        }
    }
}

pub trait PdfEngine {
    fn open<'e>(&'e self, path: &Path) -> Result<Box<dyn PdfDoc + 'e>, PdfError>;
}

/// An open document. Pages are 0-based here; `PageNumber` is 1-based.
pub trait PdfDoc {
    fn page_count(&self) -> usize;
    fn page_info(&self, page: usize) -> Result<PageInfo, PdfError>;
    /// Drawn characters in content order, including invisible text (render mode 3).
    fn chars(&self, page: usize) -> Result<Vec<RawChar>, PdfError>;
    /// Renders the page at `scale` pixels per point.
    fn render(&self, page: usize, scale: f32) -> Result<GrayImage, PdfError>;
}

pub fn read(path: &Path) -> Result<Vec<u8>, PdfError> {
    std::fs::read(path).map_err(|e| PdfError::new(PdfErrorKind::Other, e.to_string()))
}

/// Engines compiled into this build.
pub fn available() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "hayro") {
        v.push("hayro");
    }
    if cfg!(feature = "oxide") {
        v.push("oxide");
    }
    if cfg!(feature = "pdfium") {
        v.push("pdfium");
    }
    v
}

pub fn by_name(name: &str) -> Result<Box<dyn PdfEngine>, String> {
    match name {
        #[cfg(feature = "hayro")]
        "hayro" => Ok(Box::new(crate::engines::hayro::Hayro::new())),
        #[cfg(feature = "oxide")]
        "oxide" => Ok(Box::new(crate::engines::oxide::Oxide)),
        #[cfg(feature = "pdfium")]
        "pdfium" => crate::engines::pdfium::Pdfium::new().map(|e| Box::new(e) as _),
        _ => Err(format!(
            "engine {name:?} not built; available: {:?} (enable it with --features)",
            available()
        )),
    }
}
