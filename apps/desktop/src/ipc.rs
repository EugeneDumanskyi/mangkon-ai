//! The IPC boundary: argument and return types of every command. `cargo test` exports them to
//! TypeScript in `ui/src/ipc/bindings/` (ts-rs, ADR-0003); CI fails if the committed files differ.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub ocr_engine: String,
    /// Present only in builds with feature `spike-s2d`.
    pub spike_s2d: Option<SpikeS2dInfo>,
    /// Time "event received → list re-rendered" and save it with `record_ui_timing`
    /// (`MK_UI_TIMING=1`, NFR-PERF-34).
    pub ui_timing: bool,
    /// Once ingestion ends, open the source view of every mention and every linked document,
    /// then save the counts with `record_view_sweep` (`MK_UI_TIMING=1` and `MK_VIEW_SWEEP=1`;
    /// the NFR-SEC-09 network check).
    pub view_sweep: bool,
    /// The click sweep (`MK_UI_TIMING=1` and `MK_VIEW_SWEEP=clicks`; NFR-PERF-14, NFR-QA-12):
    /// click seeded random occurrence rows, time each click to the highlight, and save the
    /// samples with `record_view_sweep`.
    pub click_sweep: Option<ClickSweepInfo>,
    /// The scripted queue walk (`MK_QUEUE_WALK=1` or `2`): the UI clicks through the project and
    /// queue controls, then saves the steps with `record_queue_walk` (M3c release check).
    pub queue_walk: Option<QueueWalkInfo>,
}

/// What the queue walk works on: `MK_QUEUE_WALK_LOCATION` (where phase 1 makes the project) and
/// `MK_QUEUE_WALK_SOURCES` (folders to add, a path list like `PATH`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueWalkInfo {
    pub phase: u8,
    pub location: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpikeS2dInfo {
    /// Start the scripted scroll run as soon as the screen loads (`MK_S2D_AUTORUN=1`).
    pub autorun: bool,
}

