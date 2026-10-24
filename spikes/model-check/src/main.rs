//! Spike S1: local model reality check over TD-1 (M1). Throwaway. Method and results:
//! `docs/09-model-check.md`.
//!
//! ```text
//! model-check runtime   -m <gguf> [--llama-server <path>]
//! model-check segments  [--dump]
//! model-check extract   --model <gguf> [--label L] [--np 1] [--limit N] [--server-args "…"] [--llama-server <path>]
//! model-check score     --label L
//! model-check embed     --model <gguf> [--label L] [--np 4] [--reuse] [--server-args "…"] [--llama-server <path>]
//! model-check relations --model <gguf> [--label L] [--server-args "…"] [--llama-server <path>]
//! model-check report
//! ```
//!
//! `llama-server` is found as the app will find it (ADR-0015): `--llama-server` stands for the
//! settings path, then `PATH`. The harness talks to `127.0.0.1` only.

mod client;
mod embed;
mod extract;
mod probe;
mod relations;
mod report;
mod runtime;
mod score;
mod segments;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `spikes/`.
pub fn spikes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn testdata() -> PathBuf {
    std::env::var_os("MK_TESTDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| spikes_dir().join("../testdata"))
}

pub fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    std::fs::create_dir_all(&d).expect("create out/");
    d
}

