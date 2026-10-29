use crate::{ContentHash, GeometryError, Locator, PageNumber, Rect, Span};

/// Pixel size of the image a page was OCR'd from. OCR boxes are in pixels of this image, so the
/// viewer can scale them to whatever size it renders the page at (ADR-0011).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImageSize {
    pub width: u32,
    pub height: u32,
}

/// One recognised word of a page's OCR text.
#[derive(Clone, Copy, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OcrWord {
    /// Where the word sits in the page text.
    pub span: Span,
    /// Word box in pixels of the OCR'd image, top-left origin, y down.
    pub rect: Rect,
    /// Engine confidence in `[0, 1]`; `None` if the engine gives none.
    pub confidence: Option<f32>,
}

/// Word boxes for one page's OCR text, built when the page is OCR'd.
///
/// OCR is the only source of these boxes, and running it again may not give the same result, so
/// the boxes go into the stored [`Locator::OcrText`] instead of being rebuilt when the page is
/// viewed (INV-CO-03, ADR-0011). Like [`PageGeometry`](crate::PageGeometry), it records the
/// length and hash of the text it was built from, and maps a span only through that exact text
/// (INV-CO-04).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(try_from = "RawOcrGeometry")
)]
pub struct OcrGeometry {
    text_len: usize,
    text_hash: ContentHash,
    image: ImageSize,
    /// Sorted by span, no two overlapping. Characters between words (spaces and newlines the
    /// layout inserted) belong to no word.
    words: Vec<OcrWord>,
}

/// The decoded form of an [`OcrGeometry`], checked by the same rules as [`OcrGeometry::new`]
/// before it becomes one.
#[cfg(feature = "serde")]
#[derive(serde::Deserialize)]
struct RawOcrGeometry {
    text_len: usize,
    text_hash: ContentHash,
    image: ImageSize,
    words: Vec<OcrWord>,
}

#[cfg(feature = "serde")]
impl TryFrom<RawOcrGeometry> for OcrGeometry {
    type Error = GeometryError;

    fn try_from(r: RawOcrGeometry) -> Result<Self, GeometryError> {
        check_words(&r.words, r.text_len)?;
        Ok(Self {
            text_len: r.text_len,
            text_hash: r.text_hash,
            image: r.image,
            words: r.words,
        })
    }
}

/// Words in text order, spans inside `text_len` and not overlapping, confidences in `[0, 1]`.
fn check_words(words: &[OcrWord], text_len: usize) -> Result<(), GeometryError> {
    let mut end = 0;
    for (index, w) in words.iter().enumerate() {
        if w.span.start() < end || w.span.end() > text_len {
            return Err(GeometryError::WordSpan { index });
        }
        if w.confidence.is_some_and(|c| !(0.0..=1.0).contains(&c)) {
            return Err(GeometryError::Confidence { index });
        }
        end = w.span.end();
    }
    Ok(())
}

impl OcrGeometry {
    /// `words` must be in text order, with spans inside `page_text` that do not overlap, and
    /// confidences in `[0, 1]`.
    pub fn new(
        page_text: &str,
        image: ImageSize,
        words: Vec<OcrWord>,
    ) -> Result<Self, GeometryError> {
        let text_len = page_text.chars().count();
        check_words(&words, text_len)?;
        Ok(Self {
            text_len,
            text_hash: ContentHash::of(page_text.as_bytes()),
            image,
            words,
        })
    }

    /// The mean confidence of the page's words that have one; `None` if none has. A page under
    /// 0.6 is "Low OCR quality" (US-02.2 AC3).
    pub fn mean_confidence(&self) -> Option<f32> {
        let (sum, n) = self
            .words
            .iter()
            .filter_map(|w| w.confidence)
            .fold((0.0f64, 0usize), |(s, n), c| (s + f64::from(c), n + 1));
        (n > 0).then(|| (sum / n as f64) as f32)
    }

    pub fn text_len(&self) -> usize {
        self.text_len
    }

    pub fn text_hash(&self) -> ContentHash {
        self.text_hash
    }

    pub fn image(&self) -> ImageSize {
        self.image
    }

    pub fn words(&self) -> &[OcrWord] {
        &self.words
    }

    fn check(&self, page_text: &str, span: Span) -> Result<(), GeometryError> {
        let stored = page_text.chars().count();
        if stored != self.text_len {
            return Err(GeometryError::LengthMismatch {
                stored,
                geometry: self.text_len,
            });
        }
        if ContentHash::of(page_text.as_bytes()) != self.text_hash {
            return Err(GeometryError::HashMismatch);
        }
        if span.end() > self.text_len {
            return Err(GeometryError::SpanOutOfBounds {
                end: span.end(),
                len: self.text_len,
            });
        }
        Ok(())
    }

