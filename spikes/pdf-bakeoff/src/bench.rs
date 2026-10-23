//! Throughput (NFR-PERF-20, parse + layout only), stored-text quality vs the reference page
//! text (diagnostic), and page-view timing on the 500-page ledger (ADR-0011, ADR-0013).

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use serde::Serialize;

use crate::corpus::{Corpus, SplitMix};
use crate::engine::PdfEngine;
use crate::layout::layout;
use crate::roundtrip::RENDER_SCALE;

pub const LEDGER: &str = "synthetic/pdf/vendor-ledger-2024-2025.pdf";
pub const TD3_HUGE: &str = "pages-2000.pdf";

#[derive(Serialize, Default, Clone)]
pub struct Group {
    pub files: usize,
    pub pages: usize,
    pub page_errors: usize,
    pub secs: f64,
    pub pages_per_sec: f64,
}

#[derive(Serialize)]
pub struct Throughput {
    /// TD-1 text-layer PDFs by group, then `td1 total` and `td3 pages-2000`.
    pub groups: BTreeMap<String, Group>,
    pub open_errors: Vec<String>,
    pub quality: Quality,
}

#[derive(Serialize, Default)]
pub struct Quality {
    /// Synthetic pages with reference text.
    pub pages: usize,
    /// Character error rate over all those pages (edits / reference chars), whitespace runs
    /// collapsed to one space. Sensitive to reading order.
    pub cer: f64,
    /// The same with each page's lines sorted first: characters, spaces and line breaks right,
    /// whatever the order of the lines (tables, columns).
    pub cer_lines_sorted: f64,
    pub median_page_cer: f64,
    /// Pages with CER > 5%.
    pub pages_over_5pct: usize,
    /// Worst pages: (file p<page>, CER).
    pub worst: Vec<(String, f64)>,
}

fn add(g: &mut Group, pages: usize, errors: usize, secs: f64) {
    g.files += 1;
    g.pages += pages;
    g.page_errors += errors;
    g.secs += secs;
    g.pages_per_sec = if g.secs > 0.0 {
        g.pages as f64 / g.secs
    } else {
        0.0
    };
}

/// One file's parse pass: pages tried, page errors, seconds, and the texts of the pages kept.
struct Parsed {
    pages: usize,
    errors: usize,
    secs: f64,
    texts: Vec<(u32, String)>,
}

/// Opens the file and runs chars + layout on the given pages (all if `None`), keeping the
/// text of the pages in `keep`.
fn parse(
    engine: &dyn PdfEngine,
    path: &std::path::Path,
    pages: Option<&[u32]>,
    keep: &dyn Fn(u32) -> bool,
) -> Result<Parsed, String> {
    let t = Instant::now();
    let doc = engine.open(path).map_err(|e| e.to_string())?;
    let list: Vec<u32> = match pages {
        Some(p) => p.to_vec(),
        None => (1..=doc.page_count() as u32).collect(),
    };
    let (mut errors, mut texts) = (0, Vec::new());
    for &p in &list {
        let i = p as usize - 1;
        match doc
            .page_info(i)
            .and_then(|info| Ok(layout(&doc.chars(i)?, &info)))
        {
            Ok(laid) if keep(p) => texts.push((p, laid.text)),
            Ok(_) => {}
            Err(_) => errors += 1,
        }
    }
    Ok(Parsed {
        pages: list.len(),
        errors,
        secs: t.elapsed().as_secs_f64(),
        texts,
    })
}

pub fn throughput(
    engine: &dyn PdfEngine,
    corpus: &Corpus,
    td3_root: &std::path::Path,
) -> Throughput {
    let refs = corpus.page_texts();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut open_errors = Vec::new();
    let mut got: HashMap<(String, u32), String> = HashMap::new();
    let mut total = Group::default();
    for f in corpus.text_pdfs() {
        let keep = |p: u32| refs.contains_key(&(f.path.clone(), p));
        match parse(engine, &corpus.path(&f.path), f.pages.as_deref(), &keep) {
            Ok(Parsed {
                pages,
                errors,
                secs,
                texts,
            }) => {
                add(
                    groups.entry(f.group.into()).or_default(),
                    pages,
                    errors,
                    secs,
                );
                add(&mut total, pages, errors, secs);
                for (p, t) in texts {
                    got.insert((f.path.clone(), p), t);
                }
            }
            Err(e) => open_errors.push(format!("{}: {e}", f.path)),
        }
    }
    groups.insert("td1 total".into(), total);
    match parse(engine, &td3_root.join(TD3_HUGE), None, &|_| false) {
        Ok(Parsed {
            pages,
            errors,
            secs,
            ..
        }) => add(
            groups.entry("td3 pages-2000".into()).or_default(),
            pages,
            errors,
            secs,
        ),
        Err(e) => open_errors.push(format!("td3/{TD3_HUGE}: {e}")),
    }
    let quality = quality(&refs, &got);
    Throughput {
        groups,
        open_errors,
        quality,
    }
}

