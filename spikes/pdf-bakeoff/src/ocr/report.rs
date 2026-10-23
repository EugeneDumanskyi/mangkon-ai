//! OCR part of `report`: every S0 OCR measurement for every OCR engine, as JSON and as markdown
//! sections appended to `spikes/out/report.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;

use crate::corpus::Corpus;
use crate::fetch::ocr_model_bytes;
use crate::licenses::{self, EngineLicenses, NATIVE};
use crate::ocr::quality::{self, Quality};
use crate::ocr::td3::{self, OcrTd3Result};
use crate::roundtrip::{SampleResult, Step, Tally};
use crate::td3::Ended;
use crate::{out_dir, testdata};

/// What a build of each engine needs beyond cargo and a Rust toolchain, read from the build
/// scripts and the build logs.
const BUILD_NEEDS: &[(&str, &str)] = &[
    ("ocrs", "nothing"),
    (
        "tesseract",
        "cmake, a C++ compiler; downloads Leptonica + Tesseract sources from GitHub (no checksum)",
    ),
    (
        "onnx",
        "downloads a prebuilt ONNX Runtime from pyke's CDN (checksum in `ort`); TLS via rustls",
    ),
];

/// What the per-word confidence is, where the yes/no needs a note.
const CONFIDENCE_NOTE: &[(&str, &str)] = &[
    ("ocrs", "not in the 0.13 API"),
    ("tesseract", "per word"),
    ("onnx", "per line, copied to its words"),
];

#[derive(Serialize)]
pub struct OcrEngineReport {
    pub engine: String,
    pub error: Option<String>,
    pub model_bytes: u64,
    pub quality: Option<Quality>,
    /// All 166 `scan` mentions; Find misses included.
    pub roundtrip_all: Option<Tally>,
    /// Mentions the OCR text holds (Find passed): the rate the pass mark applies to.
    pub roundtrip_found: Option<Tally>,
    pub roundtrip_identifiers: Option<Tally>,
    pub roundtrip_hard: Option<Tally>,
    /// Failures after Find (Map, Geometry, Render).
    pub roundtrip_failures: Vec<SampleResult>,
    pub td3: Vec<OcrTd3Result>,
}

pub fn run_engine(name: &str, c: &Corpus) -> OcrEngineReport {
    let mut r = OcrEngineReport {
        engine: name.into(),
        error: None,
        model_bytes: ocr_model_bytes(name),
        quality: None,
        roundtrip_all: None,
        roundtrip_found: None,
        roundtrip_identifiers: None,
        roundtrip_hard: None,
        roundtrip_failures: Vec::new(),
        td3: Vec::new(),
    };
    let t = Instant::now();
    let e = match crate::ocr::engine::by_name(name) {
        Ok(e) => e,
        Err(err) => {
            r.error = Some(err);
            return r;
        }
    };
    let load = t.elapsed().as_secs_f64();
    eprintln!("[{name}] quality");
    let (q, stored) = quality::run(name, &*e, load, c);
    eprintln!("[{name}] round-trip");
    let results = crate::ocr::roundtrip::run(c, &crate::ocr::roundtrip::pool(c), &stored);
    let found: Vec<&SampleResult> = results.iter().filter(|x| x.step != Step::Find).collect();
    r.roundtrip_all = Some(Tally::of(&results));
    r.roundtrip_found = Some(Tally::of(found.iter().copied()));
    r.roundtrip_identifiers = Some(Tally::of(found.iter().copied().filter(|x| x.identifier)));
    r.roundtrip_hard = Some(Tally::of(found.iter().copied().filter(|x| x.hard)));
    r.roundtrip_failures = results
        .into_iter()
        .filter(|x| x.step != Step::Pass && x.step != Step::Find)
        .collect();
    r.quality = Some(q);
    eprintln!("[{name}] td3");
    r.td3 = td3::run(name, &testdata().join("td3"), &out_dir());
    r
}

fn mark(ok: bool) -> &'static str {
    if ok { "✅" } else { "❌" }
}

fn pct(t: &Tally) -> String {
    if t.total == 0 {
        return "— (0/0)".into();
    }
    format!("{:.1}% ({}/{})", t.rate() * 100.0, t.passed, t.total)
}

