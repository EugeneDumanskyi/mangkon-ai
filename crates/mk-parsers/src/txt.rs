//! TXT and MD (US-02.3 AC1): one segment per non-blank line.

use mk_kernel::{LineNumber, Locator, Span};

use crate::content::{Format, ParseError, ParsedContent, ParsedDocument, ParsedSegment, RowState};

/// UTF-8, with or without a byte-order mark. Lines end at `\n`; a `\r` before it is dropped, so
/// segment text matches the ground truth's decoded line (`docs/07-test-corpus.md` §5). Lines
/// with only whitespace give no segment but still count for line numbers. Markdown is kept as
/// written.
pub fn parse(format: Format, bytes: &[u8]) -> Result<ParsedContent, ParseError> {
    let text = crate::content::decode(bytes)?;
    let mut segments = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        let number = u32::try_from(i + 1)
            .ok()
            .and_then(LineNumber::new)
            .ok_or_else(|| ParseError::new("too_many_lines", "The file has too many lines"))?;
        let len = line.chars().count();
        segments.push(ParsedSegment {
            locator: Locator::TextLine {
                line: number,
                span: Span::new(0, len, len).expect("non-blank line"),
            },
            text: line.to_string(),
            row_state: RowState::Normal,
            cell_starts: Vec::new(),
        });
    }
    Ok(ParsedContent {
        documents: vec![ParsedDocument {
            kind: format,
            position: "whole".into(),
            columns: Vec::new(),
            segments,
        }],
        warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// US-02.7 AC1, NFR-USE-05: a non-UTF-8 file fails alone with a plain reason.
    #[test]
    fn us_02_7_ac1_other_encodings_fail_plainly() {
        for bytes in [&b"caf\xe9"[..], b"\xff\xfeh\x00i\x00"] {
            let e = parse(Format::Text, bytes).unwrap_err();
            assert_eq!(e.code, "unknown_text_encoding");
            assert_eq!(e.to_string(), "Unknown text encoding");
        }
    }
}
