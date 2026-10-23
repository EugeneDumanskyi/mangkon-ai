//! `fetch-pdfium` and `fetch-ocr`: download pinned files with `curl` (no HTTP client in the
//! dependency tree, like `corpus-gen fetch`) and check SHA-256. `libpdfium` is pinned in
//! `pdfium.json` and unpacked into `spikes/vendor/pdfium/`; OCR models and language files are
//! pinned in `ocr-models.json` and land in `spikes/vendor/ocr/<engine>/`.

use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::spikes_dir;

#[derive(Deserialize)]
struct Manifest {
    tag: String,
    platforms: std::collections::BTreeMap<String, Asset>,
}

#[derive(Deserialize)]
struct Asset {
    url: String,
    sha256: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `url` to `dest` and checks its SHA-256; removes the file on a mismatch.
fn download(url: &str, dest: &std::path::Path, sha256: &str) -> Result<Vec<u8>, String> {
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("curl failed for {url}"));
    }
    let bytes = std::fs::read(dest).map_err(|e| e.to_string())?;
    let got = hex(&Sha256::digest(&bytes));
    if got != sha256 {
        std::fs::remove_file(dest).ok();
        return Err(format!(
            "{url}: sha256 mismatch: got {got}, pinned {sha256}"
        ));
    }
    Ok(bytes)
}

#[derive(Deserialize)]
struct OcrManifest {
    engines: std::collections::BTreeMap<String, Vec<OcrAsset>>,
}

#[derive(Deserialize)]
struct OcrAsset {
    file: String,
    url: String,
    sha256: String,
    #[cfg_attr(not(feature = "ocr"), allow(dead_code))]
    bytes: u64,
}

/// Bytes of an OCR engine's model and language files, from the manifest.
#[cfg(feature = "ocr")]
pub fn ocr_model_bytes(engine: &str) -> u64 {
    let manifest: OcrManifest =
        serde_json::from_str(include_str!("../ocr-models.json")).expect("manifest parses");
    manifest
        .engines
        .get(engine)
        .map_or(0, |a| a.iter().map(|x| x.bytes).sum())
}

/// Where an OCR engine's model and language files live.
pub fn ocr_dir(engine: &str) -> std::path::PathBuf {
    spikes_dir().join("vendor/ocr").join(engine)
}

pub fn ocr() -> Result<(), String> {
    let manifest: OcrManifest =
        serde_json::from_str(include_str!("../ocr-models.json")).map_err(|e| e.to_string())?;
    for (engine, assets) in &manifest.engines {
        let dir = ocr_dir(engine);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for a in assets {
            let dest = dir.join(&a.file);
            let ok = std::fs::read(&dest)
                .map(|b| hex(&Sha256::digest(&b)) == a.sha256)
                .unwrap_or(false);
            if !ok {
                download(&a.url, &dest, &a.sha256)?;
            }
            println!("{engine}/{} sha256 ok", a.file);
        }
    }
    Ok(())
}

pub fn pdfium() -> Result<(), String> {
    let manifest: Manifest =
        serde_json::from_str(include_str!("../pdfium.json")).map_err(|e| e.to_string())?;
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let asset = manifest
        .platforms
        .get(&platform)
        .ok_or_else(|| format!("no pinned pdfium build for {platform}"))?;
    let vendor = spikes_dir().join("vendor");
    std::fs::create_dir_all(&vendor).map_err(|e| e.to_string())?;
    let archive = vendor.join("pdfium.tgz");
    let bytes = download(&asset.url, &archive, &asset.sha256)?;
    let dest = vendor.join("pdfium");
    std::fs::remove_dir_all(&dest).ok();
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&dest)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err("tar failed".into());
    }
    println!(
        "pdfium {} ({platform}, {} bytes, sha256 ok) → {}",
        manifest.tag,
        bytes.len(),
        dest.display()
    );
    Ok(())
}
