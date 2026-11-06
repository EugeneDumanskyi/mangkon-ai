//! Finding the user-installed `llama-server` (ADR-0015): the settings path, then `PATH`, no
//! other search. Its build from `--version`, the minimum check, and its backend from
//! `--list-devices`. Ported from the M1 harness (`09-model-check.md` §2).

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Minimum `llama-server` build: the build M1 tested with (ADR-0015).
/// `version: 0.4.0-dev (build 10863, commit 88ada91c1)` on RM-1.
pub const MIN_BUILD: u32 = 10863;

/// The program name looked for on `PATH`.
pub const BINARY: &str = if cfg!(windows) {
    "llama-server.exe"
} else {
    "llama-server"
};

/// Shown with every "not found" and "too old" reason (ADR-0015). ASSUMPTION: a path in the
/// repository until the project has a public URL.
pub const INSTALL_GUIDE: &str = "docs/install-llama-cpp.md";

/// How long `--version` and `--list-devices` may take. Both load the GPU backends first.
pub const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// Where the binary was found.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Settings,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: Source,
}

/// Why the runtime cannot be used before it is started, in ADR-0015's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindError {
    NotFound,
    /// `--version` gave no build number; the output, trimmed.
    VersionUnreadable(String),
    TooOld {
        found: u32,
        min: u32,
    },
    /// `--version` could not be run, or ran past [`PROBE_LIMIT`].
    Run(String),
}

impl FindError {
    pub fn code(&self) -> &'static str {
        match self {
            FindError::NotFound => "not_found",
            FindError::VersionUnreadable(_) => "version_unreadable",
            FindError::TooOld { .. } => "too_old",
            FindError::Run(_) => "not_runnable",
        }
    }

    /// The reason as the UI shows it; the install guide is linked next to it.
    pub fn message(&self) -> String {
        match self {
            FindError::NotFound => "llama-server not found".into(),
            FindError::VersionUnreadable(_) => "llama-server --version gave no build number".into(),
            FindError::TooOld { found, min } => {
                format!("llama.cpp build {found} found; build {min} or newer is needed")
            }
            FindError::Run(e) => format!("llama-server could not be run: {e}"),
        }
    }
}

impl std::fmt::Display for FindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (see {INSTALL_GUIDE})", self.message())
    }
}

impl std::error::Error for FindError {}

/// Settings path first, then `PATH` (ADR-0015). A settings path that is not an executable file
/// falls through to `PATH`.
pub fn find(settings: Option<&Path>, path_var: Option<&OsStr>) -> Result<Found, FindError> {
    if let Some(p) = settings.filter(|p| is_executable(p)) {
        return Ok(Found {
            path: p.to_path_buf(),
            source: Source::Settings,
        });
    }
    for dir in path_var.map(std::env::split_paths).into_iter().flatten() {
        let candidate = dir.join(BINARY);
        if is_executable(&candidate) {
            return Ok(Found {
                path: candidate,
                source: Source::Path,
            });
        }
    }
    Err(FindError::NotFound)
}

#[cfg(unix)]
pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub build: u32,
    pub commit: String,
    /// The semantic version, when the output has one (`0.4.0-dev`).
    pub version: Option<String>,
}

/// Reads the build from `--version` output. Two forms: `version: 0.4.0-dev (build 10863,
/// commit 88ada91c1)` (RM-1) and `version: 10863 (88ada91c1)` (older builds).
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

pub fn check_min(build: u32, min: u32) -> Result<(), FindError> {
    if build < min {
        return Err(FindError::TooOld { found: build, min });
    }
    Ok(())
}

/// Extra env vars for the child (tests).
pub type Env = [(OsString, OsString)];

/// Runs `bin args`, with stdin closed, and returns stdout and stderr together. Killed after
/// `limit`.
pub fn run_with_limit(
    bin: &Path,
    args: &[&str],
    env: &Env,
    limit: Duration,
) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.by_ref().take(256 * 1024).read_to_end(&mut buf);
            buf
        })
    })
    .collect();
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} s", limit.as_secs()));
            }
        }
    }
    let mut out = String::new();
    for r in readers {
        out.push_str(&String::from_utf8_lossy(&r.join().unwrap_or_default()));
    }
    Ok(out)
}

/// `--version`, read and checked against [`MIN_BUILD`]. llama.cpp prints it on stderr.
pub fn version(bin: &Path, env: &Env) -> Result<Build, FindError> {
    let text = run_with_limit(bin, &["--version"], env, PROBE_LIMIT).map_err(FindError::Run)?;
    let build = parse_version(&text)
        .ok_or_else(|| FindError::VersionUnreadable(text.trim().chars().take(400).collect()))?;
    check_min(build.build, MIN_BUILD)?;
    Ok(build)
}

