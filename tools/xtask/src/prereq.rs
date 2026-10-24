//! Build prerequisite check (US-14.1 AC2, NFR-MNT-01): says which tool is missing and where its
//! install steps are. No Python, no Docker.

use std::process::Command;

/// Vite 8 runs on Node `^20.19 || >=22.12`.
fn node_ok(v: (u32, u32)) -> bool {
    (v.0 == 20 && v.1 >= 19) || v >= (22, 12)
}
/// Tesseract 5.5 needs CMake 3.18.
const CMAKE_MIN: (u32, u32) = (3, 18);
const LINUX_PKGS: &[(&str, &str)] = &[
    ("webkit2gtk-4.1", "libwebkit2gtk-4.1-dev"),
    ("javascriptcoregtk-4.1", "libjavascriptcoregtk-4.1-dev"),
    ("libsoup-3.0", "libsoup-3.0-dev"),
    ("librsvg-2.0", "librsvg2-dev"),
];

pub struct Missing {
    pub what: String,
    pub how: String,
}

/// Program name, with `.cmd` for npm on Windows (it is a batch file there).
pub fn npm() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

/// First line of `program args` output, or `None` if it does not run.
fn version_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    text.lines().next().map(|l| l.trim().to_string())
}

/// The first `major.minor` in `s`.
fn major_minor(s: &str) -> Option<(u32, u32)> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let mut parts = s[start..].split(|c: char| !c.is_ascii_digit());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

pub fn check() -> Vec<Missing> {
    let mut missing = Vec::new();
    let mut need = |what: &str, how: &str| {
        missing.push(Missing {
            what: what.to_string(),
            how: how.to_string(),
        })
    };

    match version_line("node", &["--version"]).and_then(|v| major_minor(&v)) {
        Some(v) if node_ok(v) => {}
        found => need(
            &format!(
                "Node.js 20.19+ or 22.12+{}",
                found.map_or(String::new(), |(a, b)| format!(" (found {a}.{b})"))
            ),
            "https://nodejs.org/en/download (the LTS release includes npm)",
        ),
    }
    if version_line(npm(), &["--version"]).is_none() {
        need("npm", "https://nodejs.org/en/download (comes with Node.js)");
    }
    match version_line("cmake", &["--version"]).and_then(|v| major_minor(&v)) {
        Some(v) if v >= CMAKE_MIN => {}
        found => need(
            &format!(
                "CMake {}.{} or newer{}",
                CMAKE_MIN.0,
                CMAKE_MIN.1,
                found.map_or(String::new(), |(a, b)| format!(" (found {a}.{b})"))
            ),
            "https://cmake.org/download/ (or your package manager)",
        ),
    }
    if cfg!(windows) {
        let vswhere = std::env::var_os("ProgramFiles(x86)").map(|p| {
            std::path::Path::new(&p).join("Microsoft Visual Studio/Installer/vswhere.exe")
        });
        if !vswhere.is_some_and(|p| p.is_file()) {
            need(
                "Visual Studio Build Tools with \"Desktop development with C++\"",
                "https://visualstudio.microsoft.com/visual-cpp-build-tools/",
            );
        }
    } else if version_line("c++", &["--version"]).is_none() {
        need(
            "a C++ compiler (c++)",
            if cfg!(target_os = "macos") {
                "run: xcode-select --install"
            } else {
                "Debian/Ubuntu: sudo apt install build-essential"
            },
        );
    }
    if version_line("curl", &["--version"]).is_none() {
        need(
            "curl",
            "https://curl.se/download.html (or your package manager)",
        );
    }
    if cfg!(target_os = "linux") {
        if version_line("pkg-config", &["--version"]).is_none() {
            need("pkg-config", "Debian/Ubuntu: sudo apt install pkg-config");
        } else {
            let absent: Vec<&str> = LINUX_PKGS
                .iter()
                .filter(|(module, _)| {
                    !Command::new("pkg-config")
                        .args(["--exists", module])
                        .status()
                        .is_ok_and(|s| s.success())
                })
                .map(|(_, deb)| *deb)
                .collect();
            if !absent.is_empty() {
                need(
                    &format!(
                        "WebKitGTK and GTK development files ({})",
                        absent.join(", ")
                    ),
                    &format!(
                        "Debian/Ubuntu: sudo apt install {}. Other distributions: see README, \"Build from source\"",
                        absent.join(" ")
                    ),
                );
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_from_tool_output() {
        assert_eq!(major_minor("v24.22.0"), Some((24, 22)));
        assert_eq!(major_minor("cmake version 3.28.3"), Some((3, 28)));
        assert_eq!(major_minor("no digits"), None);
        assert!(node_ok((20, 19)) && node_ok((22, 12)) && node_ok((24, 0)));
        assert!(!node_ok((20, 18)) && !node_ok((21, 7)) && !node_ok((22, 11)));
    }
}
