//! A content as protocol chunks while it is parsed: its segments and identifier mentions
//! (US-04.1). This is the Stage A step after parsing, run in the worker because it is cheap and
//! needs the fresh segments (ADR-0007).

use mk_parsers::{DocumentStart, ParseError, ParsedContent, ParsedSegment, RowState, Sink};

use crate::protocol::{Chunk, Mention, Segment};

/// A parser [`Sink`] that turns what it gets into chunks: [`Chunk::Document`] for each
/// document, then its segments `chunk_segments` at a time, each batch followed by the mentions
/// in it. Only one batch is held. A mention that would cross a CSV cell edge is dropped: every
/// mention lies in one cell (INV-CO-03/04); Message-ID headers are not scanned.
///
/// `emit` errors stop the parse: the sink keeps the first one for [`ChunkSink::finish`] and
/// hands the parser a stand-in. `report` gets the running segment count, or, after
/// [`ChunkSink::in_units`], only the units the parser reports (MBOX messages, ADR-0016; PDF and
/// image pages, ADR-0017). `on_ocr` hears when a page's OCR starts and ends.
///
/// A mention in OCR text gets the lowest of its own confidence and its words' OCR confidence
/// (INV-ID-03).
pub struct ChunkSink<E, F, R> {
    emit: F,
    report: R,
    on_ocr: Option<Box<dyn FnMut(bool)>>,
    n: usize,
    columns: Vec<Option<String>>,
    batch: Vec<ParsedSegment>,
    /// Segments of the current document before `batch`.
    before: u32,
    documents: u32,
    total: u64,
    units: bool,
    error: Option<E>,
    warnings: Vec<String>,
}

impl<E, F, R> ChunkSink<E, F, R>
where
    F: FnMut(Chunk) -> Result<(), E>,
    R: FnMut(u64),
{
    pub fn new(chunk_segments: usize, emit: F, report: R) -> Self {
        ChunkSink {
            emit,
            report,
            on_ocr: None,
            n: chunk_segments.max(1),
            columns: Vec::new(),
            batch: Vec::new(),
            before: 0,
            documents: 0,
            total: 0,
            units: false,
            error: None,
            warnings: Vec::new(),
        }
    }

    /// Reports the parser's units of progress instead of segments.
    pub fn in_units(mut self) -> Self {
        self.units = true;
        self
    }

    /// Calls `f` when a page's OCR starts (`true`) and ends (`false`).
    pub fn on_ocr(mut self, f: impl FnMut(bool) + 'static) -> Self {
        self.on_ocr = Some(Box::new(f));
        self
    }

    fn send(&mut self, chunk: Chunk) -> Result<(), ParseError> {
        if self.error.is_some() {
            return Err(closed());
        }
        (self.emit)(chunk).map_err(|e| {
            self.error = Some(e);
            closed()
        })
    }

    fn flush(&mut self) -> Result<(), ParseError> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.batch);
        let mut mentions = Vec::new();
        for (i, seg) in batch.iter().enumerate() {
            let segment = self.before + i as u32;
            for hit in mk_identifiers::find_all(&seg.text) {
                if let Some(locator) = mk_parsers::mention_locator(seg, &self.columns, hit.span) {
                    let confidence = seg
                        .ocr_confidence(hit.span)
                        .map_or(hit.confidence, |c| c.min(hit.confidence));
                    mentions.push(Mention {
                        segment,
                        locator,
                        raw: hit.raw,
                        key: hit.key,
                        confidence,
                    });
                }
            }
        }
        let len = batch.len();
        let segments = batch
            .into_iter()
            .map(|seg| Segment {
                malformed: seg.row_state == RowState::Malformed,
                locator: seg.locator,
                text: seg.text,
                cell_starts: seg.cell_starts,
                ocr: seg.ocr,
            })
            .collect();
        self.send(Chunk::Segments(segments))?;
        if !mentions.is_empty() {
            self.send(Chunk::Mentions(mentions))?;
        }
        self.before += len as u32;
        self.total += len as u64;
        if !self.units {
            (self.report)(self.total);
        }
        Ok(())
    }

    /// Sends what is left. Returns the parser's warnings, or the first `emit` error.
    pub fn finish(mut self) -> Result<Vec<String>, E> {
        let flushed = self.flush();
        match self.error.take() {
            Some(e) => Err(e),
            None => {
                debug_assert!(flushed.is_ok());
                Ok(self.warnings)
            }
        }
    }

    /// The first `emit` error, if the parse stopped because of one.
    pub fn take_error(&mut self) -> Option<E> {
        self.error.take()
    }
}

fn closed() -> ParseError {
    ParseError::new("output_closed", "The result could not be sent")
}

