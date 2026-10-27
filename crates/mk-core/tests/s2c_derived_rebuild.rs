//! Spike S2c (roadmap §3, R7, K4; NFR-PERF-34): does the per-commit derived rebuild (ADR-0005)
//! stay fast at TD-2 scale with many decisions?
//!
//! 1. A project is built through the real pipeline from TD-2's TXT/MD/CSV files.
//! 2. It is topped up through the store's write API to 10,000 documents and at least 200k
//!    identifier mentions, with Zipf-spread keys; a few hot keys are in every synthetic
//!    document.
//! 3. Each phase ingests 100 more TXT/CSV files of mixed size (hot, cold and new keys) through
//!    the Stage A lane and measures "parse finished → commit done" per item (which includes
//!    waiting for the one writer); then, one at a time, 10 hot-key files, 10 new-key files and
//!    5 CSVs of 5,000 rows (the commit alone). The phase runs once with 0 decisions, then again after
//!    10,000 `ignore_identifier` decisions (80% on existing keys, 20% orphaned, some in
//!    supersede chains).
//!
//! Pass mark: p95 of the commit latency + the 500 ms debounce ≤ 2 s. Machine-dependent, so
//! `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! cargo test -p mk-core --release -- --ignored s2c --nocapture
//! ```
//!
//! Prints one JSON summary. Needs `testdata/td2` (`cargo run -p corpus-gen --release -- td2`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mk_core::{DEBOUNCE, Project};
use mk_parsers::Format;
use mk_store::NewContent;
use serde_json::{Value, json};

const DOCUMENTS: usize = 10_000;
const MENTIONS: usize = 200_000;
const DECISIONS: usize = 10_000;
const POOL: usize = 20_000;
const ZIPF_S: f64 = 1.1;
const HOT: usize = 3;
const LANE_FILES: usize = 100;
const SERIAL_FILES: usize = 10;
const PASS: Duration = Duration::from_secs(2);

/// SplitMix64: a small fixed-seed generator, so the runs are repeatable without extra crates.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Zipf ranks over `n` keys by inverse CDF.
struct Zipf(Vec<f64>);

impl Zipf {
    fn new(n: usize, s: f64) -> Self {
        let mut acc = 0.0;
        let mut cdf: Vec<f64> = (1..=n)
            .map(|k| {
                acc += 1.0 / (k as f64).powf(s);
                acc
            })
            .collect();
        for c in &mut cdf {
            *c /= acc;
        }
        Zipf(cdf)
    }

    fn sample(&self, rng: &mut Rng) -> usize {
        let u = rng.unit();
        self.0.partition_point(|c| *c < u).min(self.0.len() - 1)
    }
}

fn pool_key(rank: usize) -> String {
    format!("p{rank}@s2c.example")
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e5).round() / 100.0
}

fn stats(mut v: Vec<Duration>) -> Value {
    v.sort();
    json!({
        "n": v.len(),
        "p50_ms": ms(percentile(&v, 0.50)),
        "p95_ms": ms(percentile(&v, 0.95)),
        "max_ms": ms(v.last().copied().unwrap_or_default()),
    })
}

fn count(project: &Project, sql: &'static str) -> i64 {
    project
        .store()
        .read_raw(move |c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .unwrap()
}

/// What makes the measured files: their keys and their size.
struct Mix<'a> {
    rng: Rng,
    zipf: &'a Zipf,
    hot: &'a [String],
    fresh: usize,
}

