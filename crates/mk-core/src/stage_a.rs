//! Stage A results on the core side (architecture §5.1). A worker process reads, checks, parses
//! and extracts each content (`mk-worker`, ADR-0007) and streams chunks while it parses; the core
//! stages them in order ([`Staging`]) and commits them once, at `Done` (INV-PR-04). A large
//! result spills to a file in the project cache, so neither the core nor the commit holds a
//! whole file's result (§4 #16, ADR-0016).

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use mk_parsers::ParsedContent;
use mk_store::{DocumentOut, MentionOut, SegmentOut, StageAOutput, StagedPart, StoreError};
use mk_worker::Chunk;
use mk_worker::frame::{FrameReader, FrameWriter};

/// Why one content failed, with a stable code and a plain-English reason (NFR-USE-05,
/// US-02.7). It fails that content only (INV-CO-09).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Failure {
    pub code: String,
    pub reason: String,
    /// The technical text behind "Show details" (US-02.7 AC2); the code alone when `None`.
    pub detail: Option<String>,
}

impl Failure {
    pub(crate) fn new(code: &str, reason: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            reason: reason.into(),
            detail: None,
        }
    }

    pub(crate) fn from_worker(code: String, reason: String) -> Self {
        Self {
            code,
            reason,
            detail: None,
        }
    }

    pub(crate) fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// The worker broke the chunk order (a mention before its document or outside the segment
/// batch just before it, or a parent that is not an earlier document).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkOrderError;

/// Staged results held in memory up to this many bytes (estimated), then spilled to
/// `cache/staging/` (ASSUMPTION: 32 MB; W3a-1/2 check it).
pub const SPILL_AT: u64 = 32 << 20;

/// The staging directory of a project cache.
pub fn staging_dir(cache: &Path) -> PathBuf {
    cache.join("staging")
}

/// One item's chunks, checked for order as they come, in memory or in a spill file
/// `item-<id>.bin` of `u32` length + postcard frames. The file is removed when the staging is
/// dropped or committed.
pub struct Staging {
    chunks: Vec<Chunk>,
    bytes: u64,
    spill_at: u64,
    /// Where to spill; `None` never spills.
    file: Option<PathBuf>,
    writer: Option<FrameWriter<BufWriter<File>>>,
    documents: u32,
    /// The current document's segment count, and the first index of its last batch.
    segments: u32,
    batch_start: u32,
}

impl Staging {
    /// Staging that spills to `dir/item-<item>.bin` past `spill_at` bytes.
    pub fn new(dir: Option<&Path>, item: i64, spill_at: u64) -> Self {
        Staging {
            chunks: Vec::new(),
            bytes: 0,
            spill_at,
            file: dir.map(|d| d.join(format!("item-{item}.bin"))),
            writer: None,
            documents: 0,
            segments: 0,
            batch_start: 0,
        }
    }

    /// Staging that never spills.
    pub fn in_memory() -> Self {
        Self::new(None, 0, u64::MAX)
    }

    /// Whether the chunks went to a file.
    pub fn spilled(&self) -> bool {
        self.writer.is_some()
    }

    fn check(&mut self, chunk: &Chunk) -> Result<(), ChunkOrderError> {
        match chunk {
            Chunk::Document { parent, .. } => {
                if parent.is_some_and(|p| p >= self.documents) {
                    return Err(ChunkOrderError);
                }
                self.documents += 1;
                self.segments = 0;
                self.batch_start = 0;
            }
            Chunk::Segments(segs) => {
                if self.documents == 0 {
                    return Err(ChunkOrderError);
                }
                self.batch_start = self.segments;
                self.segments += segs.len() as u32;
            }
            Chunk::Mentions(ms) => {
                if self.documents == 0
                    || ms
                        .iter()
                        .any(|m| m.segment < self.batch_start || m.segment >= self.segments)
                {
                    return Err(ChunkOrderError);
                }
            }
        }
        Ok(())
    }

