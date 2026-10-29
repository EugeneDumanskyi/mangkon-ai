//! TD-3 broken-file set manifest (`testdata/td3/expected.json`). See `docs/07-test-corpus.md` §6.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::Generator;

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Td3Manifest {
    pub schema_version: u32,
    pub generator: Generator,
    pub files: Vec<Td3File>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Td3File {
    /// Path relative to `testdata/td3/`.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub category: Td3Category,
    pub expected: Outcome,
    /// What the file does and why the outcome is expected.
    pub reason: String,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Td3Category {
    Empty,
    Blank,
    Encrypted,
    Corrupt,
    Truncated,
    Huge,
    WrongExtension,
    MemoryBomb,
    EndlessLoop,
    /// The parser process dies (TD-3 v2: a test fault).
    Crash,
    /// A valid file of a less common shape: a multi-page TIFF, a WEBP, a scan inside an email
    /// (TD-3 v4).
    Format,
}

impl Td3Category {
    pub const ALL: [Td3Category; 11] = [
        Td3Category::Empty,
        Td3Category::Blank,
        Td3Category::Encrypted,
        Td3Category::Corrupt,
        Td3Category::Truncated,
        Td3Category::Huge,
        Td3Category::WrongExtension,
        Td3Category::MemoryBomb,
        Td3Category::EndlessLoop,
        Td3Category::Crash,
        Td3Category::Format,
    ];
}

/// The expected result of processing the file alone. Whatever the outcome, the file must
/// finish within the per-file limits (NFR-REL-06/07) and never stop the batch (NFR-REL-01).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The file fails with a plain reason (NFR-USE-05).
    Fail,
    /// The file is processed (possibly with zero text).
    Success,
}