impl Mix<'_> {
    /// 0–2 emails per line: hot keys, Zipf-spread pool keys (mostly cold tail) and new keys.
    fn line_keys(&mut self) -> Vec<String> {
        let n = self.rng.below(3);
        (0..n)
            .map(|_| match self.rng.below(10) {
                0..=1 => self.hot[self.rng.below(self.hot.len())].clone(),
                2..=5 => pool_key(self.zipf.sample(&mut self.rng)),
                _ => {
                    self.fresh += 1;
                    format!("new{}@s2c.example", self.fresh)
                }
            })
            .collect()
    }

    fn txt(&mut self, lines: usize) -> String {
        let mut s = String::new();
        for i in 0..lines {
            let keys = self.line_keys();
            let _ = writeln!(s, "line {i} note {}", keys.join(" and "));
        }
        s
    }

    fn csv(&mut self, rows: usize) -> String {
        let mut s = String::from("row,name,email,amount\n");
        for i in 0..rows {
            let email = self.line_keys().into_iter().next().unwrap_or_default();
            let _ = writeln!(
                s,
                "{i},Name {i},{email},{}.{:02}",
                self.rng.below(9000),
                i % 100
            );
        }
        s
    }

    /// 100 files: 60 small TXT, 25 large TXT, 15 CSV of 500–5,000 rows.
    fn lane_files(&mut self, dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for i in 0..LANE_FILES {
            let (name, body) = match i % 20 {
                0..=11 => {
                    let n = 5 + self.rng.below(46);
                    (format!("small{i}.txt"), self.txt(n))
                }
                12..=16 => {
                    let n = 200 + self.rng.below(801);
                    (format!("large{i}.txt"), self.txt(n))
                }
                _ => {
                    let n = 500 + self.rng.below(4501);
                    (format!("table{i}.csv"), self.csv(n))
                }
            };
            std::fs::write(dir.join(name), body).unwrap();
        }
    }
}

/// One measured phase: 100 files through the lane, then hot-only and new-only files one at a
/// time (the hot-key worst case against the cheap case).
fn phase(project: &mut Project, mix: &mut Mix<'_>, dir: &Path, cores: usize) -> Value {
    mix.lane_files(&dir.join("lane"));
    project.add_source_root(&dir.join("lane")).unwrap();
    project.set_stage_a_workers(mk_jobs::stage_a_limit(cores));
    project.rescan(false).unwrap();
    let lane = project.run_stage_a().unwrap();
    assert_eq!(lane.done, LANE_FILES);
    let mut lane_latencies = lane.commit_latencies.clone();
    lane_latencies.sort();
    let p95 = percentile(&lane_latencies, 0.95);

    project.set_stage_a_workers(1);
    let serial = |name: &str, body: &dyn Fn(usize) -> String| {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..SERIAL_FILES {
            std::fs::write(d.join(format!("f{i}.txt")), body(i)).unwrap();
        }
        project.add_source_root(&d).unwrap();
        project.rescan(false).unwrap();
        let run = project.run_stage_a().unwrap();
        assert_eq!(run.done, SERIAL_FILES);
        stats(run.commit_latencies)
    };
    // The phase name keeps the files of the two phases apart (equal bytes would be one content).
    let tag = dir.file_name().unwrap().to_string_lossy().to_string();
    let hot = mix.hot.to_vec();
    let hot_only = serial("hot", &|i| {
        hot.iter().map(|k| format!("{tag} {i} hot {k}\n")).collect()
    });
    let new_only = serial("new", &|i| {
        (0..3)
            .map(|j| format!("{i} new {tag}{i}x{j}@s2c.example\n"))
            .collect()
    });
    // The largest single commit without queueing: 5,000-row CSVs one at a time.
    let large = dir.join("large");
    std::fs::create_dir_all(&large).unwrap();
    for i in 0..5 {
        std::fs::write(large.join(format!("t{i}.csv")), mix.csv(5000)).unwrap();
    }
    project.add_source_root(&large).unwrap();
    project.rescan(false).unwrap();
    let large_csv = project.run_stage_a().unwrap();
    assert_eq!(large_csv.done, 5);
    json!({
        "lane": {
            "workers": mk_jobs::stage_a_limit(cores),
            "files": LANE_FILES,
            "latency": stats(lane.commit_latencies),
            "p95_plus_debounce_ms": ms(p95 + DEBOUNCE),
            "pass": p95 + DEBOUNCE <= PASS,
        },
        "hot_key_serial": hot_only,
        "new_key_serial": new_only,
        "large_csv_serial": stats(large_csv.commit_latencies),
    })
}

