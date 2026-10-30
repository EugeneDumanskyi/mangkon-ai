//! IPC commands. Each one is listed in `build.rs` and granted in a capability file.
//!
//! Project commands are `async`, so Tauri runs them off the main thread: a read never holds up
//! the window, and `pick_folder` can wait for the dialog it runs on the main thread.

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::ipc::{self, IpcError};
use crate::state::{AppState, Target};

/// Most rows one `connections_page` or `documents_page` call returns.
const MAX_PAGE: u32 = 1_000;

#[tauri::command]
pub fn app_info() -> ipc::AppInfo {
    mk_core::app_info().into()
}

/// Runs a native dialog (`rfd`, GTK 3 on Linux) on the main thread with the main window as its
/// parent, and waits for it off the main thread. The UI gets no file-system permission
/// (ADR-0003).
async fn dialog<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    show: impl FnOnce(rfd::FileDialog) -> Option<T> + Send + 'static,
) -> Result<Option<T>, IpcError> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| IpcError::new("internal", "The main window is gone."))?;
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(show(rfd::FileDialog::new().set_parent(&window)));
    })
    .map_err(|e| IpcError::new("internal", e.to_string()))?;
    tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| IpcError::new("internal", e.to_string()))
}

/// The native folder dialog. `null` when the user cancels.
#[tauri::command]
pub async fn pick_folder<R: Runtime>(
    app: AppHandle<R>,
    title: String,
) -> Result<Option<String>, IpcError> {
    let picked = dialog(&app, move |d| d.set_title(title).pick_folder()).await?;
    Ok(picked.map(|p| p.display().to_string()))
}

/// The native file dialog, several files at once (Add files…). Empty when the user cancels.
#[tauri::command]
pub async fn pick_files<R: Runtime>(
    app: AppHandle<R>,
    title: String,
) -> Result<Vec<String>, IpcError> {
    let picked = dialog(&app, move |d| d.set_title(title).pick_files()).await?;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.display().to_string())
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
    })
}

/// "Re-process affected files now?": queues the files the OCR languages change and reads them
/// (US-01.6 AC3). Returns how many were queued.
#[tauri::command]
pub async fn reprocess(state: State<'_, AppState>) -> Result<u32, IpcError> {
    let open = state.current()?;
    let n = open.project.reprocess(mk_core::Reprocess { ocr: true })?;
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

#[tauri::command]
pub async fn connection_count(state: State<'_, AppState>) -> Result<u32, IpcError> {
    let n = state.current()?.project.connection_count()?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// One page of connections in ranking order after `cursor` (keyset paging, ADR-0003).
#[tauri::command]
pub async fn connections_page(
    state: State<'_, AppState>,
    cursor: Option<ipc::ConnectionCursor>,
    limit: u32,
) -> Result<ipc::ConnectionsPage, IpcError> {
    let after = cursor.map(mk_core::RankKey::from);
    let page = state
        .current()?
        .project
        .connections_page(after.as_ref(), limit.min(MAX_PAGE) as usize)?;
    Ok(page.into())
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
#[tauri::command]
pub async fn source_context(
    state: State<'_, AppState>,
    mention_id: i64,
) -> Result<ipc::SourceView, IpcError> {
    state
        .current()?
        .project
        .source_context(mention_id)?
        .map(Into::into)
        .ok_or_else(|| {
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
    state
        .current()?
        .project
        .document_view(document_id)?
        .map(Into::into)
        .ok_or_else(|| {
            IpcError::new(
                "gone",
                "This document is no longer in the project. The file may have changed.",
            )
        })
}

/// One page of a PDF or image document, with the boxes of `mention_id` when it is on that
/// page (US-08.1 AC1–AC2). Renders the page on first view (render worker, page cache).
#[tauri::command]
pub async fn page_view(
    state: State<'_, AppState>,
    document_id: i64,
    page: u32,
    mention_id: Option<i64>,
) -> Result<ipc::PageView, IpcError> {
    state
        .current()?
        .project
        .page_view(document_id, page, mention_id)?
        .map(Into::into)
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
    if !ipc::view_sweep_enabled() {
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
