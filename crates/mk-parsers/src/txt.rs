//! TXT and MD (US-02.3 AC1): one segment per non-blank line, read as a stream.

use std::io::{BufRead, BufReader};

use mk_kernel::{LineNumber, Locator};

use crate::content::{Format, ParseError, ParsedSegment, Place, Sink, Source};
use crate::encoding::{Decoded, detect};

/// UTF-8, UTF-16 with a byte-order mark, or Windows-1252 (NFR-COMP-06). Lines end at `\n`; a
/// `\r` before it is dropped, so segment text matches the ground truth's decoded line
/// (`docs/07-test-corpus.md` §5). Lines with only whitespace give no segment but still count
/// for line numbers. Markdown is kept as written.
pub(crate) fn parse(
    format: Format,
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
) -> Result<(), ParseError> {
    let encoding = detect(input)?;
    sink.document(place.start(format, Vec::new()))?;
    let mut reader = BufReader::with_capacity(1 << 16, Decoded::new(input, encoding)?);
    let mut line = String::new();
    let mut number = 0u32;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(ParseError::io)? == 0 {
            return Ok(());
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| ParseError::new("too_many_lines", "The file has too many lines"))?;
        let text = line.strip_suffix('\n').unwrap_or(&line);
        let text = text.strip_suffix('\r').unwrap_or(text);
        if text.trim().is_empty() {
            continue;
        }
        let line = LineNumber::new(number).expect("counted from 1");
        if let Some(seg) =
            ParsedSegment::whole(|span| Locator::TextLine { line, span }, text.to_string())
        {
            sink.segment(seg)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::content::{Format, parse};
    use mk_kernel::Locator;

    fn lines(bytes: &[u8]) -> Vec<(u32, String)> {
        let parsed = parse(Format::Text, bytes).unwrap();
        parsed.documents[0]
            .segments
            .iter()
            .map(|s| match &s.locator {
                Locator::TextLine { line, .. } => (line.get(), s.text.clone()),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// US-02.3 AC1, INV-CO-03: line + span, line numbers count blank lines, line ends removed.
    #[test]
    fn us_02_3_ac1_lines_with_numbers() {
        assert_eq!(
            lines(b"first\r\n\r\n  \nfourth\nlast"),
            vec![
                (1, "first".into()),
                (4, "fourth".into()),
                (5, "last".into())
            ]
        );
    }

    #[test]
    fn bom_is_stripped_and_empty_file_has_no_segments() {
        assert_eq!(lines(b"\xef\xbb\xbfhello\n"), vec![(1, "hello".into())]);
        let parsed = parse(Format::Markdown, b"").unwrap();
        assert_eq!(parsed.documents.len(), 1);
        assert_eq!(parsed.documents[0].kind, Format::Markdown);
        assert_eq!(parsed.segment_count(), 0);
    }

    /// NFR-COMP-06, US-02.3 AC3: UTF-16 with a mark and Windows-1252 are decoded; line numbers
    /// count the decoded lines.
    #[test]
    fn nfr_comp_06_encodings() {
        assert_eq!(
            lines(b"caf\xe9\r\n\r\nStra\xdfe \x80"),
            vec![(1, "café".into()), (3, "Straße €".into())]
        );
        let mut le = vec![0xff, 0xfe];
        let mut be = vec![0xfe, 0xff];
        for u in "a\nZoë a@x.example\n".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
            be.extend_from_slice(&u.to_be_bytes());
        }
        let want = vec![(1, "a".to_string()), (2, "Zoë a@x.example".to_string())];
        assert_eq!(lines(&le), want);
        assert_eq!(lines(&be), want);
    }

    /// US-02.7 AC1, NFR-USE-05: a binary file or cut UTF-8 fails alone with a plain reason.
    #[test]
    fn us_02_7_ac1_unknown_encodings_fail_plainly() {
        for bytes in [&b"\x89PNG\r\n\x1a\n"[..], b"cut \xc3"] {
            let e = parse(Format::Text, bytes).unwrap_err();
            assert_eq!(e.code, "unknown_text_encoding");
            assert_eq!(e.to_string(), "Unknown text encoding");
        }
    }
}