impl From<mk_core::AppInfo> for AppInfo {
    fn from(i: mk_core::AppInfo) -> Self {
        Self {
            name: i.name.to_string(),
            version: i.version.to_string(),
            ocr_engine: i.ocr_engine,
            spike_s2d: cfg!(feature = "spike-s2d").then(|| SpikeS2dInfo {
                autorun: std::env::var_os("MK_S2D_AUTORUN").is_some_and(|v| v == "1"),
            }),
            ui_timing: ui_timing_enabled(),
            view_sweep: view_sweep_enabled(),
            click_sweep: click_sweep(),
            queue_walk: queue_walk_phase().map(|phase| QueueWalkInfo {
                phase,
                location: std::env::var("MK_QUEUE_WALK_LOCATION").unwrap_or_default(),
                sources: std::env::var_os("MK_QUEUE_WALK_SOURCES")
                    .map(|v| {
                        std::env::split_paths(&v)
                            .map(|p| p.display().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
// Spike S2d: used by commands only with feature `spike-s2d`; always exported for the UI.
#[cfg_attr(not(feature = "spike-s2d"), allow(dead_code))]
pub struct RowsRequest {
    /// Key of the last row already loaded; `null` for the first page.
    pub after_key: Option<u32>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
// Spike S2d: used by commands only with feature `spike-s2d`; always exported for the UI.
#[cfg_attr(not(feature = "spike-s2d"), allow(dead_code))]
pub struct SyntheticRow {
    pub key: u32,
    pub identifier: String,
    pub kind: String,
    pub source_count: u32,
    pub first_source: String,
    pub second_source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
// Spike S2d: used by commands only with feature `spike-s2d`; always exported for the UI.
#[cfg_attr(not(feature = "spike-s2d"), allow(dead_code))]
pub struct RowPage {
    pub rows: Vec<SyntheticRow>,
    pub next_key: Option<u32>,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
// Spike S2d: used by commands only with feature `spike-s2d`; always exported for the UI.
#[cfg_attr(not(feature = "spike-s2d"), allow(dead_code))]
pub struct FrameStats {
    pub frames: u32,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub slow_frame_share: f64,
    pub jank_frame_share: f64,
    pub idle_mean_ms: f64,
    pub idle_p95_ms: f64,
    pub idle_jank_frame_share: f64,
    pub page_fetches: u32,
    pub page_fetch_p50_ms: f64,
    pub page_fetch_p95_ms: f64,
    pub rows: u32,
    pub page_size: u32,
    pub user_agent: String,
    pub device_pixel_ratio: f64,
    pub viewport: String,
}

#[cfg(feature = "spike-s2d")]
mod spike {
    use super::*;
    use mk_core::spike_s2d as core;

    impl From<core::RowPage> for RowPage {
        fn from(p: core::RowPage) -> Self {
            Self {
                rows: p.rows.into_iter().map(SyntheticRow::from).collect(),
                next_key: p.next_key,
                total: p.total,
            }
        }
    }

    impl From<core::SyntheticRow> for SyntheticRow {
        fn from(r: core::SyntheticRow) -> Self {
            Self {
                key: r.key,
                identifier: r.identifier,
                kind: r.kind.to_string(),
                source_count: r.source_count,
                first_source: r.first_source,
                second_source: r.second_source,
            }
        }
    }

    impl From<FrameStats> for core::FrameStats {
        fn from(s: FrameStats) -> Self {
            Self {
                frames: s.frames,
                mean_ms: s.mean_ms,
                p50_ms: s.p50_ms,
                p95_ms: s.p95_ms,
                p99_ms: s.p99_ms,
                max_ms: s.max_ms,
                slow_frame_share: s.slow_frame_share,
                jank_frame_share: s.jank_frame_share,
                idle_mean_ms: s.idle_mean_ms,
                idle_p95_ms: s.idle_p95_ms,
                idle_jank_frame_share: s.idle_jank_frame_share,
                page_fetches: s.page_fetches,
                page_fetch_p50_ms: s.page_fetch_p50_ms,
                page_fetch_p95_ms: s.page_fetch_p95_ms,
                rows: s.rows,
                page_size: s.page_size,
                user_agent: s.user_agent,
                device_pixel_ratio: s.device_pixel_ratio,
                viewport: s.viewport,
            }
        }
    }
}

/// `MK_UI_TIMING=1`: the UI times its list updates (NFR-PERF-34 screen side).
pub fn ui_timing_enabled() -> bool {
    std::env::var_os("MK_UI_TIMING").is_some_and(|v| v == "1")
}

/// `MK_UI_TIMING=1` and `MK_VIEW_SWEEP=1`: the UI opens every source view once (NFR-SEC-09).
pub fn view_sweep_enabled() -> bool {
    ui_timing_enabled() && std::env::var_os("MK_VIEW_SWEEP").is_some_and(|v| v == "1")
}

/// What the click sweep clicks: `MK_VIEW_SWEEP_N` random mentions (default 50) from seed
/// `MK_VIEW_SWEEP_SEED` (default 1), then the mention ids in `MK_VIEW_SWEEP_FIXED` (comma
/// separated: the large-file cases).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClickSweepInfo {
    pub seed: u32,
    pub n: u32,
    #[ts(type = "number[]")]
    pub fixed: Vec<i64>,
}

/// `MK_UI_TIMING=1` and `MK_VIEW_SWEEP=clicks`.
pub fn click_sweep() -> Option<ClickSweepInfo> {
    if !ui_timing_enabled() || std::env::var("MK_VIEW_SWEEP").as_deref() != Ok("clicks") {
        return None;
    }
    let num = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(d)
    };
    Some(ClickSweepInfo {
        seed: num("MK_VIEW_SWEEP_SEED", 1),
        n: num("MK_VIEW_SWEEP_N", 50),
        fixed: std::env::var("MK_VIEW_SWEEP_FIXED")
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect(),
    })
}

/// `MK_QUEUE_WALK=1` (create, add, pause, cancel, retry, remove, settings, quit paused) or `2`
/// (reopen, still paused, resume to the end).
pub fn queue_walk_phase() -> Option<u8> {
    match std::env::var("MK_QUEUE_WALK").as_deref() {
        Ok("1") => Some(1),
        Ok("2") => Some(2),
        _ => None,
    }
}

/// One phase of the queue walk: each step with the time it took, and what went wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueWalk {
    pub phase: u8,
    pub steps: Vec<QueueWalkStep>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueWalkStep {
    pub name: String,
    pub ms: f64,
    pub detail: String,
}

/// One view sweep: how many mention and document views were opened, and what went wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ViewSweep {
    pub mentions: u32,
    pub documents: u32,
    /// Page views whose image loaded, the text toggle opened once on each (M3b).
    pub pages: u32,
    /// Views by body kind (`text`, `table`, `paragraphs`, `json`, `email`, `page`).
    pub kinds: std::collections::BTreeMap<String, u32>,
    pub errors: Vec<String>,
    /// The click sweep's samples; empty for the full sweep.
    pub clicks: Vec<ClickSample>,
}

/// One timed click on an occurrence row (NFR-PERF-14) and what it highlighted (NFR-QA-12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClickSample {
    #[ts(type = "number")]
    pub mention_id: i64,
    /// One of `MK_VIEW_SWEEP_FIXED`.
    pub fixed: bool,
    /// The body kind shown.
    pub kind: String,
    /// Click → the frame after the highlight is in the DOM (a page: after its image loaded).
    pub ms: f64,
    /// The mention's text, and the highlighted text in the DOM (a page image: its OCR or text
    /// side, toggled after the timing).
    pub expected: String,
    pub highlighted: String,
    pub correct: bool,
    /// Next, then Back, came back to this mention at the same scroll (US-08.2); `null` when it
    /// is the only mention of its value in the document.
    pub back_ok: Option<bool>,
}

/// One timed run: `ConnectionsChanged` received → connections list re-rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UiTiming {
    pub samples: Vec<f64>,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub user_agent: String,
}

// ---- M2d: projects, progress, connections, occurrences, source view ----

/// An error a command returns: a stable `code` for the UI and a plain message to show
/// (NFR-USE-05).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl From<mk_core::ProjectError> for IpcError {
    fn from(e: mk_core::ProjectError) -> Self {
        IpcError::new("project", e.to_string())
    }
}

impl From<mk_core::WorkspaceError> for IpcError {
    fn from(e: mk_core::WorkspaceError) -> Self {
        use mk_core::WorkspaceError as W;
        let code = match &e {
            W::Name(_) => "name",
            W::NameTaken => "name_taken",
            W::NotWritable(_) => "not_writable",
            W::FolderExists(_) => "folder_exists",
            W::NotFound(_) => "not_found",
            W::NotAProject(_) => "not_a_project",
            W::TooNew { .. } => "too_new",
            W::MigrationFailed { .. } => "migration_failed",
            W::NoEntry(_) => "no_entry",
            W::Project(_) => "project",
        };
        IpcError::new(code, e.to_string())
    }
}

/// The open project (A7: one at a time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectInfo {
    /// Its entry in the project list.
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    pub path: String,
    /// Source folders and files, as added.
    pub sources: Vec<String>,
}

/// A project in the recent list (US-01.2 AC1), newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentProject {
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    pub path: String,
    /// Milliseconds since the Unix epoch, UTC.
    #[ts(type = "number")]
    pub last_opened_at: i64,
    /// `false` when the folder no longer holds the project: the UI offers Locate… and Remove
    /// from list (US-01.2 AC2).
    pub present: bool,
}

impl From<mk_core::RecentProject> for RecentProject {
    fn from(r: mk_core::RecentProject) -> Self {
        Self {
            id: r.entry.id,
            name: r.entry.name,
            path: r.entry.path,
            last_opened_at: r.entry.last_opened_at,
            present: r.present,
        }
    }
}

fn count(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The time left as the progress panel shows it (US-03.1 AC2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Eta {
    /// Fewer than 5 files finished in this run.
    Estimating,
    Left {
        seconds: u32,
    },
    /// Nothing is running.
    None,
}

impl From<mk_core::Eta> for Eta {
    fn from(e: mk_core::Eta) -> Self {
        match e {
            mk_core::Eta::Estimating => Eta::Estimating,
            mk_core::Eta::Left(d) => Eta::Left {
                seconds: u32::try_from(d.as_secs()).unwrap_or(u32::MAX),
            },
            mk_core::Eta::None => Eta::None,
        }
    }
}

/// The progress panel (US-03.1 AC1): the project's file counts and, while a run is going on,
/// the stage, the files of this run, the files in a worker now and the time left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Progress {
    pub total: u32,
    pub done: u32,
    pub failed: u32,
    /// Waiting or in a worker now.
    pub queued: u32,
    /// Stopped by Cancel (D10).
    pub not_processed: u32,
    /// A re-scan or Stage A run is going on.
    pub ingesting: bool,
    /// The queue is paused (US-03.2); it stays paused when the project is reopened.
    pub paused: bool,
    /// `"identifiers"` while identifiers are found again from stored text (a region change,
    /// US-04.2 AC2), `"A"` while Stage A work is open; `null` when the queue is empty.
    pub stage: Option<String>,
    /// Files finished in this run, and those plus the files still to do.
    pub run_done: u32,
    pub run_total: u32,
    pub current_files: Vec<String>,
    pub eta: Eta,
}

impl Progress {
    pub fn new(c: mk_core::IngestCounts, v: mk_core::ProgressView) -> Self {
        Self {
            total: count(c.total),
            done: count(c.done),
            failed: count(c.failed),
            queued: count(c.queued + c.processing),
            not_processed: count(
                c.total
                    .saturating_sub(c.done + c.failed + c.queued + c.processing),
            ),
            ingesting: v.running,
            paused: v.paused,
            stage: v.stage.map(str::to_string),
            run_done: count(v.files_done),
            run_total: count(v.files_total),
            current_files: v.current_files,
            eta: v.eta.into(),
        }
    }
}

/// Filters of the documents list (US-02.8 AC2); `null` matches all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentFilter {
    /// `queued`, `processing`, `done`, `failed`, `not_processed` or `missing`.
    pub status: Option<String>,
    pub file_type: Option<String>,
}

impl From<DocumentFilter> for mk_core::DocumentFilter {
    fn from(f: DocumentFilter) -> Self {
        Self {
            status: f.status,
            file_type: f.file_type,
        }
    }
}

/// Where the documents list continues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentCursor {
    pub root: String,
    pub rel_path: String,
}

impl From<DocumentCursor> for mk_core::DocumentCursor {
    fn from(c: DocumentCursor) -> Self {
        Self {
            root: c.root,
            rel_path: c.rel_path,
        }
    }
}

impl From<mk_core::DocumentCursor> for DocumentCursor {
    fn from(c: mk_core::DocumentCursor) -> Self {
        Self {
            root: c.root,
            rel_path: c.rel_path,
        }
    }
}

/// One row of the documents list: one path in the project (US-02.8 AC1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentItem {
    /// What `removal_impact` and `remove_files` take.
    #[ts(type = "number")]
    pub ref_id: i64,
    /// What `retry` takes.
    #[ts(type = "number")]
    pub content_id: i64,
    pub path: String,
    pub file_type: String,
    #[ts(type = "number")]
    pub size: u64,
    /// Pages, rows or messages read; `null` for other types or before the file is read.
    pub units: Option<u32>,
    /// `pages`, `rows` or `messages`.
    pub unit: Option<String>,
    /// `queued`, `processing`, `done`, `failed`, `not_processed` or `missing`.
    pub status: String,
    pub warnings: Vec<String>,
    /// The plain reason of a failed file (US-02.7 AC2).
    pub failure_reason: Option<String>,
    /// The technical text behind "Show details".
    pub failure_detail: Option<String>,
    /// Other paths with the same content (US-01.3 AC4).
    pub other_paths: u32,
    /// Card-like numbers in the file: "Contains card-like number" (US-04.6 AC1).
    pub card_like_count: u32,
}

impl From<mk_core::DocumentRow> for DocumentItem {
    fn from(r: mk_core::DocumentRow) -> Self {
        let path = if r.rel_path.is_empty() {
            r.root.clone()
        } else {
            std::path::Path::new(&r.root)
                .join(&r.rel_path)
                .display()
                .to_string()
        };
        Self {
            ref_id: r.ref_id,
            content_id: r.content_id,
            path,
            file_type: r.file_type,
            size: r.size,
            units: r.units.map(|(n, _)| count(n)),
            unit: r.units.map(|(_, u)| u.to_string()),
            status: r.status,
            warnings: r.warnings,
            failure_reason: r.failure_reason,
            failure_detail: r.failure_detail,
            other_paths: count(r.other_paths),
            card_like_count: count(r.card_like_count),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentsPage {
    pub rows: Vec<DocumentItem>,
    /// `null` on the last page.
    pub next: Option<DocumentCursor>,
}

impl From<mk_core::DocumentsPage> for DocumentsPage {
    fn from(p: mk_core::DocumentsPage) -> Self {
        Self {
            rows: p.rows.into_iter().map(Into::into).collect(),
            next: p.next.map(Into::into),
        }
    }
}

/// A file type in the project and how many paths have it (US-02.8 AC2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentType {
    pub file_type: String,
    pub count: u32,
}

/// What a removal did (US-01.5 AC1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoveSummary {
    pub removed: u32,
    /// Files whose data was deleted: no other path had the same content.
    pub deleted: u32,
}

impl From<mk_core::RemoveOutcome> for RemoveSummary {
    fn from(o: mk_core::RemoveOutcome) -> Self {
        Self {
            removed: count(o.refs as u64),
            deleted: count(o.contents as u64),
        }
    }
}

/// A project's settings (US-01.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    /// Two letters, such as `DE`; `null` reads only numbers with a country code.
    pub phone_region: Option<String>,
    /// Tesseract codes, at least one.
    pub ocr_languages: Vec<String>,
    pub domain_stop_list: Vec<String>,
}

impl From<mk_core::ProjectSettings> for Settings {
    fn from(s: mk_core::ProjectSettings) -> Self {
        Self {
            phone_region: s.phone_region,
            ocr_languages: s.ocr_languages,
            domain_stop_list: s.domain_stop_list,
        }
    }
}

impl From<Settings> for mk_core::ProjectSettings {
    fn from(s: Settings) -> Self {
        Self {
            phone_region: s.phone_region,
            ocr_languages: s.ocr_languages,
            domain_stop_list: s.domain_stop_list,
        }
    }
}

/// The settings and what the settings screen offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsInfo {
    pub settings: Settings,
    /// OCR languages whose files are in the OCR language folder (English is built in).
    pub installed_ocr_languages: Vec<String>,
}

