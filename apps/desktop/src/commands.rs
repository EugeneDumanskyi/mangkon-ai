//! IPC commands. Each one is listed in `build.rs` and granted in a capability file.
//!
//! Project commands are `async`, so Tauri runs them off the main thread: a read never holds up
//! the window.

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::State;

use crate::ipc::{self, IpcError};
use crate::state::{AppState, Target};

/// Most rows one `connections_page` or `documents_page` call returns.
const MAX_PAGE: u32 = 1_000;

#[tauri::command]
pub fn app_info() -> ipc::AppInfo {
    mk_core::app_info().into()
}

/// Native dialogs use the async `rfd` API. The blocking one waits for GTK's main context, which
/// the Tauri event loop owns, so calling it from the main thread deadlocks (window frozen, then
/// gone). The async one lets the event loop keep running. The UI gets no file-system permission
/// (ADR-0003).
/// The native folder dialog. `null` when the user cancels.
#[tauri::command]
pub async fn pick_folder(title: String) -> Result<Option<String>, IpcError> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await;
    Ok(picked.map(|h| h.path().display().to_string()))
}

/// The native file dialog, several files at once (Add files…). Empty when the user cancels.
#[tauri::command]
pub async fn pick_files(title: String) -> Result<Vec<String>, IpcError> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_files()
        .await;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.path().display().to_string())
        .collect())
}

fn info(state: &AppState) -> Result<ipc::ProjectInfo, IpcError> {
    let open = state.current()?;
    Ok(ipc::ProjectInfo {
        id: open.entry.id,
        name: open.entry.name.clone(),
        path: open.dir.display().to_string(),
        sources: open
            .project
            .store()
            .source_roots()
            .map_err(mk_core::ProjectError::from)?
            .into_iter()
            .map(|r| r.path)
            .collect(),
    })
}

/// The recent projects, newest first (US-01.2 AC1).
#[tauri::command]
pub async fn recent_projects(
    state: State<'_, AppState>,
) -> Result<Vec<ipc::RecentProject>, IpcError> {
    let recent = state.workspace()?.recent()?;
    Ok(recent.into_iter().map(Into::into).collect())
}

/// Creates the project `name` in a new folder in `location` and opens it (US-01.1).
#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    name: String,
    location: String,
) -> Result<ipc::ProjectInfo, IpcError> {
    state.create(&name, &PathBuf::from(location))?;
    info(&state)
}

/// Opens the project in `path` (Open…), or the recent project `id` (US-01.2). The open project,
/// if any, is closed first (gap A7).
#[tauri::command]
pub async fn open_project(
    state: State<'_, AppState>,
    path: Option<String>,
    id: Option<i64>,
) -> Result<ipc::ProjectInfo, IpcError> {
    let path = path.map(PathBuf::from);
    let target = match (&path, id) {
        (Some(p), None) => Target::Folder(p),
        (None, Some(id)) => Target::Entry(id),
        _ => return Err(IpcError::new("internal", "Give a path or a project id.")),
    };
    state.open(target)?;
    info(&state)
}

/// "Locate…" for a recent project whose folder moved (US-01.2 AC2).
#[tauri::command]
pub async fn locate_project(
    state: State<'_, AppState>,
    id: i64,
    path: String,
) -> Result<ipc::RecentProject, IpcError> {
    let ws = state.workspace()?;
    let entry = ws.locate(id, &PathBuf::from(path))?;
    Ok(mk_core::RecentProject {
        entry,
        present: true,
    }
    .into())
}

/// "Remove from list" (US-01.2 AC2). The folder is not touched.
#[tauri::command]
pub async fn forget_project(state: State<'_, AppState>, id: i64) -> Result<(), IpcError> {
    Ok(state.workspace()?.forget(id)?)
}

#[tauri::command]
pub async fn close_project(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.close();
    Ok(())
}

/// The open project, or `null`.
#[tauri::command]
pub async fn current_project(
    state: State<'_, AppState>,
) -> Result<Option<ipc::ProjectInfo>, IpcError> {
    match state.current() {
        Ok(_) => info(&state).map(Some),
        Err(_) => Ok(None),
    }
}

/// Adds a folder or file and starts reading it (US-01.3).
#[tauri::command]
pub async fn add_source(state: State<'_, AppState>, path: String) -> Result<(), IpcError> {
    state.add_source(&PathBuf::from(path))
}