    /// Adds the next chunk. `Err(Ok(_))` is a chunk out of order; `Err(Err(_))` a spill file
    /// that could not be written.
    pub fn push(&mut self, chunk: Chunk) -> Result<(), Result<ChunkOrderError, io::Error>> {
        self.check(&chunk).map_err(Ok)?;
        if let Some(w) = self.writer.as_mut() {
            return w.send(&chunk).map_err(Err);
        }
        self.bytes += estimate(&chunk);
        self.chunks.push(chunk);
        if self.bytes > self.spill_at
            && let Some(path) = &self.file
        {
            self.spill(path.clone()).map_err(Err)?;
        }
        Ok(())
    }

    fn spill(&mut self, path: PathBuf) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut w = FrameWriter::new(BufWriter::with_capacity(1 << 20, File::create(&path)?));
        for c in self.chunks.drain(..) {
            w.send(&c)?;
        }
        self.chunks = Vec::new();
        self.bytes = 0;
        self.writer = Some(w);
        Ok(())
    }

    /// The staged parts in order, for [`mk_store::Store::commit_stage_a_staged`]. A spill file
    /// is read back one frame at a time and removed when the reader is dropped.
    pub fn into_parts(
        mut self,
    ) -> io::Result<Box<dyn Iterator<Item = Result<StagedPart, StoreError>> + Send>> {
        let mut docs = Parts::default();
        match self.writer.take() {
            None => {
                let chunks = std::mem::take(&mut self.chunks);
                Ok(Box::new(chunks.into_iter().map(move |c| Ok(docs.part(c)))))
            }
            Some(w) => {
                let mut file = w.into_inner().into_inner().map_err(|e| e.into_error())?;
                file.flush()?;
                drop(file);
                let path = self.file.take().expect("spilled to a file");
                let reader = SpillReader {
                    frames: FrameReader::new(BufReader::with_capacity(1 << 20, File::open(&path)?)),
                    path,
                };
                Ok(Box::new(reader.map(move |c| c.map(|c| docs.part(c)))))
            }
        }
    }

    /// The staged result as one value (small results, tests).
    pub fn into_output(self, warnings: Vec<String>) -> io::Result<StageAOutput> {
        let mut out = StageAOutput {
            warnings,
            ..StageAOutput::default()
        };
        for part in self.into_parts()? {
            match part.map_err(io::Error::other)? {
                StagedPart::Document {
                    kind,
                    position,
                    parent,
                } => out.documents.push(DocumentOut {
                    kind,
                    position,
                    parent,
                    segments: Vec::new(),
                }),
                StagedPart::Segments(s) => {
                    if let Some(d) = out.documents.last_mut() {
                        d.segments.extend(s)
                    }
                }
                StagedPart::Mentions(m) => out.mentions.extend(m),
            }
        }
        Ok(out)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        self.writer.take();
        if let Some(path) = self.file.take()
            && path.exists()
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Removes the spill file when the commit is done with it.
struct SpillReader {
    frames: FrameReader<BufReader<File>>,
    path: PathBuf,
}

impl Iterator for SpillReader {
    type Item = Result<Chunk, StoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.frames.recv::<Chunk>() {
            Ok(Some(c)) => Some(Ok(c)),
            Ok(None) => None,
            Err(e) => Some(Err(StoreError::Invalid(format!(
                "staging file could not be read: {e}"
            )))),
        }
    }
}

