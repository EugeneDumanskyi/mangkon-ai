//! ADR-0015 runtime mechanics: find the user-installed `llama-server` (settings path, then
//! `PATH`), read its build from `--version`, refuse builds below the minimum, start it on
//! `127.0.0.1` with a random port and `--api-key`, wait for `/health`, kill and restart.

use std::fmt;
use std::io::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Minimum `llama-server` build: the build M1 tested with (ADR-0015 "set from the build M1
/// tests with"). `version: 0.4.0-dev (build 10863, commit 88ada91c1)` on RM-1.
pub const MIN_BUILD: u32 = 10863;

pub const BINARY: &str = "llama-server";

/// Shown with every "not found" and "too old" reason (ADR-0015).
pub const INSTALL_GUIDE: &str = "docs/install-llama-cpp.md";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Found {
    Settings(PathBuf),
    Path(PathBuf),
}

impl Found {
    pub fn path(&self) -> &Path {
        match self {
            Found::Settings(p) | Found::Path(p) => p,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    NotFound,
    VersionUnreadable(String),
    TooOld { found: u32, min: u32 },
    Start(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::NotFound => write!(f, "llama-server not found (see {INSTALL_GUIDE})"),
            RuntimeError::VersionUnreadable(out) => write!(
                f,
                "llama-server --version gave no build number: {out:?} (see {INSTALL_GUIDE})"
            ),
            RuntimeError::TooOld { found, min } => write!(
                f,
                "llama.cpp build {found} found; build {min} or newer is needed (see {INSTALL_GUIDE})"
            ),
            RuntimeError::Start(e) => write!(f, "llama-server did not start: {e}"),
        }
    }
}

/// Settings path first, then `PATH`. No other search (ADR-0015). A settings path that does not
/// point to an executable file falls through to `PATH`.
pub fn find(
    settings: Option<&Path>,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<Found, RuntimeError> {
    if let Some(p) = settings.filter(|p| is_executable(p)) {
        return Ok(Found::Settings(p.to_path_buf()));
    }
    for dir in path_var.map(std::env::split_paths).into_iter().flatten() {
        let candidate = dir.join(BINARY);
        if is_executable(&candidate) {
            return Ok(Found::Path(candidate));
        }
    }
    Err(RuntimeError::NotFound)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Build {
    pub build: u32,
    pub commit: String,
    /// The semantic version, when the output has one (`0.4.0-dev`).
    pub version: Option<String>,
}

/// Reads the build from `llama-server --version` output. Two forms are accepted:
/// `version: 0.4.0-dev (build 10863, commit 88ada91c1)` (RM-1, build 10863) and
/// `version: 10863 (88ada91c1)` (the form ADR-0015 assumed).
pub fn parse_version(output: &str) -> Option<Build> {
    let line = output
        .lines()
        .find_map(|l| l.trim().strip_prefix("version:"))?
        .trim();
    let (head, rest) = line.split_once('(')?;
    let inner = rest.split_once(')')?.0;
    let head = head.trim();
    if let Some((b, c)) = inner.split_once(',') {
        let build = b.trim().strip_prefix("build")?.trim().parse().ok()?;
        let commit = c.trim().strip_prefix("commit")?.trim().to_string();
        return Some(Build {
            build,
            commit,
            version: Some(head.to_string()).filter(|v| !v.is_empty()),
        });
    }
    Some(Build {
        build: head.trim_start_matches('b').parse().ok()?,
        commit: inner.trim().to_string(),
        version: None,
    })
}

pub fn check_min(build: u32, min: u32) -> Result<(), RuntimeError> {
    if build < min {
        return Err(RuntimeError::TooOld { found: build, min });
    }
    Ok(())
}

/// Runs `--version` and checks the minimum. llama.cpp prints the version on stderr.
pub fn version(bin: &Path) -> Result<Build, RuntimeError> {
    let out = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| RuntimeError::Start(e.to_string()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_version(&text).ok_or_else(|| RuntimeError::VersionUnreadable(text.trim().to_string()))
}

/// 32 random bytes as hex, new for every start (NFR-SEC-06).
pub fn random_key() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("OS random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A free loopback port: bind port 0, read it, release it.
pub fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

pub struct Server {
    pub child: Child,
    pub port: u16,
    pub key: String,
    /// Time from spawn to the first `/health` 200.
    pub ready_after: Duration,
}

impl Server {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// `llama-server --host 127.0.0.1 --port <free> --api-key <random> <args>`, output to `log`,
    /// then poll `/health` until 200 or `timeout`.
    pub fn start(
        bin: &Path,
        args: &[String],
        log: &Path,
        timeout: Duration,
    ) -> Result<Self, RuntimeError> {
        let port = free_port().map_err(|e| RuntimeError::Start(e.to_string()))?;
        let key = random_key();
        let file = std::fs::File::create(log).map_err(|e| RuntimeError::Start(e.to_string()))?;
        let err = file
            .try_clone()
            .map_err(|e| RuntimeError::Start(e.to_string()))?;
        let started = Instant::now();
        let child = Command::new(bin)
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--api-key",
                &key,
            ])
            .args(args)
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(err)
            .spawn()
            .map_err(|e| RuntimeError::Start(e.to_string()))?;
        let mut s = Server {
            child,
            port,
            key,
            ready_after: Duration::ZERO,
        };
        let agent = crate::client::agent(Duration::from_secs(2));
        loop {
            if let Ok(Some(status)) = s.child.try_wait() {
                return Err(RuntimeError::Start(format!(
                    "exited with {status} before /health was ready; log: {}",
                    log.display()
                )));
            }
            if crate::client::status(&agent, &format!("{}/health", s.base()), None) == Some(200) {
                s.ready_after = started.elapsed();
                return Ok(s);
            }
            if started.elapsed() > timeout {
                s.kill();
                return Err(RuntimeError::Start(format!(
                    "/health not ready after {} s; log: {}",
                    timeout.as_secs(),
                    log.display()
                )));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// SIGKILL (what `Child::kill` sends on Unix), then reap.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// SIGTERM, wait up to 10 s, then SIGKILL. Returns true when it ended on SIGTERM.
    pub fn stop(&mut self) -> bool {
        let _ = Command::new("kill")
            .args(["-TERM", &self.pid().to_string()])
            .status();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.kill();
        false
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            self.kill();
        }
    }
}

/// Local addresses (hex, as in `/proc/net/tcp{,6}`) that listen on `port`.
#[cfg(target_os = "linux")]
pub fn listen_addrs(port: u16) -> Vec<String> {
    let want = format!("{port:04X}");
    let mut out = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // cols[1] = local "ADDR:PORT", cols[3] = state; 0A = LISTEN.
            if cols.len() > 3
                && cols[3] == "0A"
                && let Some((addr, p)) = cols[1].split_once(':')
                && p == want
            {
                out.push(addr.to_string());
            }
        }
    }
    out
}

/// IPv4 127.0.0.1 in `/proc/net/tcp` byte order.
pub const LOOPBACK_HEX: &str = "0100007F";

#[derive(Serialize, Default)]
pub struct Step {
    pub step: String,
    pub pass: bool,
    pub detail: String,
}

/// `model-check runtime`: every ADR-0015 step on the real binary. Each step is recorded; the
/// first failing step that blocks the rest ends the run.
pub fn check(settings: Option<&Path>, model: &Path, out: &Path) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut rec = |step: &str, pass: bool, detail: String| {
        println!("{} {step}: {detail}", if pass { "PASS" } else { "FAIL" });
        let _ = std::io::stdout().flush();
        steps.push(Step {
            step: step.into(),
            pass,
            detail,
        });
    };

    let found = match find(settings, std::env::var_os("PATH").as_deref()) {
        Ok(f) => {
            rec("find", true, format!("{f:?}"));
            f
        }
        Err(e) => {
            rec("find", false, e.to_string());
            return steps;
        }
    };
    let build = match version(found.path()) {
        Ok(b) => {
            rec("version", true, format!("{b:?}"));
            b
        }
        Err(e) => {
            rec("version", false, e.to_string());
            return steps;
        }
    };
    match check_min(build.build, MIN_BUILD) {
        Ok(()) => rec(
            "minimum",
            true,
            format!("build {} >= {MIN_BUILD}", build.build),
        ),
        Err(e) => {
            rec("minimum", false, e.to_string());
            return steps;
        }
    }
    // The refusal text a too-old build gets, as the app would show it.
    let refusal = check_min(build.build, build.build + 1)
        .unwrap_err()
        .to_string();
    rec(
        "refusal-text",
        refusal.starts_with("llama.cpp build "),
        refusal,
    );

    let args: Vec<String> = [
        "-m",
        &model.to_string_lossy(),
        "-c",
        "4096",
        "-np",
        "1",
        "-ngl",
        "99",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let timeout = Duration::from_secs(300);
    let mut first = match Server::start(found.path(), &args, &out.join("runtime-1.log"), timeout) {
        Ok(s) => {
            rec(
                "start+health",
                true,
                format!(
                    "port {}, ready after {:.1} s",
                    s.port,
                    s.ready_after.as_secs_f64()
                ),
            );
            s
        }
        Err(e) => {
            rec("start+health", false, e.to_string());
            return steps;
        }
    };
    probe_auth(&first, &mut rec);
    #[cfg(target_os = "linux")]
    {
        let addrs = listen_addrs(first.port);
        let ok = !addrs.is_empty() && addrs.iter().all(|a| a == LOOPBACK_HEX);
        rec(
            "loopback-only",
            ok,
            format!("listening addresses {addrs:?}"),
        );
    }
    let (old_key, old_port, old_pid) = (first.key.clone(), first.port, first.pid());
    first.kill();
    let gone = crate::client::status(
        &crate::client::agent(Duration::from_secs(2)),
        &format!("http://127.0.0.1:{old_port}/health"),
        None,
    )
    .is_none();
    rec(
        "sigkill",
        gone,
        format!("pid {old_pid} killed; port {old_port} closed: {gone}"),
    );

    let mut second = match Server::start(found.path(), &args, &out.join("runtime-2.log"), timeout) {
        Ok(s) => {
            let fresh = s.port != old_port && s.key != old_key;
            rec(
                "restart",
                fresh,
                format!(
                    "port {} (was {old_port}), new key: {}, ready after {:.1} s",
                    s.port,
                    s.key != old_key,
                    s.ready_after.as_secs_f64()
                ),
            );
            s
        }
        Err(e) => {
            rec("restart", false, e.to_string());
            return steps;
        }
    };
    let agent = crate::client::agent(Duration::from_secs(60));
    let url = format!("{}/v1/chat/completions", second.base());
    let old = crate::client::post_status(&agent, &url, Some(&old_key), &tiny_chat());
    rec(
        "old-key-rejected",
        old == Some(401),
        format!("status {old:?}"),
    );
    probe_auth(&second, &mut rec);
    let term = second.stop();
    rec("stop", true, format!("ended on SIGTERM: {term}"));
    steps
}

fn tiny_chat() -> serde_json::Value {
    serde_json::json!({
        "messages": [{"role": "user", "content": "Say OK."}],
        "max_tokens": 4,
        "temperature": 0,
        "chat_template_kwargs": {"enable_thinking": false},
    })
}

/// No key → 401; wrong key → 401; the right key → 200. `/health` needs no key.
fn probe_auth(s: &Server, rec: &mut impl FnMut(&str, bool, String)) {
    let agent = crate::client::agent(Duration::from_secs(60));
    let url = format!("{}/v1/chat/completions", s.base());
    let none = crate::client::post_status(&agent, &url, None, &tiny_chat());
    rec(
        "no-key-rejected",
        none == Some(401),
        format!("status {none:?}"),
    );
    let wrong = crate::client::post_status(&agent, &url, Some("wrong"), &tiny_chat());
    rec(
        "wrong-key-rejected",
        wrong == Some(401),
        format!("status {wrong:?}"),
    );
    let right = crate::client::post_status(&agent, &url, Some(&s.key), &tiny_chat());
    rec(
        "key-accepted",
        right == Some(200),
        format!("status {right:?}"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adr_0015_version_format_rm1() {
        let b = parse_version(
            "ggml_cuda_init: found 1 CUDA devices\nversion: 0.4.0-dev (build 10863, commit 88ada91c1)\nbuilt with GNU 13.3.0 for Linux x86_64\n",
        )
        .unwrap();
        assert_eq!(b.build, 10863);
        assert_eq!(b.commit, "88ada91c1");
        assert_eq!(b.version.as_deref(), Some("0.4.0-dev"));
    }

    #[test]
    fn adr_0015_version_format_assumed() {
        let b = parse_version("version: 11669 (4f2c1e0a)\nbuilt with clang for arm64\n").unwrap();
        assert_eq!(b.build, 11669);
        assert_eq!(b.commit, "4f2c1e0a");
        assert_eq!(b.version, None);
    }

    #[test]
    fn adr_0015_version_unreadable() {
        assert_eq!(parse_version("llama-server 1.0"), None);
        assert_eq!(parse_version("version: dev (unknown)"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn adr_0015_refuses_build_below_minimum() {
        assert!(check_min(MIN_BUILD, MIN_BUILD).is_ok());
        assert!(check_min(MIN_BUILD + 1, MIN_BUILD).is_ok());
        let e = check_min(9000, MIN_BUILD).unwrap_err();
        assert_eq!(
            e.to_string(),
            format!(
                "llama.cpp build 9000 found; build {MIN_BUILD} or newer is needed (see {INSTALL_GUIDE})"
            )
        );
        assert_eq!(
            RuntimeError::NotFound.to_string(),
            format!("llama-server not found (see {INSTALL_GUIDE})")
        );
    }

    #[test]
    fn adr_0015_random_key_differs() {
        let (a, b) = (random_key(), random_key());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }

    #[cfg(unix)]
    mod fake {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
            std::fs::create_dir_all(dir).unwrap();
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }

        fn tmp(name: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("model-check-{name}-{}", random_key()));
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn adr_0015_settings_path_before_path() {
            let d = tmp("order");
            let set = script(
                &d.join("settings"),
                "my-llama",
                "echo 'version: 10863 (abc)' >&2",
            );
            let on_path = script(&d.join("bin"), BINARY, "echo 'version: 9000 (old)' >&2");
            let path_var = std::env::join_paths([d.join("bin")]).unwrap();
            assert_eq!(
                find(Some(&set), Some(&path_var)),
                Ok(Found::Settings(set.clone()))
            );
            assert_eq!(
                find(None, Some(&path_var)),
                Ok(Found::Path(on_path.clone()))
            );
            // A settings path that is gone falls through to PATH.
            assert_eq!(
                find(Some(&d.join("missing")), Some(&path_var)),
                Ok(Found::Path(on_path.clone()))
            );
            assert_eq!(version(&set).unwrap().build, 10863);
            let old = version(&on_path).unwrap();
            assert!(matches!(
                check_min(old.build, MIN_BUILD),
                Err(RuntimeError::TooOld { found: 9000, .. })
            ));
            std::fs::remove_dir_all(d).unwrap();
        }

        #[test]
        fn adr_0015_not_found() {
            let d = tmp("missing");
            // Present but not executable: not a match.
            std::fs::write(d.join(BINARY), "x").unwrap();
            let path_var = std::env::join_paths([&d]).unwrap();
            assert_eq!(find(None, Some(&path_var)), Err(RuntimeError::NotFound));
            assert_eq!(find(None, None), Err(RuntimeError::NotFound));
            std::fs::remove_dir_all(d).unwrap();
        }

        #[test]
        fn adr_0015_version_unreadable_binary() {
            let d = tmp("garbage");
            let bin = script(&d, BINARY, "echo 'hello'");
            assert!(matches!(
                version(&bin),
                Err(RuntimeError::VersionUnreadable(_))
            ));
            std::fs::remove_dir_all(d).unwrap();
        }
    }
}
