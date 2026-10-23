//! Highlight round-trip on scans (M0 AC5, early NFR-QA-12): every TD-1 `scan` mention → span in
//! the stored OCR page text → `Locator::OcrText` (boxes stored at OCR time, no re-OCR, ADR-0011)
//! → boxes on the image that was OCR'd.
//!
//! The rate counts mentions the OCR text holds (Find passed). A Find miss is an OCR misread: the
//! product would never extract that mention, so no one could click it. Misses are reported as
//! recall, next to the rate.

use std::collections::HashMap;

use mk_bench::{Location, Subject};
use mk_kernel::{Locator, PageNumber, Rect};

use crate::corpus::Corpus;
use crate::engine::GrayImage;
use crate::ocr::layout::OcrLaid;
use crate::ocr::pages::{self, OcrPage};
use crate::roundtrip::{
    DARK, MIN_INK, MIN_V_OVERLAP, MIN_X_IOU, PAD_PX, SampleResult, Step, find_all, fmt_rects,
    score, to_rect, v_overlap, x_iou,
};

/// One ground-truth `scan` mention (derived ones included: they share the location).
pub struct ScanSample {
    pub mention: String,
    pub page: OcrPage,
    pub raw: String,
    /// Ground-truth line boxes in pixels of the OCR'd image.
    pub boxes: Vec<Rect>,
    /// Ground-truth boxes of the other words on the page (centre outside `boxes`), same pixels.
    /// On a skewed scan an axis-aligned line box also holds ink of the next line; the Render
    /// step does not count ink inside these.
    pub others: Vec<Rect>,
    pub identifier: bool,
    pub hard: bool,
}

pub fn pool(corpus: &Corpus) -> Vec<ScanSample> {
    let words: HashMap<(String, u32), Vec<Rect>> = corpus
        .gt
        .page_text
        .iter()
        .filter(|p| p.unit == mk_bench::Unit::Px)
        .map(|p| {
            let b = p.words.iter().filter_map(|w| {
                Rect::new(
                    w.b.x0() as f32,
                    w.b.y0() as f32,
                    w.b.x1() as f32,
                    w.b.y1() as f32,
                )
            });
            ((p.document.clone(), p.page), b.collect())
        })
        .collect();
    let pages: HashMap<(String, u32), OcrPage> = pages::ocr_pages(corpus)
        .into_iter()
        .map(|p| ((p.document.clone(), p.page), p))
        .collect();
    corpus
        .gt
        .mentions
        .iter()
        .filter_map(|m| {
            let Location::Scan { page, boxes, .. } = &m.location else {
                return None;
            };
            let p = pages.get(&(m.document.clone(), *page))?.clone();
            let s = p.gt_scale();
            let scaled = |r: Rect| {
                Rect::new(r.x0() * s, r.y0() * s, r.x1() * s, r.y1() * s).expect("scaled")
            };
            let boxes: Vec<Rect> = boxes.iter().map(|b| scaled(to_rect(b))).collect();
            let others = words
                .get(&(m.document.clone(), *page))
                .into_iter()
                .flatten()
                .map(|w| scaled(*w))
                .filter(|w| {
                    let (cx, cy) = ((w.x0() + w.x1()) / 2.0, (w.y0() + w.y1()) / 2.0);
                    !boxes
                        .iter()
                        .any(|b| (b.x0()..=b.x1()).contains(&cx) && (b.y0()..=b.y1()).contains(&cy))
                })
                .collect();
            Some(ScanSample {
                mention: m.id.clone(),
                page: p,
                raw: m.raw.clone(),
                boxes,
                others,
                identifier: matches!(m.subject, Subject::Identifier { .. }),
                hard: m.difficulty == mk_bench::Difficulty::Hard,
            })
        })
        .collect()
}

/// Each returned word box goes to the ground-truth line it overlaps most vertically, as a share
/// of the lower of the two heights: on a skewed scan the axis-aligned line box is taller than
/// any one word, so a share of the line height would reject good words. Each line's boxes are
/// unioned and must match it with the M0b thresholds (x-extent IoU ≥ 0.8, vertical overlap
/// ≥ 50% of the line). Returns, per ground-truth line, the boxes assigned to it.
fn geometry(gt: &[Rect], got: &[Rect]) -> Result<Vec<Vec<Rect>>, String> {
    let mut per: Vec<Vec<Rect>> = vec![Vec::new(); gt.len()];
    for b in got {
        let (best, ov) = gt
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let inter = (g.y1().min(b.y1()) - g.y0().max(b.y0())).max(0.0);
                (i, inter / g.height().min(b.height()).max(f32::EPSILON))
            })
            .fold((0, 0.0), |a, x| if x.1 > a.1 { x } else { a });
        if ov < MIN_V_OVERLAP {
            return Err(format!("box {} on no line", fmt_rects(&[*b])));
        }
        per[best].push(*b);
    }
    for (g, boxes) in gt.iter().zip(&per) {
        let Some(u) = boxes.iter().copied().reduce(|u, b| u.union(&b)) else {
            return Err(format!("line {} has no box", fmt_rects(&[*g])));
        };
        if x_iou(g, &u) < MIN_X_IOU || v_overlap(g, &u) < MIN_V_OVERLAP {
            return Err(format!(
                "line {} got {}",
                fmt_rects(&[*g]),
                fmt_rects(boxes)
            ));
        }
    }
    Ok(per)
}

