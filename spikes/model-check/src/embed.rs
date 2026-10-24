//! Embedding check (spike S1-E, ADR-0010, risk R2): recall@20 of a binary (sign) index with
//! float rescoring against exact float search, on TD-1 text windows.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use half::f16;
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::extract::{self, RunMeta};
use crate::segments::{Rng, Segments, Source};

pub const WINDOW_CHARS: usize = 500;
pub const TOP: usize = 20;
pub const POOLS: [usize; 3] = [40, 100, 200];
pub const SEEDED_QUERIES: usize = 50;
pub const BATCH: usize = 32;

/// Query and document prefixes per model family (model cards). They become catalog fields.
pub fn prefixes(model_file: &str) -> (&'static str, &'static str) {
    let f = model_file.to_lowercase();
    if f.contains("qwen3-embedding") {
        (
            "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:",
            "",
        )
    } else if f.contains("nomic-embed-text") {
        ("search_query: ", "search_document: ")
    } else {
        // bge-m3 dense retrieval uses no prefix.
        ("", "")
    }
}

/// Splits text into windows of about `max` chars at whitespace (no overlap).
pub fn windows(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > max {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Window {
    pub source: String,
    pub text: String,
}

#[derive(Deserialize)]
struct PublicPage {
    file: String,
    page: u32,
    text: String,
}

/// TD-1 synthetic text (every page, notes, email) and the public slice pages
/// (`pdf-bakeoff dump-text`), in ~500-char windows. Short windows (< 40 chars) are dropped.
pub fn corpus(root: &Path, public_text: &Path) -> Result<Vec<Window>, String> {
    let gt = mk_bench::load(&root.join("ground-truth.json")).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut push = |source: String, text: &str| {
        for w in windows(text, WINDOW_CHARS) {
            if w.chars().count() >= 40 {
                out.push(Window {
                    source: source.clone(),
                    text: w,
                });
            }
        }
    };
    for p in &gt.page_text {
        push(format!("{}#p{}", p.document, p.page), &p.text);
    }
    let segs = Segments::build(root, &gt)?;
    for s in segs
        .list
        .iter()
        .filter(|s| matches!(s.source, Source::Notes | Source::Email))
    {
        push(s.id.clone(), &s.text);
    }
    let public = std::fs::read_to_string(public_text).map_err(|e| {
        format!(
            "{}: {e}; run `pdf-bakeoff dump-text --engine hayro --dir testdata/td1/public --out {}` first",
            public_text.display(),
            public_text.display()
        )
    })?;
    for line in public.lines().filter(|l| !l.trim().is_empty()) {
        let p: PublicPage = serde_json::from_str(line).map_err(|e| e.to_string())?;
        push(format!("public/{}#p{}", p.file, p.page), &p.text);
    }
    Ok(out)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Query {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub topic: String,
}

/// Seeded known-item queries: the longest sentence (≥ 40 chars, cut at 200) of a random window.
pub fn seeded_queries(corpus: &[Window], n: usize, seed: u64) -> Vec<Query> {
    let idx: Vec<usize> = (0..corpus.len()).collect();
    let mut out = Vec::new();
    for i in Rng::new(seed).sample(&idx, corpus.len()) {
        if out.len() == n {
            break;
        }
        let best = corpus[i]
            .text
            .split_inclusive(['.', '?', '!'])
            .map(str::trim)
            .max_by_key(|s| s.chars().count())
            .unwrap_or("");
        if best.chars().count() < 40 {
            continue;
        }
        out.push(Query {
            id: format!("s{:02}", out.len() + 1),
            text: best.chars().take(200).collect(),
            topic: "seeded".into(),
        });
    }
    out
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Sign bits, 1 for > 0, packed into u64 words.
pub fn binarize(v: &[f32]) -> Vec<u64> {
    let mut out = vec![0u64; v.len().div_ceil(64)];
    for (i, x) in v.iter().enumerate() {
        if *x > 0.0 {
            out[i / 64] |= 1 << (i % 64);
        }
    }
    out
}

pub fn hamming(a: &[u64], b: &[u64]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// Indices of the `k` best by score (higher first), ties by index.
pub fn top_by_score(scores: &[(usize, f32)], k: usize) -> Vec<usize> {
    let mut v = scores.to_vec();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().take(k).map(|x| x.0).collect()
}

pub struct Index {
    pub float: Vec<Vec<f32>>,
    pub half: Vec<Vec<f16>>,
    pub bits: Vec<Vec<u64>>,
}

impl Index {
    pub fn new(vectors: Vec<Vec<f32>>) -> Self {
        let half = vectors
            .iter()
            .map(|v| v.iter().map(|x| f16::from_f32(*x)).collect())
            .collect();
        let bits = vectors.iter().map(|v| binarize(v)).collect();
        Index {
            float: vectors,
            half,
            bits,
        }
    }

    pub fn exact(&self, q: &[f32], k: usize) -> Vec<usize> {
        let s: Vec<(usize, f32)> = self
            .float
            .iter()
            .enumerate()
            .map(|(i, d)| (i, dot(q, d)))
            .collect();
        top_by_score(&s, k)
    }

    /// Hamming top `pool` (ties by index).
    pub fn binary(&self, q: &[f32], pool: usize) -> Vec<usize> {
        let qb = binarize(q);
        let mut v: Vec<(u32, usize)> = self
            .bits
            .iter()
            .enumerate()
            .map(|(i, b)| (hamming(&qb, b), i))
            .collect();
        v.sort();
        v.into_iter().take(pool).map(|x| x.1).collect()
    }

    /// Binary top `pool` → rescore with f32 (or f16 storage) → top `k`.
    pub fn rescored(&self, q: &[f32], pool: usize, k: usize, use_f16: bool) -> Vec<usize> {
        let s: Vec<(usize, f32)> = self
            .binary(q, pool)
            .into_iter()
            .map(|i| {
                let score = if use_f16 {
                    self.half[i]
                        .iter()
                        .zip(q)
                        .map(|(x, y)| x.to_f32() * y)
                        .sum()
                } else {
                    dot(q, &self.float[i])
                };
                (i, score)
            })
            .collect();
        top_by_score(&s, k)
    }
}

pub fn recall(found: &[usize], exact: &[usize]) -> f64 {
    let e: BTreeSet<_> = exact.iter().collect();
    found.iter().filter(|i| e.contains(i)).count() as f64 / exact.len().max(1) as f64
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Recall {
    pub pool: usize,
    pub rescore_f32: f64,
    pub rescore_f16: f64,
    /// Lowest single-query recall with f32 rescoring.
    pub rescore_f32_min: f64,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct EmbedResult {
    pub label: String,
    pub model_file: String,
    pub dims: usize,
    pub windows: usize,
    pub queries_hand: usize,
    pub queries_seeded: usize,
    pub query_prefix: String,
    pub document_prefix: String,
    pub windows_per_s: f64,
    pub binary_only: f64,
    pub recall: Vec<Recall>,
    /// Pool 200, split by query set.
    pub hand_rescore_f32: f64,
    pub seeded_rescore_f32: f64,
    /// Seeded queries whose own window is in the exact top 20 (a sanity check of the model).
    pub seeded_self_hit: f64,
    pub index_bytes_binary: usize,
    pub index_bytes_f32: usize,
}

pub fn evaluate(
    label: &str,
    model_file: &str,
    docs: Vec<Vec<f32>>,
    queries: &[Vec<f32>],
    hand: usize,
    seeded_src: &[usize],
    windows_per_s: f64,
) -> EmbedResult {
    let dims = docs.first().map_or(0, Vec::len);
    let n = docs.len();
    let index = Index::new(docs);
    let exact: Vec<Vec<usize>> = queries.iter().map(|q| index.exact(q, TOP)).collect();
    let mean = |f: &dyn Fn(usize) -> f64, range: std::ops::Range<usize>| {
        let len = range.len().max(1) as f64;
        range.map(f).sum::<f64>() / len
    };
    let all = 0..queries.len();
    let recall_rows = POOLS
        .iter()
        .map(|&pool| {
            let f32s: Vec<f64> = (0..queries.len())
                .map(|i| recall(&index.rescored(&queries[i], pool, TOP, false), &exact[i]))
                .collect();
            Recall {
                pool,
                rescore_f32: f32s.iter().sum::<f64>() / f32s.len().max(1) as f64,
                rescore_f16: mean(
                    &|i| recall(&index.rescored(&queries[i], pool, TOP, true), &exact[i]),
                    all.clone(),
                ),
                rescore_f32_min: f32s.iter().cloned().fold(1.0, f64::min),
            }
        })
        .collect();
    let (q_prefix, d_prefix) = prefixes(model_file);
    EmbedResult {
        label: label.into(),
        model_file: model_file.into(),
        dims,
        windows: n,
        queries_hand: hand,
        queries_seeded: queries.len() - hand,
        query_prefix: q_prefix.into(),
        document_prefix: d_prefix.into(),
        windows_per_s,
        binary_only: mean(
            &|i| recall(&index.binary(&queries[i], TOP), &exact[i]),
            all.clone(),
        ),
        recall: recall_rows,
        hand_rescore_f32: mean(
            &|i| recall(&index.rescored(&queries[i], 200, TOP, false), &exact[i]),
            0..hand,
        ),
        seeded_rescore_f32: mean(
            &|i| recall(&index.rescored(&queries[i], 200, TOP, false), &exact[i]),
            hand..queries.len(),
        ),
        seeded_self_hit: mean(
            &|i| exact[i].contains(&seeded_src[i - hand]) as u8 as f64,
            hand..queries.len(),
        ),
        index_bytes_binary: n * dims.div_ceil(8),
        index_bytes_f32: n * dims * 4,
    }
}

fn write_vectors(path: &Path, rows: &[Vec<f32>]) -> Result<(), String> {
    let dims = rows.first().map_or(0, Vec::len) as u32;
    let mut b = Vec::with_capacity(8 + rows.len() * dims as usize * 4);
    b.extend((rows.len() as u32).to_le_bytes());
    b.extend(dims.to_le_bytes());
    for r in rows {
        for x in r {
            b.extend(x.to_le_bytes());
        }
    }
    std::fs::write(path, b).map_err(|e| e.to_string())
}

fn read_vectors(path: &Path) -> Result<Vec<Vec<f32>>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let n = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
    let d = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
    Ok((0..n)
        .map(|i| {
            (0..d)
                .map(|j| {
                    let o = 8 + (i * d + j) * 4;
                    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
                })
                .collect()
        })
        .collect())
}

pub struct Inputs {
    pub corpus: Vec<Window>,
    pub queries: Vec<Query>,
    pub hand: usize,
    /// Window index each seeded query came from.
    pub seeded_src: Vec<usize>,
}

pub fn inputs(root: &Path, public_text: &Path, queries_file: &Path) -> Result<Inputs, String> {
    let corpus = corpus(root, public_text)?;
    let mut queries: Vec<Query> = serde_json::from_str(
        &std::fs::read_to_string(queries_file)
            .map_err(|e| format!("{}: {e}", queries_file.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let hand = queries.len();
    let seeded = seeded_queries(&corpus, SEEDED_QUERIES, 2);
    let seeded_src = seeded
        .iter()
        .map(|q| {
            corpus
                .iter()
                .position(|w| w.text.contains(&q.text))
                .unwrap_or(usize::MAX)
        })
        .collect();
    queries.extend(seeded);
    Ok(Inputs {
        corpus,
        queries,
        hand,
        seeded_src,
    })
}

/// `model-check embed`: start the server with `--embedding`, embed windows and queries, save the
/// vectors, evaluate. With `reuse`, evaluate saved vectors without the model.
pub fn run(job: &extract::Job, inp: &Inputs, reuse: bool) -> Result<EmbedResult, String> {
    let model_file = job
        .model
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Labels hold dots (`v1.5`), so no `with_extension`.
    let path = |ext: &str| -> PathBuf { job.out.join(format!("embed-{}.{ext}", job.label)) };
    let (docs_path, q_path) = (path("docs.f32"), path("queries.f32"));
    let (mut docs, mut queries, windows_per_s);
    if reuse {
        docs = read_vectors(&docs_path)?;
        queries = read_vectors(&q_path)?;
        let meta: RunMeta = serde_json::from_str(
            &std::fs::read_to_string(path("json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        windows_per_s = meta.items as f64 / meta.wall_s;
    } else {
        let (qp, dp) = prefixes(&model_file);
        let ctx = 2048 * job.np;
        let mut args: Vec<String> = [
            "--embedding",
            "-c",
            &ctx.to_string(),
            "-np",
            &job.np.to_string(),
            "-b",
            "2048",
            "-ub",
            "2048",
            "--cache-ram",
            "0",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.extend(job.extra_args.iter().cloned());
        let log = job.out.join(format!("embed-{}.log", job.label));
        let started_utc = extract::utc_now();
        let extract::Launch {
            mut server,
            build,
            args,
            sampler,
            load_s,
        } = extract::launch(job.settings.as_deref(), &job.model, args, &log)?;
        let client = Client::new(server.base(), server.key.clone());
        client.embed(&["warm-up".to_string()])?;
        let t = Instant::now();
        docs = Vec::with_capacity(inp.corpus.len());
        for (i, chunk) in inp.corpus.chunks(BATCH).enumerate() {
            let texts: Vec<String> = chunk.iter().map(|w| format!("{dp}{}", w.text)).collect();
            docs.extend(client.embed(&texts)?);
            if i % 20 == 0 {
                eprintln!(
                    "  {}/{} windows in {:.0} s",
                    docs.len(),
                    inp.corpus.len(),
                    t.elapsed().as_secs_f64()
                );
            }
        }
        let wall_s = t.elapsed().as_secs_f64();
        windows_per_s = docs.len() as f64 / wall_s;
        queries = Vec::new();
        for chunk in inp.queries.chunks(BATCH) {
            let texts: Vec<String> = chunk.iter().map(|q| format!("{qp}{}", q.text)).collect();
            queries.extend(client.embed(&texts)?);
        }
        let peaks = sampler.stop();
        server.stop();
        write_vectors(&docs_path, &docs)?;
        write_vectors(&q_path, &queries)?;
        let meta = RunMeta {
            label: job.label.clone(),
            task: "embed".into(),
            model_file: model_file.clone(),
            model_bytes: extract::model_bytes(&job.model),
            build,
            server_args: args,
            np: job.np,
            prompt_version: 0,
            items: docs.len(),
            load_s,
            wall_s,
            peaks,
            started_utc,
            host: extract::host(),
        };
        std::fs::write(
            path("json"),
            serde_json::to_string_pretty(&meta).expect("serializes") + "\n",
        )
        .map_err(|e| e.to_string())?;
    }
    docs.iter_mut().for_each(|v| normalize(v));
    queries.iter_mut().for_each(|v| normalize(v));
    let r = evaluate(
        &job.label,
        &model_file,
        docs,
        &queries,
        inp.hand,
        &inp.seeded_src,
        windows_per_s,
    );
    std::fs::write(
        job.out.join(format!("embed-score-{}.json", job.label)),
        serde_json::to_string_pretty(&r).expect("serializes") + "\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(r)
}

pub fn print(r: &EmbedResult) {
    println!(
        "== {} ({} dims, {} windows, {} + {} queries, {:.1} windows/s)",
        r.label, r.dims, r.windows, r.queries_hand, r.queries_seeded, r.windows_per_s
    );
    println!("binary only recall@20 {:.3}", r.binary_only);
    for x in &r.recall {
        println!(
            "pool {:>3}: f32 rescore {:.3} (min {:.2}), f16 rescore {:.3}",
            x.pool, x.rescore_f32, x.rescore_f32_min, x.rescore_f16
        );
    }
    println!(
        "pool 200 by set: hand {:.3}, seeded {:.3}; seeded self-hit {:.2}",
        r.hand_rescore_f32, r.seeded_rescore_f32, r.seeded_self_hit
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_vectors(n: usize, d: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| {
                let mut v: Vec<f32> = (0..d)
                    .map(|_| (rng.next() % 20001) as f32 / 10000.0 - 1.0)
                    .collect();
                normalize(&mut v);
                v
            })
            .collect()
    }

    #[test]
    fn adr_0010_binarize_and_hamming() {
        let a = binarize(&[0.5, -0.1, 0.0, 2.0]);
        assert_eq!(a, vec![0b1001]);
        let v: Vec<f32> = (0..130)
            .map(|i| if i % 3 == 0 { 1.0 } else { -1.0 })
            .collect();
        let b = binarize(&v);
        assert_eq!(b.len(), 3);
        assert_eq!(hamming(&b, &b), 0);
        let neg: Vec<f32> = v.iter().map(|x| -x).collect();
        assert_eq!(hamming(&b, &binarize(&neg)), 130);
    }

    #[test]
    fn adr_0010_rescore_full_pool_is_exact() {
        let docs = random_vectors(500, 64, 7);
        let qs = random_vectors(10, 64, 8);
        let index = Index::new(docs);
        for q in &qs {
            let exact = index.exact(q, TOP);
            // A pool of the whole corpus makes rescoring exact.
            assert_eq!(index.rescored(q, 500, TOP, false), exact);
            assert!(recall(&index.rescored(q, 500, TOP, true), &exact) >= 0.95);
            // Pools only add candidates, so recall cannot drop as the pool grows.
            let small = recall(&index.rescored(q, 40, TOP, false), &exact);
            let large = recall(&index.rescored(q, 200, TOP, false), &exact);
            assert!(large >= small);
        }
    }

    #[test]
    fn adr_0010_recall_on_near_duplicates() {
        // Queries that are small perturbations of corpus vectors find them through the binary scan.
        let docs = random_vectors(1000, 256, 3);
        let index = Index::new(docs.clone());
        let mut rng = Rng::new(4);
        for i in (0..1000).step_by(97) {
            let mut q: Vec<f32> = docs[i]
                .iter()
                .map(|x| x + ((rng.next() % 1001) as f32 / 1000.0 - 0.5) * 0.02)
                .collect();
            normalize(&mut q);
            assert_eq!(index.rescored(&q, 40, 1, false), vec![i]);
        }
    }

    #[test]
    fn windows_split_at_whitespace() {
        let w = windows("aaa bbb ccc ddd", 7);
        assert_eq!(w, vec!["aaa bbb", "ccc ddd"]);
        assert_eq!(windows("  \n ", 10), Vec::<String>::new());
        assert_eq!(windows("one\ntwo", 100), vec!["one two"]);
    }
}
