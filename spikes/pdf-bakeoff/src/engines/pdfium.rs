//! Candidate C: pdfium (C++), through `pdfium-render`, bound at run time to the prebuilt
//! `libpdfium` that `fetch-pdfium` puts in `spikes/vendor/pdfium/` (or `$PDFIUM_LIB`).

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use crate::engine::{
    GrayImage, PageInfo, PdfDoc, PdfEngine, PdfError, PdfErrorKind, RawChar, read,
};

pub struct Pdfium {
    lib: pdfium_render::prelude::Pdfium,
}

pub fn library_path() -> PathBuf {
    std::env::var_os("PDFIUM_LIB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::spikes_dir()
                .join("vendor/pdfium/lib")
                .join(pdfium_render::prelude::Pdfium::pdfium_platform_library_name())
        })
}

impl Pdfium {
    pub fn new() -> Result<Self, String> {
        let path = library_path();
        let bindings = pdfium_render::prelude::Pdfium::bind_to_library(&path).map_err(|e| {
            format!(
                "cannot load {}: {e:?} (run `pdf-bakeoff fetch-pdfium`)",
                path.display()
            )
        })?;
        Ok(Self {
            lib: pdfium_render::prelude::Pdfium::new(bindings),
        })
    }
}

fn err(e: PdfiumError) -> PdfError {
    let kind = match &e {
        PdfiumError::PdfiumLibraryInternalError(i) => match i {
            PdfiumInternalError::PasswordError | PdfiumInternalError::SecurityError => {
                PdfErrorKind::Encrypted
            }
            PdfiumInternalError::FormatError | PdfiumInternalError::FileError => {
                PdfErrorKind::Corrupt
            }
            _ => PdfErrorKind::Other,
        },
        _ => PdfErrorKind::Other,
    };
    PdfError::new(kind, format!("{e:?}"))
}

impl PdfEngine for Pdfium {
    fn open<'e>(&'e self, path: &Path) -> Result<Box<dyn PdfDoc + 'e>, PdfError> {
        let doc = self
            .lib
            .load_pdf_from_byte_vec(read(path)?, None)
            .map_err(err)?;
        Ok(Box::new(PdfiumDoc { doc }))
    }
}

struct PdfiumDoc<'e> {
    doc: PdfDocument<'e>,
}

impl PdfiumDoc<'_> {
    fn page(&self, page: usize) -> Result<PdfPage<'_>, PdfError> {
        self.doc.pages().get(page as PdfPageIndex).map_err(err)
    }
}

fn media(p: &PdfPage<'_>) -> Result<(f32, f32, f32, f32), PdfError> {
    let r = p.boundaries().media().map_err(err)?.bounds;
    Ok((
        r.left().value,
        r.bottom().value,
        r.right().value,
        r.top().value,
    ))
}

impl PdfDoc for PdfiumDoc<'_> {
    fn page_count(&self) -> usize {
        self.doc.pages().len() as usize
    }

    fn page_info(&self, page: usize) -> Result<PageInfo, PdfError> {
        let p = self.page(page)?;
        let (x0, y0, x1, y1) = media(&p)?;
        let rotate = match p.rotation().map_err(err)? {
            PdfPageRenderRotation::None => 0,
            PdfPageRenderRotation::Degrees90 => 90,
            PdfPageRenderRotation::Degrees180 => 180,
            PdfPageRenderRotation::Degrees270 => 270,
        };
        Ok(PageInfo {
            width: x1 - x0,
            height: y1 - y0,
            rotate,
        })
    }

    fn chars(&self, page: usize) -> Result<Vec<RawChar>, PdfError> {
        let p = self.page(page)?;
        let (x0, _, _, y1) = media(&p)?;
        let text = p.text().map_err(err)?;
        let mut out = Vec::new();
        for c in text.chars().iter() {
            // Spaces and line breaks pdfium adds itself; our layout inserts its own.
            if c.is_generated().unwrap_or(false) {
                continue;
            }
            let Some(mut ch) = c.unicode_char() else {
                continue;
            };
            // pdfium reports a drawn hyphen at a line end as U+0002; store what is printed.
            if ch == '\u{2}' && c.is_hyphen().unwrap_or(false) {
                ch = '-';
            }
            let Ok(b) = c.loose_bounds() else {
                continue;
            };
            if let Some(rect) = mk_kernel::Rect::new(
                b.left().value - x0,
                y1 - b.top().value,
                b.right().value - x0,
                y1 - b.bottom().value,
            ) {
                out.push(RawChar { ch, rect });
            }
        }
        Ok(out)
    }

    fn render(&self, page: usize, scale: f32) -> Result<GrayImage, PdfError> {
        let p = self.page(page)?;
        let cfg = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .set_clear_color(PdfColor::WHITE);
        let bmp = p.render_with_config(&cfg).map_err(err)?;
        Ok(GrayImage::from_rgba(
            bmp.width() as u32,
            bmp.height() as u32,
            &bmp.as_rgba_bytes(),
            false,
        ))
    }
}
