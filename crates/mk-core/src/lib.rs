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
//! - M2d: [`Project::ingest`] and [`Project::rebuild`] for both apps, and the slice reads
//!   (connections, occurrences, source context with the hit split out, `view`).
//! - M3c: [`Workspace`] (the project registry in `app.db`: create, open, recent, locate,
//!   forget); pause, resume and cancel on [`Project`] ([`Stop`]); the documents list, retry,
//!   removing files, settings, relocating roots and the progress panel (`manage`).
//! - M4a: keyword search ([`Project::search`]) and the source view at a hit
//!   ([`Project::segment_context`]).
//! - M4b: the identifier re-run after a region change or older rules
//!   ([`Project::rerun_identifiers`]), card-like flags in the viewer.
//! - M4c: custom patterns and their own re-run step ([`Project::add_pattern`]), column
//!   mapping ([`Project::mapping_view`], [`Project::confirm_mapping`]) and "Name (from table)"
//!   entities in the connections list.
//! - M5a: external endpoints ([`Workspace::save_endpoint`], [`Workspace::test_endpoint`]), keys
//!   in the OS secret store, per-project permissions and egress records, and
//!   [`Workspace::model_client`], the only way to a model client (ADR-0009).

mod control;
mod egress;
mod events;
mod identifiers;
mod links;
mod manage;
mod page;
mod patterns;
mod project;
mod search;
pub mod stage_a;
mod view;
mod workers;
mod workspace;

#[cfg(feature = "spike-s2d")]
pub mod spike_s2d;

use std::path::PathBuf;
use std::process::ExitCode;

pub use control::{PAUSE_GRACE, Stop};
pub use egress::{
    Access, AllowPrompt, DestinationInfo, EgressConfig, EgressRecordInfo, EndpointError,
    EndpointInfo, EndpointInput, EndpointTestInfo, KeyChange, KeyState, ProjectEndpoint,
    SecretStoreStatus, refusal_text, test_endpoint_at,
};
pub use events::{ConnectionsChanged, DEBOUNCE, Debouncer};
pub use identifiers::RerunSummary;
pub use links::{MAX_URL_CHARS, find_urls, openable_url};
pub use manage::{DocumentsPage, Eta, ProgressView, SourceRootInfo};
pub use mk_egress::{
    ChatMessage, ChatReply, ChatRequest, Destination, DestinationClass, EgressError, Embeddings,
    GateError, ModelClient, NO_STRUCTURED_OUTPUT, Purpose, SecretStoreKind, SecretString,
    TestReport, TlsRoots,
};
pub use mk_graph::model::EntityRow;
pub use mk_graph::{ConnectionRow, LOW_CONFIDENCE, RankKey, SubjectKind};
pub use mk_jobs::ItemState;
pub use mk_kernel::{Column, Locator, json_path};
pub use mk_mapping::{Assignments, ColumnType, MappingState};
pub use mk_search::Run as SnippetRun;
pub use mk_store::{
    CancelOutcome, CardFlagRow, ColumnRow, ConnectionQuery, DocumentCursor, DocumentFilter,
    DocumentInfo, DocumentRow, FailedFile, KnownFile, ListCursor, ListedConnection, MappingView,
    OccurrenceRow, RemoveOutcome, ReuseCandidate, SHOWN_SAMPLES, STATUS_MISSING, SearchCursor,
    SearchFilter, SortColumn,
};
pub use mk_workspace::{PatternSetting, ProjectSettings, RegistryEntry, Reprocess, RootKind};
pub use page::{PAGE_CACHE_LIMIT, PAGE_SCALE, PageBox, PageStatus, PageView, page_cache_dir};
pub use patterns::{CustomPatternInfo, MAX_SAMPLE_CHARS, PatternTest, TOO_BROAD, test_pattern};
pub use project::{
    HealthReport, IngestSummary, OpenSummary, Project, ProjectError, RebuildSummary, RescanSummary,
    StageASummary, file_types, project_exists,
};
pub use search::{SEARCH_PAGE, SearchHit, SearchPage, SegmentHit};
pub use view::{
    CHANGED_NOTE, CONTEXT_SEGMENTS, ConnectionsPage, ConnectionsSummary, ContextLine, ContextRow,
    DocumentLink, EmailField, EmailPart, IngestCounts, JsonRow, MAX_OCCURRENCES, Marked, Original,
    PageKind, Piece, Position, Run, SourceBody, SourceContext, WHOLE_EMAIL_SEGMENTS,
    not_found_note,
};
pub use workers::{
    Stopped, WorkerConfig, WorkerError, WorkerStop, restart_with_malloc_tunables, worker_processes,
};
pub use workspace::{RecentProject, Workspace, WorkspaceError};

/// The worker entry (ADR-0007): if this process was started as `<exe> --worker [...]`, runs the
/// worker loop on stdin/stdout and returns its exit code. Both apps call it first thing in
/// `main`, so one shipped binary is also the worker (ADR-0002 rule 5).
pub fn worker_main_if_requested() -> Option<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    (args.first().map(String::as_str) == Some("--worker")).then(|| mk_worker::run_stdio(&args[1..]))
}

/// The app's own data folder, `mangkon-ai` under the OS app-data folder (architecture §6):
/// `$XDG_DATA_HOME` or `~/.local/share` on Linux, `~/Library/Application Support` on macOS,
/// `%APPDATA%` on Windows. `None` if the OS gives no home.
pub fn app_data_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "windows") {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share")))
    };
    Some(base?.join("mangkon-ai"))
}

/// The app's OCR language folder, `<app data>/tessdata/` (ADR-0018).
pub fn tessdata_dir() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join("tessdata"))
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
