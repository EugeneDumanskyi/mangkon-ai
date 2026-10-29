//! The shared image path (ADR-0014), engine-independent: format sniffing by magic bytes, the
//! image guard, decoding PNG, JPEG, TIFF (every page) and WEBP to 8-bit grey, and rendering a
//! scanned PDF page at 300 DPI for OCR.

use std::io::Cursor;

use image::{ImageError, ImageFormat, ImageReader, Limits};

use crate::ocr::{OcrError, OcrErrorKind};
use crate::pdf::engine::{PdfDoc, PdfError};

/// The image guard, checked from the header before any pixel is decoded. ASSUMPTION (spike S0
/// values): 100 MP covers an A3 page at 600 DPI (≈ 70 MP); 512 MiB is half the 1 GB worker cap.
pub const MAX_PIXELS: u64 = 100_000_000;
pub const MAX_ALLOC: u64 = 512 << 20;
/// Scanned PDF pages are rendered at this resolution for OCR.
pub const OCR_DPI: f32 = 300.0;
/// Images with no resolution of their own are OCR'd as this. ASSUMPTION: scans are 300 DPI.
pub const IMAGE_DPI: u32 = 300;

/// 8-bit grey image, row-major, top-left origin.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl GrayImage {
    /// Luma of premultiplied RGBA over white.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Self {
        let pixels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = p[3] as u32;
                let over = |c: u8| c as u32 + (255 - a);
                ((over(p[0]) * 299 + over(p[1]) * 587 + over(p[2]) * 114) / 1000).min(255) as u8
            })
            .collect();
        Self {
            width,
            height,
            pixels,
        }
    }

    /// PNG bytes of the image, for the page view.
    pub fn to_png(&self) -> Result<Vec<u8>, OcrError> {
        use image::ImageEncoder;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let mut out = Vec::new();
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
            .write_image(
                &self.pixels,
                self.width,
                self.height,
                image::ExtendedColorType::L8,
            )
            .map_err(|e| OcrError::new(OcrErrorKind::Other, e.to_string()))?;
        Ok(out)
    }
}

/// What the bytes of a PDF or image content are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Png,
    Jpeg,
    Tiff,
    Webp,
}

/// The kind of `bytes` by their magic bytes, whatever the file name says (TD-3
/// `pdf-named.png`, `jpeg-named.pdf`). `None` for anything else, `text-named.pdf` included.
pub fn sniff(bytes: &[u8]) -> Option<Kind> {
    // A PDF header may follow some junk; readers look in the first 1 KB.
    let head = &bytes[..bytes.len().min(1024)];
    if head.windows(5).any(|w| w == b"%PDF-") {
        return Some(Kind::Pdf);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Kind::Jpeg);
    }
    if bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*")
        || bytes.starts_with(b"II+\0")
        || bytes.starts_with(b"MM\0+")
    {
        return Some(Kind::Tiff);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(Kind::Webp);
    }
    None
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

/// The pages of an image: one, or one per TIFF directory (`limit + 1` at most, so a caller can
/// tell a file over its page limit).
pub fn page_count(bytes: &[u8], kind: Kind, limit: usize) -> Result<usize, OcrError> {
    match kind {
        Kind::Tiff => Ok(tiff_pages(bytes, limit + 1)?.len()),
        Kind::Pdf => Err(not_an_image()),
        _ => Ok(1),
    }
}

fn not_an_image() -> OcrError {
    OcrError::new(OcrErrorKind::Unsupported, "a PDF, not an image")
}

fn tiff_error(e: tiff::TiffError) -> OcrError {
    let kind = match e {
        tiff::TiffError::LimitsExceeded => OcrErrorKind::TooLarge,
        tiff::TiffError::UnsupportedError(_) => OcrErrorKind::Unsupported,
        _ => OcrErrorKind::Corrupt,
    };
    OcrError::new(kind, e.to_string())
}

/// The directory offset of each TIFF page, at most `max` of them. A directory chain that loops
/// is an error (the decoder checks).
fn tiff_pages(bytes: &[u8], max: usize) -> Result<Vec<u64>, OcrError> {
    let mut d = tiff::decoder::Decoder::new(Cursor::new(bytes)).map_err(tiff_error)?;
    let mut out = Vec::new();
    loop {
        if let Some(p) = d.ifd_pointer() {
            out.push(p.0);
        }
        if out.len() >= max || !d.more_images() {
            return Ok(out);
        }
        d.next_image().map_err(tiff_error)?;
    }
}

