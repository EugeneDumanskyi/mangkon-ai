//! How kernel values are stored: locators as `loc_kind` + `loc_pos` + `loc_ref` (+ span and
//! column on mentions), hashes as 32-byte blobs, cell tables as little-endian u32s, and an OCR'd
//! page's word geometry as a little-endian blob in `segment.ocr`.

use mk_kernel::{
    Column, ContentHash, ImageSize, LineNumber, Locator, OcrGeometry, OcrWord, PageNumber, Rect,
    Span,
};

use crate::StoreError;

pub(crate) const TEXT_LINE: &str = "text_line";
pub(crate) const CSV_ROW: &str = "csv_row";
pub(crate) const DOCX_PARAGRAPH: &str = "docx_paragraph";
pub(crate) const HTML_TEXT: &str = "html_text";
pub(crate) const JSON_VALUE: &str = "json_value";
pub(crate) const EMAIL_HEADER: &str = "email_header";
pub(crate) const MIME_PART: &str = "mime_part";
pub(crate) const PDF_TEXT: &str = "pdf_text";
pub(crate) const OCR_TEXT: &str = "ocr_text";

/// A segment locator's columns. `pos` is the line, row, paragraph, text offset or page (0 for
/// the kinds named by `loc_ref`); `loc_ref` is the JSON pointer, header name or MIME part.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Stored {
    pub kind: &'static str,
    pub pos: i64,
    pub loc_ref: Option<String>,
}

/// The columns of a segment locator. An OCR locator's boxes are not stored here: they come
/// back from the segment's word geometry (`segment.ocr`).
pub(crate) fn segment_locator(loc: &Locator) -> Result<Stored, StoreError> {
    let (kind, pos, loc_ref) = match loc {
        Locator::PdfText { page, .. } => (PDF_TEXT, i64::from(page.get()), None),
        Locator::OcrText { page, .. } => (OCR_TEXT, i64::from(page.get()), None),
        Locator::TextLine { line, .. } => (TEXT_LINE, i64::from(line.get()), None),
        Locator::CsvRow { row } => (CSV_ROW, *row as i64, None),
        Locator::DocxParagraph { paragraph, .. } => (DOCX_PARAGRAPH, i64::from(*paragraph), None),
        Locator::HtmlText { offset, .. } => (HTML_TEXT, *offset as i64, None),
        Locator::JsonValue { pointer, .. } => (JSON_VALUE, 0, Some(pointer.clone())),
        Locator::EmailHeader { name, .. } => (EMAIL_HEADER, 0, Some(name.clone())),
        Locator::MimePart { part, .. } => (MIME_PART, 0, Some(part.clone())),
        other => {
            return Err(StoreError::Invalid(format!(
                "segment locator not stored yet: {other:?}"
            )));
        }
    };
    Ok(Stored { kind, pos, loc_ref })
}

/// The segment locator back from its columns, its text and its OCR blob. Its span covers the
/// whole text.
pub(crate) fn segment_from(
    kind: &str,
    pos: i64,
    loc_ref: Option<String>,
    text: &str,
    ocr: Option<&[u8]>,
) -> Result<Locator, StoreError> {
    let len = text.chars().count();
    let span = || Span::new(0, len, len).map_err(|e| StoreError::Invalid(e.to_string()));
    let named = |loc_ref: Option<String>| {
        loc_ref.ok_or_else(|| StoreError::Invalid(format!("{kind} segment without loc_ref")))
    };
    Ok(match kind {
        TEXT_LINE => Locator::TextLine {
            line: line(pos)?,
            span: span()?,
        },
        CSV_ROW => Locator::CsvRow { row: pos as u64 },
        DOCX_PARAGRAPH => Locator::DocxParagraph {
            paragraph: u32::try_from(pos)
                .map_err(|_| StoreError::Invalid(format!("bad paragraph {pos}")))?,
            span: span()?,
        },
        HTML_TEXT => Locator::HtmlText {
            offset: u64::try_from(pos)
                .map_err(|_| StoreError::Invalid(format!("bad offset {pos}")))?,
            span: span()?,
        },
        JSON_VALUE => Locator::JsonValue {
            pointer: named(loc_ref)?,
            span: span()?,
        },
        EMAIL_HEADER => Locator::EmailHeader {
            name: named(loc_ref)?,
            span: span()?,
        },
        MIME_PART => Locator::MimePart {
            part: named(loc_ref)?,
            span: span()?,
        },
        PDF_TEXT => Locator::PdfText {
            page: page(pos)?,
            span: span()?,
        },
        OCR_TEXT => ocr_locator(page(pos)?, text, ocr, span()?)?,
        other => return Err(StoreError::Invalid(format!("unknown locator kind {other}"))),
    })
}

