//! How kernel values are stored: locators as `loc_kind` + `loc_pos` (+ span and column on
//! mentions), hashes as 32-byte blobs, cell tables as little-endian u32s.

use mk_kernel::{Column, ContentHash, LineNumber, Locator, Span};

use crate::StoreError;

pub(crate) const TEXT_LINE: &str = "text_line";
pub(crate) const CSV_ROW: &str = "csv_row";

/// `(loc_kind, loc_pos)` of a segment locator. M2b stores TXT/MD lines and CSV rows; the PDF and
/// OCR kinds get their columns in M3.
pub(crate) fn segment_locator(loc: &Locator) -> Result<(&'static str, i64), StoreError> {
    match loc {
        Locator::TextLine { line, .. } => Ok((TEXT_LINE, i64::from(line.get()))),
        Locator::CsvRow { row } => Ok((CSV_ROW, *row as i64)),
        other => Err(StoreError::Invalid(format!(
            "segment locator not stored yet: {other:?}"
        ))),
    }
}

/// The segment locator back from its columns. A text line's span covers the whole line.
pub(crate) fn segment_from(kind: &str, pos: i64, text: &str) -> Result<Locator, StoreError> {
    match kind {
        TEXT_LINE => {
            let len = text.chars().count();
            Ok(Locator::TextLine {
                line: line(pos)?,
                span: Span::new(0, len, len).map_err(|e| StoreError::Invalid(e.to_string()))?,
            })
        }
        CSV_ROW => Ok(Locator::CsvRow { row: pos as u64 }),
        other => Err(StoreError::Invalid(format!("unknown locator kind {other}"))),
    }
}

fn line(pos: i64) -> Result<LineNumber, StoreError> {
    u32::try_from(pos)
        .ok()
        .and_then(LineNumber::new)
        .ok_or_else(|| StoreError::Invalid(format!("bad line number {pos}")))
}

/// A mention's column as `(column_index, column_name)`. The index is always stored; the name
/// only for a named column.
pub(crate) fn column(loc: &Locator, cell_index: Option<usize>) -> (Option<i64>, Option<String>) {
    match loc {
        Locator::CsvCell { column, .. } => match column {
            Column::Name(n) => (cell_index.map(|i| i as i64), Some(n.clone())),
            Column::Index(i) => (Some(i64::from(*i)), None),
        },
        _ => (None, None),
    }
}

/// A mention's locator from its segment's columns and its own.
pub(crate) fn mention_from(
    seg_kind: &str,
    seg_pos: i64,
    seg_text_len: usize,
    start: i64,
    end: i64,
    column_index: Option<i64>,
    column_name: Option<String>,
) -> Result<Locator, StoreError> {
    let span = Span::new(start as usize, end as usize, seg_text_len)
        .map_err(|e| StoreError::Invalid(e.to_string()))?;
    match seg_kind {
        TEXT_LINE => Ok(Locator::TextLine {
            line: line(seg_pos)?,
            span,
        }),
        CSV_ROW => {
            let column = match (column_name, column_index) {
                (Some(n), _) => Column::Name(n),
                (None, Some(i)) => Column::Index(i as u32),
                (None, None) => {
                    return Err(StoreError::Invalid("CSV mention without column".into()));
                }
            };
            Ok(Locator::CsvCell {
                row: seg_pos as u64,
                column,
                span,
            })
        }
        other => Err(StoreError::Invalid(format!("unknown locator kind {other}"))),
    }
}

pub(crate) fn hash_from(blob: &[u8]) -> Result<ContentHash, StoreError> {
    let bytes: [u8; 32] = blob
        .try_into()
        .map_err(|_| StoreError::Invalid("content hash is not 32 bytes".into()))?;
    Ok(ContentHash::from_bytes(bytes))
}

pub(crate) fn cells_to_blob(cells: &[u32]) -> Option<Vec<u8>> {
    (!cells.is_empty()).then(|| cells.iter().flat_map(|c| c.to_le_bytes()).collect())
}

pub(crate) fn cells_from_blob(blob: Option<Vec<u8>>) -> Vec<u32> {
    blob.map(|b| {
        b.as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect()
    })
    .unwrap_or_default()
}
