//! `mangkon-cli embed` and `mangkon-cli meaning` (M6b, US-06.2): vectors for meaning search and a
//! query over them, through the same `mk-core` services the desktop calls. `ingest` does not start
//! this, so `bench` and the Layer 1 kill test do not change.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mk_core::{
    AiCancelled, AiChoice, MeaningOutcome, MeaningState, MeaningView, ModelRole, Project, Workspace,
};
use serde_json::{Value, json};

use crate::ai::open_workspace;

/// How often a run prints how far it got.
const PROGRESS_EVERY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

pub enum Command {
    Status,
    /// What the start rule allows, or with `true` everything not covered yet.
    Run(bool),
    Rerun,
    Cancel,
    Query(String),
}

pub struct Options {
    pub command: Command,
    pub project: PathBuf,
    /// The app's data folder (default: the OS one).
    pub app_data: Option<PathBuf>,
    /// The models folder (default: `models/` in the app's data folder).
    pub dir: Option<PathBuf>,
    /// `llama-server` for this run only (the app's setting is not changed).
    pub llama_server: Option<PathBuf>,
    /// The local model to use from now on: the app's AI choice changes, as "Use for AI" does.
    pub model: Option<String>,
    pub json: bool,
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

fn run_inner(opts: &Options) -> Result<ExitCode> {
    if !mk_core::project_exists(&opts.project) {
        return Err(format!("no project in {}", opts.project.display()).into());
    }
    let ws = open_workspace(&opts.app_data, &opts.dir, &opts.llama_server)?;
    if let Some(id) = &opts.model {
        ws.set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Local(id.clone())))
            .map_err(|e| e.message)?;
    }
    let (project, _, _) = ws.open_project(&opts.project)?;
    ws.refresh_ai_blocks(&project)?;
    let code = match &opts.command {
        Command::Status => {
            print_view(&ws.meaning_view(&project)?, opts.json);
            ExitCode::SUCCESS
        }
        Command::Cancel => cancel(&ws, &project, opts)?,
        Command::Run(all) => {
            if *all {
                ws.start_embeddings(&project)?;
            } else {
                ws.schedule_embeddings(&project)?;
            }
            lane(&ws, &project, opts)?
        }
        Command::Rerun => {
            ws.start_embedding_rerun(&project)?;
            lane(&ws, &project, opts)?
        }
        Command::Query(text) => query(&ws, &project, text, opts)?,
    };
    drop(project);
    ws.shut_down();
    Ok(code)
}

