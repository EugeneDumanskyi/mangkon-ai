//! Spike S6 (M6b, risk R2, NFR-PERF-31, ADR-0010): on TD-2, a meaning query (query embedding +
//! binary scan + float rescore + rows) takes ≤ 1 s p95 for the top 20, recall@20 against exact
//! float search is ≥ 0.9, and memory goes back after the idle release.
//!
//! TD-2 is ingested, every window embedded through `Workspace::ai_client` with the real model and
//! committed through the store, and the index built. 100 queries with a fixed seed (M1's 50
//! hand-written + 50 seeded from TD-2's own text) are each timed as the whole service call
//! (`Workspace::meaning_search`), warm. Needs `llama-server` and the model file, so `#[ignore]`.
//! Query timing needs an idle RM-1; embedding does not.
//!
//! ```sh
//! MK_S6_PROJECT=/path/to/dir MK_S6_LLAMA=/path/to/llama-server \
//!   cargo test -p mk-core --release --test s6_meaning_td2 -- --ignored --nocapture
//! ```
//!
//! `MK_S6_PROJECT` keeps the scratch project and picks up where it stopped. `MK_S6_MODEL_ID` and
//! `MK_S6_MODELS` choose the model (default `bge-m3-q8_0` in `~/AI/models`). `MK_S6_EMBED_ONLY=1`
//! stops after the index is built.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mk_core::{
    AiChoice, DestinationClass, MeaningOutcome, MeaningTimings, ModelRole, Project, WorkerConfig,
    Workspace,
};
use mk_jobs::Trigger;
use mk_search::meaning::{
    ITEM_WINDOWS, RESCORE_POOL, TOP_K, decode_f16, encode_f16, normalize, rescore_top, scan_top_k,
    sign_bits,
};
use mk_store::{AppStore, EmbeddingRow, RunBinding, UnitScope};
use serde_json::{Value, json};

const PASS: Duration = Duration::from_secs(1);
const SEED: u64 = 0x5eed_0006;
const POOLS: [usize; 4] = [100, 200, 500, 1000];
/// Near-ties: a hit within this of the exact 20th score counts for the diagnostic.
const TIE: f32 = 1e-3;

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

fn stats(times: &[Duration]) -> Value {
    let mut s = times.to_vec();
    s.sort();
    json!({
        "n": s.len(),
        "p50_ms": ms(percentile(&s, 0.5)),
        "p95_ms": ms(percentile(&s, 0.95)),
        "max_ms": ms(s.last().copied().unwrap_or_default()),
    })
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))?
                .split_whitespace()
                .nth(1)?
                .parse::<f64>()
                .ok()
        })
        .map_or(0.0, |kb| (kb / 102.4).round() / 10.0)
}

fn install(app_dir: &Path, models: &Path, id: &str) {
    let catalog = mk_models::Catalog::shipped();
    let model = catalog.model(id).expect("model in the catalog");
    let dir = app_dir.join("models").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut total = 0;
    for f in &model.files {
        total += f.bytes;
        let link = dir.join(&f.name);
        if !link.exists() {
            std::os::unix::fs::symlink(models.join(&f.name), &link).unwrap();
        }
    }
    let app = AppStore::open(app_dir).unwrap();
    app.set_model_download(id, "installed", total, total, None)
        .unwrap();
}

