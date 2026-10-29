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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceView {
    /// `null` when the view was opened at a document, not at a mention.
    pub occurrence: Option<Occurrence>,
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
            document: SourceDocument {
                document_id: d.id,
                file_type: d.file_type,
                kind: d.kind,
                position: d.position,
                path: d.path,
            },
            parent: c.parent.map(Into::into),
            body,
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