fn norm(s: &str) -> Vec<char> {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect()
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize)
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn sorted_lines(s: &str) -> String {
    let mut lines: Vec<String> = s
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    lines.sort();
    lines.join("\n")
}

fn quality(
    refs: &HashMap<(String, u32), &mk_bench::PageText>,
    got: &HashMap<(String, u32), String>,
) -> Quality {
    let (mut edits, mut chars, mut sorted_edits) = (0usize, 0usize, 0usize);
    let mut per: Vec<(String, f64)> = Vec::new();
    for (key, r) in refs {
        let reference = norm(&r.text);
        let text = got.get(key).map(|t| norm(t)).unwrap_or_default();
        let e = levenshtein(&text, &reference);
        edits += e;
        let got_sorted = got
            .get(key)
            .map(|t| norm(&sorted_lines(t)))
            .unwrap_or_default();
        sorted_edits += levenshtein(&got_sorted, &norm(&sorted_lines(&r.text)));
        chars += reference.len();
        per.push((
            format!("{} p{}", key.0, key.1),
            e as f64 / reference.len().max(1) as f64,
        ));
    }
    per.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut cers: Vec<f64> = per.iter().map(|p| p.1).collect();
    cers.sort_by(f64::total_cmp);
    Quality {
        pages: per.len(),
        cer: if chars > 0 {
            edits as f64 / chars as f64
        } else {
            0.0
        },
        cer_lines_sorted: if chars > 0 {
            sorted_edits as f64 / chars as f64
        } else {
            0.0
        },
        median_page_cer: cers.get(cers.len() / 2).copied().unwrap_or(0.0),
        pages_over_5pct: cers.iter().filter(|c| **c > 0.05).count(),
        worst: per.into_iter().take(8).collect(),
    }
}

#[derive(Serialize)]
pub struct RenderTiming {
    pub file: String,
    pub scale: f32,
    pub pages: Vec<u32>,
    /// Fresh open + page info + chars + layout + render, per page, milliseconds.
    pub total_ms: Vec<f64>,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    /// Means of the parts.
    pub open_ms: f64,
    pub chars_layout_ms: f64,
    pub render_ms: f64,
    pub errors: Vec<String>,
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

pub fn render_timing(engine: &dyn PdfEngine, corpus: &Corpus, n: usize, seed: u64) -> RenderTiming {
    let path = corpus.path(LEDGER);
    let page_count = engine.open(&path).map(|d| d.page_count()).unwrap_or(0) as u64;
    let mut rng = SplitMix(seed);
    let pages: Vec<u32> = (0..n)
        .map(|_| (rng.next() % page_count.max(1)) as u32 + 1)
        .collect();
    let (mut total, mut open, mut cl, mut rend, mut errors) = (vec![], 0.0, 0.0, 0.0, vec![]);
    for &p in &pages {
        let i = p as usize - 1;
        let t0 = Instant::now();
        let r = engine.open(&path).and_then(|doc| {
            let t1 = Instant::now();
            let info = doc.page_info(i)?;
            let _laid = layout(&doc.chars(i)?, &info);
            let t2 = Instant::now();
            let _img = doc.render(i, RENDER_SCALE)?;
            Ok((t1, t2))
        });
        let end = Instant::now();
        match r {
            Ok((t1, t2)) => {
                total.push((end - t0).as_secs_f64() * 1e3);
                open += (t1 - t0).as_secs_f64() * 1e3;
                cl += (t2 - t1).as_secs_f64() * 1e3;
                rend += (end - t2).as_secs_f64() * 1e3;
            }
            Err(e) => errors.push(format!("p{p}: {e}")),
        }
    }
    let mut sorted = total.clone();
    sorted.sort_by(f64::total_cmp);
    let k = total.len().max(1) as f64;
    RenderTiming {
        file: LEDGER.into(),
        scale: RENDER_SCALE,
        pages,
        p50_ms: pct(&sorted, 0.5),
        p95_ms: pct(&sorted, 0.95),
        max_ms: sorted.last().copied().unwrap_or(0.0),
        total_ms: total,
        open_ms: open / k,
        chars_layout_ms: cl / k,
        render_ms: rend / k,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(levenshtein(&c("kitten"), &c("sitting")), 3);
        assert_eq!(levenshtein(&c(""), &c("abc")), 3);
        assert_eq!(norm(" a \n b  c "), c("a b c"));
    }

    #[test]
    fn percentile_nearest_rank() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(pct(&v, 0.95), 19.0);
        assert_eq!(pct(&v, 0.5), 10.0);
    }
}