/// What saving settings changes in files already read (INV-WS-07): the UI asks "Re-process
/// affected files now?" when something does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsSaved {
    /// Files with OCR'd pages, or that failed for a missing OCR language.
    pub reprocess_ocr: bool,
    /// The phone region changed: identifiers can be found again from the stored text
    /// (US-04.2 AC2). No file is read again.
    pub rerun_identifiers: bool,
}

/// A source folder or file of the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceRoot {
    #[ts(type = "number")]
    pub id: i64,
    pub path: String,
    /// `folder` or `file`.
    pub kind: String,
    /// `false` when it is not there now: the UI offers Locate… (D12).
    pub present: bool,
    /// Paths removed from it (US-01.5).
    pub excluded: u32,
}

impl From<mk_core::SourceRootInfo> for SourceRoot {
    fn from(r: mk_core::SourceRootInfo) -> Self {
        Self {
            id: r.id,
            path: r.path,
            kind: r.kind.as_str().to_string(),
            present: r.present,
            excluded: count(r.excluded as u64),
        }
    }
}

/// A file a re-scan left out, with the reason (INV-CO-11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

/// Where the connections list continues: the ranking key of the last row loaded (domain §9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionCursor {
    pub low_confidence: bool,
    pub source_count: u32,
    pub file_type_count: u32,
    pub mention_count: u32,
    pub subject_key: String,
}

