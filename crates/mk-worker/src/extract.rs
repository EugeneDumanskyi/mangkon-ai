//! A parsed content as protocol chunks: its segments and identifier mentions (US-04.1). This is
//! the Stage A step after parsing, run in the worker because it is cheap and needs the fresh
//! segments (ADR-0007).

use mk_parsers::{ParsedContent, RowState};

use crate::protocol::{Chunk, Mention, Segment};

/// Emits each document as [`Chunk::Document`], then its segments `chunk_segments` at a time,
/// each batch followed by the mentions in it. A mention that would cross a CSV cell edge is
/// dropped: every mention lies in one cell (INV-CO-03/04). Stops at the first error of `emit`.
/// `sent` is called with the running segment count.
pub fn chunks<E>(
    parsed: ParsedContent,
    chunk_segments: usize,
    mut emit: impl FnMut(Chunk) -> Result<(), E>,
    mut sent: impl FnMut(u64),
) -> Result<(), E> {
    let n = chunk_segments.max(1);
    let mut total = 0u64;
    for mut doc in parsed.documents {
        let mut mentions = Vec::new();
        for (s, seg) in doc.segments.iter().enumerate() {
            for hit in mk_identifiers::find_all(&seg.text) {
                if let Some(locator) = doc.mention_locator(s, hit.span) {
                    mentions.push(Mention {
                        segment: s as u32,
                        locator,
                        raw: hit.raw,
                        key: hit.key,
                        confidence: hit.confidence,
                    });
                }
            }
        }
        emit(Chunk::Document {
            kind: doc.kind.as_str().to_string(),
            position: std::mem::take(&mut doc.position),
        })?;
        let mut mentions = mentions.into_iter().peekable();
        let mut segments = doc.segments.into_iter().peekable();
        let mut end = 0u32;
        while segments.peek().is_some() {
            let batch: Vec<Segment> = segments
                .by_ref()
                .take(n)
                .map(|seg| Segment {
                    malformed: seg.row_state == RowState::Malformed,
                    locator: seg.locator,
                    text: seg.text,
                    cell_starts: seg.cell_starts,
                })
                .collect();
            end += batch.len() as u32;
            total += batch.len() as u64;
            emit(Chunk::Segments(batch))?;
            let in_batch: Vec<Mention> =
                std::iter::from_fn(|| mentions.next_if(|m| m.segment < end)).collect();
            if !in_batch.is_empty() {
                emit(Chunk::Mentions(in_batch))?;
            }
            sent(total);
        }
    }
    Ok(())
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

    /// The same segments and mentions come out at any chunk size; every mention follows the
    /// segment it points at.
    #[test]
    fn chunk_size_does_not_change_the_result() {
        let mut csv = String::from("name,email\n");
        for i in 0..50 {
            csv.push_str(&format!("p{i},p{i}@x.example\n"));
        }
        let parsed = mk_parsers::parse(Format::Csv, csv.as_bytes()).unwrap();
        let flat = |chunks: Vec<Chunk>| {
            let (mut segs, mut ms) = (Vec::new(), Vec::new());
            for c in chunks {
                match c {
                    Chunk::Document { .. } => {}
                    Chunk::Segments(s) => segs.extend(s),
                    Chunk::Mentions(m) => {
                        assert!(m.iter().all(|m| (m.segment as usize) < segs.len()));
                        ms.extend(m)
                    }
                }
            }
            (segs, ms)
        };
        let one = flat(collect(parsed.clone(), 1));
        assert_eq!(one.0.len(), 51);
        assert_eq!(one.1.len(), 50);
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
                position: "whole".into()
            }]
        );
    }
}