struct Args(Vec<String>);

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        let i = self.0.iter().position(|a| a == name)?;
        self.0.get(i + 1).map(String::as_str)
    }
    fn num(&self, name: &str, default: usize) -> usize {
        self.get(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
    fn model(&self) -> Result<PathBuf, String> {
        let m = self
            .get("--model")
            .or_else(|| self.get("-m"))
            .ok_or("--model <gguf> is required")?;
        let p = PathBuf::from(m);
        if !p.exists() {
            return Err(format!("{m}: no such file"));
        }
        Ok(p)
    }
    fn job(&self, task: &'static str, default_np: usize) -> Result<extract::Job<'static>, String> {
        let model = self.model()?;
        let np = self.num("--np", default_np);
        let stem = model
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let label = self.get("--label").map(str::to_string).unwrap_or_else(|| {
            if np > 1 && task == "extract" {
                format!("{stem}-np{np}")
            } else {
                stem
            }
        });
        Ok(extract::Job {
            label,
            task,
            model,
            settings: self.get("--llama-server").map(PathBuf::from),
            np,
            extra_args: self
                .get("--server-args")
                .map(|s| s.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            out: out_dir(),
        })
    }
}

fn segments() -> Result<segments::Segments, String> {
    let root = testdata().join("td1");
    let gt = mk_bench::load(&root.join("ground-truth.json")).map_err(|e| e.to_string())?;
    segments::Segments::build(&root, &gt)
}

fn print_segments(s: &segments::Segments) {
    use std::collections::BTreeMap;
    let mut by: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for seg in &s.list {
        let e = by.entry(seg.source.name()).or_default();
        e.0 += 1;
        e.1 += seg.gold.len();
        e.2 += seg.gold_keys().len();
    }
    for (src, (n, m, k)) in &by {
        println!(
            "{src:<10} {n:>4} segments, {m:>4} gold mentions, {k:>4} gold keys (per-segment sets)"
        );
    }
    let c = &s.coverage;
    println!(
        "covered {}/{} name mentions ({:.1}%); not covered by kind {:?}; not visible {}",
        c.covered,
        c.name_mentions,
        100.0 * c.covered as f64 / c.name_mentions.max(1) as f64,
        c.not_covered,
        c.not_visible.len()
    );
    for nv in &c.not_visible {
        println!("  not visible: {nv}");
    }
}

fn run(cmd: &str, args: &Args) -> Result<(), String> {
    let out = out_dir();
    match cmd {
        "runtime" => {
            let model = args.model()?;
            let steps = runtime::check(args.get("--llama-server").map(Path::new), &model, &out);
            let doc = serde_json::json!({
                "started_utc": extract::utc_now(),
                "model": model.file_name().map(|n| n.to_string_lossy().into_owned()),
                "steps": steps,
            });
            std::fs::write(
                out.join("runtime.json"),
                serde_json::to_string_pretty(&doc).expect("serializes") + "\n",
            )
            .map_err(|e| e.to_string())?;
            if steps.iter().all(|s| s.pass) {
                Ok(())
            } else {
                Err("a runtime step failed".into())
            }
        }
        "segments" => {
            let s = segments()?;
            print_segments(&s);
            if args.has("--dump") {
                let mut lines = String::new();
                for seg in &s.list {
                    lines.push_str(&serde_json::to_string(seg).expect("serializes"));
                    lines.push('\n');
                }
                std::fs::write(out.join("segments.jsonl"), lines).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        "extract" => {
            let job = args.job("extract", 1)?;
            let s = segments()?;
            let mut list = s.list;
            if let Some(n) = args.get("--limit").and_then(|v| v.parse().ok()) {
                list = segments::Rng::new(3).sample(&list, n);
            }
            eprintln!("{} segments → {}", list.len(), job.label);
            extract::run(&job, &list)?;
            score::run(&job.label, &list, &out).map(|_| ())
        }
        "score" => {
            let label = args.get("--label").ok_or("--label is required")?;
            score::run(label, &segments()?.list, &out).map(|_| ())
        }
        "embed" => {
            let job = args.job("embed", 4)?;
            let root = testdata().join("td1");
            let inp = embed::inputs(
                &root,
                &spikes_dir().join("out/public-text.jsonl"),
                &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("queries.json"),
            )?;
            eprintln!(
                "{} windows, {} queries → {}",
                inp.corpus.len(),
                inp.queries.len(),
                job.label
            );
            let r = embed::run(&job, &inp, args.has("--reuse"))?;
            embed::print(&r);
            Ok(())
        }
        "relations" => {
            let job = args.job("relations", 1)?;
            let root = testdata().join("td1");
            let gt = mk_bench::load(&root.join("ground-truth.json")).map_err(|e| e.to_string())?;
            relations::run(&job, &gt, &segments()?.list)
        }
        "report" => report::run(&out),
        _ => Err(format!("unknown command {cmd:?}; see the crate docs")),
    }
}

fn main() -> ExitCode {
    let args = Args(std::env::args().skip(1).collect());
    let Some(cmd) = args.0.first().cloned() else {
        eprintln!("usage: model-check <command> (see src/main.rs)");
        return ExitCode::FAILURE;
    };
    match run(&cmd, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod corpus_tests {
    //! Needs the local corpus (`testdata/`, `docs/07-test-corpus.md` §9). Without it the test
    //! prints a notice and passes, unless `MK_REQUIRE_TESTDATA=1` is set, as in `mk-bench`.
    use super::*;

    #[test]
    fn segments_build_from_td1() {
        if !testdata().join("td1/ground-truth.json").exists() {
            assert!(
                std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
                "testdata/td1 is missing; run corpus-gen fetch and generate"
            );
            eprintln!("skipped: testdata/td1 is missing");
            return;
        }
        let s = segments().expect("segments build");
        let ledger: Vec<_> = s
            .list
            .iter()
            .filter(|x| x.document == segments::LEDGER_DOC)
            .collect();
        let ledger_gold = ledger.iter().filter(|x| !x.gold.is_empty()).count();
        assert_eq!(ledger.len(), ledger_gold + segments::LEDGER_FILLER);
        for src in segments::Source::ALL {
            assert!(
                s.list.iter().any(|x| x.source == src),
                "no {src:?} segments"
            );
        }
        // Every gold mention is visible in its segment, and coverage is as planned (~396/467).
        assert!(
            s.coverage.not_visible.is_empty(),
            "{:?}",
            s.coverage.not_visible
        );
        assert!(s.coverage.covered >= 380, "covered {}", s.coverage.covered);
        assert!(
            s.list
                .iter()
                .all(|x| x.text.chars().count() <= segments::NOTE_CHUNK_CHARS
                    || x.source == segments::Source::Email)
        );
    }
}
