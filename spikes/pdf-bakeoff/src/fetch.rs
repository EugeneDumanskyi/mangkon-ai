//! `fetch-pdfium`: downloads the pinned prebuilt `libpdfium` with `curl` (no HTTP client in the
//! dependency tree, like `corpus-gen fetch`), checks SHA-256 against `pdfium.json`, unpacks it
//! into `spikes/vendor/pdfium/`.

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
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&archive)
        .arg(&asset.url)
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("curl failed for {}", asset.url));
    }
    let bytes = std::fs::read(&archive).map_err(|e| e.to_string())?;
    let got = hex(&Sha256::digest(&bytes));
    if got != asset.sha256 {
        std::fs::remove_file(&archive).ok();
        return Err(format!(
            "sha256 mismatch: got {got}, pinned {}",
            asset.sha256
        ));
    }
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
