//! IPC commands. Each one is listed in `build.rs` and granted in a capability file.
//!
//! Project commands are `async`, so Tauri runs them off the main thread: a read never holds up
//! the window, and `pick_folder` can wait for the dialog it runs on the main thread.

use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::ipc::{self, IpcError};
use crate::state::AppState;

/// Most rows one `connections_page` call returns.
const MAX_PAGE: u32 = 1_000;

#[tauri::command]
pub fn app_info() -> ipc::AppInfo {
    mk_core::app_info().into()
}

/// The native folder dialog (`rfd`, GTK 3 on Linux), run on the main thread with the main window
/// as its parent. `null` when the user cancels. The UI gets no file-system permission
/// (ADR-0003).
#[tauri::command]
pub async fn pick_folder<R: Runtime>(
    app: AppHandle<R>,
    title: String,
) -> Result<Option<String>, IpcError> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| IpcError::new("internal", "The main window is gone."))?;
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let picked = rfd::FileDialog::new()
            .set_title(title)
            .set_parent(&window)
            .pick_folder();
        let _ = tx.send(picked);
    })
    .map_err(|e| IpcError::new("internal", e.to_string()))?;
    let picked = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| IpcError::new("internal", e.to_string()))?;
    Ok(picked.map(|p| p.display().to_string()))
}

fn info(state: &AppState) -> Result<ipc::ProjectInfo, IpcError> {
    let open = state.current()?;
    Ok(ipc::ProjectInfo {
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

/// Opens the project in `path`, creating it if the folder is new or empty (gap A7: the open
/// project, if any, is closed first).
#[tauri::command]
pub async fn open_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<ipc::ProjectInfo, IpcError> {
    state.open(&PathBuf::from(path))?;
    info(&state)
}

#[tauri::command]
pub async fn close_project(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.close()
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

#[tauri::command]
pub async fn progress(state: State<'_, AppState>) -> Result<ipc::Progress, IpcError> {
    let open = state.current()?;
    Ok(ipc::Progress::new(
        open.project.counts()?,
        open.is_ingesting(),
    ))
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

/// The source around one mention, the hit split out (US-08.1 AC3).
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