    /// The words `span` touches, in text order.
    fn touched(&self, span: Span) -> impl Iterator<Item = &OcrWord> {
        let first = self.words.partition_point(|w| w.span.end() <= span.start());
        self.words[first..]
            .iter()
            .take_while(move |w| w.span.start() < span.end())
    }

    /// Highlight boxes for `span` of the stored `page_text`: the box of every word the span
    /// touches, whole, in text order (US-08.1 AC2). A span that covers part of a word highlights
    /// the whole word.
    pub fn highlight(&self, page_text: &str, span: Span) -> Result<Vec<Rect>, GeometryError> {
        self.check(page_text, span)?;
        let out: Vec<Rect> = self.touched(span).map(|w| w.rect).collect();
        if out.is_empty() {
            return Err(GeometryError::NoBoxes);
        }
        Ok(out)
    }

    /// OCR confidence of `span`: the lowest confidence of the words it touches (INV-ID-03).
    /// `None` if none of them has one. ASSUMPTION: the minimum, not the mean, so one badly read
    /// word is enough to flag the mention.
    pub fn confidence(&self, page_text: &str, span: Span) -> Result<Option<f32>, GeometryError> {
        self.check(page_text, span)?;
        Ok(self
            .touched(span)
            .filter_map(|w| w.confidence)
            .reduce(f32::min))
    }

