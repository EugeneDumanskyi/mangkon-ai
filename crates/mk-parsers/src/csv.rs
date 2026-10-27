//! CSV (US-02.4): one segment per row, malformed rows kept and marked (INV-CO-10).

use std::collections::HashMap;

use mk_kernel::Locator;

use crate::content::{Format, ParseError, ParsedContent, ParsedDocument, ParsedSegment, RowState};

/// Delimiters tried, in tie-break order.
const DELIMITERS: [u8; 4] = *b",;\t|";
/// Rows read to pick the delimiter.
const SAMPLE_ROWS: usize = 50;

/// UTF-8, with or without a byte-order mark. The delimiter is detected (US-02.4 AC1), and so is
/// a header row (US-02.4 AC2). A row whose cell count differs from the header's (or, without a
/// header, from the most common count) is kept as a malformed segment, and the file gets one
/// warning with the count (US-02.4 AC3, INV-CO-10). Row numbers are 0-based over the file's
/// records; a header is row 0. Blank lines are not records.
pub fn parse(bytes: &[u8]) -> Result<ParsedContent, ParseError> {
    let text = crate::content::decode(bytes)?;
    let delimiter = detect_delimiter(text);
    let rows = read(text, delimiter, usize::MAX)?;

    let header = rows.first().is_some_and(|r| looks_like_header(r));
    let expected = match (header, rows.first()) {
        (true, Some(first)) => first.len(),
        _ => mode(rows.iter().map(Vec::len)).unwrap_or(0),
    };
    let columns = if header {
        let first = &rows[0];
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for name in first {
            *seen.entry(name.trim()).or_default() += 1;
        }
        first
            .iter()
            .map(|n| {
                let n = n.trim();
                (!n.is_empty() && seen[n] == 1).then(|| n.to_string())
            })
            .collect()
    } else {
        vec![None; expected]
    };

    let mut malformed = 0usize;
    let mut segments = Vec::with_capacity(rows.len());
    for (row, cells) in rows.into_iter().enumerate() {
        let row_state = if cells.len() == expected {
            RowState::Normal
        } else {
            malformed += 1;
            RowState::Malformed
        };
        let mut text = String::new();
        let mut cell_starts = Vec::with_capacity(cells.len());
        let mut at = 0u32;
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                text.push('\t');
                at += 1;
            }
            cell_starts.push(at);
            text.push_str(cell);
            at += cell.chars().count() as u32;
        }
        segments.push(ParsedSegment {
            locator: Locator::CsvRow { row: row as u64 },
            text,
            row_state,
            cell_starts,
        });
    }
    let warnings = match malformed {
        0 => Vec::new(),
        1 => vec!["1 row has the wrong number of columns".to_string()],
        n => vec![format!("{n} rows have the wrong number of columns")],
    };
    Ok(ParsedContent {
        documents: vec![ParsedDocument {
            kind: Format::Csv,
            position: "whole".into(),
            columns,
            segments,
        }],
        warnings,
    })
}

fn read(text: &str, delimiter: u8, limit: usize) -> Result<Vec<Vec<String>>, ParseError> {
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    reader
        .records()
        .take(limit)
        .map(|r| {
            r.map(|r| r.iter().map(str::to_string).collect())
                .map_err(|e| {
                    ParseError::new(
                        "csv_unreadable",
                        format!("The table could not be read: {e}"),
                    )
                })
        })
        .collect()
}

/// The delimiter whose sample rows most often share one cell count; then the one giving more
/// cells. A file where every candidate gives one cell per row is read with `,`.
fn detect_delimiter(text: &str) -> u8 {
    let mut best = (DELIMITERS[0], 0usize, 0usize);
    for d in DELIMITERS {
        let Ok(rows) = read(text, d, SAMPLE_ROWS) else {
            continue;
        };
        let Some(common) = mode(rows.iter().map(Vec::len)) else {
            continue;
        };
        if common < 2 {
            continue;
        }
        let consistent = rows.iter().filter(|r| r.len() == common).count();
        if (consistent, common) > (best.1, best.2) {
            best = (d, consistent, common);
        }
    }
    best.0
}

/// The most common value; the smallest on a tie.
fn mode(values: impl Iterator<Item = usize>) -> Option<usize> {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(v, n)| (*n, std::cmp::Reverse(*v)))
        .map(|(v, _)| v)
}

