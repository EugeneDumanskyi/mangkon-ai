//! The shared, engine-independent PDF layout (ADR-0014, `08-engine-bakeoff.md` §4.1): chars →
//! lines → fragments (split at wide x-gaps) → reading order by recursive XY-cut → page text
//! with layout-inserted spaces and newlines. One segment per page (US-02.1 AC1). The OCR layout
//! (`ocr::layout`) shares the fragment gap and the XY-cut.
//!
//! The page view runs the same layout again on a fresh open and maps the stored span through
//! the result (ADR-0011): any change here changes the stored text of new PDFs, and old ones
//! then show the text fallback until they are read again.

use mk_kernel::{PageGeometry, Rect};

use crate::pdf::engine::{PageInfo, RawChar};

/// Thresholds in units of the line's median character height. ASSUMPTION (spike S0 values).
const SAME_LINE_OVERLAP: f32 = 0.5;
const SPACE_GAP: f32 = 0.15;
pub const FRAGMENT_GAP: f32 = 1.0;

/// A page's stored text and the box of each of its characters.
pub struct Laid {
    pub text: String,
    pub geometry: PageGeometry,
}

/// Unrotated top-left box → display box (`/Rotate` applied, clockwise), in points.
pub fn to_display(r: &Rect, info: &PageInfo) -> Rect {
    let (w, h) = (info.width, info.height);
    let (x0, y0, x1, y1) = (r.x0(), r.y0(), r.x1(), r.y1());
    let (a, b, c, d) = match info.rotate {
        90 => (h - y1, x0, h - y0, x1),
        180 => (w - x1, h - y1, w - x0, h - y0),
        270 => (y0, w - x1, y1, w - x0),
        _ => (x0, y0, x1, y1),
    };
    Rect::new(a, b, c, d).unwrap_or(*r)
}

struct Item {
    ch: char,
    rect: Rect,
    d: Rect,
}

struct Frag {
    items: Vec<usize>,
    d: Rect,
    height: f32,
}

pub fn layout(chars: &[RawChar], info: &PageInfo) -> Laid {
    let items: Vec<Item> = chars
        .iter()
        .filter(|c| !c.ch.is_whitespace() && !c.ch.is_control())
        .map(|c| Item {
            ch: c.ch,
            rect: c.rect,
            d: to_display(&c.rect, info),
        })
        .collect();

    // Lines: by vertical centre, joined while they overlap the line's first character.
    let mut order: Vec<usize> = (0..items.len()).collect();
    let cy = |i: usize| (items[i].d.y0() + items[i].d.y1()) / 2.0;
    order.sort_by(|&a, &b| {
        cy(a)
            .total_cmp(&cy(b))
            .then(items[a].d.x0().total_cmp(&items[b].d.x0()))
    });
    let mut lines: Vec<(Rect, Vec<usize>)> = Vec::new();
    for i in order {
        let d = items[i].d;
        if let Some((band, members)) = lines.last_mut() {
            let overlap = d.y1().min(band.y1()) - d.y0().max(band.y0());
            if overlap >= SAME_LINE_OVERLAP * d.height().min(band.height()) {
                members.push(i);
                continue;
            }
        }
        lines.push((d, vec![i]));
    }

    // Fragments: split each line at gaps wider than a character height.
    let mut frags: Vec<Frag> = Vec::new();
    for (_, mut members) in lines {
        members.sort_by(|&a, &b| items[a].d.x0().total_cmp(&items[b].d.x0()));
        let h = median(members.iter().map(|&i| items[i].d.height()));
        let mut cur: Vec<usize> = Vec::new();
        for i in members {
            if let Some(&prev) = cur.last()
                && items[i].d.x0() - items[prev].d.x1() > FRAGMENT_GAP * h
            {
                frags.push(frag(&items, std::mem::take(&mut cur), h));
            }
            cur.push(i);
        }
        if !cur.is_empty() {
            frags.push(frag(&items, cur, h));
        }
    }

    let rects: Vec<Rect> = frags.iter().map(|f| f.d).collect();
    let mut read = Vec::new();
    xy_cut(&rects, (0..rects.len()).collect(), &mut read);

    let mut text = String::new();
    let mut boxes = Vec::new();
    for (n, &fi) in read.iter().enumerate() {
        let f = &frags[fi];
        if n > 0 {
            text.push('\n');
            boxes.push(None);
        }
        for (k, &i) in f.items.iter().enumerate() {
            if k > 0 && items[i].d.x0() - items[f.items[k - 1]].d.x1() > SPACE_GAP * f.height {
                text.push(' ');
                boxes.push(None);
            }
            text.push(items[i].ch);
            boxes.push(Some(items[i].rect));
        }
    }
    let geometry = PageGeometry::new(&text, boxes).expect("one box per char");
    Laid { text, geometry }
}

