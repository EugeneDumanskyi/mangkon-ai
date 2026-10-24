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
