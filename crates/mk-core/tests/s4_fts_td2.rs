//! Spike S4 (M4a, risk R1, NFR-PERF-30): FTS5 keyword search on TD-2 returns the first 50
//! results in ≤ 200 ms p95, unfiltered and with a file-type filter.
//!
//! TD-2 is ingested with the index on (all 10,000 files, PDFs and OCR included), so the run
//! also records what the index costs: Stage A time and commit p95 (NFR-PERF-34), the index
//! size (NFR-CAP-08) and a full refill ('rebuild', what the v4 → v5 migration does).
//!
//! 100 queries are drawn from the index's own terms with a fixed seed: 35 rare words, 30
//! common words, 20 two- or three-word phrases from segment text, 10 prefixes, 5 `a OR b`;
//! then the same 100 with a file-type filter. Each is the whole service call
//! (`Project::search`: parse, MATCH + bm25, highlight, snippet, masking, document join), timed
//! warm (a first pass runs every query once). Machine-dependent, so `#[ignore]`; run on RM-1
//! when it is idle:
//!
//! ```sh
//! cargo test -p mk-core --release --test s4_fts_td2 -- --ignored --nocapture
//! MK_S4_PROJECT=/path/to/dir cargo test ...   # keep the TD-2 project and reuse it next time
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mk_core::{Project, SearchFilter, WorkerConfig};
use serde_json::json;

const PASS: Duration = Duration::from_millis(200);
const SEED: u64 = 0x5eed_0004;

/// File types the filtered pass takes in turn.
const TYPES: &[&str] = &[
    "pdf", "eml", "csv", "txt", "mbox", "docx", "html", "json", "image", "md",
];

/// xorshift64*: a fixed sequence for a fixed seed, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, from: &'a [T]) -> &'a T {
        &from[self.below(from.len())]
    }
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e4).round() / 10.0
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

fn stats(times: &[Duration]) -> serde_json::Value {
    let mut s = times.to_vec();
    s.sort();
    json!({
        "n": s.len(),
        "p50_ms": ms(percentile(&s, 0.5)),
        "p95_ms": ms(percentile(&s, 0.95)),
        "max_ms": ms(s.last().copied().unwrap_or_default()),
    })
}

/// Index terms with their document counts (fts5vocab), on the writer: the vocab table is a
/// temp table.
fn vocabulary(p: &Project) -> Vec<(String, i64)> {
    p.store()
        .write_raw(|c| {
            c.execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS temp.s4_vocab
                 USING fts5vocab(main, segment_fts, row)",
            )?;
            let mut stmt = c.prepare("SELECT term, doc FROM temp.s4_vocab")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let out = rows.collect::<Result<_, _>>()?;
            c.execute_batch("DROP TABLE temp.s4_vocab")?;
            Ok(out)
        })
        .unwrap()
}

/// `n` segment texts picked by id with the seed.
fn sample_texts(p: &Project, rng: &mut Rng, n: usize) -> Vec<String> {
    let max: i64 = p
        .store()
        .read_raw(|c| Ok(c.query_row("SELECT MAX(id) FROM segment", [], |r| r.get(0))?))
        .unwrap();
    let ids: Vec<i64> = (0..n * 4)
        .map(|_| 1 + rng.below(max as usize) as i64)
        .collect();
    p.store()
        .read_raw(move |c| {
            let mut stmt = c.prepare("SELECT text FROM segment WHERE id >= ?1 LIMIT 1")?;
            let mut out = Vec::new();
            for id in ids {
                if let Ok(t) = stmt.query_row([id], |r| r.get::<_, String>(0)) {
                    out.push(t);
                }
            }
            Ok(out)
        })
        .unwrap()
}

fn is_word(w: &str) -> bool {
    w.chars().count() >= 3 && w.chars().all(char::is_alphabetic)
}