fn cancel(ws: &Workspace, project: &Project, opts: &Options) -> Result<ExitCode> {
    let out = ws.cancel_embeddings(project)?;
    let text = match out {
        AiCancelled::Rerun => "The re-run was cancelled. The vectors did not change.",
        AiCancelled::Run => "The rest of the run was cancelled. What it made stays.",
        AiCancelled::Nothing => "Nothing was running.",
    };
    if opts.json {
        print_json(&json!({ "cancelled": !matches!(out, AiCancelled::Nothing) }));
    } else {
        println!("{text}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Runs the lane when there is work, then prints where it ended. A Blocked job exits 1; a start
/// still waiting is not a failure.
fn lane(ws: &Workspace, project: &Project, opts: &Options) -> Result<ExitCode> {
    let before = ws.meaning_view(project)?;
    if before.state == MeaningState::Blocked {
        print_view(&before, opts.json);
        return Ok(ExitCode::FAILURE);
    }
    let stop = AtomicBool::new(false);
    let run = std::thread::scope(|s| {
        if !opts.json {
            s.spawn(|| {
                let mut waited = Duration::ZERO;
                let mut last = None;
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(POLL);
                    waited += POLL;
                    if waited < PROGRESS_EVERY {
                        continue;
                    }
                    waited = Duration::ZERO;
                    if let Ok(v) = ws.meaning_view(project)
                        && last != Some(v.items_done)
                    {
                        last = Some(v.items_done);
                        eprintln!("{} of {} parts", v.items_done, v.items_total);
                    }
                }
            });
        }
        let run = ws.run_embeddings(project);
        stop.store(true, Ordering::SeqCst);
        run
    });
    let summary = run?;
    let view = ws.meaning_view(project)?;
    if opts.json {
        let mut out = view_json(&view);
        out["this_run"] = json!({
            "done": summary.done,
            "failed": summary.failed,
            "windows": summary.windows,
            "dropped_replies": summary.dropped_replies,
        });
        print_json(&out);
    } else {
        println!(
            "This run  {} parts done, {} failed, {} vectors",
            summary.done, summary.failed, summary.windows
        );
        print_view(&view, false);
    }
    Ok(if view.state == MeaningState::Blocked {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// One query. The index is made first if it is missing or behind. Not ready exits 1 with why.
fn query(ws: &Workspace, project: &Project, text: &str, opts: &Options) -> Result<ExitCode> {
    let mut outcome = ws.meaning_search(project, text)?;
    if outcome == MeaningOutcome::Preparing {
        eprintln!("Preparing meaning search…");
        project.prepare_meaning_index()?;
        outcome = ws.meaning_search(project, text)?;
    }
    match outcome {
        MeaningOutcome::Results { hits, timings } => {
            if opts.json {
                let hits: Vec<Value> = hits
                    .iter()
                    .map(|h| {
                        json!({
                            "score": h.score,
                            "path": h.hit.path,
                            "locator": format!("{:?}", h.hit.locator),
                            "segment_id": h.hit.segment_id,
                            "snippet": h.hit.snippet.iter().map(|r| r.text.as_str()).collect::<String>(),
                        })
                    })
                    .collect();
                print_json(&json!({
                    "hits": hits,
                    "ms": {
                        "load": timings.load.as_secs_f64() * 1e3,
                        "embed": timings.embed.as_secs_f64() * 1e3,
                        "scan": timings.scan.as_secs_f64() * 1e3,
                        "vectors": timings.vectors.as_secs_f64() * 1e3,
                        "rescore": timings.rescore.as_secs_f64() * 1e3,
                        "rows": timings.rows.as_secs_f64() * 1e3,
                    },
                }));
            } else {
                for h in &hits {
                    let snippet: String = h.hit.snippet.iter().map(|r| r.text.as_str()).collect();
                    println!(
                        "{:.3}  {}  {:?}\n       {}",
                        h.score,
                        h.hit.path.as_deref().unwrap_or("(no path)"),
                        h.hit.locator,
                        snippet.replace('\n', " ")
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        MeaningOutcome::NotReady(why) => not_ready(&why, opts),
        MeaningOutcome::Preparing => not_ready("Preparing meaning search…", opts),
    }
}

fn not_ready(why: &str, opts: &Options) -> Result<ExitCode> {
    if opts.json {
        print_json(&json!({ "not_ready": why }));
    } else {
        println!("{why}");
    }
    Ok(ExitCode::FAILURE)
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).expect("JSON values"));
}

fn view_json(v: &MeaningView) -> Value {
    json!({
        "state": v.state.as_str(),
        "reason": v.reason,
        "items_done": v.items_done,
        "items_total": v.items_total,
        "items_failed": v.items_failed,
        "destination": v.destination,
        "vectors_model": v.vectors_model,
        "rerun_model": v.rerun_model,
        "rerun_with": v.rerun_with,
        "waiting": v.waiting.as_ref().map(|w| {
            json!({ "windows": w.windows, "estimate_seconds": w.estimate.as_secs() })
        }),
    })
}

fn print_view(v: &MeaningView, json: bool) {
    if json {
        print_json(&view_json(v));
        return;
    }
    println!("Meaning search");
    println!("State     {}", v.state.as_str());
    if let Some(r) = &v.reason {
        println!("Why       {r}");
    }
    println!(
        "Parts     {} of {}, {} failed",
        v.items_done, v.items_total, v.items_failed
    );
    if let Some(d) = &v.destination {
        println!("Where     {d}");
    }
    if let Some(m) = &v.vectors_model {
        println!("Vectors   from {m}");
    }
    if let Some(m) = &v.rerun_model {
        println!("Re-run    with {m}; the vectors stay until it is done");
    }
    if let Some(m) = &v.rerun_with {
        println!("Chosen    {m}: `embed rerun` replaces the vectors with its own");
    }
    if let Some(w) = &v.waiting {
        println!(
            "Waiting   {} windows, about {} min: start with `embed run --all`",
            w.windows,
            w.estimate.as_secs().div_ceil(60)
        );
    }
}