/// A first row is a header when every cell has a letter and none looks like a number or date.
/// ASSUMPTION: good enough for exports; the column mapping screen (M4) lets the user correct it.
fn looks_like_header(row: &[String]) -> bool {
    !row.is_empty()
        && row.iter().all(|c| {
            let c = c.trim();
            let numeric = c
                .chars()
                .all(|ch| ch.is_ascii_digit() || " .,:/-+".contains(ch));
            c.chars().any(char::is_alphabetic) && !numeric
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mk_kernel::{Column, Span};

    fn rows(parsed: &ParsedContent) -> Vec<(String, RowState)> {
        parsed.documents[0]
            .segments
            .iter()
            .map(|s| (s.text.clone(), s.row_state))
            .collect()
    }

    /// US-02.4 AC1: each delimiter is detected.
    #[test]
    fn us_02_4_ac1_delimiters() {
        for d in [",", ";", "\t", "|"] {
            let csv = format!("name{d}email\nAnna{d}a@x.example\nBen{d}b@y.example\n");
            let parsed = parse(csv.as_bytes()).unwrap();
            assert_eq!(
                rows(&parsed)[1],
                ("Anna\ta@x.example".into(), RowState::Normal),
                "{d:?}"
            );
        }
    }

    /// A decimal comma does not win over the real delimiter.
    #[test]
    fn decimal_commas_do_not_fool_detection() {
        let csv = "date;amount;iban\n12.03.2024;14 650,00;GB33\n18.06.2024;9 980,00;GB34\n";
        let parsed = parse(csv.as_bytes()).unwrap();
        assert_eq!(rows(&parsed)[1].0, "12.03.2024\t14 650,00\tGB33");
    }

    /// US-02.4 AC2: header names become column names; duplicates fall back to the index.
    /// Without a header, every column is an index.
    #[test]
    fn us_02_4_ac2_header_names_columns() {
        let parsed = parse(b"id,email,note,note\n1,a@x.example,n,m\n").unwrap();
        let doc = &parsed.documents[0];
        assert_eq!(
            doc.columns,
            vec![Some("id".into()), Some("email".into()), None, None]
        );
        let span = Span::new(2, 13, 17).unwrap();
        assert_eq!(
            doc.mention_locator(1, span),
            Some(Locator::CsvCell {
                row: 1,
                column: Column::Name("email".into()),
                span
            })
        );
        let parsed = parse(b"1,a@x.example\n2,b@y.example\n").unwrap();
        let doc = &parsed.documents[0];
        assert_eq!(doc.columns, vec![None, None]);
        assert_eq!(
            doc.mention_locator(0, span_of(&doc.segments[0].text, "a@x.example")),
            Some(Locator::CsvCell {
                row: 0,
                column: Column::Index(1),
                span: span_of(&doc.segments[0].text, "a@x.example")
            })
        );
    }

    fn span_of(text: &str, needle: &str) -> Span {
        let start = text[..text.find(needle).unwrap()].chars().count();
        Span::new(start, start + needle.chars().count(), text.chars().count()).unwrap()
    }

    /// US-02.4 AC3, INV-CO-10: rows with the wrong cell count are kept, marked, and counted in
    /// one warning; the file does not fail.
    #[test]
    fn inv_co_10_malformed_rows_kept() {
        let parsed = parse(b"a,b,c\n1,2,3\n4,5\n6,7,8,9\n10,11,12\n").unwrap();
        let states: Vec<RowState> = rows(&parsed).into_iter().map(|r| r.1).collect();
        use RowState::*;
        assert_eq!(states, vec![Normal, Normal, Malformed, Malformed, Normal]);
        assert_eq!(
            parsed.warnings,
            vec!["2 rows have the wrong number of columns"]
        );
    }

    /// Quoted cells keep delimiters, quotes and line breaks; a cell with a tab still maps a span
    /// to the right cell through the offset table; BOM and CRLF are handled.
    #[test]
    fn quoted_cells_bom_and_crlf() {
        let csv =
            "\u{feff}k,v\r\n1,\"say \"\"hi\"\", ok\"\r\n2,\"tab\there\nnext a@x.example\"\r\n";
        let parsed = parse(csv.as_bytes()).unwrap();
        let doc = &parsed.documents[0];
        assert_eq!(doc.segments[1].text, "1\tsay \"hi\", ok");
        let seg = &doc.segments[2];
        assert_eq!(seg.text, "2\ttab\there\nnext a@x.example");
        assert_eq!(seg.cell_starts, vec![0, 2]);
        let span = span_of(&seg.text, "a@x.example");
        assert_eq!(seg.cell_of(span).map(|c| c.0), Some(1));
        let across = Span::new(0, 3, seg.text.chars().count()).unwrap();
        assert_eq!(seg.cell_of(across), None);
        assert_eq!(doc.mention_locator(2, across), None);
    }

    #[test]
    fn empty_file_and_encoding() {
        let parsed = parse(b"").unwrap();
        assert_eq!(parsed.segment_count(), 0);
        assert!(parsed.warnings.is_empty());
        assert_eq!(
            parse(b"a,b\n\x80,1\n").unwrap_err().code,
            "unknown_text_encoding"
        );
    }
}
