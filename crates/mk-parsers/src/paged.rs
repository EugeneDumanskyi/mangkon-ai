//! PDFs and images (US-02.1, US-02.2, ADR-0014): one segment per page. A PDF page with at least
//! [`MIN_TEXT_CHARS`] characters of text is a `PdfText` segment; a page with fewer, and every
//! page of an image, is rendered (or decoded) and OCR'd into an `OcrText` segment with its word
//! boxes. The bytes decide what a file is, not its name (TD-3 `pdf-named.png`).
//!
//! [`render_page`] gives the page view its image and, for PDF text, the character boxes of the
//! stored text, from the same engine and layout (ADR-0011).

use std::path::PathBuf;

use mk_kernel::{ImageSize, OcrGeometry, PageGeometry, PageNumber, Span};

use crate::content::{Format, ParseError, ParsedSegment, Place, RowState, Sink, Source};
use crate::image::{self, GrayImage, IMAGE_DPI, Kind, OCR_DPI};
use crate::layout::layout;
use crate::ocr::layout::layout as ocr_layout;
use crate::ocr::{OcrEngine, OcrError, OcrErrorKind, TesseractOcr};
use crate::pdf::engine::{PageInfo, PdfDoc, PdfEngine, PdfError, PdfErrorKind};
use crate::pdf::hayro::Hayro;

/// US-02.2 AC1: a PDF page with fewer non-space characters than this is OCR'd. ASSUMPTION.
pub const MIN_TEXT_CHARS: usize = 20;

/// The most pages a PDF or image may have (NFR-CAP-04/07).
pub const MAX_PAGES: usize = 2_000;

/// US-02.2 AC3: an OCR'd page whose mean word confidence is under this is "Low OCR quality".
/// ASSUMPTION: 60%.
pub const LOW_OCR_QUALITY: f32 = 0.6;

/// Where OCR language files are and which to use (US-02.2 AC4, ADR-0018).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OcrConfig {
    /// The folder holding `<language>.traineddata`.
    pub tessdata: PathBuf,
    /// `+`-joined language codes (`eng+deu`).
    pub languages: String,
}

/// The engines a PDF or image parse uses: hayro, and an OCR engine loaded on first use and kept
/// while the languages stay the same (ADR-0014: one `TessBaseAPI` per worker). Text formats
/// ignore it.
pub struct Engines {
    pdf: Hayro,
    ocr: Ocr,
}

/// The OCR half of [`Engines`], kept apart so it can load while a PDF is open.
struct Ocr {
    config: Option<OcrConfig>,
    engine: Option<Box<dyn OcrEngine>>,
}

impl Ocr {
    fn get(&mut self) -> Result<&mut dyn OcrEngine, ParseError> {
        if self.engine.is_none() {
            let Some(cfg) = &self.config else {
                return Err(ParseError::new(
                    "ocr_unavailable",
                    "OCR is not available for this file",
                ));
            };
            let engine = TesseractOcr::new(&cfg.tessdata, &cfg.languages).map_err(ocr_error)?;
            self.engine = Some(Box::new(engine));
        }
        Ok(self.engine.as_deref_mut().expect("loaded above"))
    }
}

impl Default for Engines {
    /// No OCR: a page that needs it fails the file with "OCR is not available for this file".
    fn default() -> Self {
        Self::new(None)
    }
}

impl Engines {
    pub fn new(config: Option<OcrConfig>) -> Self {
        Engines {
            pdf: Hayro::default(),
            ocr: Ocr {
                config,
                engine: None,
            },
        }
    }

    /// Engines with a given OCR engine (tests).
    pub fn with_ocr_engine(engine: Box<dyn OcrEngine>) -> Self {
        Engines {
            pdf: Hayro::default(),
            ocr: Ocr {
                config: None,
                engine: Some(engine),
            },
        }
    }

    /// Changes the OCR languages or folder. A loaded engine is dropped only if they differ.
    pub fn set_ocr(&mut self, config: Option<OcrConfig>) {
        if self.ocr.config != config {
            self.ocr.config = config;
            self.ocr.engine = None;
        }
    }

    pub fn pdf(&self) -> &dyn PdfEngine {
        &self.pdf
    }
}

/// The plain reason for a PDF engine error (NFR-USE-05).
pub(crate) fn pdf_error(e: PdfError) -> ParseError {
    match e.kind {
        PdfErrorKind::Encrypted => ParseError::new("pdf_encrypted", "PDF is password-protected"),
        PdfErrorKind::Corrupt => ParseError::new("pdf_corrupt", "Not a readable PDF"),
        PdfErrorKind::Unsupported => ParseError::new(
            "pdf_unsupported",
            "This PDF uses a feature that cannot be read",
        ),
        PdfErrorKind::Other => ParseError::new(
            "pdf_unreadable",
            format!("The PDF could not be read: {}", e.message),
        ),
    }
}

