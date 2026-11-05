//! `mangkon-cli ai` (M6a, US-10.1, US-10.4): people and organizations found by a model, through
//! the same `mk-core` services the desktop calls. Stage B never starts from `ingest`, so it stays
//! a separate step here: `bench` and the Layer 1 kill test do not change.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mk_core::{
    AiCancelled, AiChoice, AiExtractionView, AiLaneState, EgressConfig, ModelRole, ModelsConfig,
    Project, UnitEstimate, UnitScope, Workspace,
};
use serde_json::{Value, json};

use crate::manage;

/// How often a run prints how far it got.
const PROGRESS_EVERY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

pub enum Scope {
    /// What the start rule allows.
    Auto,
    All,
    Documents(Vec<PathBuf>),
}

pub enum Command {
    Status,
    Run(Scope),
    Rerun,
    Cancel,
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

pub(crate) fn open_workspace(
    app_data: &Option<PathBuf>,
    models_dir: &Option<PathBuf>,
    llama_server: &Option<PathBuf>,
) -> Result<Workspace> {
    let dir = match app_data {
        Some(d) => d.clone(),
        None => mk_core::app_data_dir().ok_or("The system gives no folder for app data")?,
    };
    let ws = Workspace::open_with_models(
        &dir,
        EgressConfig::default(),
        ModelsConfig {
            models_dir: models_dir.clone(),
            ..ModelsConfig::default()
        },
    )?;
    if let Some(p) = llama_server {
        ws.supervisor().set_settings_path(Some(p.clone()));
    }
    Ok(ws)
}

fn run_inner(opts: &Options) -> Result<ExitCode> {
    if !mk_core::project_exists(&opts.project) {
        return Err(format!("no project in {}", opts.project.display()).into());
    }
    let ws = open_workspace(&opts.app_data, &opts.dir, &opts.llama_server)?;
    if let Some(id) = &opts.model {
        ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local(id.clone())))
            .map_err(|e| e.message)?;
    }
    let (project, _, _) = ws.open_project(&opts.project)?;
    ws.refresh_ai_blocks(&project)?;
    let code = match &opts.command {
        Command::Status => {
            print_view(&ws.ai_extraction_view(&project)?, opts.json);
            ExitCode::SUCCESS
        }
        Command::Cancel => cancel(&ws, &project, opts)?,
        Command::Run(scope) => {
            match scope {
                Scope::Auto => {
                    ws.schedule_stage_b(&project)?;
                }
                Scope::All => {
                    ws.run_documents(&project, UnitScope::All)?;
                }
                Scope::Documents(paths) => {
                    let rows = manage::all_rows(&project)?;
                    let mut ids: Vec<i64> = manage::rows_at(&rows, paths)?
                        .iter()
                        .map(|r| r.content_id)
                        .collect();
                    ids.sort_unstable();
                    ids.dedup();
                    ws.run_documents(&project, UnitScope::Contents(ids))?;
                }
            }
            lane(&ws, &project, opts)?
        }
        Command::Rerun => {
            ws.start_rerun(&project)?;
            lane(&ws, &project, opts)?
        }
    };
    drop(project);
    ws.shut_down();
    Ok(code)
}

fn cancel(ws: &Workspace, project: &Project, opts: &Options) -> Result<ExitCode> {
    let out = ws.cancel_extraction(project)?;
    let text = match out {
        AiCancelled::Rerun => "The re-run was cancelled. The results did not change.",
        AiCancelled::Run => "The rest of the run was cancelled. What it found stays.",
        AiCancelled::Nothing => "Nothing was running.",
    };
    if opts.json {
        print_json(&json!({ "cancelled": !matches!(out, AiCancelled::Nothing) }));
    } else {
        println!("{text}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Runs the lane when there is work, then prints where it ended. A Blocked job exits 1; a pick
/// still waiting is not a failure.
fn lane(ws: &Workspace, project: &Project, opts: &Options) -> Result<ExitCode> {
    let before = ws.ai_extraction_view(project)?;
    if before.state == AiLaneState::Blocked {
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
                    if let Ok(v) = ws.ai_extraction_view(project)
                        && last != Some(v.units_done)
                    {
                        last = Some(v.units_done);
                        eprintln!("{} of {} units", v.units_done, v.units_total);
                    }
                }
            });
        }
        let run = ws.run_stage_b(project);
        stop.store(true, Ordering::SeqCst);
        run
    });
    let summary = run?;
    let view = ws.ai_extraction_view(project)?;
    if opts.json {
        let mut out = view_json(&view);
        out["this_run"] = json!({
            "done": summary.done,
            "failed": summary.failed,
            "mentions": summary.mentions,
            "dropped_replies": summary.dropped_replies,
        });
        print_json(&out);
    } else {
        println!(
            "This run  {} units done, {} failed, {} mentions",
            summary.done, summary.failed, summary.mentions
        );
        print_view(&view, false);
    }
    Ok(if view.state == AiLaneState::Blocked {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).expect("JSON values"));
}

fn estimate_json(e: &UnitEstimate) -> Value {
    json!({ "units": e.units, "estimate_seconds": e.estimate.as_secs() })
}

fn view_json(v: &AiExtractionView) -> Value {
    json!({
        "experimental": v.experimental,
        "state": v.state.as_str(),
        "reason": v.reason,
        "units_done": v.units_done,
        "units_total": v.units_total,
        "units_failed": v.units_failed,
        "destination": v.destination,
        "results_model": v.results_model,
        "rerun_model": v.rerun_model,
        "rerun_with": v.rerun_with,
        "mentions": v.mentions,
        "discarded": v.discarded,
        "invalid_responses": v.invalid_responses,
        "identifier_dropped": v.identifier_dropped,
        "waiting": v.waiting.as_ref().map(estimate_json),
    })
}

fn print_view(v: &AiExtractionView, json: bool) {
    if json {
        print_json(&view_json(v));
        return;
    }
    let label = if v.experimental {
        " (Experimental)"
    } else {
        ""
    };
    println!("AI extraction{label}");
    println!("State     {}", v.state.as_str());
    if let Some(r) = &v.reason {
        println!("Reason    {r}");
    }
    println!(
        "Units     {} of {}, {} failed",
        v.units_done, v.units_total, v.units_failed
    );
    if let Some(d) = &v.destination {
        println!("Where     {d}");
    }
    if let Some(m) = &v.results_model {
        println!("Results   from {m}");
    }
    if let Some(m) = &v.rerun_model {
        println!("Re-run    with {m}, not shown until it is done");
    }
    if let Some(m) = &v.rerun_with {
        println!("Chosen    {m}: `ai rerun` replaces the results with its own");
    }
    println!("Mentions  {}", v.mentions);
    println!("Discarded {} (no matching text)", v.discarded);
    println!(
        "Unread    {} responses could not be read",
        v.invalid_responses
    );
    println!(
        "Dropped   {} names inside an identifier",
        v.identifier_dropped
    );
    if let Some(w) = &v.waiting {
        println!(
            "Waiting   {} units, about {} min: pick with --documents PATH..., or --all",
            w.units,
            w.estimate.as_secs().div_ceil(60)
        );
    }
}
