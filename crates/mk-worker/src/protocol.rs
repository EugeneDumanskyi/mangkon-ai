//! Worker protocol messages (ADR-0007). Version 3 (M3b) adds the page view's `RenderPage` and
//! `PageImage` (ADR-0011), the OCR stage and settings (ADR-0017, ADR-0018), and OCR word
//! geometry on segments.

use mk_kernel::{ContentHash, IdentifierKey, Locator, OcrGeometry, PageGeometry};
use serde::{Deserialize, Serialize};

/// Protocol version. A worker says it in [`FromWorker::Hello`]; the core refuses any other.
/// Bump on any change to these messages.
pub const PROTOCOL: u32 = 3;

/// Core → worker.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ToWorker {
    /// Stage A for one content.
    Process(ProcessContent),
    /// One page of a PDF or image for the page view (ADR-0011).
    RenderPage(RenderPage),
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

/// Render page `page` (1-based) of `path`, which must still have `content_hash`: a PDF at
/// `scale` pixels per point, an image at its own size.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RenderPage {
    pub path: String,
    pub content_hash: ContentHash,
    pub file_type: String,
    pub page: u32,
    pub scale: f32,
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

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct Settings {
    /// Honour the test fault markers ([`crate::fault`]). Only tests and the kill-test driver set
    /// it; a file alone never turns it on.
    pub test_faults: bool,
    /// The project's OCR languages, `+`-joined (`eng+deu`, US-02.2 AC4).
    pub ocr_languages: String,
    /// The folder with the `.traineddata` files (ADR-0018). `None`: no OCR, and a page that
    /// needs it fails its file.
    pub tessdata: Option<String>,
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
    /// Sent every [`HEARTBEAT`](crate::HEARTBEAT) while an item is in work. `units` is the
    /// segments sent so far, or the messages (MBOX, ADR-0016) or pages (PDF and images,
    /// ADR-0017) finished.
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
    /// The page a [`ToWorker::RenderPage`] asked for, as an 8-bit grey PNG.
    PageImage(PageImage),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PageImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The pages in the file.
    pub pages: u32,
    /// PDF pages only: the unrotated page size in points and `/Rotate`.
    pub points: Option<PagePoints>,
    /// PDF pages only: the character boxes of the page text as the parser stores it, in
    /// unrotated points (ADR-0011). The core maps the stored span through it (INV-CO-04).
    pub geometry: Option<PageGeometry>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct PagePoints {
    pub width: f32,
    pub height: f32,
    pub rotate: u32,
}

/// What the worker is doing, for heartbeats.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorkStage {
    Reading,
    Parsing,
    Sending,
    /// OCR of one page: the page deadline is 30 s (ADR-0017, NFR-REL-06).
    Ocr,
    /// Rendering a page for the page view.
    Rendering,
}

/// A piece of one content's result, sent while the file is parsed. The core stages the pieces
/// in order and commits them once, at `Done` (INV-PR-04); after `Failed` it drops them.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Chunk {
    /// Starts a document; the segments and mentions that follow belong to it. Documents are
    /// numbered 0, 1, 2, … in the order they start; `parent` is the number of the document
    /// this one is an attachment of, sent before it (INV-CO-05).
    Document {
        kind: String,
        position: String,
        parent: Option<u32>,
    },
    /// The next segments of the current document, in order.
    Segments(Vec<Segment>),
    /// Mentions in the [`Chunk::Segments`] batch just before (only that batch).
    Mentions(Vec<Mention>),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Segment {
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
    /// OCR'd pages: each word's span, box and confidence (US-02.2 AC3).
    pub ocr: Option<OcrGeometry>,
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