/// Re-scans the sources (US-01.4).
#[tauri::command]
pub async fn rescan(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.rescan()
}

/// Pauses the queue; returns once no file is being read (US-03.2 AC1, NFR-REL-05).
#[tauri::command]
pub async fn pause(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.pause()
}

#[tauri::command]
pub async fn resume(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.resume()
}

/// Cancels the queue (US-03.3). Returns how many files are now Not processed.
#[tauri::command]
pub async fn cancel(state: State<'_, AppState>) -> Result<u32, IpcError> {
    state.cancel()
}

/// The progress panel (US-03.1).
#[tauri::command]
pub async fn progress(state: State<'_, AppState>) -> Result<ipc::Progress, IpcError> {
    let open = state.current()?;
    Ok(ipc::Progress::new(
        open.project.counts()?,
        open.project.progress_view()?,
    ))
}

/// One page of the documents list after `cursor`, in path order (US-02.8).
#[tauri::command]
pub async fn documents_page(
    state: State<'_, AppState>,
    filter: ipc::DocumentFilter,
    cursor: Option<ipc::DocumentCursor>,
    limit: u32,
) -> Result<ipc::DocumentsPage, IpcError> {
    let after = cursor.map(mk_core::DocumentCursor::from);
    let page = state.current()?.project.documents_page(
        &filter.into(),
        after.as_ref(),
        limit.min(MAX_PAGE) as usize,
    )?;
    Ok(page.into())
}

