//! One `llama-server` process (ADR-0015, NFR-SEC-06): `127.0.0.1`, a free port, a new 64-hex
//! key per start, the model's argv, the output kept as a 16 KiB tail in memory (no log file),
//! `/health` before Ready, SIGTERM then a kill to stop, and the plain reason when it fails.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Role;

/// The output kept per process.
pub const TAIL_BYTES: usize = 16 * 1024;
/// `/health` is asked this often while starting.
pub const HEALTH_EVERY: Duration = Duration::from_millis(250);
/// How long one `/health` request may take.
const HEALTH_REQUEST: Duration = Duration::from_secs(2);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// The catalog id; also the model name the client sends.
    pub id: String,
    pub role: Role,
    /// The GGUF files; a split model is loaded from its first part.
    pub files: Vec<PathBuf>,
    /// `-c` for extraction models.
    pub context: u32,
}

/// The arguments after `--host 127.0.0.1 --port P --api-key K` (`docs/install-llama-cpp.md`
/// §4, M1 settings in `09-model-check.md`).
pub fn model_args(spec: &ModelSpec) -> Vec<String> {
    let model = spec
        .files
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut a: Vec<String> = vec!["-m".into(), model];
    let rest: &[&str] = match spec.role {
        Role::Extraction => &["-np", "1", "--cache-ram", "0", "--reasoning", "off"],
        Role::Embeddings => &[
            "--embedding",
            "-c",
            "2048",
            "-np",
            "1",
            "-b",
            "2048",
            "-ub",
            "2048",
            "--cache-ram",
            "0",
        ],
    };
    if spec.role == Role::Extraction {
        a.extend(["-c".into(), spec.context.to_string()]);
    }
    a.extend(rest.iter().map(|s| s.to_string()));
    a
}

/// 32 random bytes as hex, new for every start (NFR-SEC-06).
pub fn random_key() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("OS random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A free loopback port: bind port 0, read it, release it.
pub fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

/// The last [`TAIL_BYTES`] the process wrote, stdout and stderr together.
#[derive(Default)]
pub struct Tail(VecDeque<u8>);

impl Tail {
    pub fn text(&self) -> String {
        let (a, b) = self.0.as_slices();
        String::from_utf8_lossy(&[a, b].concat()).into_owned()
    }

    fn push(&mut self, bytes: &[u8]) {
        self.0.extend(bytes);
        let over = self.0.len().saturating_sub(TAIL_BYTES);
        self.0.drain(..over);
    }
}

/// Why a start failed or a process stopped, in plain words, with a stable code.
#[derive(serde::Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Failure {
        Failure {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// US-09.5 AC3 example.
pub const NO_GPU_MEMORY: &str = "Not enough GPU memory";
pub const BAD_MODEL_FILE: &str = "The model file could not be loaded";

/// The plain reason in `tail` for a process that ended with `status` (spike in
/// `13-model-manager.md` §5). Model-load and port texts are from RM-1 build 10863; the
/// out-of-memory texts are ASSUMPTION until the RM-1 check.
pub fn classify(tail: &str, status: Option<ExitStatus>) -> Failure {
    let t = tail.to_ascii_lowercase();
    let oom = [
        "out of memory",
        "cudamalloc failed",
        "erroroutofdevicememory",
        "failed to allocate",
        "unable to allocate",
        "not enough memory",
    ];
    if oom.iter().any(|p| t.contains(p)) {
        return Failure::new("gpu_memory", NO_GPU_MEMORY);
    }
    if t.contains("couldn't bind http server socket") || t.contains("address already in use") {
        return Failure::new("port_in_use", "The port llama-server was given is in use");
    }
    if t.contains("failed to load model") || t.contains("error loading model") {
        return Failure::new("model_load", BAD_MODEL_FILE);
    }
    Failure::new(
        "exited",
        format!("llama-server stopped ({})", exit_text(status)),
    )
}

/// `exit 1`, `signal 9`, or `unknown exit`.
pub fn exit_text(status: Option<ExitStatus>) -> String {
    let Some(s) = status else {
        return "unknown exit".into();
    };
    if let Some(c) = s.code() {
        return format!("exit {c}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return format!("signal {sig}");
        }
    }
    "unknown exit".into()
}

/// A running `llama-server`.
pub struct Process {
    child: Child,
    pub port: u16,
    pub key: String,
    tail: Arc<Mutex<Tail>>,
    pub started: Instant,
}

/// How to start: env vars added to the child (tests point the fake at its control file).
#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    pub env: Vec<(OsString, OsString)>,
}

fn reader(mut r: impl Read + Send + 'static, tail: Arc<Mutex<Tail>>) {
    let _ = std::thread::Builder::new()
        .name("llama-server-output".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = r.read(&mut buf) {
                lock(&tail).push(&buf[..n]);
            }
        });
}