impl From<mk_core::RankKey> for ConnectionCursor {
    fn from(k: mk_core::RankKey) -> Self {
        Self {
            low_confidence: k.low_confidence,
            source_count: count(k.source_count),
            file_type_count: count(k.file_type_count),
            mention_count: count(k.mention_count),
            subject_key: k.subject_key,
        }
    }
}

impl From<ConnectionCursor> for mk_core::RankKey {
    fn from(c: ConnectionCursor) -> Self {
        Self {
            low_confidence: c.low_confidence,
            source_count: c.source_count.into(),
            file_type_count: c.file_type_count.into(),
            mention_count: c.mention_count.into(),
            subject_key: c.subject_key,
        }
    }
}

/// One connection row (US-07.1 AC1/AC2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionItem {
    /// The identifier key, `type:value`; what `occurrences` takes.
    pub key: String,
    /// The identifier type: `email`, `phone`, `phone_unnormalized` (no region, exact digits),
    /// `url`, `domain`, `iban`, `btc`, `evm`, `sol` or `tron`.
    pub kind: String,
    pub value: String,
    pub source_count: u32,
    pub file_type_count: u32,
    pub mention_count: u32,
    /// Every mention is below 0.5 confidence (US-07.1 AC4).
    pub low_confidence: bool,
}

