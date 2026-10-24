//! Application services (ADR-0002): the one place that wires the context crates together. The
//! desktop shell and the CLI call only this crate. No Tauri, no UI types.
//!
//! In M2a: [`app_info`] and, behind feature `spike-s2d`, the synthetic rows of spike S2d. Real
//! services (projects, ingestion, connections) come in M2b–M2d.

#[cfg(feature = "spike-s2d")]
pub mod spike_s2d;

/// What the About line and the CLI `--version` show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    /// The linked OCR engine, for example `Tesseract 5.5.2`.
    pub ocr_engine: String,
}

pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Mangkon AI",
        version: env!("CARGO_PKG_VERSION"),
        ocr_engine: format!("Tesseract {}", mk_parsers::TesseractOcr::engine_version()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_names_the_linked_ocr_engine() {
        let info = app_info();
        assert_eq!(info.name, "Mangkon AI");
        assert!(
            info.ocr_engine.starts_with("Tesseract 5."),
            "{}",
            info.ocr_engine
        );
    }
}
