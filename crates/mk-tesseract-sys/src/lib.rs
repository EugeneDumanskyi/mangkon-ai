//! Tesseract 5 and Leptonica, compiled from the SHA-256-pinned sources in `native/sources.toml`
//! and linked statically (ADR-0014). Only the C API calls Mangkon needs are bound, behind the
//! safe [`Tesseract`] handle.
//!
//! `mk-parsers` is the only user. English is built in ([`ENG_TRAINEDDATA`], ADR-0018): the core
//! writes it into the app's `tessdata/` folder, next to any languages the user installs there,
//! and Tesseract reads every language from that one folder.

mod ffi;

pub use ffi::{Error, Page, Tesseract, Word, version};

/// The pinned `eng.traineddata` (`tessdata_fast` 4.1.0), embedded at build time.
pub static ENG_TRAINEDDATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eng.traineddata"));

/// SHA-256 of [`ENG_TRAINEDDATA`] as pinned in `native/sources.toml` (NFR-SEC-11); the build
/// fails if the file does not match it.
pub const ENG_TRAINEDDATA_SHA256: &str = env!("MK_ENG_TRAINEDDATA_SHA256");