fn page(pos: i64) -> Result<PageNumber, StoreError> {
    u32::try_from(pos)
        .ok()
        .and_then(PageNumber::new)
        .ok_or_else(|| StoreError::Invalid(format!("bad page number {pos}")))
}

/// An OCR locator for `span` of an OCR'd page: the boxes of the words it touches, from the
/// stored geometry (INV-CO-03).
fn ocr_locator(
    page: PageNumber,
    text: &str,
    ocr: Option<&[u8]>,
    span: Span,
) -> Result<Locator, StoreError> {
    let ocr = ocr.ok_or_else(|| StoreError::Invalid("OCR segment without word geometry".into()))?;
    ocr_from_blob(ocr, text)?
        .locator(page, text, span)
        .map_err(|e| StoreError::Invalid(format!("OCR locator: {e}")))
}

/// Blob layout version of `segment.ocr`.
const OCR_BLOB_V1: u8 = 1;
/// Bytes per word: span start and end (u32), box (4 × f32), confidence (f32, NaN for none).
const OCR_WORD_BYTES: usize = 28;

/// An OCR'd page's word geometry as stored: version, image width and height, then each word.
/// The text hash is not stored: it is the segment's own text, checked when read back.
pub(crate) fn ocr_to_blob(g: &OcrGeometry) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + g.words().len() * OCR_WORD_BYTES);
    out.push(OCR_BLOB_V1);
    out.extend_from_slice(&g.image().width.to_le_bytes());
    out.extend_from_slice(&g.image().height.to_le_bytes());
    for w in g.words() {
        out.extend_from_slice(&(w.span.start() as u32).to_le_bytes());
        out.extend_from_slice(&(w.span.end() as u32).to_le_bytes());
        for v in [w.rect.x0(), w.rect.y0(), w.rect.x1(), w.rect.y1()] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&w.confidence.unwrap_or(f32::NAN).to_le_bytes());
    }
    out
}