fn note(table: &[(&str, &'static str)], engine: &str) -> &'static str {
    table
        .iter()
        .find(|(e, _)| *e == engine)
        .map_or("", |(_, n)| n)
}

fn mb(bytes: f64) -> String {
    format!("{:.1} MB", bytes / 1e6)
}

pub fn markdown(
    rs: &[OcrEngineReport],
    lic: &BTreeMap<String, EngineLicenses>,
    probe: &Value,
) -> String {
    let mut s = String::new();
    let head = |s: &mut String, first: &[&str]| {
        let cols: Vec<String> = first
            .iter()
            .map(|x| x.to_string())
            .chain(rs.iter().map(|r| r.engine.clone()))
            .collect();
        let _ = writeln!(s, "| {} |", cols.join(" | "));
        let _ = writeln!(s, "|{}", "---|".repeat(cols.len()));
    };
    let row = |s: &mut String, first: &[&str], f: &dyn Fn(&OcrEngineReport, &Quality) -> String| {
        let cells: Vec<String> = first
            .iter()
            .map(|x| x.to_string())
            .chain(rs.iter().map(|r| match (&r.error, &r.quality) {
                (Some(e), _) => format!("not run: {e}"),
                (None, Some(q)) => f(r, q),
                (None, None) => "—".into(),
            }))
            .collect();
        let _ = writeln!(s, "| {} |", cells.join(" | "));
    };
    let clean_a4 = rs
        .iter()
        .find_map(|r| r.quality.as_ref())
        .map_or(0, |q| q.pages.iter().filter(|p| p.clean_a4).count());

    let _ = writeln!(s, "\n### S0 OCR criteria\n");
    head(&mut s, &["Criterion", "Pass mark"]);
    let speed = format!(
        "OCR speed, clean A4 300 DPI pages ({clean_a4}): decode + OCR + layout, 1 thread, max page"
    );
    row(&mut s, &[&speed, "≤ 5 s/page"], &|_, q| {
        let errors = q
            .pages
            .iter()
            .filter(|p| p.clean_a4 && p.error.is_some())
            .count();
        format!(
            "{} max {:.2} s, mean {:.2} s (CPU mean {:.2} s; errors {errors})",
            mark(q.speed_max_s <= 5.0 && errors == 0),
            q.speed_max_s,
            q.speed_mean_s,
            q.speed_cpu_mean_s
        )
    });
    row(
        &mut s,
        &[
            "Character accuracy, clean tier: 1 − CER, whitespace-normalised, OCR lines in reference order",
            "≥ 95%",
        ],
        &|_, q| {
            let a = &q.accuracy["clean"];
            format!(
                "{} {:.1}% ({} pages, {} chars)",
                mark(a.accuracy >= 0.95),
                a.accuracy * 100.0,
                a.pages,
                a.ref_chars
            )
        },
    );
    row(
        &mut s,
        &[
            "Identifier recall, clean tier (raw text in OCR page text; proxy)",
            "≥ 85%",
        ],
        &|_, q| {
            let r = q.recall.get("clean").cloned().unwrap_or_default();
            format!(
                "{} {:.1}% ({}/{})",
                mark(r.rate() >= 0.85),
                r.rate() * 100.0,
                r.found,
                r.total
            )
        },
    );
    row(
        &mut s,
        &[
            "Highlight round-trip on scans, over found mentions (all 166 `scan` mentions)",
            "≥ 99%",
        ],
        &|r, _| {
            let (all, f) = (
                r.roundtrip_all.as_ref().unwrap(),
                r.roundtrip_found.as_ref().unwrap(),
            );
            format!(
                "{} {} — found {}/{}",
                mark(f.rate() >= 0.99),
                pct(f),
                f.total,
                all.total
            )
        },
    );
    row(
        &mut s,
        &["TD-3 images: each ends alone in its own process", "all"],
        &|r, _| {
            // Every child ends one way or another and the parent goes on (as M0b).
            let clean = r.td3.iter().filter(|t| t.ended == Ended::Clean).count();
            let n = r.td3.len();
            format!("{} {n}/{n} ({clean} clean exits)", mark(n > 0))
        },
    );
    row(
        &mut s,
        &["TD-3 images: outcome as in `expected.json`", "all"],
        &|r, _| {
            let ok = r.td3.iter().filter(|t| t.matches).count();
            format!(
                "{} {ok}/{}",
                mark(!r.td3.is_empty() && ok == r.td3.len()),
                r.td3.len()
            )
        },
    );
    row(
        &mut s,
        &["Word confidence (US-02.2 AC3, INV-ID-03)", "yes"],
        &|r, q| {
            let n = note(CONFIDENCE_NOTE, &r.engine);
            let yes = if q.word_confidence { "yes" } else { "no" };
            format!("{} {yes} ({n})", mark(q.word_confidence))
        },
    );
    row(
        &mut s,
        &[
            "Clean release build, Linux (fresh target dir and HOME), one command, no Python",
            "≤ 15 min",
        ],
        &|r, _| match probe[&r.engine]["secs"].as_f64() {
            Some(t) => format!(
                "{} {t:.0} s; needs: {}",
                mark(t <= 900.0),
                note(BUILD_NEEDS, &r.engine)
            ),
            None => "not measured".into(),
        },
    );
    row(
        &mut s,
        &[
            "Size: binary delta over the `ocr` build + model files",
            "counts toward ≤ 150 MB",
        ],
        &|r, _| {
            let base = probe["ocr"]["bytes"].as_f64();
            match (probe[&r.engine]["bytes"].as_f64(), base) {
                (Some(b), Some(base)) => format!(
                    "+{} binary + {} models = {}",
                    mb(b - base),
                    mb(r.model_bytes as f64),
                    mb(b - base + r.model_bytes as f64)
                ),
                _ => format!("not measured (models {})", mb(r.model_bytes as f64)),
            }
        },
    );
    row(
        &mut s,
        &[
            "Licenses AGPL-3.0-compatible: crates over `ocr`, native libraries, model files",
            "all",
        ],
        &|r, _| {
            let native: Vec<&(&str, &str, &str, &str, &str)> =
                NATIVE.iter().filter(|n| n.0 == r.engine).collect();
            let review: Vec<&str> = native
                .iter()
                .filter(|n| !licenses::compatible(n.2))
                .map(|n| n.2)
                .collect();
            match lic.get(&r.engine) {
                Some(l) => {
                    let m = if !l.flagged.is_empty() {
                        "❌"
                    } else if !review.is_empty() {
                        "⚠️"
                    } else {
                        "✅"
                    };
                    let mut names: Vec<&str> = native.iter().map(|n| n.2).collect();
                    names.sort_unstable();
                    names.dedup();
                    let extra = if review.is_empty() {
                        String::new()
                    } else {
                        format!("; review: {}", review.join(", "))
                    };
                    format!(
                        "{m} {} crates, {} flagged; native/models: {}{extra}",
                        l.crates,
                        l.flagged.len(),
                        names.join(", ")
                    )
                }
                None => "—".into(),
            }
        },
    );

    let _ = writeln!(s, "\n### OCR diagnostics (no pass mark)\n");
    head(&mut s, &["Measure"]);
    for tier in ["office", "poor", "all"] {
        let label = if tier == "all" {
            "Character accuracy, all tiers (reference order)".to_string()
        } else {
            format!("Character accuracy, {tier} tier (reference order)")
        };
        row(&mut s, &[&label], &|_, q| {
            let a = &q.accuracy[tier];
            format!("{:.1}% ({} pages)", a.accuracy * 100.0, a.pages)
        });
    }
    row(
        &mut s,
        &["Character accuracy, clean tier: lines sorted (M0b) / as stored"],
        &|_, q| {
            let a = &q.accuracy["clean"];
            format!(
                "{:.1}% / {:.1}%",
                a.accuracy_sorted * 100.0,
                a.accuracy_as_is * 100.0
            )
        },
    );
    for tier in ["office", "poor", "all"] {
        let label = if tier == "all" {
            "Identifier recall, all tiers".to_string()
        } else {
            format!("Identifier recall, {tier} tier")
        };
        row(&mut s, &[&label], &|_, q| {
            let r = q.recall.get(tier).cloned().unwrap_or_default();
            format!("{:.1}% ({}/{})", r.rate() * 100.0, r.found, r.total)
        });
    }
    row(
        &mut s,
        &["Round-trip over found: identifier mentions"],
        &|r, _| pct(r.roundtrip_identifiers.as_ref().unwrap()),
    );
    row(
        &mut s,
        &["Round-trip over found: `hard` mentions"],
        &|r, _| pct(r.roundtrip_hard.as_ref().unwrap()),
    );
    row(
        &mut s,
        &["Round-trip, all 166: failures by step (find = OCR misread)"],
        &|r, _| format!("{:?}", r.roundtrip_all.as_ref().unwrap().failed_by_step),
    );
    row(
        &mut s,
        &["Deterministic (clean pages OCR'd twice: same text and boxes)"],
        &|_, q| if q.deterministic { "yes" } else { "no" }.into(),
    );
    row(&mut s, &["Model load"], &|_, q| {
        format!("{:.2} s", q.model_load_s)
    });
    row(&mut s, &["Model / language files"], &|r, _| {
        mb(r.model_bytes as f64)
    });
    row(
        &mut s,
        &["Bytes build scripts wrote into HOME (downloads + their build cache)"],
        &|r, _| match probe[&r.engine]["fetched_bytes"].as_f64() {
            Some(b) => mb(b),
            None => "not measured".into(),
        },
    );
    row(
        &mut s,
        &["Dynamic libraries of the binary"],
        &|r, _| match probe[&r.engine]["dynamic"].as_str() {
            Some(d) => format!("`{d}`"),
            None => "not measured".into(),
        },
    );
    if let (Some(o), Some(h)) = (
        probe["ocr"]["bytes"].as_f64(),
        probe["hayro"]["bytes"].as_f64(),
    ) {
        let crates = lic.get("ocr").map_or(0, |l| l.crates);
        let _ = writeln!(
            s,
            "\nShared OCR image path (`image` crate: PNG, JPEG, TIFF; scanned PDF pages rendered \
             by hayro): +{} binary over the hayro build, {crates} crates.",
            mb(o - h)
        );
    }

    let _ = writeln!(s, "\n### TD-3 images\n");
    let files: Vec<(String, String)> = rs
        .iter()
        .find(|r| !r.td3.is_empty())
        .map(|r| {
            r.td3
                .iter()
                .map(|t| (t.file.clone(), format!("{:?}", t.expected).to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    head(&mut s, &["File", "Expected"]);
    for (file, exp) in &files {
        row(&mut s, &[&format!("`{file}`"), exp], &|r, _| match r
            .td3
            .iter()
            .find(|t| &t.file == file)
        {
            Some(t) => {
                let how = match (&t.report, t.ended) {
                    (Some(x), Ended::Clean) if x.error_kind.is_some() => {
                        format!("{:?}: {}", x.error_kind.unwrap(), x.message)
                    }
                    (Some(x), Ended::Clean) => format!("{} words", x.words),
                    (_, e) => format!("{e:?}"),
                };
                format!(
                    "{} {} ({how}; {:.1} s)",
                    mark(t.matches),
                    format!("{:?}", t.outcome).to_lowercase(),
                    t.secs
                )
            }
            None => "—".into(),
        });
    }

    let _ = writeln!(
        s,
        "\n### OCR pages (decode + OCR + layout s; character edits)\n"
    );
    let pages: Vec<(String, u32, String)> = rs
        .iter()
        .find_map(|r| r.quality.as_ref())
        .map(|q| {
            q.pages
                .iter()
                .map(|p| {
                    let tier = format!(
                        "{:?}{}, {}×{}",
                        p.tier,
                        if p.clean_a4 { " A4" } else { "" },
                        p.width,
                        p.height
                    )
                    .to_lowercase();
                    (p.file.clone(), p.page, tier)
                })
                .collect()
        })
        .unwrap_or_default();
    head(&mut s, &["Page", "Tier, px"]);
    for (file, page, tier) in &pages {
        let label = format!("`{}` p{page}", file.rsplit('/').next().unwrap_or(file));
        row(&mut s, &[&label, tier], &|_, q| match q
            .pages
            .iter()
            .find(|p| &p.file == file && p.page == *page)
        {
            Some(p) if p.error.is_some() => format!("error: {}", p.error.as_ref().unwrap()),
            Some(p) => format!("{:.2} s; {}/{}", p.total_s(), p.edits, p.ref_chars),
            None => "—".into(),
        });
    }

    let _ = writeln!(s, "\n### Scan round-trip failures after Find\n");
    for r in rs {
        let misses = r
            .roundtrip_all
            .as_ref()
            .and_then(|t| t.failed_by_step.get("find"))
            .copied()
            .unwrap_or(0);
        let _ = writeln!(
            s,
            "**{}**: {} failures; {misses} Find misses (OCR misreads, counted as recall)",
            r.engine,
            r.roundtrip_failures.len()
        );
        for f in r.roundtrip_failures.iter().take(15) {
            let _ = writeln!(
                s,
                "- {:?} `{}` p{} `{}`: {}",
                f.step, f.file, f.page, f.raw, f.detail
            );
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(
        s,
        "### Native libraries and model files (checked by hand)\n"
    );
    let _ = writeln!(s, "| Feature | Component | License | Ships as | Source |");
    let _ = writeln!(s, "|---|---|---|---|---|");
    for (f, comp, l, how, src) in NATIVE {
        let _ = writeln!(s, "| {f} | {comp} | {l} | {how} | {src} |");
    }
    s
}
