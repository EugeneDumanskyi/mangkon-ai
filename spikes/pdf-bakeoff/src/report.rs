//! `report`: every measurement for every engine, written to `spikes/out/report.json` and
//! `spikes/out/report.md` (copied into `docs/08-engine-bakeoff.md` by hand). OCR engines
//! (`ocrs`, `tesseract`, `onnx`) get their own sections from `ocr::report`.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::bench::{self, RenderTiming, Throughput};
use crate::corpus::{self, Corpus};
use crate::roundtrip::{self, PageFind, SampleResult, Step, Tally};
use crate::td3::{self, Ended, Td3Result};
use crate::{engine, licenses, out_dir, testdata};

const SAMPLES: usize = 200;
const SEED: u64 = 1;
const TIMING_PAGES: usize = 20;

#[derive(Serialize)]
pub struct EngineReport {
    pub engine: String,
    pub error: Option<String>,
    pub throughput: Option<Throughput>,
    pub render_timing: Option<RenderTiming>,
    pub roundtrip_sample: Option<Tally>,
    pub roundtrip_pool: Option<Tally>,
    pub roundtrip_identifiers: Option<Tally>,
    pub roundtrip_hard: Option<Tally>,
    pub roundtrip_failures: Vec<SampleResult>,
    pub pdf_page_find: Option<PageFind>,
    pub td3: Vec<Td3Result>,
}

fn run_engine(name: &str, c: &Corpus) -> EngineReport {
    let mut r = EngineReport {
        engine: name.into(),
        error: None,
        throughput: None,
        render_timing: None,
        roundtrip_sample: None,
        roundtrip_pool: None,
        roundtrip_identifiers: None,
        roundtrip_hard: None,
        roundtrip_failures: Vec::new(),
        pdf_page_find: None,
        td3: Vec::new(),
    };
    let e = match engine::by_name(name) {
        Ok(e) => e,
        Err(err) => {
            r.error = Some(err);
            return r;
        }
    };
    eprintln!("[{name}] throughput");
    r.throughput = Some(bench::throughput(&*e, c, &testdata().join("td3")));
    eprintln!("[{name}] render timing");
    r.render_timing = Some(bench::render_timing(&*e, c, TIMING_PAGES, SEED));
    eprintln!("[{name}] round-trip");
    let pool = c.pdf_text_pool();
    let picked: HashSet<String> = corpus::sample(&pool, SAMPLES, SEED)
        .into_iter()
        .map(|s| s.mention)
        .collect();
    let results = roundtrip::run(&*e, c, &pool);
    r.roundtrip_sample = Some(Tally::of(
        results.iter().filter(|x| picked.contains(&x.mention)),
    ));
    r.roundtrip_pool = Some(Tally::of(&results));
    r.roundtrip_identifiers = Some(Tally::of(results.iter().filter(|x| x.identifier)));
    r.roundtrip_hard = Some(Tally::of(results.iter().filter(|x| x.hard)));
    r.roundtrip_failures = results
        .into_iter()
        .filter(|x| x.step != Step::Pass)
        .collect();
    r.pdf_page_find = Some(roundtrip::find_on_page(&*e, c));
    eprintln!("[{name}] td3");
    r.td3 = td3::run(name, &testdata().join("td3"), &out_dir());
    r
}

fn sh(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn machine() -> BTreeMap<&'static str, String> {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default();
    BTreeMap::from([
        ("machine", "RM-1".to_string()),
        ("os", sh("uname", &["-sm"])),
        ("kernel", sh("uname", &["-r"])),
        ("cpu", cpu),
        // Not `nproc`: it honours the OMP_THREAD_LIMIT=1 that `main` sets for the OCR runs.
        (
            "threads",
            std::thread::available_parallelism()
                .map_or(0, |n| n.get())
                .to_string(),
        ),
        ("rustc", sh("rustc", &["-V"])),
        ("date_utc", sh("date", &["-u", "+%Y-%m-%d %H:%M"])),
    ])
}

fn mark(ok: bool) -> &'static str {
    if ok { "✅" } else { "❌" }
}

fn pct(t: &Tally) -> String {
    format!("{:.1}% ({}/{})", t.rate() * 100.0, t.passed, t.total)
}

const OCR_ENGINES: &[&str] = &["ocrs", "tesseract", "onnx"];

