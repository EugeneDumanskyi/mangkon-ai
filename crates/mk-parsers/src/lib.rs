//! BC-02 parsers and OCR adapters (ADR-0002, ADR-0014). Used by parser workers (ADR-0007).
//!
//! - [`TesseractOcr`] (M2a): a safe handle over the statically linked Tesseract.
//! - [`parse`] (M2b): TXT/MD and CSV, UTF-8 only. Since M2c it runs only in worker processes
//!   (`mk-worker`, ADR-0007); [`Format::memory_estimate`] feeds the core's memory budget.
//!
//! The `PdfEngine` and `OcrEngine` ports, the hayro adapter, the shared OCR path and the other
//! formats and encodings come in M3.

mod content;
pub mod csv;
mod ocr;
pub mod txt;

pub use content::{
    Format, ParseError, ParsedContent, ParsedDocument, ParsedSegment, RowState, parse,
};
pub use ocr::{OcrError, OcrPage, OcrWordBox, TesseractOcr};
