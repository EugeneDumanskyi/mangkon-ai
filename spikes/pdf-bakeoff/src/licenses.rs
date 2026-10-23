//! License check from `cargo metadata` (`cargo-deny` arrives in M2a): the crates each engine
//! feature adds to the shipped (normal) dependency graph, and whether every license expression
//! is AGPL-3.0-compatible (NFR-LEG-02).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::spikes_dir;

/// SPDX ids this project accepts in an AGPL-3.0-only binary. ASSUMPTION until `deny.toml`.
const ALLOWED: &[&str] = &[
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "MPL-2.0",
    "CC0-1.0",
    "0BSD",
    "BSL-1.0",
    "Unlicense",
    "LGPL-2.1-or-later",
    "LGPL-3.0-or-later",
    "GPL-3.0-or-later",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "bzip2-1.0.6",
    "NCSA",
    "OFL-1.1",
    "IJG",
    "CDLA-Permissive-2.0",
];

#[derive(Serialize)]
pub struct EngineLicenses {
    /// Crates this engine adds over the no-engine build.
    pub crates: usize,
    /// License expression → number of crates.
    pub licenses: BTreeMap<String, usize>,
    /// Crates whose license is missing or not AGPL-compatible by the `ALLOWED` list.
    pub flagged: Vec<String>,
}

fn ids(expr: &str) -> Vec<String> {
    expr.replace(['(', ')'], " ")
        .replace('/', " OR ")
        .split(" OR ")
        .flat_map(|alt| {
            alt.split(" AND ")
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Every id allowed, or (for a plain OR list) at least one alternative allowed.
pub fn compatible(expr: &str) -> bool {
    let ok = |id: &str| ALLOWED.contains(&id);
    if ids(expr).iter().all(|i| ok(i)) {
        return true;
    }
    !expr.contains(" AND ") && !expr.contains('(') && ids(expr).iter().any(|i| ok(i))
}

/// (name version → license) of the normal-dependency closure of `pdf-bakeoff` with `features`.
fn closure(features: &str) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut cmd = Command::new("cargo");
    cmd.args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(spikes_dir().join("Cargo.toml"))
        .args(["--filter-platform", "x86_64-unknown-linux-gnu"]);
    if !features.is_empty() {
        cmd.args(["--features", features]);
    }
    let out = cmd.output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut pkgs: HashMap<&str, (String, Option<String>)> = HashMap::new();
    for p in v["packages"].as_array().into_iter().flatten() {
        pkgs.insert(
            p["id"].as_str().unwrap_or_default(),
            (
                format!(
                    "{} {}",
                    p["name"].as_str().unwrap_or("?"),
                    p["version"].as_str().unwrap_or("?")
                ),
                p["license"].as_str().map(str::to_string),
            ),
        );
    }
    let nodes: HashMap<&str, &Value> = v["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|n| (n["id"].as_str().unwrap_or_default(), n))
        .collect();
    let root = v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == "pdf-bakeoff")
        .and_then(|p| p["id"].as_str())
        .ok_or("pdf-bakeoff not in metadata")?;
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for d in nodes
            .get(id)
            .and_then(|n| n["deps"].as_array())
            .into_iter()
            .flatten()
        {
            let normal = d["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null());
            if normal && let Some(pid) = d["pkg"].as_str() {
                stack.push(pid);
            }
        }
    }
    Ok(seen
        .into_iter()
        .filter(|id| *id != root)
        .filter_map(|id| pkgs.get(id).cloned())
        .collect())
}

pub fn by_engine() -> Result<BTreeMap<String, EngineLicenses>, String> {
    let base = closure("")?;
    let mut out = BTreeMap::new();
    for e in ["hayro", "oxide", "pdfium"] {
        let all = closure(e)?;
        let mut licenses = BTreeMap::new();
        let mut flagged = Vec::new();
        let mut crates = 0;
        for (name, lic) in all.iter().filter(|(n, _)| !base.contains_key(*n)) {
            crates += 1;
            let l = lic.clone().unwrap_or_else(|| "(none)".into());
            if !compatible(&l) {
                flagged.push(format!("{name}: {l}"));
            }
            *licenses.entry(l).or_default() += 1;
        }
        out.insert(
            e.to_string(),
            EngineLicenses {
                crates,
                licenses,
                flagged,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions() {
        assert!(compatible("MIT OR Apache-2.0"));
        assert!(compatible("Apache-2.0/MIT"));
        assert!(compatible("(MIT OR Apache-2.0) AND Unicode-3.0"));
        assert!(compatible("GPL-2.0-only OR MIT"));
        assert!(!compatible("GPL-2.0-only"));
        assert!(!compatible("(none)"));
        assert!(!compatible("(MIT OR Apache-2.0) AND GPL-2.0-only"));
    }
}
