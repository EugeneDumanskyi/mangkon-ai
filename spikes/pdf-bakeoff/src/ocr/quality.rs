//! OCR pass over every TD-1 OCR page: speed (NFR-PERF-21), character accuracy (NFR-QA-07),
//! identifier recall as raw-text presence (NFR-QA-06 proxy), word confidence (US-02.2 AC3) and
//! determinism. The page texts and geometries it produces are the stored OCR results the
//! round-trip reads.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use mk_bench::{Location, ScanTier, Subject};
use mk_kernel::ImageSize;
use serde::Serialize;

use crate::bench::{levenshtein, norm, sorted_lines};
use crate::corpus::Corpus;
use crate::ocr::engine::{LANGS, OcrEngine};
use crate::ocr::layout::{OcrLaid, layout};
use crate::ocr::pages::{self, OcrPage, Source};
use crate::roundtrip::find_all;

#[derive(Serialize)]
pub struct PageResult {
    pub file: String,
    pub page: u32,
    pub tier: ScanTier,
    pub source: Source,
    pub clean_a4: bool,
    pub width: u32,
    pub height: u32,
    /// Decode (or render, for PDF pages), recognise, layout: wall seconds, one thread.
    pub decode_s: f64,
    pub ocr_s: f64,
    pub layout_s: f64,
    /// CPU seconds of the process over the three steps (checks "one core").
    pub cpu_s: f64,
    pub ref_chars: usize,
    /// Edits against the reference (characters) after the OCR lines are put in reference order
    /// (`reordered`); with both sides' lines sorted (the M0b measure); and as stored.
    pub edits: usize,
    pub edits_sorted: usize,
    pub edits_as_is: usize,
    pub words: usize,
    pub words_with_confidence: usize,
    /// Same text and geometry when OCR'd again (clean pages only).
    pub deterministic: Option<bool>,
    pub error: Option<String>,
}

impl PageResult {
    pub fn total_s(&self) -> f64 {
        self.decode_s + self.ocr_s + self.layout_s
    }
}

#[derive(Serialize, Default, Clone)]
pub struct Accuracy {
    pub pages: usize,
    pub ref_chars: usize,
    /// 1 − CER, OCR lines in reference order (NFR-QA-07 definition, see `reordered`).
    pub accuracy: f64,
    /// 1 − CER, both sides' lines sorted (M0b measure; a misread first character moves a line).
    pub accuracy_sorted: f64,
    /// 1 − CER, as stored (reading order included).
    pub accuracy_as_is: f64,
}

#[derive(Serialize, Default, Clone)]
pub struct Recall {
    pub total: usize,
    pub found: usize,
    pub missed: Vec<String>,
}

impl Recall {
    pub fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.found as f64 / self.total as f64
        }
    }
}

#[derive(Serialize)]
pub struct Quality {
    pub engine: String,
    pub model_load_s: f64,
    pub pages: Vec<PageResult>,
    /// By tier name, plus `all`.
    pub accuracy: BTreeMap<String, Accuracy>,
    pub recall: BTreeMap<String, Recall>,
    /// Clean A4 pages: mean and max of decode + OCR + layout, and mean CPU seconds.
    pub speed_mean_s: f64,
    pub speed_max_s: f64,
    pub speed_cpu_mean_s: f64,
    pub word_confidence: bool,
    pub deterministic: bool,
}

/// User + system CPU seconds of this process so far (Linux, from `/proc/self/stat`).
fn cpu_secs() -> f64 {
    let Ok(s) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // Fields after the parenthesised command name; utime and stime are fields 14 and 15.
    let rest = s.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0
}

fn tier_name(t: ScanTier) -> &'static str {
    match t {
        ScanTier::Clean => "clean",
        ScanTier::Office => "office",
        ScanTier::Poor => "poor",
    }
}

