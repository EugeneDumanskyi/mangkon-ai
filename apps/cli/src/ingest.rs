//! `ingest` and `rebuild` (ADR-0002 layout, M2d): the same `mk-core` services the desktop
//! calls.

use std::path::PathBuf;
use std::process::ExitCode;

use mk_core::{IngestSummary, Project, ProjectError};
use serde_json::json;

/// Failed files listed at most.
const MAX_FAILED: usize = 1_000;

pub struct IngestOptions {
    pub project: PathBuf,
    pub sources: Vec<PathBuf>,
    pub json: bool,
}

pub struct RebuildOptions {
    pub project: PathBuf,
    pub json: bool,
}

fn fail(e: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::FAILURE
}

/// Opens (or creates) the project, adds each source (a source already there is kept), re-scans
/// and runs Stage A. Files that fail are listed with their reason (NFR-USE-05) and do not change
/// the exit code; only a failure of the project itself does.
pub fn ingest(opts: &IngestOptions) -> ExitCode {
    let (project, _) = match Project::open(&opts.project) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    for source in &opts.sources {
        if let Err(e) = project.add_source_root(source) {
            return fail(&e);
        }
    }
    match report_ingest(&project, opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn report_ingest(project: &Project, opts: &IngestOptions) -> Result<(), ProjectError> {
    let IngestSummary { rescan, stage_a } = project.ingest()?;
    let counts = project.counts()?;
    let connections = project.connection_count()?;
    let failed = project.failed_files(MAX_FAILED)?;
    if opts.json {
        let summary = json!({
            "project": opts.project.display().to_string(),
            "walked": rescan.walked,
            "hashed": rescan.hashed,
            "unchanged": rescan.unchanged,
            "skipped": rescan.skipped.len(),
            "not_read": rescan.errors.iter()
                .map(|(_, path, e)| json!({ "path": path, "error": e }))
                .collect::<Vec<_>>(),
            "run": { "done": stage_a.done, "failed": stage_a.failed },
            "files": {
                "total": counts.total,
                "done": counts.done,
                "failed": counts.failed,
                "queued": counts.queued,
            },
            "connections": connections,
            "failed_files": failed.iter()
                .map(|f| json!({
                    "path": f.path,
                    "file_type": f.file_type,
                    "code": f.code,
                    "reason": f.reason,
                }))
                .collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("JSON values")
        );
        return Ok(());
    }
    println!("Project      {}", opts.project.display());
    println!(
        "Re-scan      {} files found, {} read, {} unchanged, {} skipped",
        rescan.walked,
        rescan.hashed,
        rescan.unchanged,
        rescan.skipped.len()
    );
    println!(
        "This run     {} done, {} failed",
        stage_a.done, stage_a.failed
    );
    println!(
        "Project      {} files: {} done, {} failed, {} queued",
        counts.total, counts.done, counts.failed, counts.queued
    );
    println!("Connections  {connections}");
    for (_, path, e) in &rescan.errors {
        println!("Not read     {path}: {e}");
    }
    if !failed.is_empty() {
        println!("\nFailed files:");
        for f in &failed {
            println!(
                "  {}: {}",
                f.path.as_deref().unwrap_or("(no longer there)"),
                f.reason.as_deref().unwrap_or("no reason recorded")
            );
        }
    }
    Ok(())
}

/// Rebuilds every derived row (ADR-0005) and reports whether they equal the incremental state.
/// Fails when the project does not exist or the rebuilt rows do not match.
pub fn rebuild(opts: &RebuildOptions) -> ExitCode {
    if !mk_core::project_exists(&opts.project) {
        return fail(&format!("no project in {}", opts.project.display()));
    }
    let result = Project::open(&opts.project).and_then(|(p, _)| p.rebuild());
    let r = match result {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    if opts.json {
        let summary = json!({
            "project": opts.project.display().to_string(),
            "identifiers": r.identifiers,
            "connections": r.connections,
            "same_as_incremental": r.same_as_incremental,
            "consistent": r.consistent,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("JSON values")
        );
    } else {
        let yes = |b: bool| if b { "yes" } else { "no" };
        println!("Project              {}", opts.project.display());
        println!("Identifiers          {}", r.identifiers);
        println!("Connections          {}", r.connections);
        println!("Same as before       {}", yes(r.same_as_incremental));
        println!("Matches the sources  {}", yes(r.consistent));
    }
    if r.same_as_incremental && r.consistent {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
