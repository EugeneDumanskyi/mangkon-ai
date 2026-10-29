//! Mangkon AI desktop shell (ADR-0003). Thin: every command calls into `mk-core` (ADR-0002).
//!
//! The window loads only the embedded UI. Navigation anywhere else and new windows are refused
//! (NFR-SEC-10); the CSP and the capability files limit what the UI may do.

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod ipc;
mod nav;
mod state;

use tauri::webview::NewWindowResponse;
use tauri::{DragDropEvent, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::state::AppState;

fn main() -> std::process::ExitCode {
    // `mangkon --worker`: a parser worker (ADR-0001, ADR-0007). Before Tauri starts anything.
    if let Some(code) = mk_core::worker_main_if_requested() {
        return code;
    }
    let builder = tauri::Builder::default().manage(AppState::default());
    #[cfg(not(feature = "spike-s2d"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::app_info,
        commands::pick_folder,
        commands::open_project,
        commands::close_project,
        commands::current_project,
        commands::add_source,
        commands::progress,
        commands::connection_count,
        commands::connections_page,
        commands::occurrences,
        commands::source_context,
        commands::source_document,
        commands::page_view,
        commands::page_image,
        commands::subscribe,
        commands::record_ui_timing,
        commands::record_view_sweep
    ]);
    #[cfg(feature = "spike-s2d")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::app_info,
        commands::pick_folder,
        commands::open_project,
        commands::close_project,
        commands::current_project,
        commands::add_source,
        commands::progress,
        commands::connection_count,
        commands::connections_page,
        commands::occurrences,
        commands::source_context,
        commands::source_document,
        commands::page_view,
        commands::page_image,
        commands::subscribe,
        commands::record_ui_timing,
        commands::record_view_sweep,
        commands::synthetic_rows,
        commands::record_frame_stats
    ]);

    builder
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Mangkon AI")
                .inner_size(1200.0, 800.0)
                .min_inner_size(640.0, 480.0)
                .on_navigation(nav::allowed)
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        // Files and folders dropped on the window are added to the open project (US-01.3), in
        // Rust: the UI gets no drop paths and needs no event permission.
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let state = window.state::<AppState>();
                for path in paths {
                    if let Err(e) = state.add_source(path) {
                        state.emit(ipc::UiEvent::Error { message: e.message });
                        break;
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to start Mangkon AI");
    std::process::ExitCode::SUCCESS
}
