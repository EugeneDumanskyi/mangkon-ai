//! Mangkon AI desktop shell (ADR-0003). Thin: every command calls into `mk-core` (ADR-0002).
//!
//! The window loads only the embedded UI. Navigation anywhere else and new windows are refused
//! (NFR-SEC-10); the CSP and the capability files limit what the UI may do.

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod ipc;
mod nav;
mod os_open;
mod state;

use tauri::webview::NewWindowResponse;
use tauri::{DragDropEvent, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::state::AppState;

fn main() -> std::process::ExitCode {
    // `mangkon --worker`: a parser worker (ADR-0001, ADR-0007). Before Tauri starts anything.
    if let Some(code) = mk_core::worker_main_if_requested() {
        return code;
    }
    // Linux: once more with glibc's malloc tunables, so freed memory goes back (NFR-RES-01).
    mk_core::restart_with_malloc_tunables();
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
        commands::connections_summary,
        commands::set_ignored,
        commands::occurrences,
        commands::source_context,
        commands::source_document,
        commands::search,
        commands::search_context,
        commands::page_view,
        commands::page_image,
        commands::open_original,
        commands::show_in_folder,
        commands::open_link,
        commands::pick_files,
        commands::recent_projects,
        commands::create_project,
        commands::locate_project,
        commands::forget_project,
        commands::rescan,
        commands::pause,
        commands::resume,
        commands::cancel,
        commands::documents_page,
        commands::documents_count,
        commands::document_types,
        commands::retry,
        commands::removal_impact,
        commands::remove_files,
        commands::project_settings,
        commands::save_settings,
        commands::reprocess,
        commands::source_roots,
        commands::relocate_root,
        commands::remove_root,
        commands::custom_patterns,
        commands::add_pattern,
        commands::edit_pattern,
        commands::remove_pattern,
        commands::test_pattern,
        commands::mapping_view,
        commands::confirm_mapping,
        commands::reuse_mapping,
        commands::endpoints_list,
        commands::endpoint_save,
        commands::endpoint_delete,
        commands::endpoint_test,
        commands::resolve_destination,
        commands::secret_store_status,
        commands::project_endpoints,
        commands::endpoint_allow_prepare,
        commands::endpoint_allow_confirm,
        commands::endpoint_revoke,
        commands::egress_records,
        commands::subscribe,
        commands::record_ui_timing,
        commands::record_view_sweep,
        commands::record_queue_walk
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
        commands::connections_summary,
        commands::set_ignored,
        commands::occurrences,
        commands::source_context,
        commands::source_document,
        commands::search,
        commands::search_context,
        commands::page_view,
        commands::page_image,
        commands::open_original,
        commands::show_in_folder,
        commands::open_link,
        commands::pick_files,
        commands::recent_projects,
        commands::create_project,
        commands::locate_project,
        commands::forget_project,
        commands::rescan,
        commands::pause,
        commands::resume,
        commands::cancel,
        commands::documents_page,
        commands::documents_count,
        commands::document_types,
        commands::retry,
        commands::removal_impact,
        commands::remove_files,
        commands::project_settings,
        commands::save_settings,
        commands::reprocess,
        commands::source_roots,
        commands::relocate_root,
        commands::remove_root,
        commands::custom_patterns,
        commands::add_pattern,
        commands::edit_pattern,
        commands::remove_pattern,
        commands::test_pattern,
        commands::mapping_view,
        commands::confirm_mapping,
        commands::reuse_mapping,
        commands::endpoints_list,
        commands::endpoint_save,
        commands::endpoint_delete,
        commands::endpoint_test,
        commands::resolve_destination,
        commands::secret_store_status,
        commands::project_endpoints,
        commands::endpoint_allow_prepare,
        commands::endpoint_allow_confirm,
        commands::endpoint_revoke,
        commands::egress_records,
        commands::subscribe,
        commands::record_ui_timing,
        commands::record_view_sweep,
        commands::record_queue_walk,
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
        .build(tauri::generate_context!())
        .expect("failed to start Mangkon AI")
        // Quitting stops the run and keeps the queue's state (US-03.4 AC1).
        .run(|app, event| {
            if let RunEvent::Exit = event {
                app.state::<AppState>().close();
            }
        });
    std::process::ExitCode::SUCCESS
}