fn frag(items: &[Item], members: Vec<usize>, height: f32) -> Frag {
    let d = members
        .iter()
        .skip(1)
        .fold(items[members[0]].d, |u, &i| u.union(&items[i].d));
    Frag {
        items: members,
        d,
        height,
    }
}

pub fn median(v: impl Iterator<Item = f32>) -> f32 {
    let mut v: Vec<f32> = v.collect();
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

/// Widest empty band between the projections of `idx` on one axis: (width, cut position).
fn widest_gap(
    boxes: &[Rect],
    idx: &[usize],
    span: impl Fn(&Rect) -> (f32, f32),
) -> Option<(f32, f32)> {
    let mut iv: Vec<(f32, f32)> = idx.iter().map(|&i| span(&boxes[i])).collect();
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut end = iv[0].1;
    let mut best: Option<(f32, f32)> = None;
    for &(s, e) in &iv[1..] {
        if s > end && best.is_none_or(|(w, _)| s - end > w) {
            best = Some((s - end, (s + end) / 2.0));
        }
        end = end.max(e);
    }
    best
}

/// Recursive XY-cut over display boxes: split at the widest empty band (vertical bands first on
/// ties), top/left part first. Regions with no band fall back to top-to-bottom, left-to-right.
/// Shared by the PDF layout (fragments) and the OCR layout (line fragments).
pub fn xy_cut(boxes: &[Rect], idx: Vec<usize>, out: &mut Vec<usize>) {
    if idx.len() <= 1 {
        out.extend(idx);
        return;
    }
    let v = widest_gap(boxes, &idx, |d| (d.x0(), d.x1()));
    let h = widest_gap(boxes, &idx, |d| (d.y0(), d.y1()));
    let vertical = match (v, h) {
        (Some(v), Some(h)) => v.0 >= h.0,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => {
            let mut idx = idx;
            idx.sort_by(|&a, &b| {
                let (fa, fb) = (&boxes[a], &boxes[b]);
                (fa.y0() + fa.y1())
                    .total_cmp(&(fb.y0() + fb.y1()))
                    .then(fa.x0().total_cmp(&fb.x0()))
            });
            out.extend(idx);
            return;
        }
    };
    let at = if vertical { v.unwrap().1 } else { h.unwrap().1 };
    let (first, second): (Vec<usize>, Vec<usize>) = idx.into_iter().partition(|&i| {
        let d = &boxes[i];
        if vertical { d.x1() <= at } else { d.y1() <= at }
    });
    xy_cut(boxes, first, out);
    xy_cut(boxes, second, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(ch: char, x: f32, y: f32) -> RawChar {
        RawChar {
            ch,
            rect: Rect::new(x, y, x + 6.0, y + 10.0).unwrap(),
        }
    }

    fn word(s: &str, x: f32, y: f32) -> Vec<RawChar> {
        s.chars()
            .enumerate()
            .map(|(i, c)| ch(c, x + 6.0 * i as f32, y))
            .collect()
    }

    const A4: PageInfo = PageInfo {
        width: 595.0,
        height: 842.0,
        rotate: 0,
    };

    #[test]
    fn words_lines_and_columns() {
        let mut chars = Vec::new();
        chars.extend(word("left2", 50.0, 112.0));
        chars.extend(word("right1", 300.0, 100.0));
        chars.extend(word("left1", 50.0, 100.0));
        chars.extend(word("more", 84.0, 100.0)); // 4 pt after "left1": a space
        chars.extend(word("right2", 300.0, 112.0));
        let laid = layout(&chars, &A4);
        assert_eq!(laid.text, "left1 more\nleft2\nright1\nright2");
        assert_eq!(laid.geometry.text_len(), laid.text.chars().count());
    }

    #[test]
    fn rotated_page_reads_in_display_order() {
        // /Rotate 90: display x runs along unrotated y; display y along unrotated x.
        let info = PageInfo { rotate: 90, ..A4 };
        let chars: Vec<RawChar> = "ab"
            .chars()
            .enumerate()
            .map(|(i, c)| RawChar {
                ch: c,
                rect: Rect::new(
                    100.0,
                    700.0 - 6.0 * (i as f32 + 1.0),
                    110.0,
                    700.0 - 6.0 * i as f32,
                )
                .unwrap(),
            })
            .collect();
        assert_eq!(layout(&chars, &info).text, "ab");
    }
}
