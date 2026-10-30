//! Hands a file or a link to the OS (US-08.4 AC3, US-08.6): open with the default app, or show
//! in the file manager. No Tauri plugin (ADR-0003: no `opener` or `shell`); like the folder
//! dialog, it runs in Rust and the UI only names a document or a checked URL.
//!
//! Each program is started with its arguments and no shell, so nothing in a path or URL is
//! interpreted. The child is waited for on a thread of its own, so none is left a zombie.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Starts `cmd` detached from the app's stdio and reaps it when it exits.
fn spawn(mut cmd: Command) -> io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::Builder::new()
        .name("os-open".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

/// Opens a file with its default app, or a URL in the default browser or mail app.
pub fn open(target: &OsStr) -> io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(program);
    cmd.arg(target);
    spawn(cmd)
}

/// Shows a file selected in the file manager. Linux: `FileManager1.ShowItems` over D-Bus, else
/// the folder it is in.
pub fn reveal(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg("-R").arg(path);
        spawn(cmd)
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // `explorer` parses its own command line: `/select,` and the quoted path as one argument.
        let mut cmd = Command::new("explorer");
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
        spawn(cmd)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if show_items(path).is_ok() {
            return Ok(());
        }
        open(path.parent().unwrap_or(path).as_os_str())
    }
}

/// `org.freedesktop.FileManager1.ShowItems([file URI], "")` on the session bus.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn show_items(path: &Path) -> Result<(), dbus::Error> {
    use std::time::Duration;
    let conn = dbus::blocking::Connection::new_session()?;
    let proxy = conn.with_proxy(
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        Duration::from_secs(5),
    );
    proxy.method_call::<(), _, _, _>(
        "org.freedesktop.FileManager1",
        "ShowItems",
        (vec![file_uri(path)], ""),
    )
}

/// A `file://` URI for an absolute path: every byte but unreserved ones and `/` percent-encoded.
#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
fn file_uri(path: &Path) -> String {
    let bytes = path.as_os_str().as_encoded_bytes();
    let mut out = String::from("file://");
    for &b in bytes {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_encodes_everything_but_unreserved() {
        assert_eq!(
            file_uri(Path::new("/home/a b/Zoë#1?.pdf")),
            "file:///home/a%20b/Zo%C3%AB%231%3F.pdf"
        );
    }
}
