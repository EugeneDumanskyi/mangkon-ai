//! The shared image path, engine-independent: format sniffing, the app's size guard, decoding to
//! 8-bit grey, and rasterising scanned PDF pages with hayro at 300 DPI.

use std::io::Cursor;
use std::path::Path;

use image::{ImageError, ImageFormat, ImageReader, Limits};

use crate::engine::{GrayImage, PdfDoc};
use crate::ocr::engine::{OcrError, OcrErrorKind};

/// App guard, checked from the header before any pixel is decoded. ASSUMPTION (spike values):
/// 100 MP covers an A3 page at 600 DPI (≈ 70 MP); 512 MiB is half the 1 GiB worker cap.
pub const MAX_PIXELS: u64 = 100_000_000;
pub const MAX_ALLOC: u64 = 512 << 20;
/// Scanned PDF pages are rendered at this resolution for OCR.
pub const OCR_DPI: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Tiff,
    Pdf,
}

/// Magic bytes win over the extension (TD-3 ASSUMPTION); the extension decides only when the
/// bytes match no known format.
pub fn sniff(bytes: &[u8], path: &Path) -> Option<Format> {
    if bytes.starts_with(b"%PDF") {
        return Some(Format::Pdf);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Format::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(Format::Tiff);
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some(Format::Pdf),
        "png" => Some(Format::Png),
        "jpg" | "jpeg" => Some(Format::Jpeg),
        "tif" | "tiff" => Some(Format::Tiff),
        _ => None,
    }
}

fn kind_of(e: &ImageError) -> OcrErrorKind {
    match e {
        ImageError::Limits(_) => OcrErrorKind::TooLarge,
        ImageError::Unsupported(_) => OcrErrorKind::Unsupported,
        _ => OcrErrorKind::Corrupt,
    }
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_alloc = Some(MAX_ALLOC);
    l
}

/// Checks a declared size against the guard.
pub fn guard(width: u64, height: u64, what: &str) -> Result<(), OcrError> {
    if width.saturating_mul(height) > MAX_PIXELS {
        return Err(OcrError::new(
            OcrErrorKind::TooLarge,
            format!(
                "{what} is {width} x {height} pixels, over the {} MP limit",
                MAX_PIXELS / 1_000_000
            ),
        ));
    }
    Ok(())
}

/// Decodes a PNG, JPEG or TIFF (first page) to 8-bit grey. The header is read first and the
/// image refused if it is over the guard; decoding itself runs under an allocation cap.
pub fn decode(bytes: &[u8], format: Format) -> Result<GrayImage, OcrError> {
    let fmt = match format {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Tiff => ImageFormat::Tiff,
        Format::Pdf => {
            return Err(OcrError::new(
                OcrErrorKind::Unsupported,
                "a PDF, not an image",
            ));
        }
    };
    // The JPEG decoder fills a cut-off image with grey instead of failing; OCR of half a page
    // would pass as the whole page (TD-3 `truncated.jpg`, no partial decode).
    if format == Format::Jpeg && !bytes.windows(2).any(|w| w == [0xFF, 0xD9]) {
        return Err(OcrError::new(
            OcrErrorKind::Corrupt,
            "JPEG without an end-of-image marker",
        ));
    }
    let err = |e: ImageError| OcrError::new(kind_of(&e), e.to_string());
    let mut header = ImageReader::with_format(Cursor::new(bytes), fmt);
    header.limits(limits());
    let (w, h) = header.into_dimensions().map_err(err)?;
    guard(w.into(), h.into(), "the image")?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), fmt);
    reader.limits(limits());
    let luma = reader.decode().map_err(err)?.into_luma8();
    let (width, height) = luma.dimensions();
    Ok(GrayImage {
        width,
        height,
        pixels: luma.into_raw(),
    })
}

/// Renders a PDF page at `OCR_DPI` for OCR, after the guard on the page size at that resolution
/// and on every image the page declares.
pub fn render_pdf_page(doc: &dyn PdfDoc, page: usize) -> Result<GrayImage, OcrError> {
    let pdf = |e: crate::engine::PdfError| OcrError::new(OcrErrorKind::Corrupt, e.to_string());
    let info = doc.page_info(page).map_err(pdf)?;
    let scale = OCR_DPI / 72.0;
    let (w, h) = (
        (info.width * scale).ceil() as u64,
        (info.height * scale).ceil() as u64,
    );
    guard(w, h, "the page at 300 DPI")?;
    if let Some((iw, ih)) = doc.largest_image(page).map_err(pdf)? {
        guard(iw, ih, "an image on the page")?;
    }
    doc.render(page, scale).map_err(pdf)
}

/// Any OCR input on disk: an image, or page `page` (1-based) of a PDF rendered by hayro.
pub fn load_any(path: &Path, page: usize) -> Result<GrayImage, OcrError> {
    let bytes =
        std::fs::read(path).map_err(|e| OcrError::new(OcrErrorKind::Other, e.to_string()))?;
    match sniff(&bytes, path) {
        Some(Format::Pdf) => {
            let hayro = crate::engines::hayro::Hayro::new();
            let doc = crate::engine::PdfEngine::open(&hayro, path)
                .map_err(|e| OcrError::new(OcrErrorKind::Corrupt, e.to_string()))?;
            if page == 0 || page > doc.page_count() {
                return Err(OcrError::new(
                    OcrErrorKind::Corrupt,
                    format!("no page {page}"),
                ));
            }
            render_pdf_page(&*doc, page - 1)
        }
        Some(f) => decode(&bytes, f),
        None => Err(OcrError::new(
            OcrErrorKind::Corrupt,
            "not a known image format",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_win_over_extension() {
        let p = Path::new("x.pdf");
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0], p), Some(Format::Jpeg));
        assert_eq!(sniff(b"%PDF-1.7", Path::new("x.png")), Some(Format::Pdf));
        assert_eq!(sniff(b"", Path::new("x.png")), Some(Format::Png));
        assert_eq!(sniff(b"hello", Path::new("x.txt")), None);
    }

    #[test]
    fn guard_refuses_over_cap() {
        assert!(guard(10_000, 10_000, "x").is_ok());
        assert_eq!(
            guard(100_000, 100_000, "x").unwrap_err().kind,
            OcrErrorKind::TooLarge
        );
        assert_eq!(
            guard(u64::MAX, 2, "x").unwrap_err().kind,
            OcrErrorKind::TooLarge
        );
    }
}
