//! Frames: a `u32` little-endian payload length, then the `postcard` payload. postcard because
//! it is compact and maintained (bincode development has ended); ADR-0007 left the pick to M2.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Largest payload accepted either way. A 4k-segment chunk of long CSV rows stays far below it.
pub const MAX_FRAME: usize = 256 << 20;

/// Writes frames, reusing one buffer.
pub struct FrameWriter<W> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    /// Writes and flushes one frame.
    pub fn send<T: Serialize>(&mut self, msg: &T) -> io::Result<()> {
        self.buf.clear();
        self.buf.extend_from_slice(&[0; 4]);
        let buf = std::mem::take(&mut self.buf);
        self.buf = postcard::to_extend(msg, buf).map_err(io::Error::other)?;
        let len = self.buf.len() - 4;
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("frame of {len} bytes is over the {MAX_FRAME}-byte limit"),
            ));
        }
        self.buf[..4].copy_from_slice(&(len as u32).to_le_bytes());
        self.inner.write_all(&self.buf)?;
        self.inner.flush()
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

/// Reads frames, reusing one buffer.
pub struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    /// The next message; `None` at a clean end of stream (between frames).
    pub fn recv<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        let mut len = [0u8; 4];
        let mut got = 0;
        while got < 4 {
            match self.inner.read(&mut len[got..]) {
                Ok(0) if got == 0 => return Ok(None),
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let len = u32::from_le_bytes(len) as usize;
        if len > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame of {len} bytes is over the {MAX_FRAME}-byte limit"),
            ));
        }
        self.buf.resize(len, 0);
        self.inner.read_exact(&mut self.buf)?;
        postcard::from_bytes(&self.buf)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Chunk, FromWorker, Mention, Segment, WorkStage};
    use mk_kernel::{Column, IdentifierKey, IdentifierKind, LineNumber, Locator, Span};
    use proptest::prelude::*;

    fn round_trip(msgs: &[FromWorker]) -> Vec<FromWorker> {
        let mut w = FrameWriter::new(Vec::new());
        for m in msgs {
            w.send(m).unwrap();
        }
        let mut r = FrameReader::new(&w.inner[..]);
        let mut out = Vec::new();
        while let Some(m) = r.recv().unwrap() {
            out.push(m);
        }
        out
    }

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let span = Span::new(2, 7, 9).unwrap();
        let msgs = vec![
            FromWorker::Hello {
                protocol: 1,
                app_version: "0.0.0".into(),
                pid: 42,
            },
            FromWorker::Heartbeat {
                stage: WorkStage::Parsing,
                units: 3,
            },
            FromWorker::Chunk(Chunk::Segments(vec![Segment {
                locator: Locator::TextLine {
                    line: LineNumber::new(4).unwrap(),
                    span,
                },
                text: "ab x@y.zz".into(),
                malformed: false,
                cell_starts: vec![],
            }])),
            FromWorker::Chunk(Chunk::Mentions(vec![Mention {
                segment: 0,
                locator: Locator::CsvCell {
                    row: 1,
                    column: Column::Name("email".into()),
                    span,
                },
                raw: "x@y.zz".into(),
                key: IdentifierKey::new(IdentifierKind::Email, "x@y.zz"),
                confidence: 1.0,
            }])),
            FromWorker::Done { warnings: vec![] },
        ];
        assert_eq!(round_trip(&msgs), msgs);
    }

    #[test]
    fn truncated_and_oversized_frames_are_errors() {
        let mut w = FrameWriter::new(Vec::new());
        w.send(&FromWorker::Done { warnings: vec![] }).unwrap();
        let bytes = w.inner;
        let mut r = FrameReader::new(&bytes[..bytes.len() - 1]);
        assert!(r.recv::<FromWorker>().is_err());
        let mut r = FrameReader::new(&bytes[..2]);
        assert!(r.recv::<FromWorker>().is_err());
        let big = ((MAX_FRAME + 1) as u32).to_le_bytes();
        let mut r = FrameReader::new(&big[..]);
        assert_eq!(
            r.recv::<FromWorker>().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    /// A span that breaks its rule is refused on decode, not stored.
    #[test]
    fn decoding_rechecks_spans() {
        #[derive(Serialize)]
        enum FakeLocator {
            _PdfText,
            _OcrText,
            TextLine { line: u32, span: (usize, usize) },
        }
        let mut w = FrameWriter::new(Vec::new());
        w.send(&FakeLocator::TextLine {
            line: 1,
            span: (5, 5),
        })
        .unwrap();
        w.send(&FakeLocator::TextLine {
            line: 1,
            span: (4, 5),
        })
        .unwrap();
        let mut r = FrameReader::new(&w.inner[..]);
        assert!(r.recv::<Locator>().is_err());
        assert!(matches!(
            r.recv::<Locator>().unwrap(),
            Some(Locator::TextLine { span, .. }) if span.start() == 4
        ));
    }

    proptest! {
        #[test]
        fn any_failure_and_warnings_round_trip(
            code in "[a-z_]{1,20}",
            reason in "\\PC{0,80}",
            warnings in proptest::collection::vec("\\PC{0,40}", 0..5),
            units in any::<u64>(),
        ) {
            let msgs = vec![
                FromWorker::Heartbeat { stage: WorkStage::Sending, units },
                FromWorker::Failed { code, plain_reason: reason },
                FromWorker::Done { warnings },
            ];
            prop_assert_eq!(round_trip(&msgs), msgs);
        }
    }
}