impl<E, F, R> Sink for ChunkSink<E, F, R>
where
    F: FnMut(Chunk) -> Result<(), E>,
    R: FnMut(u64),
{
    fn document(&mut self, start: DocumentStart) -> Result<u32, ParseError> {
        self.flush()?;
        self.send(Chunk::Document {
            kind: start.kind.as_str().to_string(),
            position: start.position,
            parent: start.parent,
        })?;
        self.columns = start.columns;
        self.before = 0;
        self.documents += 1;
        Ok(self.documents - 1)
    }

    fn segment(&mut self, segment: ParsedSegment) -> Result<(), ParseError> {
        self.batch.push(segment);
        if self.batch.len() >= self.n {
            self.flush()?;
        }
        Ok(())
    }

    fn warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn progress(&mut self, units: u64) {
        if self.units {
            (self.report)(units);
        }
    }

    fn ocr(&mut self, active: bool) {
        if let Some(f) = &mut self.on_ocr {
            f(active);
        }
    }
}

/// A parsed content as chunks, the same as a streamed parse gives. Stops at the first error of
/// `emit`. `sent` is called with the running segment count.
pub fn chunks<E>(
    parsed: ParsedContent,
    chunk_segments: usize,
    emit: impl FnMut(Chunk) -> Result<(), E>,
    sent: impl FnMut(u64),
) -> Result<(), E> {
    let mut sink = ChunkSink::new(chunk_segments, emit, sent);
    for doc in parsed.documents {
        let start = DocumentStart {
            kind: doc.kind,
            position: doc.position,
            parent: doc.parent,
            columns: doc.columns,
        };
        if sink.document(start).is_err() {
            break;
        }
        for seg in doc.segments {
            if sink.segment(seg).is_err() {
                break;
            }
        }
    }
    sink.finish().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mk_parsers::Format;

    fn collect(parsed: ParsedContent, n: usize) -> Vec<Chunk> {
        let mut out = Vec::new();
        chunks::<()>(
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

    /// Segments and mentions of each document, in order; checks that every mention points into
    /// the batch just before it.
    fn flat(chunks: Vec<Chunk>) -> Vec<(Vec<Segment>, Vec<Mention>)> {
        let mut docs: Vec<(Vec<Segment>, Vec<Mention>)> = Vec::new();
        let mut batch = 0..0;
        for c in chunks {
            match c {
                Chunk::Document { .. } => docs.push(Default::default()),
                Chunk::Segments(s) => {
                    let d = docs.last_mut().unwrap();
                    batch = d.0.len()..d.0.len() + s.len();
                    d.0.extend(s);
                }
                Chunk::Mentions(m) => {
                    assert!(m.iter().all(|m| batch.contains(&(m.segment as usize))));
                    docs.last_mut().unwrap().1.extend(m);
                }
            }
        }
        docs
    }

    /// The same segments and mentions come out at any chunk size (US-02.4, streaming); every
    /// mention follows the batch it points into.
    #[test]
    fn chunk_size_does_not_change_the_result() {
        let mut csv = String::from("name,email\n");
        for i in 0..50 {
            csv.push_str(&format!("p{i},p{i}@x.example\n"));
        }
        let parsed = mk_parsers::parse(Format::Csv, csv.as_bytes()).unwrap();
        let one = flat(collect(parsed.clone(), 1));
        assert_eq!(one[0].0.len(), 51);
        assert_eq!(one[0].1.len(), 50);
        for n in [2, 7, 4096] {
            assert_eq!(flat(collect(parsed.clone(), n)), one, "chunk size {n}");
        }
    }

    #[test]
    fn empty_file_gives_a_document_and_nothing_else() {
        let parsed = mk_parsers::parse(Format::Text, b"").unwrap();
        assert_eq!(
            collect(parsed, 10),
            vec![Chunk::Document {
                kind: "txt".into(),
                position: "whole".into(),
                parent: None,
            }]
        );
    }

    /// Child documents carry their parent's index; Message-ID is not scanned; MBOX progress is
    /// in messages, not segments.
    #[test]
    fn attachments_and_mbox_units() {
        let eml = "From: a@x.example\r\nMessage-ID: <id@x.example>\r\n\
            Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\n\
            hi\r\n--b\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"n.txt\"\r\n\r\n\
            c@x.example\r\n--b--\r\n";
        let docs = collect(mk_parsers::parse(Format::Eml, eml.as_bytes()).unwrap(), 100);
        let parents: Vec<_> = docs
            .iter()
            .filter_map(|c| match c {
                Chunk::Document { parent, .. } => Some(*parent),
                _ => None,
            })
            .collect();
        assert_eq!(parents, vec![None, Some(0)]);
        let raws: Vec<String> = flat(docs)
            .into_iter()
            .flat_map(|d| d.1)
            .map(|m| m.raw)
            .collect();
        assert_eq!(raws, vec!["a@x.example", "c@x.example"]);

        let mbox = "From x\nFrom: a@x.example\n\none\nFrom y\nFrom: b@x.example\n\ntwo\n";
        let mut reports = Vec::new();
        let mut sink = ChunkSink::new(1, |_| Ok::<(), ()>(()), |u| reports.push(u)).in_units();
        mk_parsers::parse_stream(
            Format::Mbox,
            &mut std::io::Cursor::new(mbox.as_bytes()),
            &mut sink,
            &mut mk_parsers::Engines::default(),
        )
        .unwrap();
        sink.finish().unwrap();
        assert_eq!(reports, vec![1, 2]);
    }
}