/// The plain reason for an image or OCR error (NFR-USE-05).
pub(crate) fn ocr_error(e: OcrError) -> ParseError {
    match e.kind {
        OcrErrorKind::TooLarge => ParseError::new(
            "image_too_large",
            format!(
                "Image too large (limit: {} megapixels)",
                image::MAX_PIXELS / 1_000_000
            ),
        ),
        OcrErrorKind::Corrupt => ParseError::new("image_corrupt", "Not a readable image"),
        OcrErrorKind::Unsupported => {
            ParseError::new("image_unsupported", "This kind of image cannot be read")
        }
        OcrErrorKind::Language => ParseError::new(
            "ocr_language_missing",
            format!("OCR language not installed: {}", e.message),
        ),
        OcrErrorKind::Other => ParseError::new(
            "ocr_failed",
            format!("OCR failed on this file: {}", e.message),
        ),
    }
}

fn too_many_pages() -> ParseError {
    ParseError::new("file_too_large", "File too large (limit: 2,000 pages)")
}

/// What `bytes` are, or the plain reason they are neither a PDF nor an image of `format`.
fn kind_of(format: Format, bytes: &[u8]) -> Result<Kind, ParseError> {
    image::sniff(bytes).ok_or_else(|| match format {
        Format::Pdf => ParseError::new("not_a_pdf", "Not a PDF"),
        _ => ParseError::new("not_an_image", "Not an image"),
    })
}

/// Parses a PDF or image content: one document of the kind the bytes are, one segment per page.
/// [`Sink::progress`] counts finished pages (ADR-0017).
pub(crate) fn parse(
    format: Format,
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).map_err(ParseError::io)?;
    match kind_of(format, &bytes)? {
        Kind::Pdf => parse_pdf(bytes, sink, place, engines),
        kind => parse_image(&bytes, kind, sink, place, engines),
    }
}

fn parse_pdf(
    bytes: Vec<u8>,
    sink: &mut dyn Sink,
    place: &Place,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let Engines { pdf, ocr } = engines;
    let doc = pdf.open(bytes).map_err(pdf_error)?;
    let pages = doc.page_count();
    if pages > MAX_PAGES {
        return Err(too_many_pages());
    }
    if pages == 0 {
        return Err(pdf_error(PdfError::new(PdfErrorKind::Corrupt, "no pages")));
    }
    sink.document(place.start(Format::Pdf, Vec::new()))?;
    for index in 0..pages {
        let number = PageNumber::new(index as u32 + 1).expect("counted from 1");
        match pdf_page_text(&*doc, index)? {
            PageText::Text(text) => {
                if let Some(seg) = ParsedSegment::whole(
                    |span| mk_kernel::Locator::PdfText { page: number, span },
                    text,
                ) {
                    sink.segment(seg)?;
                }
            }
            PageText::NeedsOcr => {
                sink.ocr(true);
                let image = image::render_for_ocr(&*doc, index).map_err(ocr_error)?;
                ocr_page(ocr.get()?, &image, OCR_DPI as u32, number, sink)?;
                sink.ocr(false);
            }
        }
        sink.progress(index as u64 + 1);
    }
    Ok(())
}

enum PageText {
    Text(String),
    NeedsOcr,
}

/// The stored text of a PDF page, or that it goes to OCR (US-02.2 AC1).
fn pdf_page_text(doc: &dyn PdfDoc, index: usize) -> Result<PageText, ParseError> {
    let info = doc.page_info(index).map_err(pdf_error)?;
    let laid = layout(&doc.chars(index).map_err(pdf_error)?, &info);
    let chars = laid.text.chars().filter(|c| !c.is_whitespace()).count();
    Ok(if chars < MIN_TEXT_CHARS {
        PageText::NeedsOcr
    } else {
        PageText::Text(laid.text)
    })
}

