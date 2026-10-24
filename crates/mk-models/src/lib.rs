//! BC-12 model management. In M1: the model catalog format (NFR-MNT-09), its loader and
//! `validate()`. The catalog lists models and hardware tiers, not runtime binaries (ADR-0015).
//! Downloads, the hardware check and the runtime supervisor come in M5.
//!
//! The catalog file is `catalog/catalog.json`; its JSON Schema, produced from these types, is
//! `schema/catalog.schema.json`. Numbers in it come from `docs/09-model-check.md`.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Gap A6 rule (ASSUMPTION until approved at the end of M1): an embedding model is listed only if
/// binary top-200 → float rescore keeps recall@20 ≥ 0.9 against exact search (ADR-0010).
pub const MIN_BINARY_RECALL_AT_20: f64 = 0.9;

/// The model catalog shipped with one app version (US-09.3 AC2).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    /// Bumped whenever the catalog content changes.
    pub catalog_version: u32,
    pub models: Vec<Model>,
    /// From the smallest machine to the largest.
    pub tiers: Vec<Tier>,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Stage B people/org extraction (and Layer 3, if it ships).
    Extraction,
    /// Meaning search (US-06.2).
    Embeddings,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Model {
    /// Stable id, used by tiers, settings and AI runs (INV-AI-02).
    pub id: String,
    pub role: Role,
    /// Display name.
    pub name: String,
    /// Hugging Face repository, `owner/name`.
    pub hf_repo: String,
    /// Hugging Face commit the files are pinned to (40 hex).
    pub hf_revision: String,
    /// GGUF files; a split model lists every part, the first part first.
    pub files: Vec<ModelFile>,
    /// Quantization, as named in the file (`Q4_K_M`, `Q8_0`, `UD-IQ3_XXS`).
    pub quant: String,
    pub license: License,
    /// Context size the app starts the model with (`-c`).
    pub context: u32,
    /// Measured peak VRAM and RAM when run as the app runs it (`docs/09-model-check.md`).
    pub vram_bytes: u64,
    pub ram_bytes: u64,
    /// Lowest `llama-server` build this model was tested with (ADR-0015).
    pub min_llama_build: u32,
    /// Embedding models only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Embedding>,
    pub benchmark: Benchmark,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    pub name: String,
    pub bytes: u64,
    /// SHA-256 of the file, lowercase hex (INV-MM-01, NFR-SEC-11).
    pub sha256: String,
}

/// Shown before download (US-09.3 AC3, NFR-LEG-03).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct License {
    /// SPDX id, or `LicenseRef-<name>` for a model license with no SPDX id.
    pub spdx: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Embedding {
    pub dims: u32,
    /// Prepended to search queries and to stored segment text, as the model card says.
    pub query_prefix: String,
    pub document_prefix: String,
    /// Binary top-200 → float rescore → top 20, against exact float search (ADR-0010, gap A6).
    pub binary_recall_at_20: f64,
}

/// The M1 measurement behind the entry. Informative; the app shows nothing of it in v0.1.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Benchmark {
    /// Where the numbers come from, e.g. `docs/09-model-check.md §5.1`.
    pub source: String,
    /// Reference machine (`RM-1`).
    pub machine: String,
    pub llama_build: u32,
    /// Extraction: people/org precision and recall on TD-1 (NFR-QA-08/09).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall: Option<f64>,
    /// Extraction: segments per second, one request at a time (NFR-PERF-24, NFR-PERF-25).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments_per_s: Option<f64>,
    /// Embeddings: ~500-char windows per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_per_s: Option<f64>,
}

/// A hardware tier and its recommended models (US-09.3 AC1 "Recommended" badge).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tier {
    pub id: String,
    pub name: String,
    /// A machine fits the tier with this much dedicated GPU memory and system RAM …
    pub min_vram_bytes: u64,
    pub min_ram_bytes: u64,
    /// … or, on unified-memory machines (Apple silicon), with this much unified memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_unified_bytes: Option<u64>,
    /// `None`: no extraction model fits; Layer 2 needs an external endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_extraction: Option<String>,
    pub recommended_embeddings: String,
}

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

pub fn load(path: &Path) -> Result<Catalog, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|e| LoadError::Io(path.into(), e))?;
    serde_json::from_str(&text).map_err(|e| LoadError::Json(path.into(), e))
}

