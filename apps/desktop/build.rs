//! Generates the permission for each app command, so the webview can call only the commands a
//! capability file grants (ADR-0003, NFR-SEC-10). The spike S2d commands and their capability
//! exist only with feature `spike-s2d`.

const COMMANDS: &[&str] = &[
    "app_info",
    "pick_folder",
    "open_project",
    "close_project",
    "current_project",
    "add_source",
    "progress",
    "connection_count",
    "connections_page",
    "occurrences",
    "source_context",
    "source_document",
    "page_view",
    "page_image",
    "pick_files",
    "recent_projects",
    "create_project",
    "locate_project",
    "forget_project",
    "rescan",
    "pause",
    "resume",
    "cancel",
    "documents_page",
    "documents_count",
    "document_types",
    "retry",
    "removal_impact",
    "remove_files",
    "project_settings",
    "save_settings",
    "reprocess",
    "source_roots",
    "relocate_root",
    "remove_root",
    "subscribe",
    "record_ui_timing",
    "record_view_sweep",
    "record_queue_walk",
];
const SPIKE_COMMANDS: &[&str] = &["synthetic_rows", "record_frame_stats"];

fn main() {
    let spike = std::env::var_os("CARGO_FEATURE_SPIKE_S2D").is_some();
    // The manifest wants a `'static` list; a build script runs once, so leaking it is fine.
    let (commands, capabilities): (&'static [&str], _) = if spike {
        (
            [COMMANDS, SPIKE_COMMANDS].concat().leak(),
            "./capabilities/*.json",
        )
    } else {
        (COMMANDS, "./capabilities/main.json")
    };
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(commands))
        .capabilities_path_pattern(capabilities);
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
