//! BC-13 benchmark context. In M0a: the ground-truth model, loading and validation (US-14.3 AC1).
//! Scoring (US-14.3 AC2) comes in M4. M3b: the OCR character accuracy metric ([`ocr`], NFR-QA-07).

pub mod checks;
pub mod keys;
mod model;
pub mod ocr;
pub mod td3;
mod validate;

use std::fmt;
use std::path::{Path, PathBuf};

pub use model::*;

#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, std::io::Error),
    Json(PathBuf, serde_json::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            LoadError::Json(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for LoadError {}

/// SHA-256 of `bytes` as 64 lowercase hex characters: the file hash the ground truth and the TD-3
/// manifest store. Not the app's content hash, which is BLAKE3 (`mk_kernel::ContentHash`).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Loads `ground-truth.json` and its page-text JSON Lines file.
pub fn load(path: &Path) -> Result<GroundTruth, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|e| LoadError::Io(path.into(), e))?;
    let mut gt: GroundTruth =
        serde_json::from_str(&text).map_err(|e| LoadError::Json(path.into(), e))?;
    let pt_path = path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&gt.page_text_file);
    let pt = std::fs::read_to_string(&pt_path).map_err(|e| LoadError::Io(pt_path.clone(), e))?;
    gt.page_text = pt
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(|e| LoadError::Json(pt_path, e))?;
    Ok(gt)
}

/// Loads `testdata/td3/expected.json`.
pub fn load_td3(path: &Path) -> Result<td3::Td3Manifest, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|e| LoadError::Io(path.into(), e))?;
    serde_json::from_str(&text).map_err(|e| LoadError::Json(path.into(), e))
}

/// The two serialized forms of a ground truth: `ground-truth.json` (pretty) and the page-text
/// JSON Lines (one compact page per line). Both end with a newline. Used by the generator, so
/// that load and write agree.
pub fn to_files(gt: &GroundTruth) -> (String, String) {
    let mut main = serde_json::to_string_pretty(gt).expect("ground truth serializes");
    main.push('\n');
    let mut pages = String::new();
    for p in &gt.page_text {
        pages.push_str(&serde_json::to_string(p).expect("page text serializes"));
        pages.push('\n');
    }
    (main, pages)
}

/// JSON Schemas for the stored formats: (ground truth, page text line, TD-3 manifest).
pub fn schemas() -> (String, String, String) {
    let pretty = |s: schemars::Schema| {
        let mut out = serde_json::to_string_pretty(&s).expect("schema serializes");
        out.push('\n');
        out
    };
    (
        pretty(schemars::schema_for!(GroundTruth)),
        pretty(schemars::schema_for!(PageText)),
        pretty(schemars::schema_for!(td3::Td3Manifest)),
    )
}