/// A GPU backend llama.cpp can run on, or none.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Cuda,
    Vulkan,
    Metal,
    Rocm,
    Sycl,
    Cpu,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Cuda => "CUDA",
            Backend::Vulkan => "Vulkan",
            Backend::Metal => "Metal",
            Backend::Rocm => "ROCm",
            Backend::Sycl => "SYCL",
            Backend::Cpu => "CPU only",
        }
    }
}

/// One device line of `--list-devices`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// `CUDA0`, `Vulkan0`, `MTL0`.
    pub id: String,
    pub backend: Backend,
    /// The rest of the line: name and memory.
    pub description: String,
}

/// The device id prefix of each backend. CUDA is from RM-1 (`CUDA0: NVIDIA GeForce RTX 5070
/// Laptop GPU (7707 MiB, 7570 MiB free)`); the others are ASSUMPTION from the ggml backend
/// names, not seen here.
const PREFIXES: &[(&str, Backend)] = &[
    ("CUDA", Backend::Cuda),
    ("Vulkan", Backend::Vulkan),
    ("MTL", Backend::Metal),
    ("Metal", Backend::Metal),
    ("ROCm", Backend::Rocm),
    ("HIP", Backend::Rocm),
    ("SYCL", Backend::Sycl),
];

/// The devices in `--list-devices` output: lines `  <ID>: <description>` after
/// `Available devices:`. Lines of unknown backends are left out.
pub fn parse_devices(output: &str) -> Vec<Device> {
    let mut out = Vec::new();
    let mut listing = false;
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Available devices") {
            listing = true;
            continue;
        }
        if !listing {
            continue;
        }
        let Some((id, desc)) = t.split_once(':') else {
            continue;
        };
        let id = id.trim();
        let Some(backend) = PREFIXES.iter().find_map(|(p, b)| {
            id.strip_prefix(p)
                .filter(|n| n.bytes().all(|c| c.is_ascii_digit()))
                .map(|_| *b)
        }) else {
            continue;
        };
        out.push(Device {
            id: id.to_string(),
            backend,
            description: desc.trim().to_string(),
        });
    }
    out
}

/// The backends of `devices`, in first-seen order; [`Backend::Cpu`] when there are none.
pub fn backends(devices: &[Device]) -> Vec<Backend> {
    let mut out: Vec<Backend> = Vec::new();
    for d in devices {
        if !out.contains(&d.backend) {
            out.push(d.backend);
        }
    }
    if out.is_empty() {
        out.push(Backend::Cpu);
    }
    out
}

/// `--list-devices`. An error when it cannot be run; the backend is then unknown.
pub fn devices(bin: &Path, env: &Env) -> Result<Vec<Device>, String> {
    run_with_limit(bin, &["--list-devices"], env, PROBE_LIMIT).map(|t| parse_devices(&t))
}

/// What the AI models screen shows about the binary (ADR-0015 follow-up).
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct BinaryInfo {
    pub path: Option<PathBuf>,
    pub source: Option<Source>,
    pub build: Option<Build>,
    pub devices: Vec<Device>,
    /// Empty when the devices could not be read.
    pub backends: Vec<Backend>,
    /// Why it cannot be used: `(code, message)`.
    pub error: Option<(String, String)>,
}