/// The 100 queries, by category.
fn queries(p: &Project) -> Vec<(&'static str, String)> {
    let mut rng = Rng(SEED);
    let mut vocab: Vec<(String, i64)> = vocabulary(p)
        .into_iter()
        .filter(|(t, _)| is_word(t))
        .collect();
    vocab.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let common: Vec<&str> = vocab.iter().take(300).map(|(t, _)| t.as_str()).collect();
    // The 2,000 least frequent words. TD-2 is generated: its rarest words are each in ~100
    // segments, so "rare" is relative, not a count.
    let rare: Vec<&str> = vocab
        .iter()
        .rev()
        .take(2_000)
        .map(|(t, _)| t.as_str())
        .collect();
    let mid: Vec<&str> = vocab
        .iter()
        .filter(|(t, d)| (50..=5_000).contains(d) && t.chars().count() >= 5)
        .map(|(t, _)| t.as_str())
        .collect();
    let mut out = Vec::new();
    for _ in 0..35 {
        out.push(("rare", rng.pick(&rare).to_string()));
    }
    for _ in 0..30 {
        out.push(("common", rng.pick(&common).to_string()));
    }
    // Phrases: words next to each other in a segment, all different, each phrase once (TD-2's
    // filler repeats "Generated" in most segments).
    let texts = sample_texts(p, &mut rng, 100);
    let mut phrases: Vec<String> = Vec::new();
    for text in &texts {
        if phrases.len() == 20 {
            break;
        }
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let len = 2 + rng.below(2);
        if tokens.len() < len {
            continue;
        }
        let start = rng.below(tokens.len() + 1 - len);
        let found = (start..=tokens.len() - len)
            .chain(0..start)
            .map(|at| &tokens[at..at + len])
            .find(|w| {
                w.iter().all(|t| is_word(t))
                    && w.iter()
                        .enumerate()
                        .all(|(i, t)| w[..i].iter().all(|u| !u.eq_ignore_ascii_case(t)))
            });
        if let Some(w) = found {
            let phrase = format!("\"{}\"", w.join(" "));
            if !phrases.contains(&phrase) {
                phrases.push(phrase);
            }
        }
    }
    out.extend(phrases.into_iter().map(|q| ("phrase", q)));
    for _ in 0..10 {
        let t = rng.pick(&mid);
        let keep = 3 + rng.below(2);
        out.push((
            "prefix",
            format!("{}*", t.chars().take(keep).collect::<String>()),
        ));
    }
    for _ in 0..5 {
        out.push(("or", format!("{} OR {}", rng.pick(&mid), rng.pick(&rare))));
    }
    out
}