#[tauri::command]
pub async fn documents_count(
    state: State<'_, AppState>,
    filter: ipc::DocumentFilter,
) -> Result<u32, IpcError> {
    let n = state.current()?.project.documents_count(&filter.into())?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The file types in the project, for the type filter (US-02.8 AC2).
#[tauri::command]
pub async fn document_types(
    state: State<'_, AppState>,
) -> Result<Vec<ipc::DocumentType>, IpcError> {
    let types = state.current()?.project.document_types()?;
    Ok(types
        .into_iter()
        .map(|(file_type, n)| ipc::DocumentType {
            file_type,
            count: u32::try_from(n).unwrap_or(u32::MAX),
        })
        .collect())
}

/// Reads failed or not-processed files again: these `content_ids`, or all of them when `null`
/// (US-02.7 AC3). Returns how many were queued.
#[tauri::command]
pub async fn retry(
    state: State<'_, AppState>,
    content_ids: Option<Vec<i64>>,
) -> Result<u32, IpcError> {
    let open = state.current()?;
    let n = match content_ids {
        Some(ids) => open.project.retry(&ids)?,
        None => open.project.retry_all()?,
    };
    state.start_ingest(&open);
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// How many connections lose evidence if these paths are removed (US-01.5 AC2).
#[tauri::command]
pub async fn removal_impact(
    state: State<'_, AppState>,
    ref_ids: Vec<i64>,
) -> Result<u32, IpcError> {
    let n = state.current()?.project.removal_impact(&ref_ids)?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Removes paths from the project (US-01.5 AC1). The files on disk are not touched.
#[tauri::command]
pub async fn remove_files(
    state: State<'_, AppState>,
    ref_ids: Vec<i64>,
) -> Result<ipc::RemoveSummary, IpcError> {
    let out = state.current()?.project.remove_files(&ref_ids)?;
    state.emit(ipc::UiEvent::ProgressChanged);
    Ok(out.into())
}

/// The settings and the installed OCR languages (US-01.6).
#[tauri::command]
pub async fn project_settings(state: State<'_, AppState>) -> Result<ipc::SettingsInfo, IpcError> {
    let open = state.current()?;
    Ok(ipc::SettingsInfo {
        settings: open.project.settings()?.into(),
        installed_ocr_languages: open.project.installed_ocr_languages()?,
    })
}

/// Saves the settings; says what files already read they change (INV-WS-07).
#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    settings: ipc::Settings,
) -> Result<ipc::SettingsSaved, IpcError> {
    let r = state.current()?.project.save_settings(settings.into())?;
    Ok(ipc::SettingsSaved {
        reprocess_ocr: r.ocr,
        rerun_identifiers: r.identifiers,
    })
}

/// "Re-process affected files now?" (US-01.6 AC3): with `ocr`, queues the files the OCR
/// languages change; with `identifiers`, finds identifiers again from the stored text (the
/// phone region changed, US-04.2 AC2). Then runs. Returns how many files were queued.
#[tauri::command]
pub async fn reprocess(
    state: State<'_, AppState>,
    ocr: bool,
    identifiers: bool,
) -> Result<u32, IpcError> {
    let open = state.current()?;
    let n = open.project.reprocess(mk_core::Reprocess {
        ocr,
        identifiers,
        custom: false,
    })?;
    state.start_ingest(&open);
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The source folders and files, each with whether it is there now.
#[tauri::command]
pub async fn source_roots(state: State<'_, AppState>) -> Result<Vec<ipc::SourceRoot>, IpcError> {
    let roots = state.current()?.project.source_roots()?;
    Ok(roots.into_iter().map(Into::into).collect())
}

/// "Locate…" for a source that moved (D12): points it at `path` and re-scans; unchanged files
/// are not read again.
#[tauri::command]
pub async fn relocate_root(
    state: State<'_, AppState>,
    root: i64,
    path: String,
) -> Result<(), IpcError> {
    let open = state.current()?;
    open.project.relocate_root(root, &PathBuf::from(path))?;
    state.emit(ipc::UiEvent::ProjectChanged);
    state.start_ingest(&open);
    Ok(())
}

/// Removes a source with every path in it (US-01.5).
#[tauri::command]
pub async fn remove_root(
    state: State<'_, AppState>,
    root: i64,
) -> Result<ipc::RemoveSummary, IpcError> {
    let out = state.current()?.project.remove_root(root)?;
    state.emit(ipc::UiEvent::ProjectChanged);
    Ok(out.into())
}

/// The custom patterns with their match counts (US-04.7 AC4).
#[tauri::command]
pub async fn custom_patterns(
    state: State<'_, AppState>,
) -> Result<Vec<ipc::CustomPatternItem>, IpcError> {
    let list = state.current()?.project.custom_patterns()?;
    Ok(list.into_iter().map(Into::into).collect())
}

/// Saves a new pattern and starts its re-run on the stored text (US-04.7 AC3).
#[tauri::command]
pub async fn add_pattern(
    state: State<'_, AppState>,
    name: String,
    expression: String,
) -> Result<(), IpcError> {
    let open = state.current()?;
    open.project.add_pattern(&name, &expression)?;
    state.start_ingest(&open);
    Ok(())
}

/// Changes a pattern's expression; its name stays.
#[tauri::command]
pub async fn edit_pattern(
    state: State<'_, AppState>,
    name: String,
    expression: String,
) -> Result<(), IpcError> {
    let open = state.current()?;
    open.project.edit_pattern(&name, &expression)?;
    state.start_ingest(&open);
    Ok(())
}

#[tauri::command]
pub async fn remove_pattern(state: State<'_, AppState>, name: String) -> Result<(), IpcError> {
    let open = state.current()?;
    open.project.remove_pattern(&name)?;
    state.start_ingest(&open);
    Ok(())
}

/// The live test box (US-04.7 AC1/AC2); needs no project and saves nothing.
#[tauri::command]
pub async fn test_pattern(expression: String, sample: String) -> ipc::PatternTest {
    mk_core::test_pattern(&expression, &sample).into()
}

/// The mapping screen of a CSV or JSON file (US-05.1 AC1).
#[tauri::command]
pub async fn mapping_view(
    state: State<'_, AppState>,
    ref_id: i64,
) -> Result<Option<ipc::MappingView>, IpcError> {
    Ok(state
        .current()?
        .project
        .mapping_view(ref_id)?
        .map(Into::into))
}

/// Confirms and applies a file's column types (US-05.2 AC1).
#[tauri::command]
pub async fn confirm_mapping(
    state: State<'_, AppState>,
    ref_id: i64,
    assignments: Vec<ipc::ColumnAssignment>,
) -> Result<(), IpcError> {
    let assignments = ipc::assignments(assignments)?;
    state
        .current()?
        .project
        .confirm_mapping(ref_id, assignments)?;
    state.emit(ipc::UiEvent::ProjectChanged);
    Ok(())
}

/// "Use the same mapping as <file>" (US-05.2 AC2).
#[tauri::command]
pub async fn reuse_mapping(
    state: State<'_, AppState>,
    ref_id: i64,
    from_ref_id: i64,
) -> Result<(), IpcError> {
    state
        .current()?
        .project
        .reuse_mapping(ref_id, from_ref_id)?;
    state.emit(ipc::UiEvent::ProjectChanged);
    Ok(())
}

#[tauri::command]
pub async fn connection_count(
    state: State<'_, AppState>,
    filter: ipc::ConnectionFilter,
) -> Result<u32, IpcError> {
    let n = state
        .current()?
        .project
        .connection_count_for(&filter.into())?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Counts for the empty states and the "Show ignored" label (US-07.4, US-07.6).
#[tauri::command]
pub async fn connections_summary(
    state: State<'_, AppState>,
) -> Result<ipc::ConnectionsSummary, IpcError> {
    Ok(state.current()?.project.connections_summary()?.into())
}

/// One page of connections that match `filter`, in its order, after `cursor` (keyset paging,
/// ADR-0003).
#[tauri::command]
pub async fn connections_page(
    state: State<'_, AppState>,
    filter: ipc::ConnectionFilter,
    cursor: Option<ipc::ConnectionCursor>,
    limit: u32,
) -> Result<ipc::ConnectionsPage, IpcError> {
    let after = cursor.map(mk_core::ListCursor::from);
    let page = state.current()?.project.connections_page(
        &filter.into(),
        after.as_ref(),
        limit.min(MAX_PAGE) as usize,
    )?;
    Ok(page.into())
}

/// Ignores or un-ignores a connection (US-07.4). The list refreshes through the
/// `connectionsChanged` event.
#[tauri::command]
pub async fn set_ignored(
    state: State<'_, AppState>,
    key: String,
    ignored: bool,
) -> Result<(), IpcError> {
    state.current()?.project.set_ignored(&key, ignored)?;
    Ok(())
}

/// The mentions of one connection's identifier (US-07.3 AC2).
#[tauri::command]
pub async fn occurrences(
    state: State<'_, AppState>,
    key: String,
) -> Result<Vec<ipc::Occurrence>, IpcError> {
    let rows = state.current()?.project.occurrences(&key)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// The source around one mention, the hit split out (US-08.1 AC3–AC6).
/// A source view with its document's card-like numbers, masked (US-04.6 AC1).
fn with_card_flags(
    project: &mk_core::Project,
    ctx: Option<mk_core::SourceContext>,
) -> Result<Option<ipc::SourceView>, IpcError> {
    let Some(ctx) = ctx else { return Ok(None) };
    let mut view = ipc::SourceView::from(ctx);
    view.document.card_flags = project
        .card_flags(view.document.document_id)?
        .iter()
        .map(Into::into)
        .collect();
    Ok(Some(view))
}

#[tauri::command]
pub async fn source_context(
    state: State<'_, AppState>,
    mention_id: i64,
) -> Result<ipc::SourceView, IpcError> {
    let open = state.current()?;
    let ctx = open.project.source_context(mention_id)?;
    with_card_flags(&open.project, ctx)?.ok_or_else(|| {
        IpcError::new(
            "gone",
            "This place is no longer in the project. The file may have changed.",
        )
    })
}

/// One page of keyword search results after `cursor`, best first (US-06.1 AC1–AC3). The
/// query text is never logged.
#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    query: ipc::SearchQuery,
    cursor: Option<ipc::SearchCursor>,
    limit: u32,
) -> Result<ipc::SearchPage, IpcError> {
    let after = cursor.map(mk_core::SearchCursor::from);
    let page = state.current()?.project.search(
        &query.text,
        &query.filter(),
        after.as_ref(),
        limit.min(MAX_PAGE) as usize,
    )?;
    Ok(page.into())
}

/// The source around a search hit, the matched words marked (US-06.1 AC4).
#[tauri::command]
pub async fn search_context(
    state: State<'_, AppState>,
    segment_id: i64,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<ipc::SourceView, IpcError> {
    let span = start.zip(end).map(|(s, e)| (s as usize, e as usize));
    let open = state.current()?;
    let ctx = open.project.segment_context(segment_id, span)?;
    with_card_flags(&open.project, ctx)?.ok_or_else(|| {
        IpcError::new(
            "gone",
            "This place is no longer in the project. The file may have changed.",
        )
    })
}

/// A document from its start: an attachment, or the email it came in (US-08.1 AC4).
#[tauri::command]
pub async fn source_document(
    state: State<'_, AppState>,
    document_id: i64,
) -> Result<ipc::SourceView, IpcError> {
    let open = state.current()?;
    let ctx = open.project.document_view(document_id)?;
    with_card_flags(&open.project, ctx)?.ok_or_else(|| {
        IpcError::new(
            "gone",
            "This document is no longer in the project. The file may have changed.",
        )
    })
}

/// One page of a PDF or image document, with the boxes of `mention_id` or of a search `hit`
/// when it is on that page (US-08.1 AC1–AC2, US-06.1 AC4). Renders the page on first view
/// (render worker, page cache).
#[tauri::command]
pub async fn page_view(
    state: State<'_, AppState>,
    document_id: i64,
    page: u32,
    mention_id: Option<i64>,
    hit: Option<ipc::SegmentHit>,
) -> Result<ipc::PageView, IpcError> {
    let current = state.current()?;
    let view = match (mention_id, hit) {
        (None, Some(h)) => {
            let hit = mk_core::SegmentHit::new(h.segment_id, h.start as usize, h.end as usize);
            current.project.page_view_at(document_id, page, hit)?
        }
        _ => current.project.page_view(document_id, page, mention_id)?,
    };
    view.map(Into::into)
        .ok_or_else(|| IpcError::new("gone", "This page is no longer in the project."))
}

/// The PNG of a page `page_view` has shown, as raw bytes (an `ArrayBuffer` in the UI, no JSON).
#[tauri::command]
pub async fn page_image(
    state: State<'_, AppState>,
    document_id: i64,
    page: u32,
) -> Result<tauri::ipc::Response, IpcError> {
    state
        .current()?
        .project
        .page_image(document_id, page)?
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| IpcError::new("gone", "This page cannot be shown."))
}

/// The original file of a document that is still there (US-08.6). The UI names a document,
/// never a path, so it can only open files of the project.
fn original(state: &AppState, document_id: i64) -> Result<PathBuf, IpcError> {
    state
        .current()?
        .project
        .original_path(document_id)?
        .ok_or_else(|| {
            IpcError::new(
                "missing",
                "The original file is not where it was read from.",
            )
        })
}

fn os_error(e: std::io::Error) -> IpcError {
    IpcError::new("os", format!("The system could not open it: {e}"))
}

/// "Open with default app" (US-08.6).
#[tauri::command]
pub async fn open_original(state: State<'_, AppState>, document_id: i64) -> Result<(), IpcError> {
    let path = original(&state, document_id)?;
    crate::os_open::open(path.as_os_str()).map_err(os_error)
}

/// "Show in folder" (US-08.6).
#[tauri::command]
pub async fn show_in_folder(state: State<'_, AppState>, document_id: i64) -> Result<(), IpcError> {
    let path = original(&state, document_id)?;
    crate::os_open::reveal(&path).map_err(os_error)
}

/// The check `open_link` makes again on the Rust side (US-08.4 AC3): `http`, `https` or
/// `mailto`, no control characters, at most 2,048 characters.
pub fn check_link(url: &str) -> Result<(), IpcError> {
    if mk_core::openable_url(url) {
        Ok(())
    } else {
        Err(IpcError::new("refused", "This link cannot be opened."))
    }
}

/// Opens a link from a document in the system browser or mail app, after the UI has shown the
/// full URL and the user confirmed (US-08.4 AC3).
#[tauri::command]
pub async fn open_link(url: String) -> Result<(), IpcError> {
    check_link(&url)?;
    crate::os_open::open(std::ffi::OsStr::new(&url)).map_err(os_error)
}

/// Sets the channel core events go to (`UiEvent`). Replaces the previous one.
#[tauri::command]
pub async fn subscribe(
    state: State<'_, AppState>,
    channel: Channel<ipc::UiEvent>,
) -> Result<(), IpcError> {
    state.subscribe(channel);
    if ipc::ui_timing_enabled() {
        state.scripted_run_once();
    }
    Ok(())
}

/// Writes the UI's update timings to `MK_UI_TIMING_OUT` (default: `mangkon-ui-timing.json` in
/// the temp folder) and returns the path. Refused unless `MK_UI_TIMING=1`.
#[tauri::command]
pub async fn record_ui_timing(timing: ipc::UiTiming) -> Result<String, IpcError> {
    if !ipc::ui_timing_enabled() {
        return Err(IpcError::new("disabled", "UI timing is off."));
    }
    let path = std::env::var_os("MK_UI_TIMING_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mangkon-ui-timing.json"));
    let json = serde_json::to_string_pretty(&timing)
        .map_err(|e| IpcError::new("internal", e.to_string()))?;
    std::fs::write(&path, json)
        .map_err(|e| IpcError::new("io", format!("{}: {e}", path.display())))?;
    Ok(path.display().to_string())
}

/// Writes one view sweep to `MK_VIEW_SWEEP_OUT` (default: `mangkon-view-sweep.json` in the
/// temp folder), then exits the app. Refused unless the sweep is on.
#[tauri::command]
pub fn record_view_sweep<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    sweep: ipc::ViewSweep,
) -> Result<String, IpcError> {
    if !ipc::view_sweep_enabled() && ipc::click_sweep().is_none() {
        return Err(IpcError::new("disabled", "The view sweep is off."));
    }
    let path = std::env::var_os("MK_VIEW_SWEEP_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mangkon-view-sweep.json"));
    let json = serde_json::to_string_pretty(&sweep)
        .map_err(|e| IpcError::new("internal", e.to_string()))?;
    std::fs::write(&path, json)
        .map_err(|e| IpcError::new("io", format!("{}: {e}", path.display())))?;
    app.exit(0);
    Ok(path.display().to_string())
}

/// Writes one queue walk phase to `MK_QUEUE_WALK_OUT` (default: `mangkon-queue-walk-<phase>.json`
/// in the temp folder), then exits the app, which stops the run as a quit does. Refused unless
/// the walk is on.
#[tauri::command]
pub fn record_queue_walk<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    walk: ipc::QueueWalk,
) -> Result<String, IpcError> {
    if ipc::queue_walk_phase().is_none() {
        return Err(IpcError::new("disabled", "The queue walk is off."));
    }
    let path = std::env::var_os("MK_QUEUE_WALK_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("mangkon-queue-walk-{}.json", walk.phase))
        });
    let json = serde_json::to_string_pretty(&walk)
        .map_err(|e| IpcError::new("internal", e.to_string()))?;
    std::fs::write(&path, json)
        .map_err(|e| IpcError::new("io", format!("{}: {e}", path.display())))?;
    app.exit(0);
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-08.4 AC3: `open_link` refuses anything but a plain web or mail URL, whatever the UI
    /// sends.
    #[test]
    fn us_08_4_ac3_open_link_refuses_other_urls() {
        assert!(check_link("https://example.org/a?b=1").is_ok());
        assert!(check_link("mailto:anna@x.example").is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "/usr/bin/xterm",
            "--help",
            "https://a.example/\u{0}",
            "https://a.example/ x",
            "tauri://localhost/",
        ] {
            let e = check_link(bad).unwrap_err();
            assert_eq!(e.code, "refused", "{bad}");
        }
        let long = format!("https://a.example/{}", "x".repeat(2_048));
        assert!(check_link(&long).is_err());
    }
}

#[cfg(feature = "spike-s2d")]
#[tauri::command]
pub fn synthetic_rows(request: ipc::RowsRequest) -> ipc::RowPage {
    mk_core::spike_s2d::synthetic_rows(request.after_key, request.limit).into()
}

/// Writes one scroll run to `MK_S2D_OUT` (default: `mangkon-s2d.json` in the temp folder) and
/// returns the path. Exits the app afterwards if `MK_S2D_EXIT=1`, for scripted runs.
#[cfg(feature = "spike-s2d")]
#[tauri::command]
pub fn record_frame_stats<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    stats: ipc::FrameStats,
) -> Result<String, String> {
    let path = std::env::var_os("MK_S2D_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mangkon-s2d.json"));
    mk_core::spike_s2d::write_frame_stats(&path, &stats.into())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if std::env::var_os("MK_S2D_EXIT").is_some_and(|v| v == "1") {
        app.exit(0);
    }
    Ok(path.display().to_string())
}
