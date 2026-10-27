//! Application services (ADR-0002): the one place that wires the context crates together. The
//! desktop shell and the CLI call only this crate. No Tauri, no UI types.
//!
//! - [`app_info`] (M2a).
//! - [`Project`] (M2b): open with migrations and restart recovery, source roots, re-scan, the
//!   Stage A lane, ignore decisions, and [`ConnectionsChanged`] events.
//! - Worker isolation (M2c, ADR-0007): Stage A runs in worker processes ([`WorkerConfig`]) under
//!   a deadline, a memory watchdog and a memory budget; [`worker_main_if_requested`] is the
//!   `--worker` entry of both apps; [`Project::check_health`] backs the kill test.
//! - Behind feature `spike-s2d`, the synthetic rows of spike S2d (M2a).
//!
//! The UI and CLI wiring of projects comes in M2d.

mod events;
mod project;
pub mod stage_a;
mod workers;

#[cfg(feature = "spike-s2d")]
pub mod spike_s2d;

use std::process::ExitCode;

pub use events::{ConnectionsChanged, DEBOUNCE, Debouncer};
pub use mk_jobs::ItemState;
pub use project::{
    HealthReport, OpenSummary, Project, ProjectError, RescanSummary, StageASummary, file_types,
};
pub use workers::{Stopped, WorkerConfig, WorkerError, WorkerStop, worker_processes};

/// The worker entry (ADR-0007): if this process was started as `<exe> --worker [...]`, runs the
/// worker loop on stdin/stdout and returns its exit code. Both apps call it first thing in
/// `main`, so one shipped binary is also the worker (ADR-0002 rule 5).
pub fn worker_main_if_requested() -> Option<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    (args.first().map(String::as_str) == Some("--worker")).then(|| mk_worker::run_stdio(&args[1..]))
}

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
