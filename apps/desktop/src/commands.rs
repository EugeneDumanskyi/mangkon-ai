//! IPC commands. Each one is listed in `build.rs` and granted in a capability file.

use crate::ipc;

#[tauri::command]
pub fn app_info() -> ipc::AppInfo {
    mk_core::app_info().into()
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
