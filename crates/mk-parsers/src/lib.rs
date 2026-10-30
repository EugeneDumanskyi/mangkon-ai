//! BC-02 parsers and OCR adapters (ADR-0002, ADR-0014). Used by parser workers (ADR-0007).
//!
//! - [`parse_stream`] (M3a): TXT/MD, CSV, JSON, HTML, DOCX, EML and MBOX, read from a file as a
//!   stream into a [`Sink`], in UTF-8, UTF-16 (BOM) or Windows-1252 (NFR-COMP-06). It runs only
//!   in worker processes (`mk-worker`, ADR-0007); [`Format::memory_estimate`] feeds the core's
//!   memory budget. [`parse`] collects the same into memory, for tests and small inputs.
//! - M3b: PDFs and images (US-02.1, US-02.2) through the [`pdf::engine::PdfEngine`] and
//!   [`ocr::OcrEngine`] ports (ADR-0014): the hayro adapter, the shared [`layout`], the shared
//!   [`image`] path and the Tesseract adapter, given to the parsers as [`Engines`]. OCR language
//!   files live in one `tessdata` folder (ADR-0018). [`render_page`] serves the page view
//!   (ADR-0011).

mod content;
mod csv;
mod docx;
mod email;
mod encoding;
mod html;
pub mod image;
mod json;
pub mod layout;
pub mod ocr;
mod paged;
pub mod pdf;
mod txt;

pub use content::{
    Collector, DocumentStart, Format, ParseError, ParsedContent, ParsedDocument, ParsedSegment,
    RowState, Sink, Source, mention_locator, parse, parse_stream, parse_with,
};
pub use csv::columns as csv_columns;
pub use encoding::TextEncoding;
pub use ocr::TesseractOcr;
pub use paged::{
    Engines, LOW_OCR_QUALITY, MAX_PAGES, MIN_TEXT_CHARS, OcrConfig, PagePoints, RenderedPage,
    render_page,
};