impl From<mk_core::ConnectionRow> for ConnectionItem {
    fn from(r: mk_core::ConnectionRow) -> Self {
        let (kind, value) = r
            .subject_key
            .split_once(':')
            .map_or((String::new(), r.subject_key.clone()), |(k, v)| {
                (k.to_string(), v.to_string())
            });
        Self {
            key: r.subject_key,
            kind,
            value,
            source_count: count(r.source_count),
            file_type_count: count(r.file_type_count),
            mention_count: count(r.mention_count),
            low_confidence: r.low_confidence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConnectionsPage {
    pub rows: Vec<ConnectionItem>,
    /// `null` on the last page.
    pub next: Option<ConnectionCursor>,
}

impl From<mk_core::ConnectionsPage> for ConnectionsPage {
    fn from(p: mk_core::ConnectionsPage) -> Self {
        Self {
            rows: p.rows.into_iter().map(Into::into).collect(),
            next: p.next.map(Into::into),
        }
    }
}

/// Where a mention is (INV-CO-03), as the UI labels it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Location {
    /// 1-based line of a text file.
    Line { line: u32 },
    /// 0-based CSV record (the header is row 0) and the column's header name or 0-based index.
    #[serde(rename_all = "camelCase")]
    Cell {
        row: u32,
        column_name: Option<String>,
        column_index: Option<u32>,
    },
    /// A whole CSV record (a search result): 0-based, the header is row 0.
    Row { row: u32 },
    /// 1-based paragraph of a Word file.
    Paragraph { paragraph: u32 },
    /// Character offset of the block in an HTML file's extracted text.
    HtmlText { offset: u32 },
    /// A JSON value, as a path like `$.customers[12].phone` (US-02.5).
    JsonPath { path: String },
    /// An email header, by name.
    Header { name: String },
    /// An email body part, by IMAP part number (`1`, `1.2`).
    Part { part: String },
    /// 1-based page of a PDF or image; `ocr` when its text came from OCR.
    Page { page: u32, ocr: bool },
    /// A kind this build does not show yet.
    Other,
}

impl From<&mk_core::Locator> for Location {
    fn from(l: &mk_core::Locator) -> Self {
        match l {
            mk_core::Locator::TextLine { line, .. } => Location::Line { line: line.get() },
            mk_core::Locator::CsvCell { row, column, .. } => {
                let (column_name, column_index) = match column {
                    mk_core::Column::Name(n) => (Some(n.clone()), None),
                    mk_core::Column::Index(i) => (None, Some(*i)),
                };
                Location::Cell {
                    row: count(*row),
                    column_name,
                    column_index,
                }
            }
            mk_core::Locator::CsvRow { row } => Location::Row { row: count(*row) },
            mk_core::Locator::DocxParagraph { paragraph, .. } => Location::Paragraph {
                paragraph: paragraph.saturating_add(1),
            },
            mk_core::Locator::HtmlText { offset, .. } => Location::HtmlText {
                offset: count(*offset),
            },
            mk_core::Locator::JsonValue { pointer, .. } => Location::JsonPath {
                path: mk_core::json_path(pointer),
            },
            mk_core::Locator::EmailHeader { name, .. } => Location::Header { name: name.clone() },
            mk_core::Locator::MimePart { part, .. } => Location::Part { part: part.clone() },
            mk_core::Locator::PdfText { page, .. } => Location::Page {
                page: page.get(),
                ocr: false,
            },
            mk_core::Locator::OcrText { page, .. } => Location::Page {
                page: page.get(),
                ocr: true,
            },
        }
    }
}

/// One mention of a connection's identifier (US-07.3 AC2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Occurrence {
    /// What `source_context` takes.
    #[ts(type = "number")]
    pub mention_id: i64,
    /// The identifier key: the connection it belongs to.
    pub key: String,
    pub file_type: String,
    /// `null` when the file is no longer in any source folder.
    pub path: Option<String>,
    pub location: Location,
    /// The text as found.
    pub raw: String,
    pub low_confidence: bool,
}

impl From<mk_core::OccurrenceRow> for Occurrence {
    fn from(o: mk_core::OccurrenceRow) -> Self {
        Self {
            mention_id: o.mention_id,
            key: o.key,
            file_type: o.file_type,
            path: o.path,
            location: (&o.locator).into(),
            raw: o.raw,
            low_confidence: o.confidence < mk_core::LOW_CONFIDENCE,
        }
    }
}

/// A text with its hit split out on the Rust side (spans count Unicode characters, INV-CO-04).
/// `hit` is empty when the text has none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarkedText {
    pub before: String,
    pub hit: String,
    pub after: String,
    /// The same text as runs around its links (US-08.4 AC3); empty when it has none. The UI
    /// shows these instead of `before`, `hit` and `after` when there are any.
    pub runs: Vec<TextRun>,
}

