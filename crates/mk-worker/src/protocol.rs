//! Worker protocol messages (ADR-0007). `RenderPage` and `PageImage` come with the PDF page view
//! (M3).

use mk_kernel::{ContentHash, IdentifierKey, Locator};
use serde::{Deserialize, Serialize};

/// Protocol version. A worker says it in [`FromWorker::Hello`]; the core refuses any other.
/// Bump on any change to these messages.
pub const PROTOCOL: u32 = 1;

/// Core → worker.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ToWorker {
    /// Stage A for one content.
    Process(ProcessContent),
    /// Finish and exit.
    Shutdown,
}

/// Stage A for one content: read `path`, check it still has `content_hash`, parse it as
/// `file_type`, find identifiers, stream the result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ProcessContent {
    pub path: String,
    pub content_hash: ContentHash,
    pub file_type: String,
    pub limits: Limits,
    pub settings: Settings,
}

/// How the worker cuts its output. Time and memory limits are enforced by the core, not here.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Segments per [`Chunk::Segments`] frame.
    pub chunk_segments: u32,
}

impl Default for Limits {
    fn default() -> Self {
        // Spike S2a: 4k-segment chunks (docs/10-walking-skeleton.md §S2a).
        Limits {
            chunk_segments: 4096,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Settings {
    /// Honour the test fault markers ([`crate::fault`]). Only tests and the kill-test driver set
    /// it; a file alone never turns it on.
    pub test_faults: bool,
}

/// Worker → core.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum FromWorker {
    /// First message after start.
    Hello {
        protocol: u32,
        app_version: String,
        pid: u32,
    },
    /// Sent every [`HEARTBEAT`](crate::HEARTBEAT) while an item is in work.
    Heartbeat {
        stage: WorkStage,
        units: u64,
    },
    Chunk(Chunk),
    /// The item finished; every chunk was sent before this.
    Done {
        warnings: Vec<String>,
    },
    /// The item failed with a stable code and a plain-English reason (NFR-USE-05).
    Failed {
        code: String,
        plain_reason: String,
    },
}

/// What the worker is doing, for heartbeats.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorkStage {
    Reading,
    Parsing,
    Sending,
}

/// A piece of one content's result. The core assembles the pieces in order and commits them
/// once, at `Done` (INV-PR-04).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Chunk {
    /// Starts a document; the segments and mentions that follow belong to it.
    Document { kind: String, position: String },
    /// The next segments of the current document, in order.
    Segments(Vec<Segment>),
    /// Mentions in segments of the current document already sent.
    Mentions(Vec<Mention>),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Segment {
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Mention {
    /// Index of the segment in its document.
    pub segment: u32,
    pub locator: Locator,
    pub raw: String,
    pub key: IdentifierKey,
    pub confidence: f32,
}
