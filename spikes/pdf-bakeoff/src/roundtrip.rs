//! Highlight round-trip (early NFR-QA-12, M0 AC5): ground-truth mention → span in stored page
//! text → `PageGeometry` from a fresh open (the `RenderPage` path) → boxes → rendered glyphs.

use std::collections::{BTreeMap, HashMap};

use mk_bench::BBox;
use mk_kernel::{PageNumber, Rect, Span};
use serde::Serialize;

use crate::corpus::{Corpus, Sample};
use crate::engine::{GrayImage, PageInfo, PdfEngine};
use crate::layout::{layout, to_display};

/// Geometry match: x-extent IoU and vertical overlap as a share of the ground-truth height.
/// Render match: share of dark pixels in the ground-truth box that fall in the returned box.
/// ASSUMPTION (spike thresholds, recorded in `docs/08-engine-bakeoff.md`).
pub const MIN_X_IOU: f32 = 0.8;
pub const MIN_V_OVERLAP: f32 = 0.5;
pub const MIN_INK: f32 = 0.9;
pub const RENDER_SCALE: f32 = 2.0;
const DARK: u8 = 128;
/// Antialiasing margin around the returned box, in pixels.
const PAD_PX: f32 = 1.5;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Pass,
    Find,
    Map,
    Geometry,
    Render,
}

#[derive(Serialize, Clone)]
pub struct SampleResult {
    pub mention: String,
    pub file: String,
    pub page: u32,
    pub raw: String,
    pub identifier: bool,
    pub hard: bool,
    pub step: Step,
    pub detail: String,
}

#[derive(Serialize, Default)]
pub struct Tally {
    pub total: usize,
    pub passed: usize,
    pub failed_by_step: BTreeMap<String, usize>,
}

impl Tally {
    pub fn of<'a>(results: impl IntoIterator<Item = &'a SampleResult>) -> Self {
        let mut t = Tally::default();
        for r in results {
            t.total += 1;
            if r.step == Step::Pass {
                t.passed += 1;
            } else {
                *t.failed_by_step
                    .entry(format!("{:?}", r.step).to_lowercase())
                    .or_default() += 1;
            }
        }
        t
    }

    pub fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }
}

/// Every occurrence of `raw` in `text`, ignoring whitespace on both sides (a mention split
/// across lines matches across the inserted `\n`). Spans count `char`s.
pub fn find_all(text: &str, raw: &str) -> Vec<Span> {
    let (hay, pos): (Vec<char>, Vec<usize>) = text
        .chars()
        .enumerate()
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(i, c)| (c, i))
        .unzip();
    let needle: Vec<char> = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let len = text.chars().count();
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle.as_slice())
        .filter_map(|(s, _)| Span::new(pos[s], pos[s + needle.len() - 1] + 1, len).ok())
        .collect()
}

fn to_rect(b: &BBox) -> Rect {
    Rect::new(b.x0() as f32, b.y0() as f32, b.x1() as f32, b.y1() as f32).expect("valid GT box")
}

fn x_iou(a: &Rect, b: &Rect) -> f32 {
    let inter = (a.x1().min(b.x1()) - a.x0().max(b.x0())).max(0.0);
    let union = a.x1().max(b.x1()) - a.x0().min(b.x0());
    if union > 0.0 { inter / union } else { 0.0 }
}

fn v_overlap(gt: &Rect, b: &Rect) -> f32 {
    let inter = (gt.y1().min(b.y1()) - gt.y0().max(b.y0())).max(0.0);
    if gt.height() > 0.0 {
        inter / gt.height()
    } else {
        0.0
    }
}

fn matches(gt: &Rect, b: &Rect) -> bool {
    x_iou(gt, b) >= MIN_X_IOU && v_overlap(gt, b) >= MIN_V_OVERLAP
}