/// Share of dark pixels inside `gt` (and not inside another word's box, `others`) that also
/// fall inside one of `got` padded by `PAD_PX`, on the image that was OCR'd (pixel
/// coordinates). `None` if there is no such dark pixel.
fn ink_share(img: &GrayImage, gt: &Rect, got: &[Rect], others: &[Rect]) -> Option<f32> {
    let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
    let (x0, y0) = (
        clamp(gt.x0().floor(), img.width),
        clamp(gt.y0().floor(), img.height),
    );
    let (x1, y1) = (
        clamp(gt.x1().ceil(), img.width),
        clamp(gt.y1().ceil(), img.height),
    );
    let within = |b: &Rect, fx: f32, fy: f32, pad: f32| {
        fx >= b.x0() - pad && fx <= b.x1() + pad && fy >= b.y0() - pad && fy <= b.y1() + pad
    };
    let inside = |x: u32, y: u32| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        got.iter().any(|b| within(b, fx, fy, PAD_PX))
    };
    let foreign = |x: u32, y: u32| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        others.iter().any(|b| within(b, fx, fy, 0.0))
    };
    let (mut total, mut hit) = (0u32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            if img.get(x, y) < DARK && !foreign(x, y) {
                total += 1;
                hit += inside(x, y) as u32;
            }
        }
    }
    (total > 0).then(|| hit as f32 / total as f32)
}

/// Runs every sample against the stored OCR results. Page images are loaded again, as the
/// viewer would show them.
pub fn run(
    corpus: &Corpus,
    samples: &[ScanSample],
    stored: &HashMap<(String, u32), OcrLaid>,
) -> Vec<SampleResult> {
    let mut images: HashMap<(String, u32), Option<GrayImage>> = HashMap::new();
    samples
        .iter()
        .map(|s| {
            let key = s.page.key();
            let img = images
                .entry(key.clone())
                .or_insert_with(|| pages::load(corpus, &s.page).ok());
            let (step, detail) = one(s, stored.get(&key), img.as_ref());
            SampleResult {
                mention: s.mention.clone(),
                file: s.page.file.clone(),
                page: s.page.page,
                raw: s.raw.clone(),
                identifier: s.identifier,
                hard: s.hard,
                step,
                detail,
            }
        })
        .collect()
}

fn one(s: &ScanSample, laid: Option<&OcrLaid>, img: Option<&GrayImage>) -> (Step, String) {
    let Some(laid) = laid else {
        return (Step::Find, "page was not OCR'd".into());
    };
    // 1. Find.
    let spans = find_all(&laid.text, &s.raw);
    if spans.is_empty() {
        return (Step::Find, "raw text not in OCR page text".into());
    }
    // 2. Map: the stored locator holds the boxes.
    let page = PageNumber::new(s.page.page).expect("1-based page");
    let mut best: Option<(f32, Vec<Rect>)> = None;
    let mut last_err = None;
    for span in spans {
        match laid.geometry.locator(page, &laid.text, span) {
            Ok(Locator::OcrText { boxes, .. }) => {
                let sc = score(&s.boxes, &boxes);
                if best.as_ref().is_none_or(|(x, _)| sc > *x) {
                    best = Some((sc, boxes));
                }
            }
            Ok(other) => last_err = Some(format!("wrong locator kind {other:?}")),
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    let Some((sc, got)) = best else {
        return (Step::Map, last_err.unwrap_or_default());
    };
    // Every hit is elsewhere on the page: the same text occurs there, and this occurrence was
    // misread. That is a Find miss, not a wrong highlight.
    if sc <= 0.0 {
        return (Step::Find, "only other occurrences found".into());
    }
    // 3. Geometry.
    let per = match geometry(&s.boxes, &got) {
        Ok(p) => p,
        Err(e) => return (Step::Geometry, e),
    };
    // 4. Render: on the image that was OCR'd.
    let Some(img) = img else {
        return (Step::Render, "page image did not load".into());
    };
    for (g, boxes) in s.boxes.iter().zip(&per) {
        match ink_share(img, g, boxes, &s.others) {
            None => {
                return (
                    Step::Render,
                    format!("no ink under gt {}", fmt_rects(&[*g])),
                );
            }
            Some(share) if share < MIN_INK => {
                return (
                    Step::Render,
                    format!(
                        "ink inside {:.0}% for gt {}",
                        share * 100.0,
                        fmt_rects(&[*g])
                    ),
                );
            }
            _ => {}
        }
    }
    (Step::Pass, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::new(x0, y0, x1, y1).unwrap()
    }

    #[test]
    fn word_boxes_union_per_line() {
        let gt = [r(100.0, 100.0, 400.0, 130.0), r(100.0, 140.0, 200.0, 170.0)];
        let words = [
            r(98.0, 99.0, 240.0, 131.0),
            r(250.0, 99.0, 402.0, 131.0),
            r(100.0, 141.0, 199.0, 170.0),
        ];
        let per = geometry(&gt, &words).unwrap();
        assert_eq!(per[0].len(), 2);
        assert_eq!(per[1].len(), 1);
        // A word only half the line wide fails the x-extent check.
        assert!(geometry(&gt[..1], &words[..1]).is_err());
        // A box on no line fails.
        assert!(geometry(&gt[..1], &[r(100.0, 300.0, 400.0, 330.0)]).is_err());
    }
}