/// OCRs one page: (result row, stored text + geometry).
fn ocr_page(engine: &dyn OcrEngine, corpus: &Corpus, p: &OcrPage) -> (PageResult, Option<OcrLaid>) {
    let mut r = PageResult {
        file: p.file.clone(),
        page: p.page,
        tier: p.tier,
        source: p.source,
        clean_a4: p.clean_a4(),
        width: 0,
        height: 0,
        decode_s: 0.0,
        ocr_s: 0.0,
        layout_s: 0.0,
        cpu_s: 0.0,
        ref_chars: norm(&p.reference).len(),
        edits: 0,
        edits_sorted: 0,
        edits_as_is: 0,
        words: 0,
        words_with_confidence: 0,
        deterministic: None,
        error: None,
    };
    let cpu0 = cpu_secs();
    let t0 = Instant::now();
    let img = match pages::load(corpus, p) {
        Ok(i) => i,
        Err(e) => {
            r.error = Some(format!("load: {e}"));
            return (r, None);
        }
    };
    let t1 = Instant::now();
    let lines = match engine.recognize(&img, LANGS) {
        Ok(l) => l,
        Err(e) => {
            r.error = Some(format!("ocr: {e}"));
            return (r, None);
        }
    };
    let t2 = Instant::now();
    let laid = layout(
        &lines,
        ImageSize {
            width: img.width,
            height: img.height,
        },
    );
    let t3 = Instant::now();
    r.cpu_s = cpu_secs() - cpu0;
    (r.width, r.height) = (img.width, img.height);
    r.decode_s = (t1 - t0).as_secs_f64();
    r.ocr_s = (t2 - t1).as_secs_f64();
    r.layout_s = (t3 - t2).as_secs_f64();
    r.edits = levenshtein(
        &norm(&reordered(&laid.text, &p.reference)),
        &norm(&p.reference),
    );
    r.edits_sorted = levenshtein(
        &norm(&sorted_lines(&laid.text)),
        &norm(&sorted_lines(&p.reference)),
    );
    r.edits_as_is = levenshtein(&norm(&laid.text), &norm(&p.reference));
    r.words = laid.geometry.words().len();
    r.words_with_confidence = laid
        .geometry
        .words()
        .iter()
        .filter(|w| w.confidence.is_some())
        .count();
    (r, Some(laid))
}

