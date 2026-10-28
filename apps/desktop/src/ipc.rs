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

/// The open project (A7: one at a time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectInfo {
    pub path: String,
    /// Source folders and files, as added.
    pub sources: Vec<String>,
}

fn count(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// File counts for the progress panel (US-03.1 AC1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Progress {
    pub total: u32,
    pub done: u32,
    pub failed: u32,
    /// Waiting or in a worker now.
    pub queued: u32,
    /// A re-scan or Stage A run is going on.
    pub ingesting: bool,
}

impl Progress {
    pub fn new(c: mk_core::IngestCounts, ingesting: bool) -> Self {
        Self {
            total: count(c.total),
            done: count(c.done),
            failed: count(c.failed),
            queued: count(c.queued + c.processing),
            ingesting,
        }
    }
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
    /// The identifier type, for example `email`.
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
    /// A kind this build does not show yet (pages come in M3).
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
            _ => Location::Other,
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
}

impl From<mk_core::Marked> for MarkedText {
    fn from(m: mk_core::Marked) -> Self {
        Self {
            before: m.before,
            hit: m.hit,
            after: m.after,
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

/// The source around one mention (US-08.1 AC3: text and CSV).
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceView {
    pub occurrence: Occurrence,
    pub body: SourceBody,
}

impl From<mk_core::SourceContext> for SourceView {
    fn from(c: mk_core::SourceContext) -> Self {
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
                        cells: r.cells.into_iter().map(Into::into).collect(),
                        malformed: r.malformed,
                    })
                    .collect(),
            },
        };
        Self {
            occurrence: c.occurrence.into(),
            body,
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
    /// `error` is set when the run stopped because of the project itself.
    IngestFinished {
        done: u32,
        failed: u32,
        error: Option<String>,
    },
    /// Something the user did outside a command failed (a drop), with a plain message.
    Error {
        message: String,
    },
}
