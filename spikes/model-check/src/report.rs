//! `model-check report`: markdown tables from the run files in `out/` → `out/report.md`.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::embed::EmbedResult;
use crate::extract::RunMeta;
use crate::score::{Prf, Score};

fn read<T: serde::de::DeserializeOwned>(p: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn files(out: &Path, prefix: &str) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(out)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".json"))
        })
        .collect();
    v.sort();
    v
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn pr(p: &Prf) -> String {
    format!("{} / {}", pct(p.precision()), pct(p.recall()))
}

fn gb(b: u64) -> String {
    format!("{:.1}", b as f64 / 1e9)
}

fn gib_mib(m: u64) -> String {
    format!("{:.1}", m as f64 / 1024.0)
}

pub fn run(out: &Path) -> Result<(), String> {
    let mut md = String::new();
    if let Some(rt) = read::<Value>(&out.join("runtime.json")) {
        let _ = writeln!(
            md,
            "### Runtime ({})\n\n| Step | Result | Detail |\n|---|---|---|",
            rt["started_utc"].as_str().unwrap_or("")
        );
        for s in rt["steps"].as_array().into_iter().flatten() {
            let _ = writeln!(
                md,
                "| {} | {} | {} |",
                s["step"].as_str().unwrap_or(""),
                if s["pass"].as_bool() == Some(true) {
                    "✅"
                } else {
                    "❌"
                },
                s["detail"].as_str().unwrap_or("").replace('|', "\\|")
            );
        }
        md.push('\n');
    }

    let runs: Vec<(Score, RunMeta)> = files(out, "score-")
        .iter()
        .filter_map(|p| {
            let s: Score = read(p)?;
            let m: RunMeta = read(&out.join(format!("extract-{}.json", s.label)))?;
            Some((s, m))
        })
        .collect();
    if !runs.is_empty() {
        let _ = writeln!(
            md,
            "### Extraction\n\n| Run | File GB | np | P / R | Shown (≥ 0.5) P / R | Id-like dropped P / R | Person P / R | Org P / R | Type-agnostic P / R | Segments/s | Tokens in / out | Load s | RSS peak GB | VRAM peak GiB |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
        );
        for (s, m) in &runs {
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.2} | {:.0} / {:.0} | {:.0} | {} | {} |",
                s.label,
                gb(m.model_bytes),
                m.np,
                pr(&s.overall),
                pr(&s.shown),
                pr(&s.id_like_dropped),
                pr(&s.person),
                pr(&s.org),
                pr(&s.type_agnostic),
                m.items as f64 / m.wall_s,
                s.prompt_tokens_mean,
                s.completion_tokens_mean,
                m.load_s,
                gb(m.peaks.vm_hwm_bytes.max(m.peaks.rss_max_bytes)),
                gib_mib(m.peaks.vram_proc_max_mib)
            );
        }
        let _ = writeln!(
            md,
            "\n| Run | Schema ok first / retry / failed | Truncated | Predictions | Span discarded | Name not in segment | Links gold / predicted / correct | Link P / R (estimate) |\n|---|---|---|---|---|---|---|---|"
        );
        for (s, _) in &runs {
            let _ = writeln!(
                md,
                "| {} | {} / {} / {} | {} | {} | {} ({}) | {} | {} / {} / {} | {} / {} |",
                s.label,
                s.schema_ok_first,
                s.schema_ok_retry,
                s.schema_failed,
                s.truncated,
                s.predictions,
                s.span_discarded,
                pct(s.span_discarded as f64 / s.predictions.max(1) as f64),
                s.name_not_in_segment,
                s.links.gold,
                s.links.predicted,
                s.links.correct,
                pct(s.links.precision),
                pct(s.links.recall)
            );
        }
        let sources: Vec<String> = runs[0].0.by_source.keys().cloned().collect();
        let _ = writeln!(
            md,
            "\n| Run | {} |\n|---|{}",
            sources.join(" | "),
            "---|".repeat(sources.len())
        );
        for (s, _) in &runs {
            let cells: Vec<String> = sources
                .iter()
                .map(|k| s.by_source.get(k).map(pr).unwrap_or_default())
                .collect();
            let _ = writeln!(md, "| {} | {} |", s.label, cells.join(" | "));
        }
        md.push('\n');
        for (s, _) in &runs {
            let _ = writeln!(
                md,
                "- `{}` top FP: {:?}; top FN: {:?}",
                s.label, s.top_fp, s.top_fn
            );
        }
        md.push('\n');
    }

    let embeds: Vec<(EmbedResult, Option<RunMeta>)> = files(out, "embed-score-")
        .iter()
        .filter_map(|p| {
            let r: EmbedResult = read(p)?;
            let m = read(&out.join(format!("embed-{}.json", r.label)));
            Some((r, m))
        })
        .collect();
    if !embeds.is_empty() {
        let _ = writeln!(
            md,
            "### Embeddings\n\n| Model | Dims | Windows | Binary only | Pool 40 | Pool 100 | Pool 200 | Pool 200, f16 | Min (pool 200) | Hand / seeded (pool 200) | Seeded self-hit | Windows/s | RSS GB | VRAM GiB |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
        );
        for (r, m) in &embeds {
            let at = |pool: usize| {
                r.recall
                    .iter()
                    .find(|x| x.pool == pool)
                    .cloned()
                    .unwrap_or_default()
            };
            let _ = writeln!(
                md,
                "| {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.2} | {:.3} / {:.3} | {:.2} | {:.0} | {} | {} |",
                r.label,
                r.dims,
                r.windows,
                r.binary_only,
                at(40).rescore_f32,
                at(100).rescore_f32,
                at(200).rescore_f32,
                at(200).rescore_f16,
                at(200).rescore_f32_min,
                r.hand_rescore_f32,
                r.seeded_rescore_f32,
                r.seeded_self_hit,
                r.windows_per_s,
                m.as_ref()
                    .map(|m| gb(m.peaks.vm_hwm_bytes.max(m.peaks.rss_max_bytes)))
                    .unwrap_or_default(),
                m.as_ref()
                    .map(|m| gib_mib(m.peaks.vram_proc_max_mib))
                    .unwrap_or_default(),
            );
        }
        md.push('\n');
    }

    for p in files(out, "relations-score-") {
        if let Some(v) = read::<Value>(&p) {
            let _ = writeln!(
                md,
                "Relations `{}`: {}",
                v["label"].as_str().unwrap_or(""),
                v
            );
        }
    }

    let _ = writeln!(md, "\nRuns:");
    for (_, m) in &runs {
        let _ = writeln!(
            md,
            "- `{}`: {} UTC, build {}, `{}`",
            m.label,
            m.started_utc,
            m.build.build,
            m.server_args.join(" ")
        );
    }
    for (r, m) in &embeds {
        if let Some(m) = m {
            let _ = writeln!(
                md,
                "- `{}`: {} UTC, build {}, `{}`",
                r.label,
                m.started_utc,
                m.build.build,
                m.server_args.join(" ")
            );
        }
    }
    std::fs::write(out.join("report.md"), &md).map_err(|e| e.to_string())?;
    print!("{md}");
    Ok(())
}
