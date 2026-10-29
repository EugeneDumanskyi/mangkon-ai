//! DOCX (US-02.3 AC1): one segment per paragraph of `word/document.xml`, read as a stream
//! (ADR-0013: `zip` + `quick-xml`).

use std::io::{self, BufReader, Read, SeekFrom};

use mk_kernel::Locator;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

use crate::content::{Format, ParseError, ParsedSegment, Place, Sink, Source};

/// The WordprocessingML main namespace.
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The most `word/document.xml` is read to, counted as it is inflated, not from the size the
/// ZIP declares (a ZIP bomb lies about it). ASSUMPTION: 256 MB.
pub const MAX_ENTRY: u64 = 256 << 20;

/// The magic bytes of an OLE compound file. Word saves a password-protected `.docx` as one.
const CFB: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];

fn damaged() -> ParseError {
    ParseError::new("docx_damaged", "Word file is damaged")
}

fn too_large() -> ParseError {
    ParseError::new(
        "docx_too_large",
        "Word file's text is too large to read (over 256 MB)",
    )
}

/// Paragraph index = every `w:p` in body order, table cells included (the ground-truth rule,
/// `docs/07-test-corpus.md` §5). Text = the paragraph's `w:t` runs joined, `w:tab` as `\t`,
/// `w:br` and `w:cr` as `\n`. Paragraphs with only whitespace give no segment but keep their
/// index. ASSUMPTION: headers, footers and footnotes are not read in v0.1.
///
/// An OLE file fails with "Word file is password-protected"; a bad ZIP or one without
/// `word/document.xml` with "Word file is damaged".
pub(crate) fn parse(
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
) -> Result<(), ParseError> {
    let mut magic = [0u8; 8];
    let n = input.read(&mut magic).map_err(ParseError::io)?;
    if n == 8 && magic == CFB {
        return Err(ParseError::new(
            "docx_encrypted",
            "Word file is password-protected",
        ));
    }
    input.seek(SeekFrom::Start(0)).map_err(ParseError::io)?;
    let mut zip = zip::ZipArchive::new(input).map_err(|_| damaged())?;
    let entry = zip.by_name("word/document.xml").map_err(|_| damaged())?;
    sink.document(place.start(Format::Docx, Vec::new()))?;
    let capped = Capped {
        inner: entry,
        left: MAX_ENTRY,
    };
    let mut reader = quick_xml::NsReader::from_reader(BufReader::with_capacity(1 << 16, capped));
    reader.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    // Open paragraphs (a text box can hold a paragraph inside another): index and text.
    let mut open: Vec<(u32, String)> = Vec::new();
    let mut next = 0u32;
    let mut in_text = false;
    loop {
        let (ns, event) = match reader.read_resolved_event_into(&mut buf) {
            Ok(e) => e,
            Err(e) => return Err(xml_error(&e)),
        };
        let in_w = matches!(ns, ResolveResult::Bound(n) if n.as_ref() == W);
        match event {
            Event::Start(e) if in_w => match e.local_name().as_ref() {
                "p" => {
                    open.push((next, String::new()));
                    next += 1;
                }
                "t" => in_text = true,
                "tab" => push(&mut open, "\t"),
                "br" | "cr" => push(&mut open, "\n"),
                _ => {}
            },
            Event::End(e) if in_w => match e.local_name().as_ref() {
                "p" => {
                    if let Some((paragraph, text)) = open.pop()
                        && !text.trim().is_empty()
                        && let Some(seg) = ParsedSegment::whole(
                            |span| Locator::DocxParagraph { paragraph, span },
                            text,
                        )
                    {
                        sink.segment(seg)?;
                    }
                }
                "t" => in_text = false,
                _ => {}
            },
            Event::Text(t) if in_text => push(&mut open, &t.xml10_content()),
            Event::CData(t) if in_text => push(&mut open, &t.into_inner()),
            Event::GeneralRef(r) if in_text => {
                let c = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c),
                    Ok(None) => match &*r {
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "amp" => Some('&'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                    Err(_) => None,
                };
                if let Some(c) = c {
                    push(&mut open, c.encode_utf8(&mut [0; 4]));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
        buf.clear();
    }
}

fn push(open: &mut [(u32, String)], s: &str) {
    if let Some((_, text)) = open.last_mut() {
        text.push_str(s);
    }
}

fn xml_error(e: &quick_xml::Error) -> ParseError {
    if let quick_xml::Error::Io(io) = e
        && io.get_ref().is_some_and(|r| r.is::<OverCap>())
    {
        return too_large();
    }
    damaged()
}

#[derive(Debug)]
struct OverCap;

impl std::fmt::Display for OverCap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("entry over the size cap")
    }
}

impl std::error::Error for OverCap {}

/// Fails with [`OverCap`] once more than `left` bytes were read.
struct Capped<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            return Err(io::Error::other(OverCap));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;
    use crate::content::parse;

    /// A minimal DOCX with `document.xml` as given (deflated).
    pub(crate) fn docx(document_xml: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(b"<Types/>").unwrap();
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn body(inner: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?><w:document xmlns:w=\"{}\"><w:body>{inner}</w:body></w:document>",
            W
        )
    }

    fn paragraphs(bytes: &[u8]) -> Vec<(u32, String)> {
        let parsed = parse(Format::Docx, bytes).unwrap();
        parsed.documents[0]
            .segments
            .iter()
            .map(|s| match &s.locator {
                Locator::DocxParagraph { paragraph, .. } => (*paragraph, s.text.clone()),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// US-02.3 AC1: paragraph index over every `w:p` (table cells too, empty ones counted),
    /// runs joined, tabs and breaks kept, entities decoded.
    #[test]
    fn us_02_3_ac1_paragraphs() {
        let xml = body(concat!(
            "<w:p><w:r><w:t>Tomas </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>Rein</w:t></w:r>",
            "<w:r><w:t>holt</w:t><w:tab/><w:t>a&amp;b &#x41;</w:t><w:br/><w:t>next</w:t></w:r></w:p>",
            "<w:p/>",
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
            "<w:p><w:r><w:delText>gone</w:delText><w:t xml:space=\"preserve\"> last </w:t></w:r></w:p>",
        ));
        assert_eq!(
            paragraphs(&docx(&xml)),
            vec![
                (0, "Tomas Reinholt\ta&b A\nnext".into()),
                (2, "cell".into()),
                (3, " last ".into())
            ]
        );
    }

    /// The guards: OLE (password-protected), not a ZIP, no `document.xml`, cut short.
    #[test]
    fn broken_word_files_fail_plainly() {
        let mut cfb = CFB.to_vec();
        cfb.extend_from_slice(&[0; 504]);
        let e = parse(Format::Docx, &cfb).unwrap_err();
        assert_eq!(e.message, "Word file is password-protected");
        assert_eq!(
            parse(Format::Docx, b"plain text").unwrap_err().message,
            "Word file is damaged"
        );
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("other.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"<x/>").unwrap();
        let no_doc = zip.finish().unwrap().into_inner();
        assert_eq!(
            parse(Format::Docx, &no_doc).unwrap_err().code,
            "docx_damaged"
        );
        let good = docx(&body("<w:p><w:r><w:t>x</w:t></w:r></w:p>"));
        let cut = &good[..good.len() / 2];
        assert_eq!(parse(Format::Docx, cut).unwrap_err().code, "docx_damaged");
    }

    /// The cap counts inflated bytes.
    #[test]
    fn capped_reader_stops_at_the_cap() {
        let mut c = Capped {
            inner: &[0u8; 100][..],
            left: 64,
        };
        let mut out = Vec::new();
        let e = c.read_to_end(&mut out).unwrap_err();
        assert!(e.get_ref().unwrap().is::<OverCap>());
    }
}