pub fn run(names: &[String], c: &Corpus) -> Result<(), String> {
    let out = out_dir();
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let (ocr_names, pdf_names): (Vec<&String>, Vec<&String>) = names
        .iter()
        .partition(|n| OCR_ENGINES.contains(&n.as_str()));
    #[cfg(not(feature = "ocr"))]
    if !ocr_names.is_empty() {
        return Err(format!(
            "{ocr_names:?} need an OCR feature (ocrs, tesseract, onnx)"
        ));
    }
    #[cfg(feature = "ocr")]
    let mut ocr_reports = Vec::new();
    #[cfg(feature = "ocr")]
    for n in &ocr_names {
        let r = crate::ocr::report::run_engine(n, c);
        std::fs::write(
            out.join(format!("report-{n}.json")),
            serde_json::to_string_pretty(&r).expect("serializes"),
        )
        .map_err(|e| e.to_string())?;
        ocr_reports.push(r);
    }
    let mut reports = Vec::new();
    for n in pdf_names {
        let r = run_engine(n, c);
        std::fs::write(
            out.join(format!("report-{n}.json")),
            serde_json::to_string_pretty(&r).expect("serializes"),
        )
        .map_err(|e| e.to_string())?;
        reports.push(r);
    }
    let lic = licenses::by_engine().map_err(|e| format!("licenses: {e}"))?;
    let probe: Value = std::fs::read_to_string(out.join("build-probe.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let m = machine();
    #[cfg(feature = "ocr")]
    let ocr_json = serde_json::to_value(&ocr_reports).expect("serializes");
    #[cfg(not(feature = "ocr"))]
    let ocr_json = Value::Array(Vec::new());
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "machine": m, "engines": reports, "ocr_engines": ocr_json, "licenses": lic,
            "native_licenses": licenses::NATIVE, "build_probe": probe,
        }))
        .expect("serializes"),
    )
    .map_err(|e| e.to_string())?;
    #[cfg_attr(not(feature = "ocr"), allow(unused_mut))]
    let mut md = markdown(&m, &reports, &lic, &probe);
    #[cfg(feature = "ocr")]
    md.push_str(&crate::ocr::report::markdown(&ocr_reports, &lic, &probe));
    std::fs::write(out.join("report.md"), &md).map_err(|e| e.to_string())?;
    println!("{md}");
    Ok(())
}