/// Sum over ground-truth boxes of the best IoU with a returned box: picks among occurrences.
fn score(gt: &[Rect], got: &[Rect]) -> f32 {
    gt.iter()
        .map(|g| got.iter().map(|b| g.iou(b)).fold(0.0, f32::max))
        .sum()
}

/// Share of dark pixels inside `gt` that are also inside `got` (both unrotated points).
/// `None` if there is no ink under `gt` at all.
fn ink_share(img: &GrayImage, info: &PageInfo, gt: &Rect, got: &Rect) -> Option<f32> {
    let px = |r: &Rect, pad: f32| {
        let d = to_display(r, info);
        (
            ((d.x0() * RENDER_SCALE - pad).floor().max(0.0)) as u32,
            ((d.y0() * RENDER_SCALE - pad).floor().max(0.0)) as u32,
            ((d.x1() * RENDER_SCALE + pad).ceil() as u32).min(img.width),
            ((d.y1() * RENDER_SCALE + pad).ceil() as u32).min(img.height),
        )
    };
    let (gx0, gy0, gx1, gy1) = px(gt, 0.0);
    let (bx0, by0, bx1, by1) = px(got, PAD_PX);
    let (mut total, mut inside) = (0u32, 0u32);
    for y in gy0..gy1 {
        for x in gx0..gx1 {
            if img.get(x, y) < DARK {
                total += 1;
                if (bx0..bx1).contains(&x) && (by0..by1).contains(&y) {
                    inside += 1;
                }
            }
        }
    }
    (total > 0).then(|| inside as f32 / total as f32)
}

