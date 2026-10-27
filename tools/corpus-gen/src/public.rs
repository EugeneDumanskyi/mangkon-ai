//! The public slice (`td1/public/`): US federal public-domain works. The files are not
//! committed; `fetch` downloads them from the source URLs in `public-manifest.json` and checks
//! their hashes. `build` adds their entries and page-level identifier mentions (partial
//! annotation) to the ground truth.

use std::path::Path;
use std::process::Command;

use mk_bench::{Annotation, FileEntry, FileKind, IdType, IdentifierKey, Origin};
use serde::Deserialize;

use crate::truth::Collector;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[allow(dead_code)]
    note: String,
    files: Vec<PublicFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicFile {
    /// Relative to `td1/public/`.
    path: String,
    title: String,
    source_url: String,
    license_basis: String,
    retrieved: String,
    sha256: String,
    bytes: u64,
    kind: FileKind,
    pages: u32,
    purpose: String,
    identifiers: Vec<PublicIdent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicIdent {
    page: u32,
    #[serde(rename = "type")]
    id_type: IdType,
    /// Normalized key value.
    value: String,
    /// Text as printed.
    raw: String,
}

const MANIFEST: &str = include_str!("../public-manifest.json");

fn manifest() -> Result<Manifest, Vec<String>> {
    serde_json::from_str(MANIFEST).map_err(|e| vec![format!("public-manifest.json: {e}")])
}

/// Downloads every missing or changed public file into `td1/public/` with `curl` and checks
/// its hash. Returns the number of files downloaded.
pub fn fetch(td1: &Path) -> Result<usize, Vec<String>> {
    let dir = td1.join("public");
    std::fs::create_dir_all(&dir).map_err(|e| vec![format!("{}: {e}", dir.display())])?;
    let mut errors = Vec::new();
    let mut fetched = 0;
    for f in manifest()?.files {
        let dest = dir.join(&f.path);
        let hash_ok =
            |p: &Path| std::fs::read(p).is_ok_and(|b| mk_bench::sha256_hex(&b) == f.sha256);
        if hash_ok(&dest) {
            continue;
        }
        let part = dir.join(format!("{}.part", f.path));
        println!("fetch {}", f.source_url);
        let status = Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&part)
            .arg(&f.source_url)
            .status();
        match status {
            Err(e) => errors.push(format!("{}: cannot run curl: {e}", f.path)),
            Ok(s) if !s.success() => errors.push(format!("{}: curl failed ({s})", f.path)),
            Ok(_) if !hash_ok(&part) => errors.push(format!(
                "{}: sha256 differs from the manifest; the source may have changed",
                f.path
            )),
            Ok(_) => match std::fs::rename(&part, &dest) {
                Ok(()) => fetched += 1,
                Err(e) => errors.push(format!("{}: {e}", dest.display())),
            },
        }
        let _ = std::fs::remove_file(&part);
    }
    if errors.is_empty() {
        Ok(fetched)
    } else {
        Err(errors)
    }
}

pub fn build(c: &mut Collector, td1: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for f in manifest()?.files {
        let rel = format!("public/{}", f.path);
        match std::fs::read(td1.join(&rel)) {
            Err(e) => errors.push(format!("{rel}: {e} (run `corpus-gen fetch` first)")),
            Ok(b) if mk_bench::sha256_hex(&b) != f.sha256 => {
                errors.push(format!("{rel}: sha256 differs from the manifest"))
            }
            Ok(_) => {}
        }
        if f.title.is_empty()
            || f.source_url.is_empty()
            || f.license_basis.is_empty()
            || f.retrieved.is_empty()
        {
            errors.push(format!(
                "{rel}: title, source_url, license_basis and retrieved are required"
            ));
        }
        c.public_file(FileEntry {
            path: rel.clone(),
            sha256: f.sha256,
            bytes: f.bytes,
            origin: Origin::Public,
            kind: f.kind,
            pages: Some(f.pages),
            encoding: None,
            scan_tier: None,
            dpi: None,
            annotation: Annotation::Partial,
            purpose: f.purpose,
        });
        let doc = format!("p-{}", crate::corpus::stem(&f.path));
        c.whole(&doc, &rel, f.kind);
        for i in f.identifiers {
            c.public_mention(
                &doc,
                IdentifierKey {
                    id_type: i.id_type,
                    value: i.value,
                    pattern: None,
                },
                &i.raw,
                i.page,
            );
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