fn markdown(
    m: &BTreeMap<&str, String>,
    rs: &[EngineReport],
    lic: &BTreeMap<String, licenses::EngineLicenses>,
    probe: &Value,
) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Machine: {} — {}, {} threads, {}; {}; run {} UTC.\n",
        m["machine"], m["os"], m["threads"], m["cpu"], m["rustc"], m["date_utc"]
    );
    let head = |s: &mut String, first: &[&str]| {
        let cols: Vec<String> = first
            .iter()
            .map(|x| x.to_string())
            .chain(rs.iter().map(|r| r.engine.clone()))
            .collect();
        let _ = writeln!(s, "| {} |", cols.join(" | "));
        let _ = writeln!(s, "|{}", "---|".repeat(cols.len()));
    };
    let row = |s: &mut String, first: &[&str], f: &dyn Fn(&EngineReport) -> String| {
        let cells: Vec<String> = first
            .iter()
            .map(|x| x.to_string())
            .chain(rs.iter().map(|r| {
                if let Some(e) = &r.error {
                    format!("not run: {e}")
                } else {
                    f(r)
                }
            }))
            .collect();
        let _ = writeln!(s, "| {} |", cells.join(" | "));
    };

    if rs.is_empty() {
        return s;
    }
    let _ = writeln!(s, "### S0 PDF criteria\n");
    head(&mut s, &["Criterion", "Pass mark"]);
    row(
        &mut s,
        &[
            "Text throughput, TD-1 text-layer PDFs (parse + layout, 1 thread)",
            "≥ 20 pages/s",
        ],
        &|r| {
            let g = &r.throughput.as_ref().unwrap().groups["td1 total"];
            format!(
                "{} {:.0} pages/s ({} pages, {:.2} s)",
                mark(g.pages_per_sec >= 20.0 && g.page_errors == 0),
                g.pages_per_sec,
                g.pages,
                g.secs
            )
        },
    );
    row(
        &mut s,
        &[
            "Page view on the 500-page ledger: fresh open + boxes + render at 2×, p95 of 20 pages",
            "≤ 1,000 ms",
        ],
        &|r| {
            let t = r.render_timing.as_ref().unwrap();
            format!(
                "{} p95 {:.0} ms (p50 {:.0}, max {:.0}; errors {})",
                mark(t.p95_ms <= 1000.0 && t.errors.is_empty()),
                t.p95_ms,
                t.p50_ms,
                t.max_ms,
                t.errors.len()
            )
        },
    );
    row(
        &mut s,
        &["Highlight round-trip, 200 samples (seed 1)", "≥ 99%"],
        &|r| {
            let t = r.roundtrip_sample.as_ref().unwrap();
            format!("{} {}", mark(t.rate() >= 0.99), pct(t))
        },
    );
    row(
        &mut s,
        &["TD-3 PDFs: each ends alone in its own process", "all"],
        &|r| {
            format!(
                "{} {}/{}",
                mark(!r.td3.is_empty()),
                r.td3.len(),
                r.td3.len()
            )
        },
    );
    row(
        &mut s,
        &[
            "TD-3 PDFs: outcome as in `expected.json` (engine-decided files)",
            "all",
        ],
        &|r| {
            let scored: Vec<&Td3Result> = r
                .td3
                .iter()
                .filter(|t| !td3::APP_GUARD.contains(&t.file.as_str()))
                .collect();
            let ok = scored.iter().filter(|t| t.matches).count();
            format!("{} {}/{}", mark(ok == scored.len()), ok, scored.len())
        },
    );
    row(
        &mut s,
        &[
            "TD-3 PDFs: failures with a plain reason (clean error, no kill)",
            "info",
        ],
        &|r| {
            let fails: Vec<&Td3Result> = r
                .td3
                .iter()
                .filter(|t| t.outcome == mk_bench::td3::Outcome::Fail)
                .collect();
            let clean = fails.iter().filter(|t| t.ended == Ended::Clean).count();
            format!("{}/{}", clean, fails.len())
        },
    );
    row(
        &mut s,
        &[
            "Password-protected PDF reported as such (US-02.1 AC2)",
            "yes",
        ],
        &|r| {
            let t = r
                .td3
                .iter()
                .find(|t| t.file == "encrypted-user-password.pdf");
            let ok = t.and_then(|t| t.report.as_ref()).and_then(|r| r.error_kind)
                == Some(engine::PdfErrorKind::Encrypted);
            mark(ok).to_string()
        },
    );
    row(
        &mut s,
        &["Clean release build, Linux (fresh target dir)", "≤ 15 min"],
        &|r| {
            let p = &probe[&r.engine];
            match p["secs"].as_f64() {
                Some(t) => format!("{} {:.0} s", mark(t <= 900.0), t),
                None => "not measured".into(),
            }
        },
    );
    row(
        &mut s,
        &[
            "Binary size delta over the no-engine build (+ shared library)",
            "counts toward ≤ 150 MB",
        ],
        &|r| {
            let p = &probe[&r.engine];
            let base = probe["baseline"]["bytes"].as_f64().unwrap_or(0.0);
            match p["bytes"].as_f64() {
                Some(b) => {
                    let lib = p["lib_bytes"].as_f64().unwrap_or(0.0);
                    if lib > 0.0 {
                        format!("+{:.1} MB (+ {:.1} MB `.so`)", (b - base) / 1e6, lib / 1e6)
                    } else {
                        format!("+{:.1} MB", (b - base) / 1e6)
                    }
                }
                None => "not measured".into(),
            }
        },
    );
    row(
        &mut s,
        &["Licenses AGPL-3.0-compatible (`cargo metadata`)", "all"],
        &|r| match lic.get(&r.engine) {
            Some(l) => {
                let extra = if r.engine == "pdfium" {
                    " + libpdfium (see notices)"
                } else {
                    ""
                };
                format!(
                    "{} {} crates added, {} flagged{extra}",
                    mark(l.flagged.is_empty()),
                    l.crates,
                    l.flagged.len()
                )
            }
            None => "—".into(),
        },
    );

    let _ = writeln!(s, "\n### Diagnostics (no pass mark)\n");
    head(&mut s, &["Measure"]);
    row(
        &mut s,
        &["Round-trip, full pool (identifiers + names)"],
        &|r| pct(r.roundtrip_pool.as_ref().unwrap()),
    );
    row(&mut s, &["Round-trip, identifier mentions"], &|r| {
        pct(r.roundtrip_identifiers.as_ref().unwrap())
    });
    row(&mut s, &["Round-trip, `hard` mentions"], &|r| {
        pct(r.roundtrip_hard.as_ref().unwrap())
    });
    row(&mut s, &["Round-trip failures by step (pool)"], &|r| {
        format!("{:?}", r.roundtrip_pool.as_ref().unwrap().failed_by_step)
    });
    row(
        &mut s,
        &["Public identifiers found on their page (`pdf_page`)"],
        &|r| {
            let f = r.pdf_page_find.as_ref().unwrap();
            format!("{}/{}", f.found, f.total)
        },
    );
    row(
        &mut s,
        &["Stored text vs reference, CER (synthetic text-layer pages)"],
        &|r| {
            let q = &r.throughput.as_ref().unwrap().quality;
            format!(
                "{:.2}% over {} pages (median page {:.2}%, {} pages > 5%)",
                q.cer * 100.0,
                q.pages,
                q.median_page_cer * 100.0,
                q.pages_over_5pct
            )
        },
    );
    row(
        &mut s,
        &["Same, lines sorted first (order-insensitive)"],
        &|r| {
            format!(
                "{:.2}%",
                r.throughput.as_ref().unwrap().quality.cer_lines_sorted * 100.0
            )
        },
    );
    for g in [
        "synthetic text",
        "synthetic mixed (text pages)",
        "synthetic ocr-layer",
        "public",
        "td3 pages-2000",
    ] {
        let label = format!("Throughput: {g}");
        row(&mut s, &[&label], &|r| {
            let t = r.throughput.as_ref().unwrap();
            match t.groups.get(g) {
                Some(x) => format!(
                    "{:.0} pages/s ({} pages, {} page errors)",
                    x.pages_per_sec, x.pages, x.page_errors
                ),
                None => "—".into(),
            }
        });
    }
    row(
        &mut s,
        &["Page view parts, mean ms: open / chars + layout / render"],
        &|r| {
            let t = r.render_timing.as_ref().unwrap();
            format!(
                "{:.0} / {:.0} / {:.0}",
                t.open_ms, t.chars_layout_ms, t.render_ms
            )
        },
    );
    row(&mut s, &["Open errors (TD-1)"], &|r| {
        r.throughput.as_ref().unwrap().open_errors.len().to_string()
    });

    let _ = writeln!(s, "\n### TD-3 PDFs\n");
    let files: Vec<(String, String, String)> = rs
        .iter()
        .find(|r| !r.td3.is_empty())
        .map(|r| {
            r.td3
                .iter()
                .map(|t| {
                    (
                        t.file.clone(),
                        format!("{:?}", t.category),
                        format!("{:?}", t.expected).to_lowercase(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    head(&mut s, &["File", "Category", "Expected"]);
    for (file, cat, exp) in &files {
        row(&mut s, &[&format!("`{file}`"), cat, exp], &|r| match r
            .td3
            .iter()
            .find(|t| &t.file == file)
        {
            Some(t) if td3::APP_GUARD.contains(&t.file.as_str()) => format!(
                "app guard (engine: {:?}, {:.0} MB)",
                t.outcome, t.peak_rss_mb
            )
            .to_lowercase(),
            Some(t) => {
                let how = match (t.ended, t.report.as_ref().and_then(|x| x.error_kind)) {
                    (Ended::Clean, Some(k)) if t.outcome == mk_bench::td3::Outcome::Fail => {
                        format!("clean, {k:?}")
                    }
                    (Ended::Clean, _) => {
                        format!("{} pages", t.report.as_ref().map_or(0, |x| x.pages))
                    }
                    (e, _) => format!("{e:?}"),
                };
                let outcome = format!("{:?}", t.outcome).to_lowercase();
                format!(
                    "{} {outcome} ({how}, {:.1} s, {:.0} MB)",
                    mark(t.matches),
                    t.secs,
                    t.peak_rss_mb
                )
            }
            None => "—".into(),
        });
    }

    let _ = writeln!(s, "\n### Round-trip failures (full pool)\n");
    for r in rs {
        let _ = writeln!(
            s,
            "**{}** — {} failures",
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

    let _ = writeln!(s, "### Licenses added per engine\n");
    for (e, l) in lic {
        let list: Vec<String> = l
            .licenses
            .iter()
            .map(|(k, v)| format!("{k} ×{v}"))
            .collect();
        let _ = writeln!(
            s,
            "- **{e}** ({} crates): {}{}",
            l.crates,
            list.join("; "),
            if l.flagged.is_empty() {
                String::new()
            } else {
                format!(" — flagged: {}", l.flagged.join(", "))
            }
        );
    }
    s
}
