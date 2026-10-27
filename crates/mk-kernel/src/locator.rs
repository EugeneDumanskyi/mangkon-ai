use std::fmt;
use std::num::NonZeroU32;

use crate::{ImageSize, Rect, Span};

/// 1-based page number of a paged document.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct PageNumber(NonZeroU32);

impl PageNumber {
    /// `None` for page 0: pages count from 1.
    pub fn new(n: u32) -> Option<Self> {
        NonZeroU32::new(n).map(Self)
    }

    pub fn get(&self) -> u32 {
        self.0.get()
    }

    /// 0-based index, as engines number pages.
    pub fn index(&self) -> usize {
        self.0.get() as usize - 1
    }
}

impl fmt::Display for PageNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 1-based line number of a text file, counting every line of the decoded file (empty ones too).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct LineNumber(NonZeroU32);

impl LineNumber {
    /// `None` for line 0: lines count from 1.
    pub fn new(n: u32) -> Option<Self> {
        NonZeroU32::new(n).map(Self)
    }

    pub fn get(&self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for LineNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A CSV column: its header name when the file has a header with a unique, non-empty name for
/// it, otherwise its 0-based index.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Column {
    Name(String),
    Index(u32),
}

/// Exact address of a segment in its document (domain §3.3, INV-CO-03).
///
/// The kind must match the document kind. Drafted kinds:
/// - `PdfText` (M0b): text-layer PDF page.
/// - `OcrText` (M0c): OCR'd page of an image or scanned PDF.
///
/// - `TextLine` (M2b): one line of a TXT or MD file.
/// - `CsvRow` / `CsvCell` (M2b): a CSV row as a segment, and a place in one of its cells.
///
/// Still to come, each in the milestone that adds its parser: DOCX paragraph + span, HTML text
/// offset + span, JSON path, email header name, MIME part + span (M3).
#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Locator {
    /// A place in the stored text of one PDF page (US-02.1 AC1). In M0b a page is one segment,
    /// so `span` counts characters of the page's stored text.
    ///
    /// No boxes are stored: the page view asks the same engine for the page's
    /// [`PageGeometry`](crate::PageGeometry) when it renders the page (ADR-0011) and maps the
    /// span through it.
    PdfText { page: PageNumber, span: Span },
    /// A place in the OCR text of one page of an image or scanned PDF (US-02.2, US-08.1 AC2).
    /// `span` counts characters of the page's stored OCR text.
    ///
    /// Unlike `PdfText` it keeps its boxes: OCR is their only source, and OCR'ing again may not
    /// give the same words (ADR-0011). `boxes` are the boxes of the words the span touches, in
    /// pixels of the OCR'd image, whose size is `image`; the viewer scales them to its render.
    /// Built by [`OcrGeometry::locator`](crate::OcrGeometry::locator).
    OcrText {
        page: PageNumber,
        span: Span,
        boxes: Vec<Rect>,
        image: ImageSize,
    },
    /// A place in one line of a TXT or MD file (US-02.3 AC1). One segment per non-empty line,
    /// its stored text being the line without its line end; `span` counts characters of it.
    TextLine { line: LineNumber, span: Span },
    /// One CSV row as a segment (US-02.4). `row` is 0-based over the file's records, the header
    /// (if any) being row 0. The stored text is the row's cells joined by `\t`, with a table of
    /// where each cell starts, so a span in it can be mapped back to its cell.
    CsvRow { row: u64 },
    /// A place in one cell of a CSV row: the row, the cell's column, and a `span` in the **row's
    /// stored text** (INV-CO-04), which lies inside that cell.
    CsvCell {
        row: u64,
        column: Column,
        span: Span,
    },
}

impl Locator {
    pub fn page(&self) -> Option<PageNumber> {
        match self {
            Locator::PdfText { page, .. } | Locator::OcrText { page, .. } => Some(*page),
            Locator::TextLine { .. } | Locator::CsvRow { .. } | Locator::CsvCell { .. } => None,
        }
    }

    pub fn span(&self) -> Option<Span> {
        match self {
            Locator::PdfText { span, .. }
            | Locator::OcrText { span, .. }
            | Locator::TextLine { span, .. }
            | Locator::CsvCell { span, .. } => Some(*span),
            Locator::CsvRow { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_zero_rejected() {
        assert_eq!(PageNumber::new(0), None);
        let p = PageNumber::new(3).unwrap();
        assert_eq!((p.get(), p.index()), (3, 2));
    }

    /// INV-CO-03: a PDF locator is page + span, both always valid.
    #[test]
    fn inv_co_03_pdf_locator_is_page_and_span() {
        let page = PageNumber::new(7).unwrap();
        let span = Span::new(4, 9, 20).unwrap();
        let loc = Locator::PdfText { page, span };
        assert_eq!(loc.page(), Some(page));
        assert_eq!(loc.span(), Some(span));
    }

    /// INV-CO-03: a TXT/MD locator is line + span; a CSV mention is row + column + span, and
    /// the CSV row segment itself has no span.
    #[test]
    fn inv_co_03_text_and_csv_locators() {
        assert_eq!(LineNumber::new(0), None);
        let line = LineNumber::new(12).unwrap();
        let span = Span::new(0, 5, 5).unwrap();
        let loc = Locator::TextLine { line, span };
        assert_eq!((loc.page(), loc.span()), (None, Some(span)));
        assert_eq!(Locator::CsvRow { row: 3 }.span(), None);
        let cell = Locator::CsvCell {
            row: 3,
            column: Column::Name("email".into()),
            span,
        };
        assert_eq!((cell.page(), cell.span()), (None, Some(span)));
    }
}