fn fmt_rects(r: &[Rect]) -> String {
    r.iter()
        .map(|r| format!("[{:.1},{:.1},{:.1},{:.1}]", r.x0(), r.y0(), r.x1(), r.y1()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs every sample. The stored text comes from one parse pass per file; every sample then
/// opens the file afresh, as the page view would.
pub fn run(engine: &dyn PdfEngine, corpus: &Corpus, samples: &[Sample]) -> Vec<SampleResult> {
    // Parse pass: stored text per (file, page).
    let mut stored: HashMap<(String, u32), Result<String, String>> = HashMap::new();
    let mut by_file: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for s in samples {
        by_file.entry(&s.file).or_default().push(s.page);
    }
    for (file, pages) in by_file {
        let doc = engine.open(&corpus.path(file));
        for p in pages {
            let text = match &doc {
                Err(e) => Err(format!("open: {e}")),
                Ok(doc) => doc
                    .page_info(p as usize - 1)
                    .and_then(|info| Ok(layout(&doc.chars(p as usize - 1)?, &info).text))
                    .map_err(|e| e.to_string()),
            };
            stored.insert((file.to_string(), p), text);
        }
    }
    samples
        .iter()
        .map(|s| {
            let (step, detail) = one(engine, corpus, s, &stored[&(s.file.clone(), s.page)]);
            SampleResult {
                mention: s.mention.clone(),
                file: s.file.clone(),
                page: s.page,
                raw: s.raw.clone(),
                identifier: s.identifier,
                hard: s.hard,
                step,
                detail,
            }
        })
        .collect()
}

fn one(
    engine: &dyn PdfEngine,
    corpus: &Corpus,
    s: &Sample,
    stored: &Result<String, String>,
) -> (Step, String) {
    let stored = match stored {
        Ok(t) => t,
        Err(e) => return (Step::Find, e.clone()),
    };
    // 1. Find.
    let spans = find_all(stored, &s.raw);
    if spans.is_empty() {
        return (Step::Find, "raw text not in stored page text".into());
    }
    let page = PageNumber::new(s.page).expect("1-based page");
    // 2. Map, through geometry from a fresh open.
    let doc = match engine.open(&corpus.path(&s.file)) {
        Ok(d) => d,
        Err(e) => return (Step::Map, format!("reopen: {e}")),
    };
    let geo = doc
        .page_info(page.index())
        .and_then(|info| Ok((info, layout(&doc.chars(page.index())?, &info).geometry)));
    let (info, geometry) = match geo {
        Ok(g) => g,
        Err(e) => return (Step::Map, e.to_string()),
    };
    let gt: Vec<Rect> = s.boxes.iter().map(to_rect).collect();
    let mut best: Option<(f32, Vec<Rect>)> = None;
    let mut last_err = None;
    for span in spans {
        match geometry.highlight(stored, span) {
            Ok(b) => {
                let sc = score(&gt, &b);
                if best.as_ref().is_none_or(|(x, _)| sc > *x) {
                    best = Some((sc, b));
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    let Some((_, got)) = best else {
        return (
            Step::Map,
            format!("{}", last_err.expect("one error per span")),
        );
    };
    // 3. Geometry, compared in display orientation so "x" is the reading direction.
    let disp = |v: &[Rect]| v.iter().map(|r| to_display(r, &info)).collect::<Vec<_>>();
    let (gd, bd) = (disp(&gt), disp(&got));
    let pairs: Vec<Option<&Rect>> = gd
        .iter()
        .map(|g| bd.iter().position(|b| matches(g, b)).map(|i| &got[i]))
        .collect();
    let extra = bd.iter().any(|b| !gd.iter().any(|g| matches(g, b)));
    if pairs.iter().any(Option::is_none) || extra {
        return (
            Step::Geometry,
            format!("gt {} got {}", fmt_rects(&gt), fmt_rects(&got)),
        );
    }
    // 4. Render.
    let img = match doc.render(page.index(), RENDER_SCALE) {
        Ok(i) => i,
        Err(e) => return (Step::Render, format!("render: {e}")),
    };
    for (g, b) in gt.iter().zip(pairs) {
        match ink_share(&img, &info, g, b.expect("checked")) {
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

#[derive(Serialize)]
pub struct PageFind {
    pub total: usize,
    pub found: usize,
    pub missed: Vec<String>,
}

/// Public files: identifier mentions with a page only. Checks the raw text is on that page of
/// the stored text (no boxes in the ground truth).
pub fn find_on_page(engine: &dyn PdfEngine, corpus: &Corpus) -> PageFind {
    let items = corpus.pdf_page_identifiers();
    let mut texts: HashMap<(String, u32), String> = HashMap::new();
    let mut docs = HashMap::new();
    let (mut found, mut missed) = (0, Vec::new());
    for (m, file, page) in &items {
        let key = (file.clone(), *page);
        if !texts.contains_key(&key) {
            let doc = docs
                .entry(file.clone())
                .or_insert_with(|| engine.open(&corpus.path(file)));
            let text = match doc {
                Ok(d) => d
                    .page_info(*page as usize - 1)
                    .and_then(|i| Ok(layout(&d.chars(*page as usize - 1)?, &i).text))
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            texts.insert(key.clone(), text);
        }
        if find_all(&texts[&key], &m.raw).is_empty() {
            missed.push(format!("{} p{} {:?}", file, page, m.raw));
        } else {
            found += 1;
        }
    }
    PageFind {
        total: items.len(),
        found,
        missed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ignores_whitespace_and_line_breaks() {
        let text = "Ref: GB86 QMTB\n6099 x";
        let spans = find_all(text, "GB86 QMTB 6099");
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start(), spans[0].end()), (5, 19));
        assert!(find_all(text, "QMTB7").is_empty());
    }

    #[test]
    fn geometry_match_thresholds() {
        let gt = Rect::new(10.0, 10.0, 110.0, 20.0).unwrap();
        assert!(matches(&gt, &Rect::new(12.0, 11.0, 108.0, 22.0).unwrap()));
        assert!(!matches(&gt, &Rect::new(10.0, 16.0, 110.0, 30.0).unwrap()));
        assert!(!matches(&gt, &Rect::new(10.0, 10.0, 60.0, 20.0).unwrap()));
    }
}