/// Embeds every window not covered yet, in the lane's shape: one request at a time, up to 32
/// windows, one commit per item.
fn embed_all(ws: &Workspace, project: &Project, id: &str) -> Value {
    let store = project.store();
    let t = Instant::now();
    let schedule = store
        .schedule_embeddings(UnitScope::Auto, Trigger::Add)
        .unwrap();
    let schedule_s = t.elapsed().as_secs_f64();
    store
        .bind_embedding_run(RunBinding {
            identity: mk_ai::RunIdentity::local(id),
            model_name: id.into(),
            destination_host: "127.0.0.1".into(),
            destination_class: DestinationClass::ThisMachine.as_str().into(),
            prompt_version: 0,
        })
        .unwrap();
    let doc_prefix = mk_models::Catalog::shipped()
        .model(id)
        .and_then(|m| m.embedding.as_ref())
        .map(|e| e.document_prefix.clone())
        .unwrap_or_default();
    // Started only when there is work, so a finished project's first query really is cold.
    let mut client = None;
    let t = Instant::now();
    let (mut windows, mut items, mut chars) = (0u64, 0u64, 0u64);
    let mut last = Instant::now();
    let mut peak_item = 0usize;
    while let Some(item) = store.start_next_embedding_item().unwrap() {
        let client =
            client.get_or_insert_with(|| ws.ai_client(project, ModelRole::Embeddings).unwrap());
        let mut rows = Vec::with_capacity(item.windows.len());
        peak_item = peak_item.max(item.windows.len());
        for part in item.windows.chunks(ITEM_WINDOWS) {
            let inputs: Vec<String> = part
                .iter()
                .map(|w| format!("{doc_prefix}{}", w.text))
                .collect();
            chars += inputs.iter().map(|i| i.chars().count() as u64).sum::<u64>();
            let reply = client.client.embeddings(&inputs).unwrap();
            for (w, mut v) in part.iter().zip(reply.vectors) {
                normalize(&mut v);
                rows.push(EmbeddingRow {
                    segment: w.segment,
                    char_start: w.char_start,
                    char_end: w.char_end,
                    vector: encode_f16(&v),
                });
            }
        }
        windows += rows.len() as u64;
        items += 1;
        assert!(
            store
                .commit_embedding_item(item.item, item.attempt, rows)
                .unwrap()
        );
        if last.elapsed() > Duration::from_secs(60) {
            last = Instant::now();
            let p = store.embedding_progress().unwrap();
            eprintln!(
                "embed: {windows} windows, {items} items, {:.1} windows/s, {} items left, rss {} MB",
                windows as f64 / t.elapsed().as_secs_f64(),
                p.open,
                rss_mb()
            );
        }
    }
    if let Some(client) = client {
        client.succeeded();
    }
    json!({
        "items_scheduled_now": schedule.units,
        "schedule_s": schedule_s,
        "windows_now": windows,
        "items_now": items,
        "largest_item_windows": peak_item,
        "mean_window_chars": chars as f64 / windows.max(1) as f64,
        "embed_s": t.elapsed().as_secs_f64(),
        "windows_per_s": windows as f64 / t.elapsed().as_secs_f64().max(1e-9),
    })
}

/// M1's seeded known-item queries: the longest sentence (≥ 40 chars, cut at 200) of a random
/// stored window, here over TD-2.
fn seeded(project: &Project, n: usize) -> Vec<String> {
    let mut rng = Rng(SEED);
    let max: i64 = project
        .store()
        .read_raw(|c| Ok(c.query_row("SELECT MAX(id) FROM segment_embedding", [], |r| r.get(0))?))
        .unwrap();
    let mut out: Vec<String> = Vec::new();
    while out.len() < n {
        let id = 1 + rng.below(max as usize) as i64;
        let text: Option<String> = project
            .store()
            .read_raw(|c| {
                Ok(c.query_row(
                    "SELECT substr(s.text, e.char_start + 1, e.char_end - e.char_start)
                     FROM segment_embedding e JOIN segment s ON s.id = e.segment_id
                     WHERE e.id >= ?1 LIMIT 1",
                    [id],
                    |r| r.get(0),
                )
                .ok())
            })
            .unwrap();
        let Some(text) = text else { continue };
        let Some(best) = text
            .split(['.', '!', '?', '\n'])
            .map(str::trim)
            .filter(|s| s.chars().count() >= 40)
            .max_by_key(|s| s.chars().count())
        else {
            continue;
        };
        let cut: String = best.chars().take(200).collect();
        if !out.contains(&cut) {
            out.push(cut);
        }
    }
    out
}