/// The JSON Schema of the catalog file.
pub fn schema() -> String {
    let mut out =
        serde_json::to_string_pretty(&schemars::schema_for!(Catalog)).expect("schema serializes");
    out.push('\n');
    out
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Catalog {
    /// Checks the catalog. Returns every problem found, not just the first.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut e = Vec::new();
        if self.schema_version != SCHEMA_VERSION {
            e.push(format!(
                "schema_version {} (expected {SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        let mut ids = BTreeSet::new();
        for m in &self.models {
            let id = &m.id;
            if !ids.insert(id.as_str()) {
                e.push(format!("model {id}: duplicate id"));
            }
            if m.hf_repo.split('/').filter(|p| !p.is_empty()).count() != 2 {
                e.push(format!(
                    "model {id}: hf_repo {:?} is not owner/name",
                    m.hf_repo
                ));
            }
            if !is_hex(&m.hf_revision, 40) {
                e.push(format!("model {id}: hf_revision is not a 40-hex commit"));
            }
            if m.files.is_empty() {
                e.push(format!("model {id}: no files"));
            }
            for f in &m.files {
                if !is_hex(&f.sha256, 64) {
                    e.push(format!(
                        "model {id}: {} sha256 is not 64 lowercase hex",
                        f.name
                    ));
                }
                if f.bytes == 0 || !f.name.ends_with(".gguf") {
                    e.push(format!("model {id}: {} must be a non-empty .gguf", f.name));
                }
            }
            // NFR-LEG-03: a license is shown before download.
            if m.license.spdx.trim().is_empty() || !m.license.url.starts_with("https://") {
                e.push(format!(
                    "model {id}: license needs an SPDX id and an https URL"
                ));
            }
            if m.context == 0 || m.ram_bytes == 0 {
                e.push(format!("model {id}: context and ram_bytes must be set"));
            }
            match (m.role, &m.embedding) {
                (Role::Embeddings, None) => e.push(format!(
                    "model {id}: embedding model without embedding info"
                )),
                (Role::Extraction, Some(_)) => {
                    e.push(format!("model {id}: extraction model with embedding info"))
                }
                (Role::Embeddings, Some(emb)) => {
                    if emb.dims == 0 {
                        e.push(format!("model {id}: dims must be > 0"));
                    }
                    if emb.binary_recall_at_20 < MIN_BINARY_RECALL_AT_20 {
                        e.push(format!(
                            "model {id}: binary recall@20 {} < {MIN_BINARY_RECALL_AT_20} (gap A6)",
                            emb.binary_recall_at_20
                        ));
                    }
                }
                (Role::Extraction, None) => {}
            }
        }
        let role: HashMap<&str, Role> = self
            .models
            .iter()
            .map(|m| (m.id.as_str(), m.role))
            .collect();
        let mut check = |tier: &str, model: &str, want: Role| match role.get(model) {
            None => e.push(format!("tier {tier}: unknown model {model}")),
            // INV-MM-04: a stage uses only a model of the matching role.
            Some(r) if *r != want => {
                e.push(format!("tier {tier}: {model} is {r:?}, expected {want:?}"))
            }
            _ => {}
        };
        let mut tier_ids = BTreeSet::new();
        for t in &self.tiers {
            if let Some(x) = &t.recommended_extraction {
                check(&t.id, x, Role::Extraction);
            }
            check(&t.id, &t.recommended_embeddings, Role::Embeddings);
        }
        for t in &self.tiers {
            if !tier_ids.insert(t.id.as_str()) {
                e.push(format!("tier {}: duplicate id", t.id));
            }
        }
        for w in self.tiers.windows(2) {
            if w[1].min_vram_bytes < w[0].min_vram_bytes || w[1].min_ram_bytes < w[0].min_ram_bytes
            {
                e.push(format!(
                    "tier {}: smaller than tier {} before it",
                    w[1].id, w[0].id
                ));
            }
        }
        if e.is_empty() { Ok(()) } else { Err(e) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, role: Role) -> Model {
        Model {
            id: id.into(),
            role,
            name: id.into(),
            hf_repo: "owner/repo".into(),
            hf_revision: "0".repeat(40),
            files: vec![ModelFile {
                name: format!("{id}.gguf"),
                bytes: 1,
                sha256: "a".repeat(64),
            }],
            quant: "Q8_0".into(),
            license: License {
                spdx: "Apache-2.0".into(),
                url: "https://example.org/LICENSE".into(),
            },
            context: 8192,
            vram_bytes: 1,
            ram_bytes: 1,
            min_llama_build: 10863,
            embedding: (role == Role::Embeddings).then(|| Embedding {
                dims: 768,
                query_prefix: String::new(),
                document_prefix: String::new(),
                binary_recall_at_20: 0.95,
            }),
            benchmark: Benchmark {
                source: "docs/09-model-check.md".into(),
                machine: "RM-1".into(),
                llama_build: 10863,
                precision: None,
                recall: None,
                segments_per_s: None,
                windows_per_s: None,
            },
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            schema_version: SCHEMA_VERSION,
            catalog_version: 1,
            models: vec![model("x", Role::Extraction), model("e", Role::Embeddings)],
            tiers: vec![Tier {
                id: "min".into(),
                name: "Minimum".into(),
                min_vram_bytes: 8,
                min_ram_bytes: 16,
                min_unified_bytes: None,
                recommended_extraction: Some("x".into()),
                recommended_embeddings: "e".into(),
            }],
        }
    }

    #[test]
    fn nfr_mnt_09_valid_catalog_passes() {
        assert_eq!(catalog().validate(), Ok(()));
    }

    #[test]
    fn inv_mm_04_tier_role_must_match() {
        let mut c = catalog();
        c.tiers[0].recommended_extraction = Some("e".into());
        c.tiers[0].recommended_embeddings = "nope".into();
        let errs = c.validate().unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("expected Extraction")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|e| e.contains("unknown model nope")),
            "{errs:?}"
        );
    }

    #[test]
    fn nfr_sec_11_hash_and_nfr_leg_03_license() {
        let mut c = catalog();
        c.models[0].files[0].sha256 = "ABC".into();
        c.models[0].license.url = "http://example.org".into();
        c.models[1].hf_revision = "main".into();
        let errs = c.validate().unwrap_err();
        assert_eq!(errs.len(), 3, "{errs:?}");
    }

    #[test]
    fn gap_a6_binary_recall_rule() {
        let mut c = catalog();
        c.models[1].embedding.as_mut().unwrap().binary_recall_at_20 = 0.85;
        assert!(c.validate().unwrap_err()[0].contains("gap A6"));
        c.models[1].embedding = None;
        assert!(c.validate().unwrap_err()[0].contains("without embedding info"));
    }

    #[test]
    fn duplicate_ids_and_tier_order() {
        let mut c = catalog();
        c.models.push(model("x", Role::Extraction));
        let mut big = c.tiers[0].clone();
        big.id = "big".into();
        big.min_ram_bytes = 4;
        c.tiers.push(big);
        let errs = c.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("duplicate id")), "{errs:?}");
        assert!(
            errs.iter().any(|e| e.contains("smaller than tier")),
            "{errs:?}"
        );
    }
}
