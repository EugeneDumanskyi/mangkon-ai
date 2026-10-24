//! Generates the permission for each app command, so the webview can call only the commands a
//! capability file grants (ADR-0003, NFR-SEC-10). The spike S2d commands and their capability
//! exist only with feature `spike-s2d`.

const COMMANDS: &[&str] = &["app_info"];
const SPIKE_COMMANDS: &[&str] = &["app_info", "synthetic_rows", "record_frame_stats"];

fn main() {
    let spike = std::env::var_os("CARGO_FEATURE_SPIKE_S2D").is_some();
    let (commands, capabilities) = if spike {
        (SPIKE_COMMANDS, "./capabilities/*.json")
    } else {
        (COMMANDS, "./capabilities/main.json")
    };
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(commands))
        .capabilities_path_pattern(capabilities);
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