/// The OCR lines, each moved to the reference line it matches (edit distance as a share of the
/// longer line). Pairs are taken cheapest first, one OCR line per reference line, so repeated
/// lines each find their own place; OCR lines left over (a reference line split in two) join
/// their best match. Ties keep OCR order. Reading-order neutral, and a real edit distance: only
/// the OCR side is permuted, whole lines at a time. Sorting both sides (M0b) does not work for
/// OCR: one misread first character moves the line and costs its whole length.
pub fn reordered(got: &str, reference: &str) -> String {
    let lines =
        |s: &str| -> Vec<Vec<char>> { s.lines().map(norm).filter(|l| !l.is_empty()).collect() };
    let (ocr, refs) = (lines(got), lines(reference));
    let cost = |a: &[char], b: &[char]| levenshtein(a, b) as f64 / a.len().max(b.len()) as f64;
    let mut pairs: Vec<(f64, usize, usize)> = (0..ocr.len())
        .flat_map(|i| (0..refs.len()).map(move |j| (i, j)))
        .map(|(i, j)| (cost(&ocr[i], &refs[j]), i, j))
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    let mut place: Vec<Option<usize>> = vec![None; ocr.len()];
    let mut taken = vec![false; refs.len()];
    for &(_, i, j) in &pairs {
        if place[i].is_none() && !taken[j] {
            (place[i], taken[j]) = (Some(j), true);
        }
    }
    for &(_, i, j) in &pairs {
        place[i].get_or_insert(j);
    }
    let mut order: Vec<(usize, usize)> = place
        .iter()
        .enumerate()
        .map(|(i, j)| (j.unwrap_or(0), i))
        .collect();
    order.sort();
    order
        .into_iter()
        .map(|(_, i)| ocr[i].iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn accuracy<'a>(rows: impl Iterator<Item = &'a PageResult>) -> Accuracy {
    let mut a = Accuracy::default();
    let mut e = [0usize; 3];
    for r in rows {
        a.pages += 1;
        a.ref_chars += r.ref_chars;
        // A page that failed counts as every character wrong.
        let failed = r.error.is_some();
        for (sum, x) in e.iter_mut().zip([r.edits, r.edits_sorted, r.edits_as_is]) {
            *sum += if failed { r.ref_chars } else { x };
        }
    }
    let rate = |x: usize| 1.0 - x as f64 / a.ref_chars.max(1) as f64;
    a.accuracy = rate(e[0]);
    a.accuracy_sorted = rate(e[1]);
    a.accuracy_as_is = rate(e[2]);
    a
}

/// Runs the pass. Returns the measurements and the stored OCR result per (file, page).
pub fn run(
    name: &str,
    engine: &dyn OcrEngine,
    model_load_s: f64,
    corpus: &Corpus,
) -> (Quality, HashMap<(String, u32), OcrLaid>) {
    let pages = pages::ocr_pages(corpus);
    let mut rows = Vec::new();
    let mut stored = HashMap::new();
    for p in &pages {
        eprintln!("[{name}] ocr {} p{}", p.file, p.page);
        let (mut row, laid) = ocr_page(engine, corpus, p);
        if let Some(laid) = laid {
            if p.tier == ScanTier::Clean {
                let again = ocr_page(engine, corpus, p).1;
                row.deterministic =
                    Some(again.is_some_and(|a| a.text == laid.text && a.geometry == laid.geometry));
            }
            stored.insert(p.key(), laid);
        }
        rows.push(row);
    }

    let mut acc = BTreeMap::new();
    for t in [ScanTier::Clean, ScanTier::Office, ScanTier::Poor] {
        acc.insert(
            tier_name(t).to_string(),
            accuracy(rows.iter().filter(|r| r.tier == t)),
        );
    }
    acc.insert("all".into(), accuracy(rows.iter()));

    // Identifier mentions on OCR pages: is the raw text in the OCR page text?
    let tier_of: HashMap<(String, u32), ScanTier> =
        pages.iter().map(|p| (p.key(), p.tier)).collect();
    let file_of: HashMap<&str, &str> = pages
        .iter()
        .map(|p| (p.document.as_str(), p.file.as_str()))
        .collect();
    let mut recall: BTreeMap<String, Recall> = BTreeMap::new();
    for m in &corpus.gt.mentions {
        let (Location::Scan { page, .. }, Subject::Identifier { .. }) = (&m.location, &m.subject)
        else {
            continue;
        };
        if m.derived_from.is_some() {
            continue;
        }
        let Some(file) = file_of.get(m.document.as_str()) else {
            continue;
        };
        let key = (file.to_string(), *page);
        let Some(tier) = tier_of.get(&key) else {
            continue;
        };
        let found = stored
            .get(&key)
            .is_some_and(|l: &OcrLaid| !find_all(&l.text, &m.raw).is_empty());
        for bucket in [tier_name(*tier), "all"] {
            let r = recall.entry(bucket.to_string()).or_default();
            r.total += 1;
            if found {
                r.found += 1;
            } else {
                r.missed.push(format!("{file} p{page} {:?}", m.raw));
            }
        }
    }

    let speed: Vec<&PageResult> = rows
        .iter()
        .filter(|r| r.clean_a4 && r.error.is_none())
        .collect();
    let k = speed.len().max(1) as f64;
    let q = Quality {
        engine: name.to_string(),
        model_load_s,
        speed_mean_s: speed.iter().map(|r| r.total_s()).sum::<f64>() / k,
        speed_max_s: speed.iter().map(|r| r.total_s()).fold(0.0, f64::max),
        speed_cpu_mean_s: speed.iter().map(|r| r.cpu_s).sum::<f64>() / k,
        word_confidence: rows.iter().any(|r| r.words > 0)
            && rows.iter().all(|r| r.words_with_confidence == r.words),
        deterministic: rows.iter().all(|r| r.deterministic != Some(false)),
        accuracy: acc,
        recall,
        pages: rows,
    };
    (q, stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordered_follows_reference_lines() {
        let reference = "1. Services here\n2. Fees there\nTotal 10";
        // Second column first, and a misread first character: still in reference order.
        let got = "Total 10\nI. Services here\n2. Fees there";
        assert_eq!(
            reordered(got, reference),
            "I. Services here\n2. Fees there\nTotal 10"
        );
        let e = levenshtein(&norm(&reordered(got, reference)), &norm(reference));
        assert_eq!(e, 1);
        // A repeated line goes to each of its places.
        let reference = "EUR\nNet 5\nEUR\nTax 1";
        assert_eq!(reordered("Net 5\nTax 1\nEUR\nEUR", reference), reference);
    }
}
