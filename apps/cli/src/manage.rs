//! `projects`, `retry`, `remove` and `relocate` (M3c): the same `mk-core` services the desktop
//! calls for the project list (US-01.1, US-01.2), retry (US-02.7 AC3), removing files (US-01.5)
//! and moved sources (D12).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mk_core::{DocumentFilter, DocumentRow, Project, Workspace};
use serde_json::json;

pub enum ProjectsCommand {
    List,
    Create { name: String, location: PathBuf },
    Forget { path: PathBuf },
}

pub struct ProjectsOptions {
    pub command: ProjectsCommand,
    /// The app's data folder, which holds the project list; the OS one when `None`.
    pub app_data: Option<PathBuf>,
    pub json: bool,
}

pub enum RetryTarget {
    AllFailed,
    Paths(Vec<PathBuf>),
}

pub struct RetryOptions {
    pub project: PathBuf,
    pub target: RetryTarget,
    pub tessdata: Option<PathBuf>,
    pub json: bool,
}

pub struct RemoveOptions {
    pub project: PathBuf,
    pub paths: Vec<PathBuf>,
    pub json: bool,
}

pub struct RelocateOptions {
    pub project: PathBuf,
    pub root: PathBuf,
    pub to: PathBuf,
    pub tessdata: Option<PathBuf>,
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

fn print_json(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).expect("JSON values"));
}

/// Lists, creates or forgets projects in the app's project list.
pub fn projects(opts: &ProjectsOptions) -> ExitCode {
    finish(run_projects(opts))
}

fn run_projects(opts: &ProjectsOptions) -> Result<()> {
    let ws = match &opts.app_data {
        Some(dir) => Workspace::open(dir)?,
        None => Workspace::open_default()?,
    };
    match &opts.command {
        ProjectsCommand::List => {
            let recent = ws.recent()?;
            if opts.json {
                print_json(&json!(
                    recent
                        .iter()
                        .map(|r| json!({
                            "name": r.entry.name,
                            "path": r.entry.path,
                            "last_opened_at": r.entry.last_opened_at,
                            "present": r.present,
                        }))
                        .collect::<Vec<_>>()
                ));
            } else if recent.is_empty() {
                println!("No projects yet.");
            } else {
                for r in &recent {
                    let gone = if r.present { "" } else { "  (not found)" };
                    println!("{}  {}{gone}", r.entry.name, r.entry.path);
                }
            }
        }
        ProjectsCommand::Create { name, location } => {
            let (_, entry) = ws.create(name, location)?;
            if opts.json {
                print_json(&json!({ "name": entry.name, "path": entry.path }));
            } else {
                println!("Created {} in {}", entry.name, entry.path);
            }
        }
        ProjectsCommand::Forget { path } => {
            let entry = ws
                .entry_at(path)?
                .ok_or_else(|| format!("{} is not in the project list", path.display()))?;
            ws.forget(entry.id)?;
            if opts.json {
                print_json(&json!({ "forgotten": entry.path }));
            } else {
                println!("Removed {} from the list; its folder is kept", entry.name);
            }
        }
    }
    Ok(())
}

fn open(dir: &Path, tessdata: Option<&PathBuf>) -> Result<Project> {
    if !mk_core::project_exists(dir) {
        return Err(format!("no project in {}", dir.display()).into());
    }
    let (mut project, _) = Project::open(dir)?;
    project.set_tessdata_dir(tessdata.cloned().or_else(mk_core::tessdata_dir));
    Ok(project)
}

fn all_rows(project: &Project) -> Result<Vec<DocumentRow>> {
    let filter = DocumentFilter::default();
    let mut rows = Vec::new();
    let mut after = None;
    loop {
        let page = project.documents_page(&filter, after.as_ref(), 1_000)?;
        rows.extend(page.rows);
        match page.next {
            Some(n) => after = Some(n),
            None => return Ok(rows),
        }
    }
}

fn row_path(r: &DocumentRow) -> PathBuf {
    if r.rel_path.is_empty() {
        PathBuf::from(&r.root)
    } else {
        Path::new(&r.root).join(&r.rel_path)
    }
}