/// A part of a text, in the hit or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TextPiece {
    pub text: String,
    pub hit: bool,
}

/// Plain text or a link found in the shown text. A link is opened only through `open_link`,
/// after the UI has shown its full URL and the user confirmed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum TextRun {
    Text { text: String, hit: bool },
    Link { url: String, pieces: Vec<TextPiece> },
}

impl From<mk_core::Piece> for TextPiece {
    fn from(p: mk_core::Piece) -> Self {
        Self {
            text: p.text,
            hit: p.hit,
        }
    }
}

impl From<mk_core::Marked> for MarkedText {
    fn from(m: mk_core::Marked) -> Self {
        Self {
            before: m.before,
            hit: m.hit,
            after: m.after,
            runs: m
                .runs
                .into_iter()
                .map(|r| match r {
                    mk_core::Run::Text(p) => TextRun::Text {
                        text: p.text,
                        hit: p.hit,
                    },
                    mk_core::Run::Link { url, pieces } => TextRun::Link {
                        url,
                        pieces: pieces.into_iter().map(Into::into).collect(),
                    },
                })
                .collect(),
        }
    }
}

/// "Mention 2 of 5" in this document, with the mentions to go to (US-08.2 AC1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MentionPosition {
    /// 1-based.
    pub index: u32,
    pub count: u32,
    #[ts(type = "number | null")]
    pub prev: Option<i64>,
    #[ts(type = "number | null")]
    pub next: Option<i64>,
}

/// The original file when the view opened (US-08.5): there, gone, or changed since it was read.
/// The view shows the stored text in every case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
#[ts(export)]
pub enum OriginalFile {
    Present { path: String },
    Missing { path: String },
    Changed { path: String },
}

impl From<mk_core::Original> for OriginalFile {
    fn from(o: mk_core::Original) -> Self {
        match o {
            mk_core::Original::Present { path } => Self::Present { path },
            mk_core::Original::Missing { path } => Self::Missing { path },
            mk_core::Original::Changed { path } => Self::Changed { path },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceLine {
    pub number: u32,
    pub text: MarkedText,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceRow {
    pub row: u32,
    pub cells: Vec<MarkedText>,
    pub malformed: bool,
}

/// One row of a JSON outline: a key or `[n]` on the way to a value (`value` is `null`), or a
/// value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JsonRow {
    pub depth: u32,
    pub label: String,
    pub value: Option<MarkedText>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmailField {
    pub name: String,
    pub value: MarkedText,
}

/// One body part of an email, as lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmailPart {
    pub part: String,
    pub lines: Vec<MarkedText>,
}

/// A document to open with `source_document`: an attachment, or the email it came in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentLink {
    #[ts(type = "number")]
    pub document_id: i64,
    pub label: String,
}

impl From<mk_core::DocumentLink> for DocumentLink {
    fn from(l: mk_core::DocumentLink) -> Self {
        Self {
            document_id: l.document_id,
            label: l.label,
        }
    }
}

/// The source around one mention, or a document from its start (US-08.1 AC3–AC6). Every part
/// is plain text; the UI shows it as text nodes only (US-08.4, gap A8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SourceBody {
    Text {
        lines: Vec<SourceLine>,
    },
    Table {
        header: Option<Vec<String>>,
        rows: Vec<SourceRow>,
    },
    /// DOCX or HTML text blocks.
    Paragraphs {
        paragraphs: Vec<MarkedText>,
    },
    Json {
        rows: Vec<JsonRow>,
    },
    Email {
        headers: Vec<EmailField>,
        parts: Vec<EmailPart>,
        attachments: Vec<DocumentLink>,
    },
    /// The stored text of one page of a PDF or image (US-08.3): the page view's text side and
    /// its fallback when the page cannot be shown.
    #[serde(rename_all = "camelCase")]
    Page {
        /// 1-based.
        page: u32,
        /// The last page with stored text.
        pages: u32,
        /// `null` when the page has no stored text.
        text: Option<PageText>,
        lines: Vec<MarkedText>,
        /// OCR pages: the mean word confidence, 0 to 1 (US-02.2 AC3).
        ocr_quality: Option<f32>,
    },
}

/// Where a page's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PageText {
    /// The PDF's text layer.
    Pdf,
    Ocr,
}