/// OCRs one page image into an `OcrText` segment (none if no word was found), with a warning
/// when its quality is low (US-02.2 AC3).
fn ocr_page(
    engine: &mut dyn OcrEngine,
    image: &GrayImage,
    dpi: u32,
    page: PageNumber,
    sink: &mut dyn Sink,
) -> Result<(), ParseError> {
    let lines = engine.recognize(image, dpi).map_err(ocr_error)?;
    let laid = ocr_layout(
        &lines,
        ImageSize {
            width: image.width,
            height: image.height,
        },
    );
    if laid.text.is_empty() {
        return Ok(());
    }
    if laid
        .geometry
        .mean_confidence()
        .is_some_and(|c| c < LOW_OCR_QUALITY)
    {
        sink.warning(format!("Page {page}: Low OCR quality"));
    }
    let len = laid.text.chars().count();
    let span = Span::new(0, len, len).expect("non-empty text");
    let locator = laid
        .geometry
        .locator(page, &laid.text, span)
        .map_err(|e| ParseError::new("internal_error", format!("OCR layout: {e}")))?;
    sink.segment(ParsedSegment {
        locator,
        text: laid.text,
        row_state: RowState::Normal,
        cell_starts: Vec::new(),
        ocr: Some(laid.geometry),
    })
}

fn parse_image(
    bytes: &[u8],
    kind: Kind,
    sink: &mut dyn Sink,
    place: &Place,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let pages = image::page_count(bytes, kind, MAX_PAGES).map_err(ocr_error)?;
    if pages > MAX_PAGES {
        return Err(too_many_pages());
    }
    // Decode the first page before loading OCR, so a broken image fails as one.
    let first = image::decode(bytes, kind, 0).map_err(ocr_error)?;
    sink.document(place.start(Format::Image, Vec::new()))?;
    let mut image = Some(first);
    for index in 0..pages {
        sink.ocr(true);
        let img = match image.take() {
            Some(i) => i,
            None => image::decode(bytes, kind, index).map_err(ocr_error)?,
        };
        let number = PageNumber::new(index as u32 + 1).expect("counted from 1");
        ocr_page(engines.ocr.get()?, &img, IMAGE_DPI, number, sink)?;
        sink.ocr(false);
        sink.progress(index as u64 + 1);
    }
    Ok(())
}

/// The point size and rotation of a rendered PDF page.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PagePoints {
    /// Unrotated page box size in points.
    pub width: f32,
    pub height: f32,
    /// `/Rotate`: 0, 90, 180 or 270.
    pub rotate: u32,
}

/// A page as the page view shows it (ADR-0011).
#[derive(Clone, PartialEq, Debug)]
pub struct RenderedPage {
    /// The page in display orientation.
    pub image: GrayImage,
    /// PDF pages only.
    pub points: Option<PagePoints>,
    /// PDF pages only: character boxes of the page's text as the parser stores it, in
    /// unrotated points. The caller checks it against the stored text (INV-CO-04).
    pub geometry: Option<PageGeometry>,
    pub pages: u32,
}

/// Renders page `page` (1-based) of a PDF at `scale` pixels per point, or decodes page `page`
/// of an image (its own size; `scale` is not used).
pub fn render_page(
    format: Format,
    bytes: Vec<u8>,
    page: u32,
    scale: f32,
    engines: &Engines,
) -> Result<RenderedPage, ParseError> {
    let no_page = || ParseError::new("no_such_page", format!("The file has no page {page}"));
    let index = PageNumber::new(page).ok_or_else(no_page)?.index();
    match kind_of(format, &bytes)? {
        Kind::Pdf => {
            let doc = engines.pdf().open(bytes).map_err(pdf_error)?;
            let pages = doc.page_count();
            if index >= pages {
                return Err(no_page());
            }
            let info: PageInfo = doc.page_info(index).map_err(pdf_error)?;
            let (w, h) = info.display_size();
            image::guard(
                (w * scale).ceil() as u64,
                (h * scale).ceil() as u64,
                "the page",
            )
            .map_err(ocr_error)?;
            let laid = layout(&doc.chars(index).map_err(pdf_error)?, &info);
            let image = doc.render(index, scale).map_err(pdf_error)?;
            Ok(RenderedPage {
                image,
                points: Some(PagePoints {
                    width: info.width,
                    height: info.height,
                    rotate: info.rotate,
                }),
                geometry: Some(laid.geometry),
                pages: pages as u32,
            })
        }
        kind => {
            let pages = image::page_count(&bytes, kind, MAX_PAGES).map_err(ocr_error)?;
            if index >= pages {
                return Err(no_page());
            }
            Ok(RenderedPage {
                image: image::decode(&bytes, kind, index).map_err(ocr_error)?,
                points: None,
                geometry: None,
                pages: pages as u32,
            })
        }
    }
}

/// The confidence of a span of an OCR'd segment: the lowest of the words it touches
/// (INV-ID-03). `None` for other segments.
pub(crate) fn ocr_confidence(ocr: Option<&OcrGeometry>, text: &str, span: Span) -> Option<f32> {
    ocr?.confidence(text, span).ok().flatten()
}