fn index_bytes(p: &Project) -> i64 {
    p.store()
        .read_raw(|c| {
            Ok(c.query_row(
                "SELECT (SELECT COALESCE(SUM(length(block)), 0) FROM segment_fts_data)
                      + (SELECT COALESCE(SUM(length(sz)), 0) FROM segment_fts_docsize)
                      + (SELECT COUNT(*) * 16 FROM segment_fts_idx)",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap()
}

#[test]
#[ignore = "S4 / NFR-PERF-30: timing on RM-1 with testdata/td2"]
fn s4_fts_td2() {
    let td2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td2");
    assert!(
        td2.join("td2-manifest.json").exists(),
        "testdata/td2 is missing: cargo run -p corpus-gen --release -- td2"
    );
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::env::var_os("MK_S4_PROJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| tmp.path().join("project"));
    let (mut project, _) = Project::open(&dir).unwrap();
    let program = std::env::var_os("MK_WORKER_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_mk-test-worker").into());
    *project.worker_config_mut() = WorkerConfig::with_program(program);
    project.set_tessdata_dir(Some(tmp.path().join("tessdata")));

    let mut ingest = json!(null);
    if project.store().source_roots().unwrap().is_empty() {
        project.add_source_root(&td2).unwrap();
        let t = Instant::now();
        let s = project.ingest().unwrap();
        let took = t.elapsed();
        let mut commits = s.stage_a.commit_latencies.clone();
        commits.sort();
        ingest = json!({
            "files_done": s.stage_a.done,
            "files_failed": s.stage_a.failed,
            "ingest_s": (took.as_secs_f64() * 10.0).round() / 10.0,
            "commit_p95_ms": ms(percentile(&commits, 0.95)),
            "commit_max_ms": ms(commits.last().copied().unwrap_or_default()),
        });
    }
    let segments: i64 = project
        .store()
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))?))
        .unwrap();
    let db_bytes = std::fs::metadata(dir.join("project.db"))
        .map(|m| m.len())
        .unwrap_or(0);
    let fts_bytes = index_bytes(&project);
    let t = Instant::now();
    project.store().rebuild_search_index().unwrap();
    let refill = t.elapsed();

    let qs = queries(&project);
    let by_type = |i: usize| SearchFilter {
        file_types: vec![TYPES[i % TYPES.len()].to_string()],
        document: None,
    };
    let run = |q: &str, f: &SearchFilter| {
        let t = Instant::now();
        let page = project.search(q, f, None, mk_core::SEARCH_PAGE).unwrap();
        (t.elapsed(), page.hits.len())
    };
    // Warm-up pass.
    for (i, (_, q)) in qs.iter().enumerate() {
        run(q, &SearchFilter::default());
        run(q, &by_type(i));
    }
    let mut plain: BTreeMap<&str, Vec<Duration>> = BTreeMap::new();
    let mut filtered: BTreeMap<&str, Vec<Duration>> = BTreeMap::new();
    let mut all_plain = Vec::new();
    let mut all_filtered = Vec::new();
    let mut empty = 0;
    for (i, (kind, q)) in qs.iter().enumerate() {
        let (d, n) = run(q, &SearchFilter::default());
        empty += usize::from(n == 0);
        plain.entry(kind).or_default().push(d);
        all_plain.push(d);
        let (d, _) = run(q, &by_type(i));
        filtered.entry(kind).or_default().push(d);
        all_filtered.push(d);
    }
    let p95 = |v: &[Duration]| {
        let mut s = v.to_vec();
        s.sort();
        percentile(&s, 0.95)
    };
    let pass = p95(&all_plain) <= PASS && p95(&all_filtered) <= PASS;
    let slowest: Vec<_> = {
        let mut v: Vec<(Duration, &str)> = qs
            .iter()
            .map(|(_, q)| q.as_str())
            .zip(all_plain.iter().copied())
            .map(|(q, d)| (d, q))
            .collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.0));
        v.into_iter()
            .take(5)
            .map(|(d, q)| json!({"query": q, "ms": ms(d)}))
            .collect()
    };
    // Worst cases, outside the 100: the most common word (in most segments), very short
    // prefixes, a phrase of common words. Each timed warm, best of 3 and worst of 3.
    let top = vocabulary(&project)
        .into_iter()
        .filter(|(t, _)| is_word(t))
        .max_by_key(|(_, d)| *d)
        .map(|(t, _)| t)
        .unwrap_or_default();
    let worst: Vec<_> = [
        top.clone(),
        "co*".into(),
        "c*".into(),
        format!("\"{top} example\""),
    ]
    .into_iter()
    .map(|q| {
        run(&q, &SearchFilter::default());
        let times: Vec<Duration> = (0..3)
            .map(|_| run(&q, &SearchFilter::default()).0)
            .collect();
        json!({
            "query": q,
            "min_ms": ms(*times.iter().min().unwrap()),
            "max_ms": ms(*times.iter().max().unwrap()),
        })
    })
    .collect();
    let summary = json!({
        "check": "s4_fts_td2",
        "segments": segments,
        "ingest": ingest,
        "db_bytes": db_bytes,
        "fts_index_bytes": fts_bytes,
        "fts_refill_s": (refill.as_secs_f64() * 100.0).round() / 100.0,
        "queries": qs.len(),
        "queries_without_results": empty,
        "unfiltered": stats(&all_plain),
        "unfiltered_by_kind": plain.iter().map(|(k, v)| (k.to_string(), stats(v))).collect::<BTreeMap<_, _>>(),
        "filtered": stats(&all_filtered),
        "filtered_by_kind": filtered.iter().map(|(k, v)| (k.to_string(), stats(v))).collect::<BTreeMap<_, _>>(),
        "slowest_unfiltered": slowest,
        "worst_cases": worst,
        "pass_mark_ms": PASS.as_millis(),
        "pass": pass,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert!(pass, "NFR-PERF-30 pass mark missed");
}
