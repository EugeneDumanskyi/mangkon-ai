use std::fmt;
use std::num::NonZeroU32;

use crate::Span;

/// 1-based page number of a paged document.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
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

/// Exact address of a segment in its document (domain §3.3, INV-CO-03).
///
/// The kind must match the document kind. Drafted kinds:
/// - `PdfText` (M0b): text-layer PDF page.
///
/// Still to come, each in the milestone that adds its parser:
/// - OCR page: page + word boxes + span (M0c). Unlike `PdfText` it keeps its boxes, because
///   the OCR result is the only source of them.
/// - TXT/MD line + span, DOCX paragraph + span, HTML text offset + span, CSV row + column,
///   JSON path, email header name, MIME part + span (M2/M3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Locator {
    /// A place in the stored text of one PDF page (US-02.1 AC1). In M0b a page is one segment,
    /// so `span` counts characters of the page's stored text.
    ///
    /// No boxes are stored: the page view asks the same engine for the page's
    /// [`PageGeometry`](crate::PageGeometry) when it renders the page (ADR-0011) and maps the
    /// span through it.
    PdfText { page: PageNumber, span: Span },
}

impl Locator {
    pub fn page(&self) -> Option<PageNumber> {
        match self {
            Locator::PdfText { page, .. } => Some(*page),
        }
    }

    pub fn span(&self) -> Option<Span> {
        match self {
            Locator::PdfText { span, .. } => Some(*span),
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
}