    /// The stored locator for `span` on `page`: page + the touched word boxes + span (INV-CO-03).
    pub fn locator(
        &self,
        page: PageNumber,
        page_text: &str,
        span: Span,
    ) -> Result<Locator, GeometryError> {
        Ok(Locator::OcrText {
            page,
            span,
            boxes: self.highlight(page_text, span)?,
            image: self.image,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::new(x0, y0, x1, y1).unwrap()
    }

    const IMAGE: ImageSize = ImageSize {
        width: 2479,
        height: 3508,
    };

    /// Page text `IBAN: GB86 QMTB\nTotal 9` with one word per run of non-spaces.
    fn sample() -> (&'static str, OcrGeometry) {
        let text = "IBAN: GB86 QMTB\nTotal 9";
        let word = |s, e, x, y, c| OcrWord {
            span: Span::new(s, e, 23).unwrap(),
            rect: r(x, y, x + 50.0, y + 20.0),
            confidence: c,
        };
        let words = vec![
            word(0, 5, 0.0, 0.0, Some(0.98)),
            word(6, 10, 60.0, 0.0, Some(0.71)),
            word(11, 15, 120.0, 0.0, Some(0.93)),
            word(16, 21, 0.0, 30.0, Some(0.99)),
            word(22, 23, 60.0, 30.0, None),
        ];
        (text, OcrGeometry::new(text, IMAGE, words).unwrap())
    }

    /// INV-CO-03: an OCR locator is page + word boxes + span, and keeps the image size its
    /// boxes are measured in.
    #[test]
    fn inv_co_03_ocr_locator_is_page_boxes_span() {
        let (text, g) = sample();
        let page = PageNumber::new(2).unwrap();
        let span = Span::new(6, 15, 23).unwrap(); // "GB86 QMTB"
        let loc = g.locator(page, text, span).unwrap();
        assert_eq!(
            loc,
            Locator::OcrText {
                page,
                span,
                boxes: vec![r(60.0, 0.0, 110.0, 20.0), r(120.0, 0.0, 170.0, 20.0)],
                image: IMAGE,
            }
        );
        assert_eq!(loc.page(), Some(page));
        assert_eq!(loc.span(), Some(span));
    }

    /// INV-CO-04: boxes and confidence come only through geometry built from the same text.
    #[test]
    fn inv_co_04_ocr_text_drift_is_an_error() {
        let (_, g) = sample();
        let span = Span::new(0, 4, 23).unwrap();
        assert_eq!(
            g.highlight("IBAN: GB86 QMTB\nTotal 90", span),
            Err(GeometryError::LengthMismatch {
                stored: 24,
                geometry: 23
            })
        );
        assert_eq!(
            g.highlight("IBAN: GB86 QMT8\nTotal 9", span),
            Err(GeometryError::HashMismatch)
        );
        assert_eq!(
            g.confidence("IBAN: GB86 QMT8\nTotal 9", span),
            Err(GeometryError::HashMismatch)
        );
    }

    /// INV-CO-04: a span past the page text is rejected, and so are word spans that overlap or
    /// run past the text.
    #[test]
    fn inv_co_04_ocr_span_past_text_rejected() {
        let (text, g) = sample();
        let span = Span::new(20, 25, 100).unwrap();
        assert_eq!(
            g.highlight(text, span),
            Err(GeometryError::SpanOutOfBounds { end: 25, len: 23 })
        );
        let w = |s, e| OcrWord {
            span: Span::new(s, e, 100).unwrap(),
            rect: r(0.0, 0.0, 1.0, 1.0),
            confidence: None,
        };
        assert_eq!(
            OcrGeometry::new("abc def", IMAGE, vec![w(0, 3), w(2, 7)]),
            Err(GeometryError::WordSpan { index: 1 })
        );
        assert_eq!(
            OcrGeometry::new("abc", IMAGE, vec![w(0, 4)]),
            Err(GeometryError::WordSpan { index: 0 })
        );
        let mut bad = w(0, 3);
        bad.confidence = Some(1.5);
        assert_eq!(
            OcrGeometry::new("abc", IMAGE, vec![bad]),
            Err(GeometryError::Confidence { index: 0 })
        );
    }

    /// US-08.1 AC2: the highlight is the touched word boxes, not a line union; a partly covered
    /// word is highlighted whole; a span of only inserted characters has nothing to draw.
    #[test]
    fn us_08_1_ac2_highlight_returns_word_boxes() {
        let (text, g) = sample();
        let span = Span::new(11, 21, 23).unwrap(); // "QMTB\nTotal": two lines
        assert_eq!(
            g.highlight(text, span).unwrap(),
            vec![r(120.0, 0.0, 170.0, 20.0), r(0.0, 30.0, 50.0, 50.0)]
        );
        let span = Span::new(7, 9, 23).unwrap(); // "B8" inside "GB86"
        assert_eq!(
            g.highlight(text, span).unwrap(),
            vec![r(60.0, 0.0, 110.0, 20.0)]
        );
        let span = Span::new(15, 16, 23).unwrap(); // "\n"
        assert_eq!(g.highlight(text, span), Err(GeometryError::NoBoxes));
    }

    /// INV-ID-03: a mention's OCR confidence is its lowest word confidence; words without one
    /// do not count.
    #[test]
    fn inv_id_03_confidence_is_lowest_word() {
        let (text, g) = sample();
        let span = Span::new(0, 15, 23).unwrap();
        assert_eq!(g.confidence(text, span), Ok(Some(0.71)));
        let span = Span::new(16, 23, 23).unwrap(); // "Total 9": 0.99 and none
        assert_eq!(g.confidence(text, span), Ok(Some(0.99)));
        let span = Span::new(22, 23, 23).unwrap();
        assert_eq!(g.confidence(text, span), Ok(None));
    }

    /// Random page: words of 1–6 letters separated by spaces or newlines, each with a box.
    fn page() -> impl Strategy<Value = (String, Vec<OcrWord>)> {
        proptest::collection::vec(
            (
                1usize..7,
                (0f32..2000.0, 0f32..3000.0, 1f32..200.0, 1f32..60.0),
                proptest::bool::ANY,
            ),
            1..30,
        )
        .prop_map(|ws| {
            let total: usize = ws.iter().map(|w| w.0 + 1).sum::<usize>() - 1;
            let mut text = String::new();
            let mut words = Vec::new();
            for (i, (len, (x, y, w, h), newline)) in ws.into_iter().enumerate() {
                if i > 0 {
                    text.push(if newline { '\n' } else { ' ' });
                }
                let start = text.chars().count();
                text.extend(std::iter::repeat_n('w', len));
                words.push(OcrWord {
                    span: Span::new(start, start + len, total).unwrap(),
                    rect: r(x, y, x + w, y + h),
                    confidence: Some(0.5),
                });
            }
            (text, words)
        })
    }

    proptest! {
        /// NFR-QA-12, US-08.1 AC2: the highlight is exactly the boxes of the words the span
        /// touches, in text order, so every highlighted character's word is drawn and nothing
        /// else is.
        #[test]
        fn nfr_qa_12_ocr_highlight_covers_span_words((text, words) in page(), a in 0usize..200, b in 0usize..200) {
            let len = text.chars().count();
            let (start, end) = (a.min(b) % len, (a.max(b) % len) + 1);
            prop_assume!(start < end);
            let span = Span::new(start, end, len).unwrap();
            let g = OcrGeometry::new(&text, IMAGE, words.clone()).unwrap();
            let want: Vec<Rect> = words.iter().filter(|w| w.span.overlaps(&span)).map(|w| w.rect).collect();
            match g.highlight(&text, span) {
                Ok(got) => prop_assert_eq!(got, want),
                Err(e) => {
                    prop_assert_eq!(e, GeometryError::NoBoxes);
                    prop_assert!(want.is_empty());
                }
            }
        }
    }
}
