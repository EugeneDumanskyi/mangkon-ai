//! Spike S2d (K7): deterministic rows shaped like the connections list, served in keyset pages,
//! and the frame statistics the scroll test writes back. Throwaway: removed once the real
//! connections list exists (M2d). Results are in `docs/10-walking-skeleton.md` §S2d.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The row count the S2d pass mark is set at (NFR-PERF-04).
pub const TOTAL_ROWS: u32 = 100_000;
/// The largest page the UI may ask for.
pub const MAX_PAGE: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticRow {
    /// Keyset paging key; rows are sorted by it.
    pub key: u32,
    pub identifier: String,
    pub kind: &'static str,
    pub source_count: u32,
    pub first_source: String,
    pub second_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPage {
    pub rows: Vec<SyntheticRow>,
    /// Pass back as `after_key` to get the next page; `None` after the last row.
    pub next_key: Option<u32>,
    pub total: u32,
}

/// Rows with key greater than `after_key` (from the start if `None`), at most `limit` of them.
pub fn synthetic_rows(after_key: Option<u32>, limit: u32) -> RowPage {
    let start = after_key.map_or(0, |k| k.saturating_add(1));
    let end = start
        .saturating_add(limit.clamp(1, MAX_PAGE))
        .min(TOTAL_ROWS);
    let rows: Vec<_> = (start..end).map(row).collect();
    let next_key = (end < TOTAL_ROWS).then(|| end - 1);
    RowPage {
        rows,
        next_key,
        total: TOTAL_ROWS,
    }
}

const FOLDERS: [&str; 6] = ["inbox", "ledger", "scans", "exports", "contracts", "notes"];
const EXTENSIONS: [&str; 6] = ["mbox", "csv", "pdf", "json", "docx", "txt"];
const DOMAINS: [&str; 5] = [
    "example.org",
    "harbor.test",
    "north.test",
    "acme.test",
    "mail.test",
];

fn row(key: u32) -> SyntheticRow {
    let h = splitmix(u64::from(key));
    let (kind, identifier) = match h % 4 {
        0 => (
            "email",
            format!("user{key}@{}", DOMAINS[(h >> 8) as usize % DOMAINS.len()]),
        ),
        1 => (
            "phone",
            format!("+1 555 {:03} {:04}", (h >> 12) % 1000, key % 10_000),
        ),
        2 => (
            "iban",
            format!("GB{:02}NWBK6016{:08}", (h >> 20) % 100, key),
        ),
        _ => ("wallet", format!("bc1q{:016x}", h)),
    };
    let source = |salt: u64| {
        let s = splitmix(h ^ salt);
        let i = s as usize % FOLDERS.len();
        format!(
            "{}/{}-{:04}.{}",
            FOLDERS[i],
            FOLDERS[i],
            (s >> 16) % 10_000,
            EXTENSIONS[i]
        )
    };
    SyntheticRow {
        key,
        identifier,
        kind,
        source_count: 2 + ((h >> 32) % 39) as u32,
        first_source: source(1),
        second_source: source(2),
    }
}

/// SplitMix64: a fixed, seedless mix so rows are the same on every run and OS.
fn splitmix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What one scripted scroll run measured in the webview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    /// Frames while scrolling (the idle control phase is not included).
    pub frames: u32,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    /// Share of frames slower than 16.7 ms (one 60 Hz frame).
    pub slow_frame_share: f64,
    /// Share of frames slower than 25 ms (the NFR-PERF-04 jank mark).
    pub jank_frame_share: f64,
    /// Control: the same numbers with the list still (300 frames before the scroll starts).
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

/// Writes one run as pretty JSON.
pub fn write_frame_stats(path: &Path, stats: &FrameStats) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(stats).map_err(std::io::Error::other)?;
    std::fs::write(path, json + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyset_pages_cover_every_row_once() {
        let mut after = None;
        let mut seen = 0;
        loop {
            let page = synthetic_rows(after, 777);
            for (i, r) in page.rows.iter().enumerate() {
                assert_eq!(r.key, seen + i as u32);
            }
            seen += page.rows.len() as u32;
            match page.next_key {
                Some(k) => after = Some(k),
                None => break,
            }
        }
        assert_eq!(seen, TOTAL_ROWS);
    }

    #[test]
    fn rows_are_deterministic_and_limits_hold() {
        assert_eq!(synthetic_rows(Some(41), 3), synthetic_rows(Some(41), 3));
        assert_eq!(synthetic_rows(None, 0).rows.len(), 1);
        assert_eq!(synthetic_rows(None, 50_000).rows.len(), MAX_PAGE as usize);
        assert!(synthetic_rows(Some(TOTAL_ROWS - 1), 10).rows.is_empty());
        assert!(synthetic_rows(Some(u32::MAX), 10).rows.is_empty());
    }
}
