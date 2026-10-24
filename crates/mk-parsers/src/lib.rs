//! BC-02 parsers and OCR adapters (ADR-0002, ADR-0014). Used by parser workers (ADR-0007).
//!
//! In M2a: only [`TesseractOcr`], a safe handle over the statically linked Tesseract, so the build
//! and link of the native engine are proven from the start. TXT/CSV parsers come in M2b; the
//! `PdfEngine` and `OcrEngine` ports, the hayro adapter and the shared OCR path come in M3.

mod ocr;

pub use ocr::{OcrError, OcrPage, OcrWordBox, TesseractOcr};