fn timed_query(ws: &Workspace, project: &Project, q: &str) -> (Duration, usize, MeaningTimings) {
    let t = Instant::now();
    match ws.meaning_search(project, q).unwrap() {
        MeaningOutcome::Results { hits, timings } => (t.elapsed(), hits.len(), timings),
        other => panic!("not ready: {other:?}"),
    }
}

/// Every vector of the run as floats, flat.
fn load_all(project: &Project, run: i64, dims: usize) -> (Vec<i64>, Vec<f32>) {
    let (mut ids, mut all) = (Vec::new(), Vec::new());
    let mut after = 0;
    loop {
        let page = project
            .store()
            .embedding_rows_page(run, after, 8192)
            .unwrap();
        let Some((last, _)) = page.last() else { break };
        after = *last;
        for (id, bytes) in &page {
            ids.push(*id);
            all.extend(decode_f16(bytes, dims).unwrap());
        }
    }
    (ids, all)
}

/// The exact top 20 by float dot product, on all cores: rows and scores.
fn exact_top(all: &[f32], dims: usize, q: &[f32]) -> Vec<(u32, f32)> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rows = all.len() / dims;
    let per = rows.div_ceil(threads).max(1);
    let found: Vec<Vec<(u32, f32)>> = std::thread::scope(|s| {
        let handles: Vec<_> = all
            .chunks(per * dims)
            .enumerate()
            .map(|(chunk, part)| {
                s.spawn(move || {
                    let mut best: Vec<(u32, f32)> = Vec::with_capacity(TOP_K * 2);
                    let mut limit = f32::NEG_INFINITY;
                    for (i, v) in part.chunks_exact(dims).enumerate() {
                        let d: f32 = v.iter().zip(q).map(|(a, b)| a * b).sum();
                        if d > limit {
                            best.push(((chunk * per + i) as u32, d));
                            if best.len() == TOP_K * 2 {
                                best.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                                best.truncate(TOP_K);
                                limit = best[TOP_K - 1].1;
                            }
                        }
                    }
                    best
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut best: Vec<(u32, f32)> = found.into_iter().flatten().collect();
    best.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    best.truncate(TOP_K);
    best
}

#[test]
#[ignore = "S6 / NFR-PERF-31: real model, TD-2 and an idle RM-1 for the timing"]
fn s6_meaning_td2() {
    let td2 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td2");
    assert!(
        td2.join("td2-manifest.json").exists(),
        "testdata/td2 is missing: cargo run -p corpus-gen --release -- td2"
    );
    let tmp = tempfile::tempdir().unwrap();
    let base = std::env::var_os("MK_S6_PROJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| tmp.path().to_path_buf());
    let id = std::env::var("MK_S6_MODEL_ID").unwrap_or_else(|_| "bge-m3-q8_0".into());
    let models = std::env::var_os("MK_S6_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join("AI/models"));
    let app_dir = base.join("app");
    std::fs::create_dir_all(&app_dir).unwrap();
    install(&app_dir, &models, &id);
    let ws = Workspace::open(&app_dir).unwrap();
    if let Some(llama) = std::env::var_os("MK_S6_LLAMA") {
        ws.set_llama_server_path(Some(Path::new(&llama))).unwrap();
    }
    ws.set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Local(id.clone())))
        .unwrap();

    let dir = base.join("project");
    let (mut project, _) = Project::open(&dir).unwrap();
    let program = std::env::var_os("MK_WORKER_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_mk-test-worker").into());
    *project.worker_config_mut() = WorkerConfig::with_program(program);
    project.set_tessdata_dir(Some(tmp.path().join("tessdata")));
    if project.store().source_roots().unwrap().is_empty() {
        project.add_source_root(&td2).unwrap();
        let t = Instant::now();
        let s = project.ingest().unwrap();
        eprintln!(
            "ingest: {} files done, {} failed, {:.1} s",
            s.stage_a.done,
            s.stage_a.failed,
            t.elapsed().as_secs_f64()
        );
    }
    let embed = embed_all(&ws, &project, &id);
    let store = project.store();
    let run = store.active_embedding_run().unwrap().expect("a run");
    let dims = run.dims.unwrap() as usize;
    let vectors: i64 = store
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM segment_embedding", [], |r| r.get(0))?))
        .unwrap();
    let segments: i64 = store
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM segment", [], |r| r.get(0))?))
        .unwrap();
    let t = Instant::now();
    assert!(project.prepare_meaning_index().unwrap());
    let build = t.elapsed();
    let index_bytes = std::fs::read_dir(project.layout().index())
        .unwrap()
        .flatten()
        .map(|e| e.metadata().map_or(0, |m| m.len()))
        .max()
        .unwrap_or(0);
    let db_bytes = std::fs::metadata(dir.join("project.db"))
        .map(|m| m.len())
        .unwrap_or(0);
    eprintln!("{embed}");
    eprintln!(
        "index: {vectors} vectors, {segments} segments, built in {:.1} s, {} MB; db {} MB",
        build.as_secs_f64(),
        index_bytes >> 20,
        db_bytes >> 20
    );
    if std::env::var_os("MK_S6_EMBED_ONLY").is_some() {
        return;
    }

    let mut queries = mk_bench::names::hand_written_queries();
    queries.extend(seeded(&project, 50));
    assert_eq!(queries.len(), 100);

    // Cold: the model and the index both to start, then the index alone after its idle release.
    let rss_before = rss_mb();
    let (cold_all, _, cold_timings) = timed_query(&ws, &project, &queries[0]);
    let rss_loaded = rss_mb();
    project.set_meaning_idle(Duration::from_millis(50));
    while project.meaning_index_in_memory() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let rss_released = rss_mb();
    project.set_meaning_idle(mk_core::MEANING_IDLE);
    let (cold_index, _, cold_index_timings) = timed_query(&ws, &project, &queries[0]);

    // Warm-up pass, then the timed pass.
    for q in &queries {
        timed_query(&ws, &project, q);
    }
    let (mut whole, mut embed_t, mut scan_t, mut vectors_t, mut rescore_t, mut rows_t) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let mut short = 0;
    for q in &queries {
        let (d, n, t) = timed_query(&ws, &project, q);
        short += usize::from(n < TOP_K);
        whole.push(d);
        embed_t.push(t.embed);
        scan_t.push(t.scan);
        vectors_t.push(t.vectors);
        rescore_t.push(t.rescore);
        rows_t.push(t.rows);
    }
    let mut sorted = whole.clone();
    sorted.sort();
    let p95 = percentile(&sorted, 0.95);
    let rss_after_queries = rss_mb();
    project.set_meaning_idle(Duration::from_millis(50));
    while project.meaning_index_in_memory() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let rss_end = rss_mb();

    // Recall, against exact float search over every vector.
    let client = ws.ai_client(&project, ModelRole::Embeddings).unwrap();
    let mut query_vectors = Vec::new();
    for q in &queries {
        let mut v = client
            .client
            .embeddings(std::slice::from_ref(q))
            .unwrap()
            .vectors
            .remove(0);
        normalize(&mut v);
        query_vectors.push(v);
    }
    drop(client);
    let bytes = std::fs::read(
        std::fs::read_dir(project.layout().index())
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("vectors-"))
            .unwrap()
            .path(),
    )
    .unwrap();
    let index = mk_search::meaning::IndexFile::decode(&bytes).unwrap();
    drop(bytes);
    let (ids, all) = load_all(&project, run.id, dims);
    assert_eq!(ids, index.ids);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut recall = [0f64; POOLS.len()];
    let mut recall_tie = [0f64; POOLS.len()];
    let mut recall_hand = [0f64; POOLS.len()];
    let mut recall_seeded = [0f64; POOLS.len()];
    let mut worst = [1f64; POOLS.len()];
    let mut self_hits = 0;
    for (i, q) in query_vectors.iter().enumerate() {
        let exact = exact_top(&all, dims, q);
        let want: Vec<i64> = exact.iter().map(|&(r, _)| ids[r as usize]).collect();
        let floor = exact.last().unwrap().1 - TIE;
        let codes = scan_top_k(&index.codes, &sign_bits(q), POOLS[3], threads);
        for (p, pool) in POOLS.iter().enumerate() {
            let pooled: Vec<(i64, Vec<f32>)> = codes[..*pool]
                .iter()
                .map(|&r| {
                    let start = r as usize * dims;
                    (ids[r as usize], all[start..start + dims].to_vec())
                })
                .collect();
            let top = rescore_top(q, &pooled, TOP_K);
            let hit = top.iter().filter(|t| want.contains(&t.0)).count();
            let hit_tie = top.iter().filter(|t| t.1 >= floor).count();
            let r = hit as f64 / TOP_K as f64;
            recall[p] += r;
            recall_tie[p] += hit_tie as f64 / TOP_K as f64;
            worst[p] = worst[p].min(r);
            if i < 50 {
                recall_hand[p] += r;
            } else {
                recall_seeded[p] += r;
            }
        }
        if i >= 50 {
            // A known-item query: its own text should come out on top.
            let hit = exact.iter().any(|&(r, _)| {
                let id = ids[r as usize];
                project
                    .store()
                    .read_raw(|c| {
                        Ok(c.query_row(
                            "SELECT instr(substr(s.text, e.char_start + 1,
                                                 e.char_end - e.char_start), ?2) > 0
                             FROM segment_embedding e JOIN segment s ON s.id = e.segment_id
                             WHERE e.id = ?1",
                            rusqlite::params![id, queries[i]],
                            |row| row.get::<_, bool>(0),
                        )?)
                    })
                    .unwrap()
            });
            self_hits += usize::from(hit);
        }
    }
    let per_pool: Vec<Value> = POOLS
        .iter()
        .enumerate()
        .map(|(p, pool)| {
            json!({
                "pool": pool,
                "recall_at_20": recall[p] / 100.0,
                "recall_at_20_near_ties": recall_tie[p] / 100.0,
                "hand": recall_hand[p] / 50.0,
                "seeded": recall_seeded[p] / 50.0,
                "worst_query": worst[p],
            })
        })
        .collect();
    let shipped = POOLS
        .iter()
        .position(|&p| p == RESCORE_POOL)
        .expect("the shipped pool is among those measured");
    let pass = p95 <= PASS && recall[shipped] / 100.0 >= 0.9;
    let summary = json!({
        "check": "s6_meaning_td2",
        "model": id,
        "segments": segments,
        "vectors": vectors,
        "dims": dims,
        "embed": embed,
        "index_build_s": build.as_secs_f64(),
        "index_bytes": index_bytes,
        "db_bytes": db_bytes,
        "whole_call": stats(&whole),
        "steps": {
            "embed": stats(&embed_t),
            "scan": stats(&scan_t),
            "vectors": stats(&vectors_t),
            "rescore": stats(&rescore_t),
            "rows": stats(&rows_t),
        },
        "queries_with_fewer_than_20": short,
        "cold": {
            "model_and_index_ms": ms(cold_all),
            "model_and_index_load_ms": ms(cold_timings.load),
            "index_only_ms": ms(cold_index),
            "index_only_load_ms": ms(cold_index_timings.load),
        },
        "rss_mb": {
            "before_first_query": rss_before,
            "index_loaded": rss_loaded,
            "released": rss_released,
            "after_all_queries": rss_after_queries,
            "released_at_end": rss_end,
        },
        "recall": per_pool,
        "seeded_query_found_itself_in_exact_top_20": self_hits,
        "pass_mark_ms": PASS.as_millis(),
        "pass": pass,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert!(pass, "S6 pass mark missed");
}
