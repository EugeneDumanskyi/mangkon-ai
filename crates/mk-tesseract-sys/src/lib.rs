//! Tesseract 5 and Leptonica, compiled from the SHA-256-pinned sources in `native/sources.toml`
//! and linked statically (ADR-0014). Only the C API calls Mangkon needs are bound, behind the
//! safe [`Tesseract`] handle.
//!
//! `mk-parsers` is the only user. The shared OCR path, the `OcrEngine` port and the hayro adapter
//! come in M3.

mod ffi;

use std::path::Path;

pub use ffi::{Error, Page, Tesseract, Word, version};

/// The folder holding the pinned `eng.traineddata` (`tessdata_fast` 4.1.0), as placed by the
/// build. It is a build-machine path: packaging ships the file with the app (M3).
pub fn bundled_tessdata_dir() -> &'static Path {
    Path::new(env!("MK_TESSDATA_DIR"))
}
