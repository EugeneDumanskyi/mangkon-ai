//! Stage A results on the core side (architecture §5.1). Since M2c a worker process reads,
//! checks, parses and extracts each content (`mk-worker`, ADR-0007); the core assembles the
//! chunks it streams into one [`StageAOutput`] and commits it once, at `Done` (INV-PR-04).

use mk_parsers::ParsedContent;
use mk_store::{DocumentOut, MentionOut, SegmentOut, StageAOutput};
use mk_worker::Chunk;

/// Why one content failed, with a stable code and a plain-English reason (NFR-USE-05,
/// US-02.7). It fails that content only (INV-CO-09).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Failure {
    pub code: String,
    pub reason: String,
}

impl Failure {
    pub(crate) fn new(code: &str, reason: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            reason: reason.into(),
        }
    }

    pub(crate) fn from_worker(code: String, reason: String) -> Self {
        Self { code, reason }
    }
}

/// The worker broke the chunk order (a mention before its document or segment).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkOrderError;

/// Builds a [`StageAOutput`] from a worker's chunks, in the order they came.
#[derive(Default)]
pub struct Assembler {
    out: StageAOutput,
}

impl Assembler {
    pub fn push(&mut self, chunk: Chunk) -> Result<(), ChunkOrderError> {
        match chunk {
            Chunk::Document { kind, position } => self.out.documents.push(DocumentOut {
                kind,
                position,
                segments: Vec::new(),
            }),
            Chunk::Segments(segs) => {
                let doc = self.out.documents.last_mut().ok_or(ChunkOrderError)?;
                doc.segments.extend(segs.into_iter().map(|s| SegmentOut {
                    locator: s.locator,
                    text: s.text,
                    malformed: s.malformed,
                    cell_starts: s.cell_starts,
                }));
            }
            Chunk::Mentions(ms) => {
                let document = self
                    .out
                    .documents
                    .len()
                    .checked_sub(1)
                    .ok_or(ChunkOrderError)?;
                let sent = self.out.documents[document].segments.len();
                for m in ms {
                    let segment = m.segment as usize;
                    if segment >= sent {
                        return Err(ChunkOrderError);
                    }
                    self.out.mentions.push(MentionOut {
                        document,
                        segment,
                        locator: m.locator,
                        raw: m.raw,
                        key: m.key,
                        confidence: m.confidence,
                    });
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self, warnings: Vec<String>) -> StageAOutput {
        self.out.warnings = warnings;
        self.out
    }
}

/// Segments and identifier mentions of an already parsed content, in process, through the same
/// steps a worker runs. For benchmarks and tests that fill a project through the store (spike
/// S2c); untrusted files go through workers.
pub fn extract(parsed: &ParsedContent) -> StageAOutput {
    let mut asm = Assembler::default();
    let warnings = parsed.warnings.clone();
    mk_worker::extract::chunks::<ChunkOrderError>(
        parsed.clone(),
        usize::MAX,
        |c| asm.push(c),
        |_| {},
    )
    .expect("chunks from mk-worker are in order");
    asm.finish(warnings)
}
