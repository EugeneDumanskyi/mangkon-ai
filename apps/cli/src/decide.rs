//! `proposals`, `merge`, `split` and `decisions` (M7a): the same `mk-core` services the review queue
//! will call, for merge proposals (US-11.2, US-11.3), undo (US-11.4), split (US-11.1 AC2) and the
//! decision log (US-11.5 AC2).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mk_core::{DecisionRow, DecisionStatus, EntityType, MergeEdit, Project, ProposalCursor};
use serde_json::json;

const PAGE: usize = 500;

pub struct ProposalsOptions {
    pub project: PathBuf,
    /// At most this many rows; all of them when `None`.
    pub limit: Option<usize>,
    pub json: bool,
}

pub enum MergeAction {
    Accept {
        name: Option<String>,
        entity_type: Option<EntityType>,
    },
    Reject,
    Undo,
}

pub struct MergeOptions {
    pub project: PathBuf,
    pub action: MergeAction,
    pub a: String,
    pub b: String,
    pub json: bool,
}

pub enum SplitAction {
    /// The entity's mentions, with the ids to pick from.
    List(String),
    Split(String, Vec<i64>),
    Undo(String),
}

pub struct SplitOptions {
    pub project: PathBuf,
    pub action: SplitAction,
    pub json: bool,
}

pub struct DecisionsOptions {
    pub project: PathBuf,
    pub before: Option<i64>,
    pub limit: usize,
    pub json: bool,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn finish(r: Result<()>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn open(dir: &Path) -> Result<Project> {
    if !mk_core::project_exists(dir) {
        return Err(format!("no project in {}", dir.display()).into());
    }
    Ok(Project::open(dir)?.0)
}

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).expect("JSON values"));
}

/// Makes the proposals, then lists them, highest score first.
pub fn proposals(opts: &ProposalsOptions) -> ExitCode {
    finish(run_proposals(opts))
}

fn run_proposals(opts: &ProposalsOptions) -> Result<()> {
    let project = open(&opts.project)?;
    project.refresh_proposals()?;
    let mut rows = Vec::new();
    let mut after: Option<ProposalCursor> = None;
    loop {
        let page = project.proposals(after.as_ref(), PAGE)?;
        let Some(last) = page.last() else { break };
        after = Some(ProposalCursor {
            score: last.score,
            a: last.a.clone(),
            b: last.b.clone(),
        });
        let done = page.len() < PAGE;
        rows.extend(page);
        if done || opts.limit.is_some_and(|l| rows.len() >= l) {
            break;
        }
    }
    if let Some(limit) = opts.limit {
        rows.truncate(limit);
    }
    if opts.json {
        print_json(&json!(
            rows.iter()
                .map(|p| json!({
                    "a": p.a,
                    "b": p.b,
                    "left": { "key": p.left_key, "name": p.left_name },
                    "right": { "key": p.right_key, "name": p.right_name },
                    "score": p.score,
                    "reasons": p.reasons,
                }))
                .collect::<Vec<_>>()
        ));
    } else if rows.is_empty() {
        println!("No merge proposals.");
    } else {
        for p in &rows {
            println!(
                "{:.2}  {} / {}\n      {} | {}\n      {}",
                p.score,
                p.left_name,
                p.right_name,
                p.a,
                p.b,
                p.reasons.join(" · "),
            );
        }
    }
    Ok(())
}

/// Accepts, rejects or undoes a merge of two name keys.
pub fn merge(opts: &MergeOptions) -> ExitCode {
    finish(run_merge(opts))
}

fn run_merge(opts: &MergeOptions) -> Result<()> {
    let project = open(&opts.project)?;
    let (a, b) = (opts.a.as_str(), opts.b.as_str());
    let (seq, done) = match &opts.action {
        MergeAction::Accept { name, entity_type } => {
            project.refresh_proposals()?;
            let edit = MergeEdit {
                name: name.clone(),
                entity_type: *entity_type,
            };
            (project.accept_proposal(a, b, edit)?, "Merged")
        }
        MergeAction::Reject => (project.reject_proposal(a, b)?, "Rejected"),
        MergeAction::Undo => (project.undo_merge(a, b)?, "Undone, and stored as rejected:"),
    };
    if opts.json {
        print_json(&json!({ "seq": seq }));
    } else {
        println!("{done} {a} and {b} (decision {seq})");
    }
    Ok(())
}

/// Lists an entity's mentions, moves some of them to a new entity, or puts them back.
pub fn split(opts: &SplitOptions) -> ExitCode {
    finish(run_split(opts))
}