/// The document a source view shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceDocument {
    #[ts(type = "number")]
    pub document_id: i64,
    pub file_type: String,
    /// What it is: `txt`, `csv`, `eml`, `mbox`, … (an attachment has its own kind).
    pub kind: String,
    /// Where it sits in its file: `whole`, `message 3`, `part 2: notes.txt`.
    pub position: String,
    /// `null` when the file is no longer in any source folder.
    pub path: Option<String>,
    /// Card-like numbers in the document, masked, in reading order (US-04.6 AC1).
    pub card_flags: Vec<CardFlag>,
}

/// A card-like number: never shown or stored in full (US-04.6, NFR-SEC-14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CardFlag {
    /// `**** **** **** 1234`.
    pub masked: String,
    pub location: Location,
}

impl From<&mk_core::CardFlagRow> for CardFlag {
    fn from(f: &mk_core::CardFlagRow) -> Self {
        Self {
            masked: f.masked(),
            location: (&f.locator).into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceView {
    /// `null` when the view was opened at a document, not at a mention.
    pub occurrence: Option<Occurrence>,
    /// Where the mention is among its value's mentions in this document; `null` for a document.
    pub position: Option<MentionPosition>,
    /// The marked place: the mention's or the search hit's; `null` for a document. The page
    /// view places its boxes from it.
    pub hit: Option<SegmentHit>,
    /// `null` when no path of the file is known.
    pub original: Option<OriginalFile>,
    pub document: SourceDocument,
    /// The email this document is an attachment of (US-08.1 AC4).
    pub parent: Option<DocumentLink>,
    pub body: SourceBody,
}

impl From<mk_core::SourceContext> for SourceView {
    fn from(c: mk_core::SourceContext) -> Self {
        let marked = |v: Vec<mk_core::Marked>| v.into_iter().map(Into::into).collect();
        let body = match c.body {
            mk_core::SourceBody::Text { lines } => SourceBody::Text {
                lines: lines
                    .into_iter()
                    .map(|l| SourceLine {
                        number: l.number,
                        text: l.text.into(),
                    })
                    .collect(),
            },
            mk_core::SourceBody::Table { header, rows } => SourceBody::Table {
                header,
                rows: rows
                    .into_iter()
                    .map(|r| SourceRow {
                        row: count(r.row),
                        cells: marked(r.cells),
                        malformed: r.malformed,
                    })
                    .collect(),
            },
            mk_core::SourceBody::Paragraphs { paragraphs } => SourceBody::Paragraphs {
                paragraphs: marked(paragraphs),
            },
            mk_core::SourceBody::Json { rows } => SourceBody::Json {
                rows: rows
                    .into_iter()
                    .map(|r| JsonRow {
                        depth: r.depth,
                        label: r.label,
                        value: r.value.map(Into::into),
                    })
                    .collect(),
            },
            mk_core::SourceBody::Email {
                headers,
                parts,
                attachments,
            } => SourceBody::Email {
                headers: headers
                    .into_iter()
                    .map(|h| EmailField {
                        name: h.name,
                        value: h.value.into(),
                    })
                    .collect(),
                parts: parts
                    .into_iter()
                    .map(|p| EmailPart {
                        part: p.part,
                        lines: marked(p.lines),
                    })
                    .collect(),
                attachments: attachments.into_iter().map(Into::into).collect(),
            },
            mk_core::SourceBody::Page {
                page,
                pages,
                kind,
                lines,
                ocr_quality,
            } => SourceBody::Page {
                page,
                pages,
                text: kind.map(|k| match k {
                    mk_core::PageKind::Text => PageText::Pdf,
                    mk_core::PageKind::Ocr => PageText::Ocr,
                }),
                lines: marked(lines),
                ocr_quality,
            },
        };
        let d = c.document;
        Self {
            occurrence: c.occurrence.map(Into::into),
            position: c.position.map(|p| MentionPosition {
                index: p.index,
                count: p.count,
                prev: p.prev,
                next: p.next,
            }),
            hit: c.hit.map(Into::into),
            original: c.original.map(Into::into),
            document: SourceDocument {
                document_id: d.id,
                file_type: d.file_type,
                kind: d.kind,
                position: d.position,
                path: d.path,
                card_flags: Vec::new(),
            },
            parent: c.parent.map(Into::into),
            body,
        }
    }
}

// ---- M4a: keyword search ----

/// A keyword search as typed, with its filters (US-06.1 AC1–AC3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchQuery {
    pub text: String,
    /// File types to keep, such as `pdf`; empty keeps all.
    pub file_types: Vec<String>,
    /// One document to search in.
    #[ts(type = "number | null")]
    pub document: Option<i64>,
}

impl SearchQuery {
    pub fn filter(&self) -> mk_core::SearchFilter {
        mk_core::SearchFilter {
            file_types: self.file_types.clone(),
            document: self.document,
        }
    }
}

/// Where a result list continues: the last row's score and segment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchCursor {
    pub score: f64,
    #[ts(type = "number")]
    pub segment_id: i64,
}

impl From<SearchCursor> for mk_core::SearchCursor {
    fn from(c: SearchCursor) -> Self {
        Self {
            score: c.score,
            segment_id: c.segment_id,
        }
    }
}

impl From<mk_core::SearchCursor> for SearchCursor {
    fn from(c: mk_core::SearchCursor) -> Self {
        Self {
            score: c.score,
            segment_id: c.segment_id,
        }
    }
}

/// A place in a segment's stored text, in characters (INV-CO-04).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SegmentHit {
    #[ts(type = "number")]
    pub segment_id: i64,
    pub start: u32,
    pub end: u32,
}

