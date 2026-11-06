//! `llm-bench` (M6c, NFR-MNT-06): the people-and-organization and embedding models on TD-1 through
//! the same services the desktop calls, with their timers. Run by hand before a release; it needs
//! `llama-server` and the model files, so it is not in CI. Each run is a fresh project and a fresh
//! start of the models. Replies are not repeatable (`docs/14-ai-extraction.md` §4), so the median of
//! the runs decides. Targets and counts go to stdout; names only into the `--json` file.
//!
//! `synthetic/` and `public/` are two roots of one project: the ground-truth files next to them
//! are the answers, not part of the corpus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mk_bench::GroundTruth;
use mk_bench::names::{self, FoundName, NameReport};
use mk_core::{AiChoice, ConnectionQuery, ModelRole, Project, ProjectSettings, Workspace};
use mk_search::meaning::{
    RESCORE_POOL, TOP_K, decode_f16, dot, normalize, rescore_top, scan_top_k, sign_bits,
};
use serde_json::{Value, json};

use crate::ai::open_workspace;
use crate::bench::gt_document;

/// The two parts of TD-1, each a root of the one project.
const PARTS: [&str; 2] = ["synthetic", "public"];
const REGION: &str = "GB";
/// How often the first person or organization is looked for (NFR-PERF-12). The desktop adds its
/// own 500 ms debounce before the screen shows it.
const POLL: Duration = Duration::from_millis(100);
/// M1's seeded known-item queries.
const SEEDED_QUERIES: usize = 50;
const QUERY_SEED: u64 = 0x5eed_0006;

const PRECISION_TARGET: f64 = 85.0;
const RECALL_TARGET: f64 = 70.0;
const FIRST_CONNECTION_TARGET_S: f64 = 300.0;
const STAGE_B_TARGET_S: f64 = 1200.0;
const UNITS_PER_SECOND_TARGET: f64 = 1.0;
const EMBEDDING_RECALL_TARGET: f64 = 0.9;