/// A copy of a TIFF whose first-directory pointer names page `page`'s directory, so a decoder
/// that reads only the first image reads that page.
fn tiff_page(bytes: &[u8], page: usize) -> Result<Vec<u8>, OcrError> {
    let offsets = tiff_pages(bytes, page + 1)?;
    let Some(&offset) = offsets.get(page) else {
        return Err(OcrError::new(
            OcrErrorKind::Corrupt,
            format!("no page {}", page + 1),
        ));
    };
    let mut copy = bytes.to_vec();
    let little = bytes.starts_with(b"II");
    let big = bytes.get(2..4) == Some(if little { &[0x2B, 0] } else { &[0, 0x2B] });
    if big {
        let v = if little {
            offset.to_le_bytes()
        } else {
            offset.to_be_bytes()
        };
        copy[8..16].copy_from_slice(&v);
    } else {
        let o = u32::try_from(offset)
            .map_err(|_| OcrError::new(OcrErrorKind::Corrupt, "TIFF offset out of range"))?;
        let v = if little {
            o.to_le_bytes()
        } else {
            o.to_be_bytes()
        };
        copy[4..8].copy_from_slice(&v);
    }
    Ok(copy)
}

/// Decodes page `page` (0-based) of a PNG, JPEG, TIFF or WEBP to 8-bit grey. The header is read
/// first and the image refused if it is over the guard; decoding runs under an allocation cap.
pub fn decode(bytes: &[u8], kind: Kind, page: usize) -> Result<GrayImage, OcrError> {
    let fmt = match kind {
        Kind::Png => ImageFormat::Png,
        Kind::Jpeg => ImageFormat::Jpeg,
        Kind::Tiff => ImageFormat::Tiff,
        Kind::Webp => ImageFormat::WebP,
        Kind::Pdf => return Err(not_an_image()),
    };
    // The JPEG decoder fills a cut-off image with grey instead of failing; OCR of half a page
    // would pass as the whole page (TD-3 `truncated.jpg`).
    if kind == Kind::Jpeg && !bytes.windows(2).any(|w| w == [0xFF, 0xD9]) {
        return Err(OcrError::new(
            OcrErrorKind::Corrupt,
            "JPEG without an end-of-image marker",
        ));
    }
    let owned;
    let bytes = if kind == Kind::Tiff && page > 0 {
        owned = tiff_page(bytes, page)?;
        &owned[..]
    } else if page > 0 {
        return Err(OcrError::new(
            OcrErrorKind::Corrupt,
            format!("no page {}", page + 1),
        ));
    } else {
        bytes
    };
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

/// Renders a PDF page (0-based) at [`OCR_DPI`] for OCR, after the guard on the page size at that
/// resolution and on every image the page declares.
pub fn render_for_ocr(doc: &dyn PdfDoc, page: usize) -> Result<GrayImage, OcrError> {
    let pdf = |e: PdfError| OcrError::new(OcrErrorKind::Corrupt, e.to_string());
    let info = doc.page_info(page).map_err(pdf)?;
    let scale = OCR_DPI / 72.0;
    let (w, h) = info.display_size();
    guard(
        (w * scale).ceil() as u64,
        (h * scale).ceil() as u64,
        "the page at 300 DPI",
    )?;
    if let Some((iw, ih)) = doc.largest_image(page).map_err(pdf)? {
        guard(iw, ih, "an image on the page")?;
    }
    doc.render(page, scale).map_err(pdf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_decide() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Kind::Jpeg));
        assert_eq!(sniff(b"%PDF-1.7"), Some(Kind::Pdf));
        assert_eq!(sniff(b"junk\n%PDF-1.4"), Some(Kind::Pdf));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Kind::Webp));
        assert_eq!(sniff(b"II*\0"), Some(Kind::Tiff));
        assert_eq!(sniff(b"hello"), None);
        assert_eq!(sniff(b""), None);
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

    fn grey(w: u32, h: u32, v: u8) -> image::GrayImage {
        image::GrayImage::from_pixel(w, h, image::Luma([v]))
    }

    /// US-02.2 AC2: every page of a multi-page TIFF decodes, each its own size.
    #[test]
    fn multi_page_tiff_pages() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = tiff::encoder::TiffEncoder::new(&mut buf).unwrap();
            for (w, h, v) in [(10u32, 20u32, 10u8), (30, 15, 200)] {
                let img = grey(w, h, v);
                enc.write_image::<tiff::encoder::colortype::Gray8>(w, h, img.as_raw())
                    .unwrap();
            }
        }
        let bytes = buf.into_inner();
        assert_eq!(page_count(&bytes, Kind::Tiff, 10).unwrap(), 2);
        let p0 = decode(&bytes, Kind::Tiff, 0).unwrap();
        let p1 = decode(&bytes, Kind::Tiff, 1).unwrap();
        assert_eq!((p0.width, p0.height, p0.pixels[0]), (10, 20, 10));
        assert_eq!((p1.width, p1.height, p1.pixels[0]), (30, 15, 200));
        assert!(decode(&bytes, Kind::Tiff, 2).is_err());
    }

    #[test]
    fn png_round_trip() {
        let img = GrayImage {
            width: 3,
            height: 2,
            pixels: vec![0, 50, 100, 150, 200, 250],
        };
        let png = img.to_png().unwrap();
        assert_eq!(sniff(&png), Some(Kind::Png));
        assert_eq!(decode(&png, Kind::Png, 0).unwrap(), img);
    }
}
