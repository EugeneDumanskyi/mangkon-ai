//! The navigation guard: the window may show only the app's own pages (NFR-SEC-10, ADR-0003).

use tauri::Url;

/// The Vite dev server, allowed only in dev builds (no `custom-protocol`).
const DEV_URL: &str = "http://localhost:5173";

/// `true` for the embedded UI: `tauri://localhost` on Linux and macOS, `http(s)://tauri.localhost`
/// on Windows. In dev builds, also the Vite dev server.
pub fn allowed(url: &Url) -> bool {
    is_app_origin(url) || (tauri::is_dev() && url.as_str().starts_with(DEV_URL))
}

fn is_app_origin(url: &Url) -> bool {
    match (url.scheme(), url.host_str()) {
        ("tauri", Some("localhost")) => url.port().is_none(),
        ("http" | "https", Some("tauri.localhost")) => url.port().is_none(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> bool {
        is_app_origin(&Url::parse(s).unwrap())
    }

    #[test]
    fn nfr_sec_10_only_app_pages_load() {
        assert!(ok("tauri://localhost/index.html"));
        assert!(ok("http://tauri.localhost/"));
        assert!(ok("https://tauri.localhost/assets/x.js"));

        assert!(!ok("https://example.org/"));
        assert!(!ok("http://localhost:5173/"));
        assert!(!ok("http://tauri.localhost.example.org/"));
        assert!(!ok("http://tauri.localhost:8080/"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("data:text/html,<script>1</script>"));
        assert!(!ok("javascript:alert(1)"));
    }
}
