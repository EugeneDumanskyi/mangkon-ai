//! Candidate B: pdf_oxide (pure Rust). Character boxes from `extract_chars`, render from
//! `rendering::PageRenderer`.

use std::path::Path;

use pdf_oxide::PdfDocument;
use pdf_oxide::error::Error;
use pdf_oxide::rendering::{PageRenderer, RenderOptions};

use crate::engine::{
    GrayImage, PageInfo, PdfDoc, PdfEngine, PdfError, PdfErrorKind, RawChar, read,
};

pub struct Oxide;

fn err(e: Error) -> PdfError {
    let kind = match &e {
        Error::EncryptedPdf => PdfErrorKind::Encrypted,
        Error::InvalidHeader(_)
        | Error::InvalidXref
        | Error::ParseError { .. }
        | Error::UnexpectedEof
        | Error::InvalidPdf(_)
        | Error::ObjectNotFound(..) => PdfErrorKind::Corrupt,
        Error::Unsupported(_) | Error::UnsupportedFilter(_) | Error::UnsupportedVersion(_) => {
            PdfErrorKind::Unsupported
        }
        _ => PdfErrorKind::Other,
    };
    PdfError::new(kind, e.to_string())
}

impl PdfEngine for Oxide {
    fn open<'e>(&'e self, path: &Path) -> Result<Box<dyn PdfDoc + 'e>, PdfError> {
        let doc = PdfDocument::from_bytes(read(path)?).map_err(err)?;
        if doc.is_encrypted() && !doc.is_authenticated() {
            return Err(PdfError::new(
                PdfErrorKind::Encrypted,
                "encrypted; the empty password does not open it",
            ));
        }
        let pages = doc.page_count().map_err(err)?;
        Ok(Box::new(OxideDoc { doc, pages }))
    }
}

struct OxideDoc {
    doc: PdfDocument,
    pages: usize,
}

impl OxideDoc {
    fn media_box(&self, page: usize) -> Result<(f32, f32, f32, f32), PdfError> {
        let (a, b, c, d) = self.doc.get_page_media_box(page).map_err(err)?;
        Ok((a.min(c), b.min(d), a.max(c), b.max(d)))
    }
}

impl PdfDoc for OxideDoc {
    fn page_count(&self) -> usize {
        self.pages
    }

    fn page_info(&self, page: usize) -> Result<PageInfo, PdfError> {
        let (x0, y0, x1, y1) = self.media_box(page)?;
        let rotate = self.doc.get_page_rotation(page).map_err(err)?;
        Ok(PageInfo {
            width: x1 - x0,
            height: y1 - y0,
            rotate: rotate.rem_euclid(360) as u32,
        })
    }

    fn chars(&self, page: usize) -> Result<Vec<RawChar>, PdfError> {
        let (x0, _, _, y1) = self.media_box(page)?;
        let chars = self.doc.extract_chars(page).map_err(err)?;
        // `bbox` is in PDF user space (y up), `y` at the bottom edge.
        Ok(chars
            .into_iter()
            .filter_map(|c| {
                let b = c.bbox;
                let rect = mk_kernel::Rect::new(
                    b.x - x0,
                    y1 - (b.y + b.height),
                    b.x + b.width - x0,
                    y1 - b.y,
                )?;
                Some(RawChar { ch: c.char, rect })
            })
            .collect())
    }

    fn render(&self, page: usize, scale: f32) -> Result<GrayImage, PdfError> {
        let opts = RenderOptions::with_dpi((scale * 72.0).round() as u32).as_raw();
        let img = PageRenderer::new(opts)
            .render_page(&self.doc, page)
            .map_err(err)?;
        Ok(GrayImage::from_rgba(
            img.width, img.height, &img.data, false,
        ))
    }
}
