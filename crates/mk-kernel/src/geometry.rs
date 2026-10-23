use std::fmt;

use crate::{ContentHash, Span};

/// Axis-aligned box `[x0, y0, x1, y1]` on a page.
///
/// PDF points in the page's **unrotated** box, top-left origin, y down: the convention of the
/// ground truth (`docs/07-test-corpus.md` §5). Always finite with `x0 ≤ x1` and `y0 ≤ y1`;
/// zero width or height is allowed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    /// `None` if a coordinate is not finite or the box is reversed.
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Self> {
        let finite = [x0, y0, x1, y1].iter().all(|v| v.is_finite());
        (finite && x0 <= x1 && y0 <= y1).then_some(Self { x0, y0, x1, y1 })
    }

    pub fn x0(&self) -> f32 {
        self.x0
    }
    pub fn y0(&self) -> f32 {
        self.y0
    }
    pub fn x1(&self) -> f32 {
        self.x1
    }
    pub fn y1(&self) -> f32 {
        self.y1
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn area(&self) -> f32 {
        self.width() * self.height()
    }

    /// Smallest box holding both.
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// Area of the overlap; 0 when the boxes do not overlap.
    pub fn intersection_area(&self, other: &Rect) -> f32 {
        let w = self.x1.min(other.x1) - self.x0.max(other.x0);
        let h = self.y1.min(other.y1) - self.y0.max(other.y0);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }

    /// Intersection over union, in `[0, 1]`. 0 when both boxes have no area.
    pub fn iou(&self, other: &Rect) -> f32 {
        let inter = self.intersection_area(other);
        let union = self.area() + other.area() - inter;
        if union > 0.0 { inter / union } else { 0.0 }
    }

    /// True if `other` lies inside `self` (edges included).
    pub fn contains(&self, other: &Rect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && other.x1 <= self.x1 && other.y1 <= self.y1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeometryError {
    /// The engine gave a different number of boxes than the text has characters.
    BoxCount { boxes: usize, chars: usize },
    /// The stored text and the text the geometry was built from differ in length.
    LengthMismatch { stored: usize, geometry: usize },
    /// Same length, different text: the engine no longer extracts what was stored.
    HashMismatch,
    /// `span.end` is past the page text.
    SpanOutOfBounds { end: usize, len: usize },
    /// Every character in the span is layout-inserted (space, newline): nothing to draw.
    NoBoxes,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeometryError::BoxCount { boxes, chars } => {
                write!(f, "{boxes} boxes for {chars} characters")
            }
            GeometryError::LengthMismatch { stored, geometry } => write!(
                f,
                "stored text has {stored} characters, page geometry has {geometry}"
            ),
            GeometryError::HashMismatch => {
                f.write_str("stored text differs from the text the page geometry was built from")
            }
            GeometryError::SpanOutOfBounds { end, len } => {
                write!(f, "span end {end} is past page text length {len}")
            }
            GeometryError::NoBoxes => f.write_str("span covers no drawn characters"),
        }
    }
}

impl std::error::Error for GeometryError {}

/// Character boxes for one page's stored text, from the engine that renders the page.
///
/// Built when the page is rendered (ADR-0011): the worker re-extracts the page with the same
/// engine and layout, and returns this next to the image. It records the length and hash of the
/// text it was built from, so a [`Locator::PdfText`](crate::Locator) span is only mapped through
/// it when that text is exactly the stored text (INV-CO-04); any drift is an error, not a wrong
/// highlight (NFR-QA-12).
#[derive(Clone, Debug, PartialEq)]
pub struct PageGeometry {
    text_len: usize,
    text_hash: ContentHash,
    /// One per `char` of the page text. `None` for characters the layout inserted.
    boxes: Vec<Option<Rect>>,
}

impl PageGeometry {
    /// `boxes` holds one entry per `char` of `page_text`.
    pub fn new(page_text: &str, boxes: Vec<Option<Rect>>) -> Result<Self, GeometryError> {
        let chars = page_text.chars().count();
        if boxes.len() != chars {
            return Err(GeometryError::BoxCount {
                boxes: boxes.len(),
                chars,
            });
        }
        Ok(Self {
            text_len: chars,
            text_hash: ContentHash::of(page_text.as_bytes()),
            boxes,
        })
    }

    pub fn text_len(&self) -> usize {
        self.text_len
    }

    pub fn text_hash(&self) -> ContentHash {
        self.text_hash
    }

    pub fn boxes(&self) -> &[Option<Rect>] {
        &self.boxes
    }