/// The word geometry of an OCR'd segment whose stored text is `text`.
pub(crate) fn ocr_from_blob(blob: &[u8], text: &str) -> Result<OcrGeometry, StoreError> {
    let bad = |why: &str| StoreError::Invalid(format!("OCR geometry: {why}"));
    let (head, words) = blob.split_at_checked(9).ok_or_else(|| bad("too short"))?;
    if head[0] != OCR_BLOB_V1 || words.len() % OCR_WORD_BYTES != 0 {
        return Err(bad("unknown layout"));
    }
    let u = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"));
    let f = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"));
    let image = ImageSize {
        width: u(head, 1),
        height: u(head, 5),
    };
    let len = text.chars().count();
    let words = words
        .as_chunks::<OCR_WORD_BYTES>()
        .0
        .iter()
        .map(|w| {
            let span = Span::new(u(w, 0) as usize, u(w, 4) as usize, len)
                .map_err(|e| bad(&e.to_string()))?;
            let rect =
                Rect::new(f(w, 8), f(w, 12), f(w, 16), f(w, 20)).ok_or_else(|| bad("box"))?;
            let c = f(w, 24);
            Ok(OcrWord {
                span,
                rect,
                confidence: (!c.is_nan()).then_some(c),
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    OcrGeometry::new(text, image, words).map_err(|e| bad(&e.to_string()))
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

/// A mention's columns: its span and CSV column; the rest is its segment's.
pub(crate) struct MentionCols {
    pub start: i64,
    pub end: i64,
    pub column_index: Option<i64>,
    pub column_name: Option<String>,
}

/// A mention's locator from its segment's columns (text and OCR blob included) and its own.
pub(crate) fn mention_from(
    seg_kind: &str,
    seg_pos: i64,
    seg_ref: Option<String>,
    seg_text: &str,
    seg_ocr: Option<&[u8]>,
    m: MentionCols,
) -> Result<Locator, StoreError> {
    let span = Span::new(m.start as usize, m.end as usize, seg_text.chars().count())
        .map_err(|e| StoreError::Invalid(e.to_string()))?;
    if seg_kind == OCR_TEXT {
        return ocr_locator(page(seg_pos)?, seg_text, seg_ocr, span);
    }
    if seg_kind == CSV_ROW {
        let column = match (m.column_name, m.column_index) {
            (Some(n), _) => Column::Name(n),
            (None, Some(i)) => Column::Index(i as u32),
            (None, None) => {
                return Err(StoreError::Invalid("CSV mention without column".into()));
            }
        };
        return Ok(Locator::CsvCell {
            row: seg_pos as u64,
            column,
            span,
        });
    }
    segment_from(seg_kind, seg_pos, seg_ref, seg_text, seg_ocr)?
        .with_span(span)
        .ok_or_else(|| StoreError::Invalid(format!("no mention locator for {seg_kind}")))
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn kinds(n: u32, name: String, text_len: usize) -> Vec<Locator> {
        let whole = Span::new(0, text_len, text_len).unwrap();
        vec![
            Locator::TextLine {
                line: LineNumber::new(n + 1).unwrap(),
                span: whole,
            },
            Locator::DocxParagraph {
                paragraph: n,
                span: whole,
            },
            Locator::HtmlText {
                offset: u64::from(n),
                span: whole,
            },
            Locator::JsonValue {
                pointer: format!("/{name}/{n}"),
                span: whole,
            },
            Locator::EmailHeader {
                name: name.clone(),
                span: whole,
            },
            Locator::MimePart {
                part: format!("{}.{n}", n % 3 + 1),
                span: whole,
            },
            Locator::PdfText {
                page: PageNumber::new(n % 2000 + 1).unwrap(),
                span: whole,
            },
        ]
    }

    proptest! {
        /// INV-CO-03: every stored segment kind and its mentions come back as they went in.
        #[test]
        fn inv_co_03_store_codec_round_trip(
            n in 0u32..1_000_000,
            name in "[A-Za-z~/ -]{1,12}",
            text_len in 1usize..200,
            a in 0usize..200,
            b in 0usize..200,
        ) {
            let (start, end) = (a.min(b) % text_len, (a.max(b) % text_len) + 1);
            prop_assume!(start < end);
            let text = "x".repeat(text_len);
            for loc in kinds(n, name.clone(), text_len) {
                let stored = segment_locator(&loc).unwrap();
                let back =
                    segment_from(stored.kind, stored.pos, stored.loc_ref.clone(), &text, None)
                        .unwrap();
                prop_assert_eq!(&back, &loc);
                let span = Span::new(start, end, text_len).unwrap();
                let mention = loc.with_span(span).unwrap();
                let cols = MentionCols {
                    start: start as i64,
                    end: end as i64,
                    column_index: None,
                    column_name: None,
                };
                let back =
                    mention_from(stored.kind, stored.pos, stored.loc_ref, &text, None, cols)
                        .unwrap();
                prop_assert_eq!(back, mention);
            }
            let row = Locator::CsvRow { row: u64::from(n) };
            let stored = segment_locator(&row).unwrap();
            prop_assert_eq!(segment_from(stored.kind, stored.pos, None, &text, None).unwrap(), row);
        }

        /// INV-CO-03, US-02.2 AC3: an OCR'd page's words, boxes and confidences come back from
        /// the blob, and a mention's boxes are those of the words its span touches.
        #[test]
        fn inv_co_03_ocr_geometry_round_trip(
            words in proptest::collection::vec(("[a-z]{1,8}", 0.0f32..1.0, any::<bool>()), 1..30),
            page_n in 1u32..2000,
        ) {
            let mut text = String::new();
            let mut out = Vec::new();
            for (i, (w, c, has)) in words.iter().enumerate() {
                if i > 0 {
                    text.push(' ');
                }
                let start = text.chars().count();
                text.push_str(w);
                let end = text.chars().count();
                out.push((start, end, i as f32 * 20.0, *c, *has));
            }
            let len = text.chars().count();
            let words: Vec<OcrWord> = out
                .iter()
                .map(|&(s, e, x, c, has)| OcrWord {
                    span: Span::new(s, e, len).unwrap(),
                    rect: Rect::new(x, 5.0, x + 15.0, 25.0).unwrap(),
                    confidence: has.then_some(c),
                })
                .collect();
            let image = ImageSize { width: 2480, height: 3508 };
            let geo = OcrGeometry::new(&text, image, words).unwrap();
            let blob = ocr_to_blob(&geo);
            prop_assert_eq!(&ocr_from_blob(&blob, &text).unwrap(), &geo);
            let page = PageNumber::new(page_n).unwrap();
            let whole = Span::new(0, len, len).unwrap();
            let seg = geo.locator(page, &text, whole).unwrap();
            let stored = segment_locator(&seg).unwrap();
            prop_assert_eq!(
                &segment_from(stored.kind, stored.pos, None, &text, Some(&blob)).unwrap(),
                &seg
            );
            let (s, e) = (out[0].0, out[out.len() / 2].1);
            let cols = MentionCols {
                start: s as i64,
                end: e as i64,
                column_index: None,
                column_name: None,
            };
            let m = mention_from(stored.kind, stored.pos, None, &text, Some(&blob), cols).unwrap();
            prop_assert_eq!(m, geo.locator(page, &text, Span::new(s, e, len).unwrap()).unwrap());
        }
    }
}
