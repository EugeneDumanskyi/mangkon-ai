//! The `PdfEngine` port (ADR-0002, ADR-0014): what a PDF engine gives the parser and the page
//! view. hayro is the one adapter; the trait keeps a swap possible.

use std::fmt;

use mk_kernel::Rect;

use crate::image::GrayImage;

/// Why a document or page could not be read. `kind` decides the plain reason the product shows
/// (NFR-USE-05); `message` is the engine's own text, for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfError {
    pub kind: PdfErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfErrorKind {
    /// Needs a password (US-02.1 AC2).
    Encrypted,
    /// Not a readable PDF: bad header, no objects, no pages.
    Corrupt,
    /// A valid PDF using something the engine cannot do.
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

impl std::error::Error for PdfError {}

/// Unrotated page box size in points, and `/Rotate` (0, 90, 180, 270).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageInfo {
    pub width: f32,
    pub height: f32,
    pub rotate: u32,
}

impl PageInfo {
    /// Width and height as displayed, `/Rotate` applied.
    pub fn display_size(&self) -> (f32, f32) {
        if self.rotate % 180 == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// One drawn character, as the engine sees it. `rect` is in PDF points in the **unrotated**
/// MediaBox, top-left origin, y down (the ground-truth convention).
#[derive(Debug, Clone, Copy)]
pub struct RawChar {
    pub ch: char,
    pub rect: Rect,
}

/// A PDF engine. Opening takes the whole file: PDF needs random access, and attachments come
/// from memory.
pub trait PdfEngine: Send + Sync {
    fn open<'e>(&'e self, bytes: Vec<u8>) -> Result<Box<dyn PdfDoc + 'e>, PdfError>;
}

/// An open document. Pages are 0-based here; `PageNumber` is 1-based.
pub trait PdfDoc {
    fn page_count(&self) -> usize;
    fn page_info(&self, page: usize) -> Result<PageInfo, PdfError>;
    /// Drawn characters in content order, including invisible text (render mode 3).
    fn chars(&self, page: usize) -> Result<Vec<RawChar>, PdfError>;
    /// Renders the page at `scale` pixels per point, in display orientation.
    fn render(&self, page: usize, scale: f32) -> Result<GrayImage, PdfError>;
    /// Declared pixel size of the largest image the page's resources hold, read without
    /// decoding it: input to the image guard before a page is rendered for OCR. `None` if there
    /// is no image, or the engine cannot tell.
    fn largest_image(&self, _page: usize) -> Result<Option<(u64, u64)>, PdfError> {
        Ok(None)
    }
}