    /// Highlight boxes for `span` of the stored `page_text`: one box per line the span touches
    /// (lines split at `\n`), each the union of that line's character boxes. Lines with no drawn
    /// characters are skipped.
    pub fn highlight(&self, page_text: &str, span: Span) -> Result<Vec<Rect>, GeometryError> {
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
        let mut out = Vec::new();
        let mut line: Option<Rect> = None;
        let chars = page_text.chars().skip(span.start()).take(span.len());
        for (ch, b) in chars.zip(&self.boxes[span.start()..span.end()]) {
            if ch == '\n' {
                out.extend(line.take());
                continue;
            }
            if let Some(b) = b {
                line = Some(line.map_or(*b, |l| l.union(b)));
            }
        }
        out.extend(line);
        if out.is_empty() {
            return Err(GeometryError::NoBoxes);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::new(x0, y0, x1, y1).unwrap()
    }

    /// Page text `ab cd\nef` with 10×10 character cells; the space and newline have no box.
    fn sample() -> (&'static str, PageGeometry) {
        let text = "ab cd\nef";
        let cell = |col: f32, row: f32| {
            Some(r(
                col * 10.0,
                row * 10.0,
                col * 10.0 + 10.0,
                row * 10.0 + 10.0,
            ))
        };
        let boxes = vec![
            cell(0.0, 0.0),
            cell(1.0, 0.0),
            None,
            cell(3.0, 0.0),
            cell(4.0, 0.0),
            None,
            cell(0.0, 1.0),
            cell(1.0, 1.0),
        ];
        (text, PageGeometry::new(text, boxes).unwrap())
    }

    #[test]
    fn rect_rejects_reversed_and_non_finite() {
        assert!(Rect::new(1.0, 0.0, 0.0, 1.0).is_none());
        assert!(Rect::new(0.0, 1.0, 1.0, 0.0).is_none());
        assert!(Rect::new(0.0, 0.0, f32::NAN, 1.0).is_none());
        assert!(Rect::new(0.0, 0.0, 0.0, 0.0).is_some());
    }

    #[test]
    fn rect_iou() {
        let a = r(0.0, 0.0, 10.0, 10.0);
        assert_eq!(a.iou(&a), 1.0);
        assert_eq!(a.iou(&r(20.0, 0.0, 30.0, 10.0)), 0.0);
        assert!((a.iou(&r(5.0, 0.0, 15.0, 10.0)) - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn box_count_must_match_chars() {
        assert_eq!(
            PageGeometry::new("abc", vec![None, None]),
            Err(GeometryError::BoxCount { boxes: 2, chars: 3 })
        );
    }

    /// INV-CO-04: a span maps to boxes only through geometry built from the same stored text.
    #[test]
    fn inv_co_04_text_drift_is_an_error() {
        let (_, g) = sample();
        let span = Span::new(0, 2, 8).unwrap();
        assert_eq!(
            g.highlight("ab cd\nef!", span),
            Err(GeometryError::LengthMismatch {
                stored: 9,
                geometry: 8
            })
        );
        assert_eq!(
            g.highlight("ab cd\neF", span),
            Err(GeometryError::HashMismatch)
        );
    }

    /// INV-CO-04: a span past the page text is rejected.
    #[test]
    fn inv_co_04_span_past_text_rejected() {
        let (text, g) = sample();
        let span = Span::new(6, 9, 100).unwrap();
        assert_eq!(
            g.highlight(text, span),
            Err(GeometryError::SpanOutOfBounds { end: 9, len: 8 })
        );
    }

    /// NFR-QA-12: one box per line; inserted spaces add no area.
    #[test]
    fn nfr_qa_12_one_box_per_line() {
        let (text, g) = sample();
        let span = Span::new(3, 8, 8).unwrap(); // "cd\nef"
        assert_eq!(
            g.highlight(text, span).unwrap(),
            vec![r(30.0, 0.0, 50.0, 10.0), r(0.0, 10.0, 20.0, 20.0)]
        );
        let span = Span::new(0, 5, 8).unwrap(); // "ab cd"
        assert_eq!(
            g.highlight(text, span).unwrap(),
            vec![r(0.0, 0.0, 50.0, 10.0)]
        );
    }

    #[test]
    fn span_of_only_inserted_chars_has_no_boxes() {
        let (text, g) = sample();
        let span = Span::new(5, 6, 8).unwrap(); // "\n"
        assert_eq!(g.highlight(text, span), Err(GeometryError::NoBoxes));
    }

    /// Random page: lines of characters, some without boxes.
    fn page() -> impl Strategy<Value = (String, Vec<Option<Rect>>)> {
        proptest::collection::vec(
            proptest::collection::vec(
                (
                    proptest::char::range('a', 'z'),
                    proptest::option::of((0f32..500.0, 0f32..700.0, 0f32..20.0, 0f32..20.0)),
                ),
                0..12,
            ),
            1..6,
        )
        .prop_map(|lines| {
            let mut text = String::new();
            let mut boxes = Vec::new();
            for (i, line) in lines.into_iter().enumerate() {
                if i > 0 {
                    text.push('\n');
                    boxes.push(None);
                }
                for (ch, b) in line {
                    text.push(ch);
                    boxes.push(b.map(|(x, y, w, h)| r(x, y, x + w, y + h)));
                }
            }
            (text, boxes)
        })
    }

    proptest! {
        /// NFR-QA-12, INV-CO-04: every highlight box is built only from the span's own character
        /// boxes, and there is at most one per line the span touches.
        #[test]
        fn nfr_qa_12_highlight_inside_span_boxes((text, boxes) in page(), a in 0usize..80, b in 0usize..80) {
            let len = boxes.len();
            prop_assume!(len > 0);
            let (start, end) = (a.min(b) % len, (a.max(b) % len) + 1);
            prop_assume!(start < end);
            let span = Span::new(start, end, len).unwrap();
            let g = PageGeometry::new(&text, boxes.clone()).unwrap();
            let own: Vec<Rect> = boxes[start..end].iter().flatten().copied().collect();
            match g.highlight(&text, span) {
                Ok(out) => {
                    let lines = text.chars().skip(start).take(end - start).filter(|c| *c == '\n').count() + 1;
                    prop_assert!(out.len() <= lines);
                    let all = own.iter().skip(1).fold(own[0], |u, b| u.union(b));
                    for h in &out {
                        prop_assert!(all.contains(h));
                        prop_assert!(own.iter().any(|b| h.contains(b)));
                    }
                    for b in &own {
                        prop_assert!(out.iter().any(|h| h.contains(b)));
                    }
                }
                Err(e) => {
                    prop_assert_eq!(e, GeometryError::NoBoxes);
                    prop_assert!(own.is_empty());
                }
            }
        }
    }
}