impl Drop for SpillReader {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Numbers documents while chunks become parts.
#[derive(Default)]
struct Parts {
    documents: usize,
}

impl Parts {
    fn part(&mut self, chunk: Chunk) -> StagedPart {
        match chunk {
            Chunk::Document {
                kind,
                position,
                parent,
            } => {
                self.documents += 1;
                StagedPart::Document {
                    kind,
                    position,
                    parent,
                }
            }
            Chunk::Segments(segs) => StagedPart::Segments(
                segs.into_iter()
                    .map(|s| SegmentOut {
                        locator: s.locator,
                        text: s.text,
                        malformed: s.malformed,
                        cell_starts: s.cell_starts,
                        ocr: s.ocr,
                    })
                    .collect(),
            ),
            Chunk::Mentions(ms) => {
                let document = self.documents.saturating_sub(1);
                StagedPart::Mentions(
                    ms.into_iter()
                        .map(|m| MentionOut {
                            document,
                            segment: m.segment as usize,
                            locator: m.locator,
                            raw: m.raw,
                            key: m.key,
                            confidence: m.confidence,
                        })
                        .collect(),
                )
            }
        }
    }
}

/// Bytes a chunk holds in memory, roughly: its text plus a fixed cost per segment and mention.
fn estimate(chunk: &Chunk) -> u64 {
    match chunk {
        Chunk::Document { position, .. } => 64 + position.len() as u64,
        Chunk::Segments(s) => s
            .iter()
            .map(|s| {
                96 + s.text.len() as u64
                    + 4 * s.cell_starts.len() as u64
                    + s.ocr.as_ref().map_or(0, |o| 40 * o.words().len() as u64)
            })
            .sum(),
        Chunk::Mentions(m) => m.iter().map(|m| 128 + 2 * m.raw.len() as u64).sum(),
    }
}

/// Removes staging files left by a run that was killed (ADR-0006: its items run again).
pub fn clear_staging(cache: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(staging_dir(cache)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Segments and identifier mentions of an already parsed content, in process, through the same
/// steps a worker runs. For benchmarks and tests that fill a project through the store (spike
/// S2c); untrusted files go through workers.
pub fn extract(parsed: &ParsedContent) -> StageAOutput {
    let mut staging = Staging::in_memory();
    mk_worker::extract::chunks::<ChunkOrderError>(
        parsed.clone(),
        usize::MAX,
        |c| staging.push(c).map_err(|_| ChunkOrderError),
        |_| {},
    )
    .expect("chunks from mk-worker are in order");
    staging
        .into_output(parsed.warnings.clone())
        .expect("in-memory staging")
}

#[cfg(test)]
mod tests {
    use mk_parsers::Format;

    use super::*;

    fn chunks_of(text: &str, format: Format, n: usize) -> Vec<Chunk> {
        let parsed = mk_parsers::parse(format, text.as_bytes()).unwrap();
        let mut out = Vec::new();
        mk_worker::extract::chunks::<()>(
            parsed,
            n,
            |c| {
                out.push(c);
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        out
    }

    /// A staging that spills gives the same parts as one in memory, and removes its file.
    #[test]
    fn spill_file_gives_the_same_parts_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let mut csv = String::from("name,email\n");
        for i in 0..500 {
            csv.push_str(&format!("p{i},p{i}@x.example\n"));
        }
        let chunks = chunks_of(&csv, Format::Csv, 7);
        let mut mem = Staging::in_memory();
        let mut disk = Staging::new(Some(dir.path()), 9, 1024);
        for c in chunks {
            mem.push(c.clone()).unwrap();
            disk.push(c).unwrap();
        }
        assert!(disk.spilled());
        let file = dir.path().join("item-9.bin");
        assert!(file.exists());
        let a: Vec<_> = mem.into_parts().unwrap().map(Result::unwrap).collect();
        let b: Vec<_> = disk.into_parts().unwrap().map(Result::unwrap).collect();
        assert_eq!(a, b);
        assert!(!file.exists());
        // Dropped without a commit (a failed item): the file goes too.
        let mut dropped = Staging::new(Some(dir.path()), 10, 0);
        for c in chunks_of("a@x.example\n", Format::Text, 1) {
            dropped.push(c).unwrap();
        }
        assert!(dir.path().join("item-10.bin").exists());
        drop(dropped);
        assert!(!dir.path().join("item-10.bin").exists());
    }

    /// Mentions only into the batch just before them; parents only earlier documents.
    #[test]
    fn chunk_order_is_checked() {
        let doc = |parent| Chunk::Document {
            kind: "txt".into(),
            position: "whole".into(),
            parent,
        };
        let mut s = Staging::in_memory();
        assert!(s.push(doc(Some(0))).is_err());
        let chunks = chunks_of("a@x.example\nb@x.example\n", Format::Text, 1);
        let Chunk::Mentions(first) = chunks[2].clone() else {
            panic!("{chunks:?}")
        };
        let mut s = Staging::in_memory();
        for c in chunks.iter().take(4).cloned() {
            s.push(c).unwrap();
        }
        assert!(matches!(
            s.push(Chunk::Mentions(first)),
            Err(Ok(ChunkOrderError))
        ));
    }
}
