//! Stage A for one content, in process (architecture §5.1): read the file, check it is still the
//! content that was scanned, parse it, find identifiers. M2c moves the parse into a worker
//! process; this function then runs on the worker's output.

use std::panic::{AssertUnwindSafe, catch_unwind};

use mk_kernel::ContentHash;
use mk_parsers::{Format, ParsedContent, RowState};
use mk_store::{DocumentOut, MentionOut, SegmentOut, StageAOutput, StartedItem};

/// Why one content failed, with a stable code and a plain-English reason (NFR-USE-05,
/// US-02.7). It fails that content only (INV-CO-09).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Failure {
    pub code: &'static str,
    pub reason: String,
}

impl Failure {
    fn new(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

/// Reads, checks, parses and extracts one started item.
pub fn process(item: &StartedItem) -> Result<StageAOutput, Failure> {
    let path = item
        .path
        .as_ref()
        .ok_or_else(|| Failure::new("file_missing", "The file is no longer there"))?;
    let bytes = std::fs::read(path).map_err(|e| {
        Failure::new(
            "file_unreadable",
            format!("The file could not be read: {e}"),
        )
    })?;
    if ContentHash::of(&bytes) != item.hash {
        return Err(Failure::new(
            "file_changed",
            "File changed during processing",
        ));
    }
    let format = Format::from_name(&item.file_type)
        .ok_or_else(|| Failure::new("unsupported_type", "This file type is not supported yet"))?;
    let parsed = catch_unwind(AssertUnwindSafe(|| mk_parsers::parse(format, &bytes)))
        .map_err(|_| {
            Failure::new(
                "parser_crashed",
                "The file could not be read: the parser stopped",
            )
        })?
        .map_err(|e| Failure::new(e.code, e.message))?;
    Ok(extract(&parsed))
}

/// Segments and identifier mentions of a parsed content (US-04.1). A mention that would cross a
/// CSV cell edge is dropped: every mention lies in one cell (INV-CO-03/04).
pub fn extract(parsed: &ParsedContent) -> StageAOutput {
    let mut out = StageAOutput {
        warnings: parsed.warnings.clone(),
        ..Default::default()
    };
    for (d, doc) in parsed.documents.iter().enumerate() {
        for (s, seg) in doc.segments.iter().enumerate() {
            for hit in mk_identifiers::find_all(&seg.text) {
                if let Some(locator) = doc.mention_locator(s, hit.span) {
                    out.mentions.push(MentionOut {
                        document: d,
                        segment: s,
                        locator,
                        raw: hit.raw,
                        key: hit.key,
                        confidence: hit.confidence,
                    });
                }
            }
        }
        out.documents.push(DocumentOut {
            kind: doc.kind.as_str().to_string(),
            position: doc.position.clone(),
            segments: doc
                .segments
                .iter()
                .map(|seg| SegmentOut {
                    locator: seg.locator.clone(),
                    text: seg.text.clone(),
                    malformed: seg.row_state == RowState::Malformed,
                    cell_starts: seg.cell_starts.clone(),
                })
                .collect(),
        });
    }
    out
}