/// Finds the binary and reads its build and devices.
pub fn probe(settings: Option<&Path>, path_var: Option<&OsStr>, env: &Env) -> BinaryInfo {
    let mut info = BinaryInfo {
        path: None,
        source: None,
        build: None,
        devices: Vec::new(),
        backends: Vec::new(),
        error: None,
    };
    let found = match find(settings, path_var) {
        Ok(f) => f,
        Err(e) => {
            info.error = Some((e.code().into(), e.message()));
            return info;
        }
    };
    info.path = Some(found.path.clone());
    info.source = Some(found.source);
    let text = match run_with_limit(&found.path, &["--version"], env, PROBE_LIMIT) {
        Ok(t) => t,
        Err(e) => {
            let e = FindError::Run(e);
            info.error = Some((e.code().into(), e.message()));
            return info;
        }
    };
    match parse_version(&text) {
        Some(b) => {
            if let Err(e) = check_min(b.build, MIN_BUILD) {
                info.error = Some((e.code().into(), e.message()));
            }
            info.build = Some(b);
        }
        None => {
            let e = FindError::VersionUnreadable(String::new());
            info.error = Some((e.code().into(), e.message()));
            return info;
        }
    }
    if let Ok(d) = devices(&found.path, env) {
        info.backends = backends(&d);
        info.devices = d;
    }
    info
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
    fn adr_0015_version_format_older() {
        let b = parse_version("version: 11669 (4f2c1e0a)\nbuilt with clang for arm64\n").unwrap();
        assert_eq!(
            (b.build, b.commit.as_str(), b.version),
            (11669, "4f2c1e0a", None)
        );
        assert_eq!(parse_version("llama-server 1.0"), None);
        assert_eq!(parse_version("version: dev (unknown)"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn adr_0015_refusal_texts() {
        assert!(check_min(MIN_BUILD, MIN_BUILD).is_ok());
        let e = check_min(9000, MIN_BUILD).unwrap_err();
        assert_eq!(
            e.message(),
            "llama.cpp build 9000 found; build 10863 or newer is needed"
        );
        assert_eq!(
            e.to_string(),
            "llama.cpp build 9000 found; build 10863 or newer is needed (see docs/install-llama-cpp.md)"
        );
        assert_eq!(FindError::NotFound.message(), "llama-server not found");
    }

    /// The backend of the build: from `--list-devices` (RM-1 output first).
    #[test]
    fn adr_0015_backend_from_devices() {
        let rm1 = "ggml_cuda_init: found 1 CUDA devices\nAvailable devices:\n  CUDA0: NVIDIA GeForce RTX 5070 Laptop GPU (7707 MiB, 7570 MiB free)\n";
        let d = parse_devices(rm1);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].id, "CUDA0");
        assert_eq!(d[0].backend, Backend::Cuda);
        assert!(d[0].description.starts_with("NVIDIA GeForce RTX 5070"));
        assert_eq!(backends(&d), vec![Backend::Cuda]);
        let mixed = "Available devices:\n  Vulkan0: AMD Radeon (8 GiB)\n  Vulkan1: Intel UHD\n  MTL0: Apple M2\n";
        assert_eq!(
            backends(&parse_devices(mixed)),
            vec![Backend::Vulkan, Backend::Metal]
        );
        // A CPU build lists no devices (ASSUMPTION).
        assert_eq!(
            backends(&parse_devices("Available devices:\n")),
            vec![Backend::Cpu]
        );
        // A colon in a log line before the list is not a device.
        assert!(parse_devices("load_backend: loaded CUDA backend\n").is_empty());
    }

    #[cfg(unix)]
    mod scripts {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
            std::fs::create_dir_all(dir).unwrap();
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            // Another test thread may fork while this file is open for writing; its copy of
            // the descriptor closes at its exec. Until then running the script fails with
            // "Text file busy".
            for _ in 0..100 {
                match Command::new(&p)
                    .arg("--noop-check")
                    .env("NOOP", "1")
                    .spawn()
                {
                    Err(e) if e.raw_os_error() == Some(26) => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Ok(mut c) => {
                        let _ = c.kill();
                        let _ = c.wait();
                        break;
                    }
                    Err(_) => break,
                }
            }
            p
        }

        #[test]
        fn adr_0015_settings_path_before_path() {
            let d = tempfile::tempdir().unwrap();
            let set = script(
                &d.path().join("settings"),
                "my-llama",
                "echo 'version: 10863 (abc)' >&2",
            );
            let on_path = script(
                &d.path().join("bin"),
                BINARY,
                "echo 'version: 9000 (old)' >&2",
            );
            let path_var = std::env::join_paths([d.path().join("bin")]).unwrap();
            let f = find(Some(&set), Some(&path_var)).unwrap();
            assert_eq!(
                (f.path.as_path(), f.source),
                (set.as_path(), Source::Settings)
            );
            let f = find(None, Some(&path_var)).unwrap();
            assert_eq!(f.source, Source::Path);
            // A settings path that is gone falls through to PATH.
            let f = find(Some(&d.path().join("missing")), Some(&path_var)).unwrap();
            assert_eq!(f.path, on_path);
            assert_eq!(version(&set, &[]).unwrap().build, 10863);
            assert!(matches!(
                version(&on_path, &[]),
                Err(FindError::TooOld { found: 9000, .. })
            ));
            let info = probe(None, Some(&path_var), &[]);
            assert_eq!(info.build.unwrap().build, 9000);
            assert_eq!(info.error.unwrap().0, "too_old");
        }

        #[test]
        fn adr_0015_not_found_and_unreadable() {
            let d = tempfile::tempdir().unwrap();
            // Present but not executable: not a match.
            std::fs::write(d.path().join(BINARY), "x").unwrap();
            let path_var = std::env::join_paths([d.path()]).unwrap();
            assert_eq!(find(None, Some(&path_var)), Err(FindError::NotFound));
            assert_eq!(find(None, None), Err(FindError::NotFound));
            let bin = script(&d.path().join("g"), BINARY, "echo hello");
            assert!(matches!(
                version(&bin, &[]),
                Err(FindError::VersionUnreadable(_))
            ));
        }

        #[test]
        fn probe_limit_kills_a_hanging_binary() {
            let d = tempfile::tempdir().unwrap();
            let bin = script(d.path(), BINARY, "exec sleep 30");
            let t = Instant::now();
            let e =
                run_with_limit(&bin, &["--version"], &[], Duration::from_millis(300)).unwrap_err();
            assert!(e.contains("no answer"), "{e}");
            assert!(t.elapsed() < Duration::from_secs(5));
        }
    }
}
