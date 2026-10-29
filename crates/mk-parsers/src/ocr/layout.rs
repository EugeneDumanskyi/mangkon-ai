//! The shared OCR layout (ADR-0014), engine-independent: engine lines → fragments (split at
//! wide gaps, as in the PDF layout) → reading order by the same XY-cut → page text (words joined
//! by a space, fragments by `\n`) with an `OcrGeometry` holding each word's span, box and
//! confidence.

use mk_kernel::{ImageSize, OcrGeometry, Rect, Span};

use crate::layout::{FRAGMENT_GAP, median, xy_cut};
use crate::ocr::{OcrLine, OcrWord};

/// A page's OCR text and its word geometry.
pub struct OcrLaid {
    pub text: String,
    pub geometry: OcrGeometry,
}

/// A word as the engine gave it, split at any whitespace inside its text: the box is divided by
/// character count. Control characters are dropped.
fn clean(w: &OcrWord) -> Vec<OcrWord> {
    let all: Vec<char> = w.text.chars().filter(|c| !c.is_control()).collect();
    let first = all
        .iter()
        .position(|c| !c.is_whitespace())
        .unwrap_or(all.len());
    let last = all
        .iter()
        .rposition(|c| !c.is_whitespace())
        .map_or(first, |e| e + 1);
    let chars = &all[first..last];
    let unit = w.rect.width() / chars.len().max(1) as f32;
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let (x0, x1) = (
            w.rect.x0() + unit * start as f32,
            w.rect.x0() + unit * i as f32,
        );
        if let Some(rect) = Rect::new(x0, w.rect.y0(), x1, w.rect.y1()) {
            out.push(OcrWord {
                text: chars[start..i].iter().collect(),
                rect,
                confidence: w.confidence,
            });
        }
    }
    out
}

pub fn layout(lines: &[OcrLine], image: ImageSize) -> OcrLaid {
    // Fragments: each engine line, split where the gap between words is wider than the line's
    // median word height.
    let mut frags: Vec<Vec<OcrWord>> = Vec::new();
    for line in lines {
        let mut words: Vec<OcrWord> = line.words.iter().flat_map(clean).collect();
        if words.is_empty() {
            continue;
        }
        words.sort_by(|a, b| a.rect.x0().total_cmp(&b.rect.x0()));
        let h = median(words.iter().map(|w| w.rect.height()));
        let mut cur: Vec<OcrWord> = Vec::new();
        for w in words {
            if let Some(prev) = cur.last()
                && w.rect.x0() - prev.rect.x1() > FRAGMENT_GAP * h
            {
                frags.push(std::mem::take(&mut cur));
            }
            cur.push(w);
        }
        frags.push(cur);
    }
    let boxes: Vec<Rect> = frags
        .iter()
        .map(|f| f.iter().skip(1).fold(f[0].rect, |u, w| u.union(&w.rect)))
        .collect();
    let mut read = Vec::new();
    xy_cut(&boxes, (0..boxes.len()).collect(), &mut read);

    let mut text = String::new();
    let mut spans: Vec<(usize, usize, &OcrWord)> = Vec::new();
    let mut len = 0usize;
    for (n, &fi) in read.iter().enumerate() {
        if n > 0 {
            text.push('\n');
            len += 1;
        }
        for (k, w) in frags[fi].iter().enumerate() {
            if k > 0 {
                text.push(' ');
                len += 1;
            }
            let n = w.text.chars().count();
            text.push_str(&w.text);
            spans.push((len, len + n, w));
            len += n;
        }
    }
    let words = spans
        .into_iter()
        .map(|(s, e, w)| mk_kernel::OcrWord {
            span: Span::new(s, e, len).expect("word inside text"),
            rect: w.rect,
            confidence: w.confidence.map(|c| c.clamp(0.0, 1.0)),
        })
        .collect();
    let geometry = OcrGeometry::new(&text, image, words).expect("ordered word spans");
    OcrLaid { text, geometry }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(t: &str, x: f32, y: f32) -> OcrWord {
        OcrWord {
            text: t.into(),
            rect: Rect::new(x, y, x + 10.0 * t.chars().count() as f32, y + 20.0).unwrap(),
            confidence: Some(0.9),
        }
    }

    const IMAGE: ImageSize = ImageSize {
        width: 1000,
        height: 1000,
    };

    #[test]
    fn columns_and_word_spans() {
        // One engine line that spans two columns, and a second line in the left column.
        let lines = vec![
            OcrLine {
                words: vec![word("right", 600.0, 100.0), word("left", 100.0, 100.0)],
            },
            OcrLine {
                words: vec![word("more", 100.0, 130.0), word("text", 150.0, 130.0)],
            },
        ];
        let laid = layout(&lines, IMAGE);
        assert_eq!(laid.text, "left\nmore text\nright");
        let spans: Vec<(usize, usize)> = laid
            .geometry
            .words()
            .iter()
            .map(|w| (w.span.start(), w.span.end()))
            .collect();
        assert_eq!(spans, vec![(0, 4), (5, 9), (10, 14), (15, 20)]);
    }

    #[test]
    fn whitespace_inside_a_word_splits_it() {
        let w = OcrWord {
            text: "ab cd".into(),
            rect: Rect::new(0.0, 0.0, 50.0, 10.0).unwrap(),
            confidence: None,
        };
        let parts = clean(&w);
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].rect.x0(), parts[0].rect.x1()), (0.0, 20.0));
        assert_eq!((parts[1].rect.x0(), parts[1].rect.x1()), (30.0, 50.0));
    }
}
