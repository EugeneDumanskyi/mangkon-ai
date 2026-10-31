//! CSV (US-02.4): one segment per row, malformed rows kept and marked (INV-CO-10), read as a
//! stream (§4 #16, NFR-CAP-05).

use std::collections::HashMap;
use std::io::{BufReader, Read};

use mk_kernel::Locator;

use crate::content::{Format, ParseError, ParsedSegment, Place, RowState, Sink, Source};
use crate::encoding::{Decoded, TextEncoding, detect};

/// Delimiters tried, in tie-break order.
const DELIMITERS: [u8; 4] = *b",;\t|";
/// Rows read to pick the delimiter, the header and, without a header, the expected cell count.
const SAMPLE_ROWS: usize = 50;

/// Text in the encodings of NFR-COMP-06. The delimiter (US-02.4 AC1) and a header row (US-02.4
/// AC2) are found from the first [`SAMPLE_ROWS`] rows; then the file is read again from the
/// start, one row at a time, so it is never held whole. A row whose cell count differs from the
/// header's (or, without a header, from the most common count in the sample, ASSUMPTION) is
/// kept as a malformed segment, and the file gets one warning with the count (US-02.4 AC3,
/// INV-CO-10). Row numbers are 0-based over the file's records; a header is row 0. Blank lines
/// are not records.
pub(crate) fn parse(
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
) -> Result<(), ParseError> {
    let encoding = detect(input)?;
    let delimiter = detect_delimiter(input, encoding)?;
    let sample = read(Decoded::new(input, encoding)?, delimiter, SAMPLE_ROWS)?;

    let header = sample.first().is_some_and(|r| mk_kernel::csv_header_row(r));
    let expected = match (header, sample.first()) {
        (true, Some(first)) => first.len(),
        _ => mode(sample.iter().map(Vec::len)).unwrap_or(0),
    };
    let columns = if header {
        mk_kernel::csv_column_names(&sample[0])
    } else {
        vec![None; expected]
    };
    drop(sample);
    sink.document(place.start(Format::Csv, columns))?;

    let mut reader = reader(Decoded::new(input, encoding)?, delimiter);
    let mut record = ::csv::StringRecord::new();
    let mut malformed = 0usize;
    let mut row = 0u64;
    while reader.read_record(&mut record).map_err(unreadable)? {
        let row_state = if record.len() == expected {
            RowState::Normal
        } else {
            malformed += 1;
            RowState::Malformed
        };
        let mut text = String::with_capacity(record.as_slice().len() + record.len());
        let mut cell_starts = Vec::with_capacity(record.len());
        let mut at = 0u32;
        for (i, cell) in record.iter().enumerate() {
            if i > 0 {
                text.push('\t');
                at += 1;
            }
            cell_starts.push(at);
            text.push_str(cell);
            at += cell.chars().count() as u32;
        }
        sink.segment(ParsedSegment {
            locator: Locator::CsvRow { row },
            text,
            row_state,
            cell_starts,
            ocr: None,
        })?;
        row += 1;
    }
    match malformed {
        0 => {}
        1 => sink.warning("1 row has the wrong number of columns".to_string()),
        n => sink.warning(format!("{n} rows have the wrong number of columns")),
    }
    Ok(())
}

fn reader<R: Read>(input: R, delimiter: u8) -> ::csv::Reader<BufReader<R>> {
    ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(BufReader::with_capacity(1 << 16, input))
}

fn unreadable(e: ::csv::Error) -> ParseError {
    ParseError::new(
        "csv_unreadable",
        format!("The table could not be read: {e}"),
    )
}

/// The first `limit` records.
fn read<R: Read>(input: R, delimiter: u8, limit: usize) -> Result<Vec<Vec<String>>, ParseError> {
    reader(input, delimiter)
        .records()
        .take(limit)
        .map(|r| {
            r.map(|r| r.iter().map(str::to_string).collect())
                .map_err(unreadable)
        })
        .collect()
}

/// The delimiter whose sample rows most often share one cell count; then the one giving more
/// cells. A file where every candidate gives one cell per row is read with `,`.
fn detect_delimiter(input: &mut dyn Source, encoding: TextEncoding) -> Result<u8, ParseError> {
    let mut best = (DELIMITERS[0], 0usize, 0usize);
    for d in DELIMITERS {
        let Ok(rows) = read(Decoded::new(&mut *input, encoding)?, d, SAMPLE_ROWS) else {
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
    Ok(best.0)
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

/// The column names a CSV document's first record (row 0) gives, by the rule the parser uses
/// ([`mk_kernel::csv_columns`]). Lets a stored document get its columns back (M4b).
pub fn columns(first_row: &[String]) -> Vec<Option<String>> {
    mk_kernel::csv_columns(first_row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::ParsedContent;
    use mk_kernel::{Column, Span};

    fn parse(bytes: &[u8]) -> Result<ParsedContent, ParseError> {
        crate::content::parse(Format::Csv, bytes)
    }

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
            parse(b"a,b\n\x01\xff,1\n").unwrap_err().code,
            "unknown_text_encoding"
        );
        // US-02.3 AC3: Windows-1252 cells are decoded.
        let parsed = parse(b"name;city\nM\xfcller;K\xf6ln\n").unwrap();
        assert_eq!(rows(&parsed)[1].0, "Müller\tKöln");
    }

    /// Without a header the expected cell count is the sample's most common one, so a ragged
    /// row past the sample is still marked.
    #[test]
    fn expected_count_from_the_sample() {
        let mut csv = String::new();
        for i in 0..80 {
            csv.push_str(&format!("{i},x{i}@y.example\n"));
        }
        csv.push_str("80,x,extra\n");
        let parsed = parse(csv.as_bytes()).unwrap();
        assert_eq!(parsed.segment_count(), 81);
        assert_eq!(rows(&parsed)[80].1, RowState::Malformed);
        assert_eq!(
            parsed.warnings,
            vec!["1 row has the wrong number of columns"]
        );
    }
}