pub struct Options {
    pub testdata: PathBuf,
    /// The extraction model's local ID.
    pub model: String,
    /// The embedding model's local ID; without it the embeddings lane and NFR-PERF-13 are left out.
    pub embedding_model: Option<String>,
    pub runs: usize,
    pub app_data: Option<PathBuf>,
    pub dir: Option<PathBuf>,
    pub llama_server: Option<PathBuf>,
    pub tessdata: Option<PathBuf>,
    /// Keep the projects in DIR (default: a temporary folder).
    pub work: Option<PathBuf>,
    /// Write the whole report, names included, to this file.
    pub json: Option<PathBuf>,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn run(opts: &Options) -> ExitCode {
    match run_inner(opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// One run's numbers.
struct Run {
    stage_a_s: f64,
    files_failed: u64,
    first_connection_s: Option<f64>,
    stage_b_s: f64,
    units: u64,
    units_failed: u64,
    mentions: u64,
    discarded: u64,
    unread_replies: u64,
    identifier_dropped: u64,
    dropped_replies: u64,
    embeddings: Option<Embeddings>,
    /// From the end of Stage A until both lanes are done.
    both_lanes_s: f64,
    rebuild_matches: bool,
    healthy: bool,
    unmapped: Vec<String>,
    names: NameReport,
}

struct Embeddings {
    lane_s: f64,
    parts: u64,
    parts_failed: u64,
    windows: u64,
    recall_at_20: f64,
    worst_query: f64,
    queries: usize,
}

impl Run {
    fn valid(&self) -> bool {
        self.units_failed == 0
            && self.files_failed == 0
            && self.rebuild_matches
            && self.healthy
            && self.unmapped.is_empty()
            && self.embeddings.as_ref().is_none_or(|e| e.parts_failed == 0)
    }

    fn units_per_second(&self) -> f64 {
        self.units as f64 / self.stage_b_s.max(f64::EPSILON)
    }

    fn json(&self) -> Value {
        json!({
            "stage_a_s": self.stage_a_s,
            "files_failed": self.files_failed,
            "first_connection_s": self.first_connection_s,
            "stage_b_s": self.stage_b_s,
            "units": self.units,
            "units_failed": self.units_failed,
            "units_per_second": self.units_per_second(),
            "mentions": self.mentions,
            "discarded": self.discarded,
            "unread_replies": self.unread_replies,
            "identifier_dropped": self.identifier_dropped,
            "dropped_replies": self.dropped_replies,
            "embeddings": self.embeddings.as_ref().map(|e| json!({
                "lane_s": e.lane_s,
                "parts": e.parts,
                "parts_failed": e.parts_failed,
                "windows": e.windows,
                "recall_at_20": e.recall_at_20,
                "worst_query": e.worst_query,
                "queries": e.queries,
            })),
            "both_lanes_s": self.both_lanes_s,
            "rebuild_matches": self.rebuild_matches,
            "healthy": self.healthy,
            "unmapped": self.unmapped,
            "names": self.names,
        })
    }
}

/// One target, on the median of the runs.
struct Target {
    id: &'static str,
    what: &'static str,
    goal: String,
    value: Option<f64>,
    met: bool,
}

fn run_inner(opts: &Options) -> Result<ExitCode> {
    let gt = mk_bench::load(&opts.testdata.join("ground-truth.json"))?;
    let temp;
    let work = match &opts.work {
        Some(dir) => dir.clone(),
        None => {
            temp = tempfile::tempdir()?;
            temp.path().to_path_buf()
        }
    };
    // The AI choice is the app's setting. It goes back to what it was at the end.
    let saved = {
        let ws = open_workspace(&opts.app_data, &opts.dir, &opts.llama_server)?;
        let saved = [
            saved_choice(&ws, ModelRole::Extraction)?,
            saved_choice(&ws, ModelRole::Embeddings)?,
        ];
        ws.shut_down();
        saved
    };
    let mut runs = Vec::new();
    let mut error = None;
    for n in 1..=opts.runs {
        eprintln!("Run {n} of {}", opts.runs);
        match one_run(opts, &gt, &work.join(format!("run-{n}"))) {
            Ok(run) => {
                print_run(n, &run);
                runs.push(run);
            }
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    let ws = open_workspace(&opts.app_data, &opts.dir, &opts.llama_server)?;
    let restored = saved
        .into_iter()
        .try_for_each(|(role, choice)| ws.set_ai_choice(role, choice).map(|_| ()));
    ws.shut_down();
    if let Some(e) = error {
        return Err(e);
    }
    restored.map_err(|e| e.message)?;

    let targets = targets(&runs);
    println!();
    for t in &targets {
        let value = t
            .value
            .map_or_else(|| "-".to_string(), |v| format!("{v:.2}"));
        println!(
            "{:<12} {:<34} {:>9} (target {}) {}",
            t.id,
            t.what,
            value,
            t.goal,
            if t.met { "met" } else { "NOT MET" }
        );
    }
    if let Some(path) = &opts.json {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let report = json!({
            "model": opts.model,
            "embedding_model": opts.embedding_model,
            "runs": runs.iter().map(Run::json).collect::<Vec<_>>(),
            "targets": targets.iter().map(|t| json!({
                "id": t.id, "what": t.what, "goal": t.goal, "median": t.value, "met": t.met,
            })).collect::<Vec<_>>(),
        });
        let mut text = serde_json::to_string_pretty(&report)?;
        text.push('\n');
        std::fs::write(path, text)?;
    }
    Ok(if runs.iter().all(Run::valid) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn saved_choice(ws: &Workspace, role: ModelRole) -> Result<(ModelRole, Option<AiChoice>)> {
    let view = ws.ai_choice(role).map_err(|e| e.message)?;
    Ok((role, view.explicit.then_some(view.choice).flatten()))
}

fn one_run(opts: &Options, gt: &GroundTruth, dir: &Path) -> Result<Run> {
    if dir.exists() {
        return Err(format!(
            "{} already exists: a run needs a fresh project",
            dir.display()
        )
        .into());
    }
    let ws = open_workspace(&opts.app_data, &opts.dir, &opts.llama_server)?;
    let result = one_run_in(opts, gt, dir, &ws);
    ws.shut_down();
    result
}

fn one_run_in(opts: &Options, gt: &GroundTruth, dir: &Path, ws: &Workspace) -> Result<Run> {
    ws.set_ai_choice(
        ModelRole::Extraction,
        Some(AiChoice::Local(opts.model.clone())),
    )
    .map_err(|e| e.message)?;
    if let Some(id) = &opts.embedding_model {
        ws.set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Local(id.clone())))
            .map_err(|e| e.message)?;
    }
    std::fs::create_dir_all(dir)?;
    let (mut project, _) = Project::open(dir)?;
    project.set_tessdata_dir(opts.tessdata.clone().or_else(mk_core::tessdata_dir));
    project.save_settings(ProjectSettings {
        phone_region: Some(REGION.into()),
        ..project.settings()?
    })?;
    let mut roots = BTreeMap::new();
    for part in PARTS {
        let source = opts.testdata.join(part);
        if source.is_dir() {
            roots.insert(project.add_source_root(&source)?, part);
        }
    }

    let people = ConnectionQuery {
        kinds: vec!["person".into(), "org".into()],
        ..ConnectionQuery::default()
    };
    let start = Instant::now();
    let stop = AtomicBool::new(false);
    let (stage_a_s, first_connection_s, lanes) = std::thread::scope(|s| {
        let project = &project;
        let watcher = s.spawn(|| {
            loop {
                // Read before the look, so a run shorter than one poll still sees its last rows.
                let stopping = stop.load(Ordering::SeqCst);
                if project.connection_count_for(&people).is_ok_and(|n| n > 0) {
                    return Some(start.elapsed().as_secs_f64());
                }
                if stopping {
                    return None;
                }
                std::thread::sleep(POLL);
            }
        });
        let lanes = (|| -> Result<(f64, Lanes)> {
            project.ingest()?;
            let stage_a_s = start.elapsed().as_secs_f64();
            Ok((stage_a_s, run_lanes(opts, ws, project)?))
        })();
        stop.store(true, Ordering::SeqCst);
        let first = watcher.join().expect("the watcher does not panic");
        match lanes {
            Ok((stage_a_s, lanes)) => Ok((stage_a_s, first, lanes)),
            Err(e) => Err(e),
        }
    })?;
    eprintln!(
        "Stage A {stage_a_s:.0} s; the lanes {:.0} s; checking",
        lanes.both_s
    );

    let view = ws.ai_extraction_view(&project)?;
    let (found, unmapped) = found_names(&project, gt, &roots)?;
    let names = names::score(gt, &found);
    let embeddings = match &lanes.embeddings {
        None => None,
        Some(e) => {
            let (recall_at_20, worst_query, queries) = embedding_recall(ws, &project)?;
            Some(Embeddings {
                lane_s: e.lane_s,
                parts: e.summary.done,
                parts_failed: e.summary.failed,
                windows: e.summary.windows,
                recall_at_20,
                worst_query,
                queries,
            })
        }
    };
    let rebuilt = project.rebuild()?;
    Ok(Run {
        stage_a_s,
        files_failed: project.counts()?.failed,
        first_connection_s,
        stage_b_s: lanes.stage_b_s,
        units: lanes.stage_b.done,
        units_failed: lanes.stage_b.failed,
        mentions: view.mentions,
        discarded: view.discarded,
        unread_replies: view.invalid_responses,
        identifier_dropped: view.identifier_dropped,
        dropped_replies: lanes.stage_b.dropped_replies,
        embeddings,
        both_lanes_s: lanes.both_s,
        rebuild_matches: rebuilt.same_as_incremental && rebuilt.consistent,
        healthy: project.check_health()?.is_healthy(),
        unmapped,
        names,
    })
}

struct Lanes {
    stage_b_s: f64,
    stage_b: mk_core::StageBSummary,
    embeddings: Option<EmbeddingLane>,
    both_s: f64,
}

struct EmbeddingLane {
    lane_s: f64,
    summary: mk_core::EmbeddingSummary,
}

/// Both lanes on threads of their own, as the desktop runs them (`apps/desktop/src/state.rs`).
fn run_lanes(opts: &Options, ws: &Workspace, project: &Project) -> Result<Lanes> {
    ws.schedule_stage_b(project)?;
    if let Some(w) = ws.ai_extraction_view(project)?.waiting {
        return Err(format!(
            "{} units wait for a pick: Stage B did not start by itself (NFR-PERF-12)",
            w.units
        )
        .into());
    }
    if opts.embedding_model.is_some() {
        ws.schedule_embeddings(project)?;
    }
    let start = Instant::now();
    let (stage_b, embeddings) = std::thread::scope(|s| {
        let embeddings = opts.embedding_model.is_some().then(|| {
            s.spawn(|| {
                let summary = ws.run_embeddings(project);
                (start.elapsed().as_secs_f64(), summary)
            })
        });
        let stage_b = ws.run_stage_b(project);
        let stage_b_s = start.elapsed().as_secs_f64();
        let embeddings = embeddings.map(|h| h.join().expect("the lane does not panic"));
        ((stage_b_s, stage_b), embeddings)
    });
    let both_s = start.elapsed().as_secs_f64();
    let (stage_b_s, stage_b) = stage_b;
    let stage_b = stage_b?;
    if let Some(reason) = &stage_b.blocked {
        return Err(format!("Stage B was Blocked: {reason}").into());
    }
    let embeddings = match embeddings {
        None => None,
        Some((lane_s, summary)) => {
            let summary = summary?;
            if let Some(reason) = &summary.blocked {
                return Err(format!("Embeddings were Blocked: {reason}").into());
            }
            Some(EmbeddingLane { lane_s, summary })
        }
    };
    Ok(Lanes {
        stage_b_s,
        stage_b,
        embeddings,
        both_s,
    })
}

/// The names a model stored, with their ground-truth documents, and the stored documents the
/// ground truth does not have. A file found under two paths counts for both.
fn found_names(
    project: &Project,
    gt: &GroundTruth,
    roots: &BTreeMap<i64, &str>,
) -> Result<(Vec<FoundName>, Vec<String>)> {
    let store = project.store();
    let mut ids: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let mut unmapped = Vec::new();
    for r in store.stored_refs()? {
        let Some(part) = roots.get(&r.root).filter(|_| r.present) else {
            continue;
        };
        let file = format!("{part}/{}", r.rel_path);
        let Some(content) = store.content(&r.hash)? else {
            continue;
        };
        let mut documents: Vec<i64> = store
            .segments(content.id)?
            .iter()
            .map(|s| s.document_id)
            .collect();
        documents.sort_unstable();
        documents.dedup();
        for d in documents {
            match gt_document(project, gt, &file, d)? {
                Ok(id) => ids.entry(d).or_default().push(id),
                Err(position) => unmapped.push(format!("{file} ({position})")),
            }
        }
    }
    let rows: Vec<(i64, Option<u32>, String, String, f64)> = store.read_raw(|c| {
        let mut stmt = c.prepare(
            "SELECT m.document_id,
                    CASE WHEN s.loc_kind IN ('pdf_text', 'ocr_text') THEN s.loc_pos END,
                    m.name_key, m.raw, m.confidence
             FROM entity_mention m JOIN segment s ON s.id = m.segment_id
             WHERE m.producer = 'model'",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    let mut found = Vec::new();
    for (document, page, key, raw, confidence) in rows {
        for id in ids.get(&document).into_iter().flatten() {
            found.push(FoundName {
                document: id.clone(),
                page,
                key: key.clone(),
                raw: raw.clone(),
                confidence: confidence as f32,
            });
        }
    }
    Ok((found, unmapped))
}

/// Recall@20 of the binary scan with rescoring at the production pool against exact search over
/// the stored vectors (ADR-0010), with M1's queries, so a new catalog model can be checked against
/// its `binary_recall_at_20`. Returns (mean, worst query, queries).
fn embedding_recall(ws: &Workspace, project: &Project) -> Result<(f64, f64, usize)> {
    let store = project.store();
    let run = store
        .active_embedding_run()?
        .ok_or("the embeddings lane left no vectors")?;
    let dims = run.dims.ok_or("the embeddings lane left no vectors")? as usize;
    let (mut ids, mut all, mut codes) = (Vec::new(), Vec::new(), Vec::new());
    let mut after = 0;
    loop {
        let page = store.embedding_rows_page(run.id, after, 8192)?;
        let Some((last, _)) = page.last() else { break };
        after = *last;
        for (id, bytes) in &page {
            let v = decode_f16(bytes, dims).ok_or("a stored vector has the wrong size")?;
            codes.extend(sign_bits(&v));
            all.extend(v);
            ids.push(*id);
        }
    }
    let mut queries = names::hand_written_queries();
    queries.extend(seeded_queries(project, SEEDED_QUERIES)?);
    let client = ws
        .ai_client(project, ModelRole::Embeddings)
        .map_err(|e| format!("{e:?}"))?;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let (mut sum, mut worst) = (0.0, 1.0f64);
    for q in &queries {
        let mut v = client
            .client
            .embeddings(std::slice::from_ref(q))
            .map_err(|e| format!("{e:?}"))?
            .vectors
            .remove(0);
        normalize(&mut v);
        let mut exact: Vec<(i64, f32)> = ids
            .iter()
            .zip(all.chunks_exact(dims))
            .map(|(id, row)| (*id, dot(row, &v)))
            .collect();
        exact.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        exact.truncate(TOP_K);
        let pool: Vec<(i64, Vec<f32>)> = scan_top_k(&codes, &sign_bits(&v), RESCORE_POOL, threads)
            .into_iter()
            .map(|row| {
                let start = row as usize * dims;
                (ids[row as usize], all[start..start + dims].to_vec())
            })
            .collect();
        let top = rescore_top(&v, &pool, TOP_K);
        let hits = top
            .iter()
            .filter(|t| exact.iter().any(|e| e.0 == t.0))
            .count();
        let recall = hits as f64 / exact.len().max(1) as f64;
        sum += recall;
        worst = worst.min(recall);
    }
    Ok((sum / queries.len() as f64, worst, queries.len()))
}

/// M1's seeded known-item queries: the longest sentence (40 characters or more, cut at 200) of a
/// random stored window. A small corpus may give fewer than `n`.
fn seeded_queries(project: &Project, n: usize) -> Result<Vec<String>> {
    let mut state = QUERY_SEED;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };
    let store = project.store();
    let max: i64 = store.read_raw(|c| {
        Ok(c.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM segment_embedding",
            [],
            |r| r.get(0),
        )?)
    })?;
    let mut out: Vec<String> = Vec::new();
    for _ in 0..n * 50 {
        if out.len() == n || max == 0 {
            break;
        }
        let id = 1 + (next() % max as u64) as i64;
        let text: Option<String> = store.read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT substr(s.text, e.char_start + 1, e.char_end - e.char_start)
                 FROM segment_embedding e JOIN segment s ON s.id = e.segment_id
                 WHERE e.id >= ?1 LIMIT 1",
            )?;
            let mut rows = stmt.query([id])?;
            Ok(match rows.next()? {
                Some(r) => r.get(0)?,
                None => None,
            })
        })?;
        let Some(text) = text else { continue };
        let best = text
            .split(['.', '!', '?', '\n'])
            .map(str::trim)
            .filter(|s| s.chars().count() >= 40)
            .max_by_key(|s| s.chars().count());
        if let Some(best) = best {
            let cut: String = best.chars().take(200).collect();
            if !out.contains(&cut) {
                out.push(cut);
            }
        }
    }
    Ok(out)
}

fn targets(runs: &[Run]) -> Vec<Target> {
    let median = |f: &dyn Fn(&Run) -> Option<f64>| {
        let values: Vec<f64> = runs.iter().filter_map(f).collect();
        names::median(&values)
    };
    let at_least = |id, what, goal: f64, value: Option<f64>| Target {
        id,
        what,
        goal: format!(">= {goal}"),
        value,
        met: value.is_some_and(|v| v >= goal),
    };
    let at_most = |id, what, goal: f64, value: Option<f64>| Target {
        id,
        what,
        goal: format!("<= {goal}"),
        value,
        met: value.is_some_and(|v| v <= goal),
    };
    vec![
        at_least(
            "NFR-QA-08",
            "precision, shown (%)",
            PRECISION_TARGET,
            median(&|r| Some(r.names.overall.shown.precision())),
        ),
        at_least(
            "NFR-QA-09",
            "recall, shown (%)",
            RECALL_TARGET,
            median(&|r| Some(r.names.overall.shown.recall())),
        ),
        at_most(
            "NFR-PERF-12",
            "first person or org after drop (s)",
            FIRST_CONNECTION_TARGET_S,
            median(&|r| r.first_connection_s),
        ),
        at_most(
            "NFR-PERF-13",
            "both lanes after Stage A (s)",
            STAGE_B_TARGET_S,
            median(&|r| r.embeddings.is_some().then_some(r.both_lanes_s)),
        ),
        at_least(
            "NFR-PERF-24",
            "extraction (units/s)",
            UNITS_PER_SECOND_TARGET,
            median(&|r| Some(r.units_per_second())),
        ),
        at_least(
            "ADR-0010",
            "embedding recall@20",
            EMBEDDING_RECALL_TARGET,
            median(&|r| r.embeddings.as_ref().map(|e| e.recall_at_20)),
        ),
    ]
}

fn print_run(n: usize, r: &Run) {
    let row = |name: &str, x: &mk_bench::names::Row| {
        println!(
            "  {name:<14} shown P {:>5.1} R {:>5.1}   all P {:>5.1} R {:>5.1}   ({} / {} / {})",
            x.shown.precision(),
            x.shown.recall(),
            x.all.precision(),
            x.all.recall(),
            x.shown.tp,
            x.shown.fp,
            x.shown.fn_,
        );
    };
    println!("Run {n}");
    row("overall", &r.names.overall);
    row("person", &r.names.person);
    row("org", &r.names.org);
    row("type ignored", &r.names.type_agnostic);
    for source in names::SOURCES {
        row(source, &r.names.by_source[source]);
    }
    println!(
        "  links          shown P {:>5.1} R {:>5.1} ({} of {}, {} predicted)",
        r.names.links_shown.precision(),
        r.names.links_shown.recall(),
        r.names.links_shown.correct,
        r.names.links_shown.gold,
        r.names.links_shown.predicted,
    );
    println!(
        "  units          {} ({} failed), {:.2}/s; {} mentions, {} discarded, {} unread, {} on an identifier, {} late",
        r.units,
        r.units_failed,
        r.units_per_second(),
        r.mentions,
        r.discarded,
        r.unread_replies,
        r.identifier_dropped,
        r.dropped_replies,
    );
    let first = r
        .first_connection_s
        .map_or_else(|| "never".to_string(), |s| format!("{s:.1} s"));
    println!(
        "  times          Stage A {:.1} s, first person or org {first}, extraction {:.1} s, both lanes {:.1} s",
        r.stage_a_s, r.stage_b_s, r.both_lanes_s,
    );
    if let Some(e) = &r.embeddings {
        println!(
            "  embeddings     {} parts ({} failed), {} windows, {:.1} s; recall@20 {:.4}, worst {:.2}, {} queries",
            e.parts, e.parts_failed, e.windows, e.lane_s, e.recall_at_20, e.worst_query, e.queries,
        );
    }
    println!(
        "  checks         files failed: {}, rebuild matches: {}, healthy: {}, unmapped documents: {}",
        r.files_failed,
        r.rebuild_matches,
        r.healthy,
        r.unmapped.len(),
    );
}