#[test]
#[ignore = "spike S2c: timing on RM-1 with testdata/td2"]
fn s2c_derived_rebuild() {
    let td2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td2");
    assert!(
        td2.join("td2-manifest.json").exists(),
        "testdata/td2 is missing: cargo run -p corpus-gen --release -- td2"
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(td2.join("td2-manifest.json")).unwrap()).unwrap();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let tmp = tempfile::tempdir().unwrap();
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() =
        mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));

    // Step 1: TD-2's TXT/MD/CSV through the real pipeline.
    let t = Instant::now();
    project.add_source_root(&td2).unwrap();
    let scan = project.rescan(false).unwrap();
    let build = project.run_stage_a().unwrap();
    let step1 = t.elapsed();
    let real_documents = count(&project, "SELECT COUNT(*) FROM document");
    let real_mentions = count(&project, "SELECT COUNT(*) FROM identifier_mention");
    let real_segments = count(&project, "SELECT COUNT(*) FROM segment");

    // Step 2: synthetic top-up to 10,000 documents and ≥ 200k mentions. The hot keys are the
    // most mentioned real key plus two synthetic ones, in every synthetic document.
    let top_real: Option<String> = project
        .store()
        .read_raw(|c| {
            Ok(c.query_row(
                "SELECT key FROM identifier ORDER BY mention_count DESC, key LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok())
        })
        .unwrap();
    let mut hot: Vec<String> = (0..HOT).map(|i| format!("hot{i}@s2c.example")).collect();
    if let Some(k) = top_real {
        hot[0] = k.strip_prefix("email:").unwrap_or(&k).to_string();
    }
    let zipf = Zipf::new(POOL, ZIPF_S);
    let mut rng = Rng(20261027);
    let synthetic = DOCUMENTS.saturating_sub(real_documents as usize);
    let per_doc = MENTIONS
        .saturating_sub(real_mentions as usize)
        .div_ceil(synthetic.max(1))
        .max(HOT + 5);
    let t = Instant::now();
    let contents: Vec<NewContent> = (0..synthetic)
        .map(|i| NewContent {
            hash: mk_kernel::ContentHash::of(format!("s2c synthetic {i}").as_bytes()),
            file_type: if i % 3 == 0 { "csv" } else { "txt" }.into(),
            size: 4096,
        })
        .collect();
    let items = project.store().enqueue_contents(contents).unwrap();
    let mut topup_latencies = Vec::with_capacity(items.len());
    for item in items {
        project.store().start_item(item).unwrap().unwrap();
        let mut text = String::new();
        for k in &hot {
            let _ = writeln!(text, "hot {k}");
        }
        for j in 0..per_doc - HOT {
            let _ = writeln!(text, "{j} {}", pool_key(zipf.sample(&mut rng)));
        }
        let parsed = mk_parsers::parse(Format::Text, text.as_bytes()).unwrap();
        let output = mk_core::stage_a::extract(&parsed);
        let c = Instant::now();
        project.store().commit_stage_a(item, output).unwrap();
        topup_latencies.push(c.elapsed());
    }
    let step2 = t.elapsed();
    let hot_documents: Vec<Value> = hot
        .iter()
        .map(|k| {
            let key = format!("email:{k}");
            let docs: i64 = project
                .store()
                .read_raw(move |c| {
                    Ok(c.query_row(
                        "SELECT COUNT(DISTINCT document_id) FROM identifier_mention
                         WHERE identifier_key = ?1",
                        [key],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            json!({ "key": k, "documents": docs })
        })
        .collect();
    let scale = json!({
        "documents": count(&project, "SELECT COUNT(*) FROM document"),
        "contents": count(&project, "SELECT COUNT(*) FROM content"),
        "mentions": count(&project, "SELECT COUNT(*) FROM identifier_mention"),
        "segments": count(&project, "SELECT COUNT(*) FROM segment"),
        "identifiers": count(&project, "SELECT COUNT(*) FROM identifier"),
        "connections": count(&project, "SELECT COUNT(*) FROM connection"),
        "hot_keys": hot_documents,
    });

    let mut mix = Mix {
        rng: Rng(7),
        zipf: &zipf,
        hot: &hot,
        fresh: 0,
    };
    let zero = phase(&mut project, &mut mix, &tmp.path().join("phase0"), cores);

    // Step 3: 10,000 decisions. 80% on existing keys (10% of those re-decide a key already
    // decided, so supersede chains form, including on a hot key), 20% on keys never seen.
    let existing: Vec<String> = project
        .store()
        .read_raw(|c| {
            let mut s = c.prepare("SELECT key FROM identifier ORDER BY key")?;
            let rows = s.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    let t = Instant::now();
    let mut decided: Vec<String> = Vec::new();
    let mut decision_latencies = Vec::with_capacity(DECISIONS);
    let mut ignored = std::collections::HashMap::<String, bool>::new();
    for i in 0..DECISIONS {
        let key = if i < 6 {
            format!("email:{}", hot[1])
        } else if i % 5 == 0 {
            format!("email:orphan{i}@s2c.example")
        } else if !decided.is_empty() && rng.below(10) == 0 {
            decided[rng.below(decided.len())].clone()
        } else {
            let k = existing[rng.below(existing.len())].clone();
            decided.push(k.clone());
            k
        };
        let now = !ignored.get(&key).copied().unwrap_or(false);
        ignored.insert(key.clone(), now);
        let c = Instant::now();
        project.set_ignored(&key, now).unwrap();
        decision_latencies.push(c.elapsed());
    }
    let step3 = t.elapsed();
    let statuses: Vec<(String, i64)> = project
        .store()
        .read_raw(|c| {
            let mut s =
                c.prepare("SELECT status, COUNT(*) FROM decision_status GROUP BY status")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();

    let ten_k = phase(&mut project, &mut mix, &tmp.path().join("phase10k"), cores);

    // The incremental rows still equal a rebuild from scratch at this scale (ADR-0005).
    let incremental = project.store().derived_state().unwrap();
    let t = Instant::now();
    project.store().rebuild_all().unwrap();
    let rebuild = t.elapsed();
    assert_eq!(incremental, project.store().derived_state().unwrap());

    let pass = zero["lane"]["pass"].as_bool() == Some(true)
        && ten_k["lane"]["pass"].as_bool() == Some(true);
    let db_bytes = std::fs::metadata(project.layout().database()).map_or(0, |m| m.len());
    let summary = json!({
        "spike": "S2c",
        "cores": cores,
        "td2": {
            "copies": manifest["copies"],
            "text_files": scan.walked,
            "done": build.done,
            "failed": build.failed,
            "documents": real_documents,
            "segments": real_segments,
            "mentions": real_mentions,
            "build_s": step1.as_secs_f64(),
            "commit": stats(build.commit_latencies),
        },
        "topup": {
            "documents": synthetic,
            "mentions_per_document": per_doc,
            "build_s": step2.as_secs_f64(),
            "commit": stats(topup_latencies),
        },
        "scale": scale,
        "decisions": {
            "count": DECISIONS,
            "append_s": step3.as_secs_f64(),
            "append": stats(decision_latencies),
            "status": statuses.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
        },
        "phase_0_decisions": zero,
        "phase_10k_decisions": ten_k,
        "rebuild_all_s": rebuild.as_secs_f64(),
        "database_bytes": db_bytes,
        "pass_mark_ms": ms(PASS),
        "pass": pass,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert!(pass, "S2c pass mark missed");
}