/// The rows at each of `paths` or inside it. A path that matches nothing is an error.
fn rows_at<'a>(rows: &'a [DocumentRow], paths: &[PathBuf]) -> Result<Vec<&'a DocumentRow>> {
    let mut out = Vec::new();
    for p in paths {
        let want = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let before = out.len();
        out.extend(rows.iter().filter(|r| row_path(r).starts_with(&want)));
        if out.len() == before {
            return Err(format!("{} is not in the project", p.display()).into());
        }
    }
    out.sort_by_key(|r| r.ref_id);
    out.dedup_by_key(|r| r.ref_id);
    Ok(out)
}

/// Queues failed and not-processed files again and reads them (US-02.7 AC3).
pub fn retry(opts: &RetryOptions) -> ExitCode {
    finish(run_retry(opts))
}

fn run_retry(opts: &RetryOptions) -> Result<()> {
    let project = open(&opts.project, opts.tessdata.as_ref())?;
    let queued = match &opts.target {
        RetryTarget::AllFailed => project.retry_all()?,
        RetryTarget::Paths(paths) => {
            let rows = all_rows(&project)?;
            let mut ids: Vec<i64> = rows_at(&rows, paths)?
                .iter()
                .map(|r| r.content_id)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            project.retry(&ids)?
        }
    };
    let run = project.ingest()?;
    let failed = project.failed_files(1_000)?;
    if opts.json {
        print_json(&json!({
            "queued": queued,
            "run": { "done": run.stage_a.done, "failed": run.stage_a.failed },
            "failed_files": failed.iter()
                .map(|f| json!({ "path": f.path, "reason": f.reason }))
                .collect::<Vec<_>>(),
        }));
    } else {
        println!("Queued    {queued}");
        println!(
            "This run  {} done, {} failed",
            run.stage_a.done, run.stage_a.failed
        );
    }
    Ok(())
}

/// Removes files from the project (US-01.5). The files on disk are not touched.
pub fn remove(opts: &RemoveOptions) -> ExitCode {
    finish(run_remove(opts))
}

fn run_remove(opts: &RemoveOptions) -> Result<()> {
    let project = open(&opts.project, None)?;
    let rows = all_rows(&project)?;
    let refs: Vec<i64> = rows_at(&rows, &opts.paths)?
        .iter()
        .map(|r| r.ref_id)
        .collect();
    let impact = project.removal_impact(&refs)?;
    let out = project.remove_files(&refs)?;
    let connections = project.connection_count()?;
    if opts.json {
        print_json(&json!({
            "removed": out.refs,
            "contents_deleted": out.contents,
            "connections_losing_evidence": impact,
            "connections": connections,
        }));
    } else {
        println!(
            "Removed      {} files ({} with data deleted)",
            out.refs, out.contents
        );
        println!("Connections  {impact} lost evidence, {connections} now");
    }
    Ok(())
}

/// Points a source at where it was moved, then re-scans: unchanged files are found again and
/// nothing is read again (D12).
pub fn relocate(opts: &RelocateOptions) -> ExitCode {
    finish(run_relocate(opts))
}

fn run_relocate(opts: &RelocateOptions) -> Result<()> {
    let project = open(&opts.project, opts.tessdata.as_ref())?;
    let want = std::fs::canonicalize(&opts.root).unwrap_or_else(|_| opts.root.clone());
    let root = project
        .source_roots()?
        .into_iter()
        .find(|r| Path::new(&r.path) == want || Path::new(&r.path) == opts.root)
        .ok_or_else(|| format!("{} is not a source of the project", opts.root.display()))?;
    project.relocate_root(root.id, &opts.to)?;
    let run = project.ingest()?;
    if opts.json {
        print_json(&json!({
            "root": root.id,
            "to": opts.to.display().to_string(),
            "walked": run.rescan.walked,
            "hashed": run.rescan.hashed,
            "unchanged": run.rescan.unchanged,
            "run": { "done": run.stage_a.done, "failed": run.stage_a.failed },
        }));
    } else {
        println!("Moved     {} to {}", root.path, opts.to.display());
        println!(
            "Re-scan   {} files found, {} hashed, {} unchanged",
            run.rescan.walked, run.rescan.hashed, run.rescan.unchanged
        );
        println!(
            "This run  {} done, {} failed",
            run.stage_a.done, run.stage_a.failed
        );
    }
    Ok(())
}