impl From<mk_core::SegmentHit> for SegmentHit {
    fn from(h: mk_core::SegmentHit) -> Self {
        Self {
            segment_id: h.segment_id,
            start: count(h.span.start() as u64),
            end: count(h.span.end() as u64),
        }
    }
}

/// One search result (US-06.1 AC1): what `search_context` opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    #[ts(type = "number")]
    pub segment_id: i64,
    #[ts(type = "number")]
    pub document_id: i64,
    pub file_type: String,
    /// The file's path, or the one last seen when it is missing; `null` when none is known.
    pub path: Option<String>,
    /// The file is no longer where it was (domain D4: "Missing" badge).
    pub missing: bool,
    pub location: Location,
    /// Plain text with the matched words marked; card-like numbers masked (NFR-SEC-14).
    pub snippet: Vec<TextPiece>,
    /// The first match, where the source view opens; `null` if it could not be placed.
    pub hit: Option<SegmentHit>,
}

impl From<mk_core::SearchHit> for SearchHit {
    fn from(h: mk_core::SearchHit) -> Self {
        Self {
            segment_id: h.segment_id,
            document_id: h.document_id,
            file_type: h.file_type,
            path: h.path,
            missing: h.missing,
            location: (&h.locator).into(),
            snippet: h
                .snippet
                .into_iter()
                .map(|r| TextPiece {
                    text: r.text,
                    hit: r.hit,
                })
                .collect(),
            hit: h.hit.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// `null` on the last page.
    pub next: Option<SearchCursor>,
}

impl From<mk_core::SearchPage> for SearchPage {
    fn from(p: mk_core::SearchPage) -> Self {
        Self {
            hits: p.hits.into_iter().map(Into::into).collect(),
            next: p.next.map(Into::into),
        }
    }
}

/// Whether a page view shows the page image (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PageStatus {
    Shown,
    /// The original file is gone: the stored text is shown (US-08.5).
    OriginalMissing,
    /// The original file changed since it was read: the stored text is shown.
    Changed,
    /// The page could not be shown for the reason in the note: the stored text is shown.
    TextOnly,
}

/// A highlight box as fractions of the page image, from its top left (0 to 1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One page of a PDF or image, with the hit's boxes (US-08.1 AC1–AC2). The image itself comes
/// from `page_image` as PNG bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageView {
    #[ts(type = "number")]
    pub document_id: i64,
    /// 1-based.
    pub page: u32,
    pub pages: u32,
    pub status: PageStatus,
    /// The image's size in pixels; 0 when not shown.
    pub width: u32,
    pub height: u32,
    /// One box per line (PDF text) or per word (OCR); empty when the hit is not on this page.
    pub boxes: Vec<PageBox>,
    /// Why the page or its highlight is not shown.
    pub note: Option<String>,
}

impl From<mk_core::PageView> for PageView {
    fn from(v: mk_core::PageView) -> Self {
        Self {
            document_id: v.document_id,
            page: v.page,
            pages: v.pages,
            status: match v.status {
                mk_core::PageStatus::Shown => PageStatus::Shown,
                mk_core::PageStatus::OriginalMissing => PageStatus::OriginalMissing,
                mk_core::PageStatus::Changed => PageStatus::Changed,
                mk_core::PageStatus::TextOnly => PageStatus::TextOnly,
            },
            width: v.width,
            height: v.height,
            boxes: v
                .boxes
                .into_iter()
                .map(|b| PageBox {
                    x: b.x,
                    y: b.y,
                    width: b.width,
                    height: b.height,
                })
                .collect(),
            note: v.note,
        }
    }
}

/// What the core tells the UI over the `subscribe` channel. No rows: the UI re-queries
/// (architecture §7, US-07.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum UiEvent {
    /// Connection rows changed (debounced 500 ms).
    ConnectionsChanged,
    /// A project was opened or closed, or its sources changed (also after a drop).
    ProjectChanged,
    IngestStarted,
    /// Files finished or started, or the queue was paused or resumed (at most every 500 ms).
    ProgressChanged,
    /// `error` is set when the run stopped because of the project itself. The add summary
    /// (US-01.3 AC5): new paths, those with content already stored, and the files left out (at
    /// most 100 listed).
    #[serde(rename_all = "camelCase")]
    IngestFinished {
        done: u32,
        failed: u32,
        error: Option<String>,
        /// `pause`, `cancel` or `close` when the run was stopped.
        interrupted: Option<String>,
        added: u32,
        duplicates: u32,
        skipped: u32,
        skipped_files: Vec<SkippedFile>,
    },
    /// Something the user did outside a command failed (a drop), with a plain message.
    Error {
        message: String,
    },
}