impl Process {
    /// Starts `bin` on `port` with a new key. Does not wait for health.
    pub fn spawn(
        bin: &Path,
        spec: &ModelSpec,
        port: u16,
        opts: &SpawnOptions,
    ) -> std::io::Result<Process> {
        let key = random_key();
        let mut cmd = Command::new(bin);
        cmd.args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--api-key",
            &key,
        ])
        .args(model_args(spec))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        // The app ran itself again with its own malloc tunables (NFR-RES-01); llama-server
        // gets the user's own value back.
        if let Some(user) = std::env::var_os("MK_USER_GLIBC_TUNABLES") {
            if user.is_empty() {
                cmd.env_remove("GLIBC_TUNABLES");
            } else {
                cmd.env("GLIBC_TUNABLES", user);
            }
            cmd.env_remove("MK_USER_GLIBC_TUNABLES");
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let tail = Arc::new(Mutex::new(Tail::default()));
        if let Some(o) = child.stdout.take() {
            reader(o, tail.clone());
        }
        if let Some(e) = child.stderr.take() {
            reader(e, tail.clone());
        }
        Ok(Process {
            child,
            port,
            key,
            tail,
            started: Instant::now(),
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn tail(&self) -> String {
        lock(&self.tail).text()
    }

    /// `Some(status)` once the process has ended.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Asks `/health` every [`HEALTH_EVERY`] until 200, the process ends, `timeout` passes or
    /// `cancelled()` says so. `Err` holds the reason; the process is then killed.
    pub fn wait_healthy(
        &mut self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Failure> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(HEALTH_REQUEST))
            .build()
            .into();
        let url = format!("http://127.0.0.1:{}/health", self.port);
        loop {
            if let Some(status) = self.exited() {
                // Give the output readers a moment to take the last lines.
                std::thread::sleep(Duration::from_millis(50));
                return Err(classify(&self.tail(), Some(status)));
            }
            if cancelled() {
                self.kill();
                return Err(Failure::new("stopped", "Stopped while starting"));
            }
            if let Ok(r) = agent.get(&url).call()
                && r.status().as_u16() == 200
            {
                return Ok(());
            }
            if self.started.elapsed() > timeout {
                self.kill();
                return Err(Failure::new(
                    "health_timeout",
                    format!(
                        "llama-server did not get ready within {} s",
                        timeout.as_secs()
                    ),
                ));
            }
            std::thread::sleep(HEALTH_EVERY);
        }
    }

    /// A kill (SIGKILL on Unix), then reap.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// SIGTERM, wait up to `grace`, then a kill. True when it ended on SIGTERM.
    pub fn stop(&mut self, grace: Duration) -> bool {
        if self.exited().is_some() {
            return true;
        }
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &self.pid().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let t = Instant::now();
            while t.elapsed() < grace {
                if self.exited().is_some() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        #[cfg(not(unix))]
        let _ = grace;
        self.kill();
        false
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.exited().is_none() {
            self.kill();
        }
    }
}

/// Local addresses (hex, as in `/proc/net/tcp{,6}`) listening on `port`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfr_sec_06_random_key_differs() {
        let (a, b) = (random_key(), random_key());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn argv_per_role() {
        let x = ModelSpec {
            id: "x".into(),
            role: Role::Extraction,
            files: vec!["/m/x.gguf".into()],
            context: 8192,
        };
        assert_eq!(
            model_args(&x).join(" "),
            "-m /m/x.gguf -c 8192 -np 1 --cache-ram 0 --reasoning off"
        );
        let e = ModelSpec {
            role: Role::Embeddings,
            ..x
        };
        assert_eq!(
            model_args(&e).join(" "),
            "-m /m/x.gguf --embedding -c 2048 -np 1 -b 2048 -ub 2048 --cache-ram 0"
        );
    }

    /// Texts from RM-1, build 10863 (`13-model-manager.md` §5).
    #[test]
    fn us_09_5_ac3_failure_texts() {
        let bad = "E llama_model_load: error loading model: llama_model_loader: failed to load model from /x/bad.gguf\n\
                   E srv    load_model: failed to load model, '/x/bad.gguf'\n\
                   E srv  llama_server: exiting due to model loading error\n";
        assert_eq!(classify(bad, None).message, BAD_MODEL_FILE);
        let port = "E srv         start: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 18998\n";
        assert_eq!(classify(port, None).code, "port_in_use");
        let oom = "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 20480.00 MiB on device 0: cudaMalloc failed: out of memory\n\
                   E llama_model_load: error loading model: unable to allocate CUDA0 buffer\n";
        assert_eq!(classify(oom, None).message, NO_GPU_MEMORY);
        assert_eq!(
            classify("something else", None).message,
            "llama-server stopped (unknown exit)"
        );
    }

    #[test]
    fn tail_keeps_the_last_16_kib() {
        let mut t = Tail::default();
        t.push(&vec![b'a'; TAIL_BYTES]);
        t.push(b"end");
        let s = t.text();
        assert_eq!(s.len(), TAIL_BYTES);
        assert!(s.ends_with("aend"));
    }
}