fn run_split(opts: &SplitOptions) -> Result<()> {
    let project = open(&opts.project)?;
    match &opts.action {
        SplitAction::List(key) => {
            let rows = project.occurrences(key)?;
            if opts.json {
                print_json(&json!(
                    rows.iter()
                        .map(|r| json!({
                            "mention": r.mention_id,
                            "raw": r.raw,
                            "path": r.path,
                            "locator": format!("{:?}", r.locator),
                        }))
                        .collect::<Vec<_>>()
                ));
            } else {
                for r in &rows {
                    println!(
                        "{}  {}  {}  {:?}",
                        r.mention_id,
                        r.raw,
                        r.path.as_deref().unwrap_or("(no path)"),
                        r.locator,
                    );
                }
            }
        }
        SplitAction::Split(key, mentions) => {
            let made = project.split_mentions(key, mentions)?;
            let (seq, new_key) = (made.seq, made.key);
            if opts.json {
                print_json(&json!({ "seq": seq, "key": new_key }));
            } else {
                println!(
                    "Moved {} mentions to {new_key} (decision {seq})",
                    mentions.len()
                );
            }
        }
        SplitAction::Undo(key) => {
            let seq = project.undo_split(key)?;
            if opts.json {
                print_json(&json!({ "seq": seq }));
            } else {
                println!("Put the mentions of {key} back (decision {seq})");
            }
        }
    }
    Ok(())
}

/// The decision log, newest first, with each decision's time, before and after, and state.
pub fn decisions(opts: &DecisionsOptions) -> ExitCode {
    finish(run_decisions(opts))
}

fn status_name(d: &DecisionRow) -> &'static str {
    match d.status {
        None => "-",
        Some(DecisionStatus::Active) => "Active",
        Some(DecisionStatus::Superseded) => "Superseded",
        Some(DecisionStatus::Orphaned) => "Orphaned",
    }
}

/// A decision's `before` or `after` on one line: a split's is one mention key per line.
fn one_line(text: Option<&str>) -> String {
    let Some(text) = text else {
        return "-".to_string();
    };
    match text.lines().count() {
        0 | 1 => text.to_string(),
        n => format!("{n} mentions"),
    }
}

fn run_decisions(opts: &DecisionsOptions) -> Result<()> {
    let project = open(&opts.project)?;
    let rows = project.decisions(opts.before, opts.limit)?;
    if opts.json {
        print_json(&json!(
            rows.iter()
                .map(|d| json!({
                    "seq": d.seq,
                    "kind": d.kind.as_str(),
                    "anchor": d.anchor,
                    "before": d.before,
                    "after": d.after,
                    "decided_at": d.decided_at,
                    "status": d.status.map(|_| status_name(d)),
                }))
                .collect::<Vec<_>>()
        ));
    } else if rows.is_empty() {
        println!("No decisions yet.");
    } else {
        for d in &rows {
            println!(
                "{}  {}  {}  {}  {}  {} -> {}",
                d.seq,
                d.decided_at,
                d.kind.as_str(),
                status_name(d),
                d.anchor,
                one_line(d.before.as_deref()),
                one_line(d.after.as_deref()),
            );
        }
    }
    Ok(())
}

/// The process the decision kill test kills: makes every kind of decision on `dir`'s entities, over
/// and over, reporting each one after its commit. Stops only when killed or when nothing can be
/// decided. Not in the help.
pub fn kill_target(dir: &Path) -> ExitCode {
    finish(run_kill_target(dir))
}

fn run_kill_target(dir: &Path) -> Result<()> {
    use std::io::Write as _;

    let project = open(dir)?;
    let say = |line: String| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    say("started".to_string());
    let mut round = 0u64;
    loop {
        round += 1;
        project.refresh_proposals()?;
        let mut done = Vec::new();
        if let Some(p) = project.proposals(None, 1)?.into_iter().next() {
            done.push(project.accept_proposal(&p.a, &p.b, MergeEdit::default()));
            done.push(project.undo_merge(&p.a, &p.b));
            done.push(project.reject_proposal(&p.a, &p.b));
        }
        let entities = project.store().derived_state()?.entities;
        for key in entities.keys().filter(|k| !k.starts_with("split:")) {
            done.push(project.rename_entity(key, &format!("Name {round}")));
            done.push(project.set_entity_note(key, &format!("Note {round}")));
            done.push(project.change_entity_type(key, EntityType::Person));
            let picked: Vec<i64> = project
                .occurrences(key)?
                .iter()
                .take(1)
                .map(|o| o.mention_id)
                .collect();
            if let Ok(made) = project.split_mentions(key, &picked) {
                say(format!("decided {}", made.seq));
                done.push(project.undo_split(&made.key));
            }
            done.push(project.set_ignored(key, round % 2 == 1));
        }
        for seq in done.into_iter().flatten().filter(|seq| *seq > 0) {
            say(format!("decided {seq}"));
        }
    }
}
