//! `ingest` and `rebuild` (ADR-0002 layout, M2d) and `render` (M3b): the same `mk-core`
//! services the desktop calls.

use std::path::PathBuf;
use std::process::ExitCode;

use mk_core::{IngestSummary, PageStatus, Project, ProjectError};
use serde_json::json;

/// Failed files listed at most.
const MAX_FAILED: usize = 1_000;

pub struct IngestOptions {
    pub project: PathBuf,
    pub sources: Vec<PathBuf>,
    /// Sets the project's OCR languages (US-02.2 AC4) before reading.
    pub ocr_languages: Option<String>,
    /// The OCR language folder; the app's `tessdata/` when `None` (ADR-0018).
    pub tessdata: Option<PathBuf>,
    pub json: bool,
}

pub struct RenderOptions {
    pub project: PathBuf,
    pub target: RenderTarget,
    pub out: PathBuf,
    pub tessdata: Option<PathBuf>,
    pub json: bool,
}

/// What `render` shows: a mention's page, the first mention of a connection key, or a page of
/// a document.
pub enum RenderTarget {
    Mention(i64),
    Key(String),
    Page { document: i64, page: u32 },
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
    let (mut project, _) = match Project::open(&opts.project) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    project.set_tessdata_dir(opts.tessdata.clone().or_else(mk_core::tessdata_dir));
    if let Some(langs) = &opts.ocr_languages
        && let Err(e) = project.set_ocr_languages(langs)
    {
        return fail(&e);
    }
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
    let IngestSummary {
        rescan, stage_a, ..
    } = project.ingest()?;
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
            "added": rescan.outcome.added,
            "duplicates": rescan.outcome.duplicates,
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
        "Added        {} added, {} duplicates",
        rescan.outcome.added, rescan.outcome.duplicates
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

/// Renders a page as the page view shows it (ADR-0011) into a PNG file, and prints the page
/// view: status, size and the highlight boxes as fractions of the page. For scripted checks of
/// US-08.1 AC1–AC2. Exits 1 when the page image cannot be shown.
pub fn render(opts: &RenderOptions) -> ExitCode {
    if !mk_core::project_exists(&opts.project) {
        return fail(&format!("no project in {}", opts.project.display()));
    }
    let (mut project, _) = match Project::open(&opts.project) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };
    project.set_tessdata_dir(opts.tessdata.clone().or_else(mk_core::tessdata_dir));
    match render_page(&project, opts) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => fail(&e),
    }
}

fn render_page(
    project: &Project,
    opts: &RenderOptions,
) -> Result<bool, Box<dyn std::error::Error>> {
    let (document, page, mention) = match &opts.target {
        RenderTarget::Page { document, page } => (*document, *page, None),
        RenderTarget::Mention(id) => {
            let o = project
                .store()
                .mention(*id)?
                .ok_or_else(|| format!("no mention {id}"))?;
            let page = o.locator.page().ok_or("that mention is not on a page")?;
            (o.document_id, page.get(), Some(*id))
        }
        RenderTarget::Key(key) => {
            let o = project
                .occurrences(key)?
                .into_iter()
                .find(|o| o.locator.page().is_some())
                .ok_or_else(|| format!("no mention of {key} on a page"))?;
            let page = o.locator.page().expect("found on a page").get();
            (o.document_id, page, Some(o.mention_id))
        }
    };
    let view = project
        .page_view(document, page, mention)?
        .ok_or("that document has no pages")?;
    let shown = view.status == PageStatus::Shown;
    if shown {
        let png = project
            .page_image(document, page)?
            .ok_or("the page image is gone")?;
        std::fs::write(&opts.out, png).map_err(|e| format!("{}: {e}", opts.out.display()))?;
    }
    let status = format!("{:?}", view.status);
    if opts.json {
        let summary = json!({
            "document": document,
            "page": view.page,
            "pages": view.pages,
            "status": status,
            "width": view.width,
            "height": view.height,
            "boxes": view.boxes.iter()
                .map(|b| json!([b.x, b.y, b.width, b.height]))
                .collect::<Vec<_>>(),
            "note": view.note,
            "out": shown.then(|| opts.out.display().to_string()),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&summary).expect("JSON values")
        );
    } else {
        println!(
            "Page     {} of {} (document {document})",
            view.page, view.pages
        );
        println!("Status   {status}");
        if let Some(note) = &view.note {
            println!("Note     {note}");
        }
        if shown {
            println!(
                "Image    {} x {} px, {}",
                view.width,
                view.height,
                opts.out.display()
            );
        }
        for b in &view.boxes {
            println!(
                "Box      x {:.4} y {:.4} w {:.4} h {:.4}",
                b.x, b.y, b.width, b.height
            );
        }
    }
    Ok(shown)
}
