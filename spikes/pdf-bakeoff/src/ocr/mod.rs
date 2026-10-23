//! Spike S0, OCR rows (M0c): OCR candidates behind `OcrEngine`, on the TD-1 scans and the TD-3
//! images. Throwaway like the rest of the crate; the trait and the shared image path and layout
//! are written to move into `mk-parsers` (M3).

pub mod engine;
pub mod engines;
pub mod image;
pub mod layout;
pub mod pages;
pub mod quality;
pub mod report;
pub mod roundtrip;
pub mod td3;
