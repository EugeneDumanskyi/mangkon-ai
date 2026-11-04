//! A stand-in `llama-server` (the `mk-fake-llama-server` binary, and `mk-core`'s copy of it):
//! answers `--version` and `--list-devices`, then listens where it is told and serves
//! `/health`, `/v1/chat/completions` and `/v1/embeddings`, checking `--api-key` as the real
//! one does. What it does wrong comes from a JSON control file named by
//! [`CONTROL_ENV`], read at every start, so a test can change it between restarts.

use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The env var naming the control file.
pub const CONTROL_ENV: &str = "MK_FAKE_LLAMA_CONTROL";

/// How the fake behaves. Every field is optional in the file.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Control {
    /// The build `--version` prints; 10863 when unset.
    pub build: Option<u32>,
    /// `--list-devices` output; one fake CUDA GPU when unset.
    pub devices: Option<String>,
    /// `/health` answers 503 (loading) this long after start.
    pub start_delay_ms: u64,
    /// `oom`: CUDA out-of-memory lines and exit 1. `bad_model`: model-load lines and exit 1.
    /// `exit`: exit 3 at once, no output.
    pub fail: Option<String>,
    /// Abort (SIGABRT) this long after start.
    pub crash_after_ms: Option<u64>,
    /// The first start that finds this file missing creates it and fails as if its port were
    /// taken.
    pub bind_fail_once: Option<PathBuf>,
    /// Embedding size; 8 when unset.
    pub dims: Option<usize>,
    /// Every start appends `<pid> <port>` here.
    pub starts_log: Option<PathBuf>,
}

impl Control {
    /// Writes the control file.
    pub fn write(&self, path: &Path) {
        std::fs::write(path, serde_json::to_vec(self).expect("control serializes"))
            .expect("write control file");
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn reply(w: &mut TcpStream, status: u16, body: &str) {
    let _ = write!(
        w,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = w.flush();
}

fn handle(stream: TcpStream, key: &str, ready_at: Instant, dims: usize, model: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut r = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut w = stream;
    let Some((path, headers)) = super::read_head(&mut r) else {
        return;
    };
    let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let len: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len.min(1 << 20)];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    if path == "/health" {
        if Instant::now() < ready_at {
            reply(
                &mut w,
                503,
                r#"{"error":{"code":503,"message":"Loading model"}}"#,
            );
        } else {
            reply(&mut w, 200, r#"{"status":"ok"}"#);
        }
        return;
    }
    let auth_ok = header("authorization").as_deref() == Some(&format!("Bearer {key}"));
    eprintln!("fake: {path} auth={}", if auth_ok { "ok" } else { "bad" });
    if !auth_ok {
        reply(
            &mut w,
            401,
            r#"{"error":{"code":401,"message":"Invalid API Key","type":"authentication_error"}}"#,
        );
        return;
    }
    match path.as_str() {
        "/v1/chat/completions" => {
            let content = if body.get("response_format").is_some() {
                r#"{"greeting":"Hello","words":1}"#
            } else {
                "Hello"
            };
            let out = serde_json::json!({
                "model": model,
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}}],
            });
            reply(&mut w, 200, &out.to_string());
        }
        "/v1/embeddings" => {
            let n = body["input"].as_array().map_or(1, Vec::len);
            let data: Vec<_> = (0..n)
                .map(|i| serde_json::json!({"index": i, "embedding": vec![0.5f32; dims]}))
                .collect();
            let out = serde_json::json!({"model": model, "data": data});
            reply(&mut w, 200, &out.to_string());
        }
        _ => reply(
            &mut w,
            404,
            r#"{"error":{"code":404,"message":"File Not Found"}}"#,
        ),
    }
}

/// The fake's `main`.
pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let control: Control = std::env::var_os(CONTROL_ENV)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if args.iter().any(|a| a == "--version") {
        eprintln!(
            "version: 0.4.0-dev (build {}, commit fake0000)\nbuilt with test for test",
            control.build.unwrap_or(10863)
        );
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--list-devices") {
        print!(
            "{}",
            control
                .devices
                .as_deref()
                .unwrap_or("Available devices:\n  CUDA0: Fake GPU (8192 MiB, 8000 MiB free)\n")
        );
        return ExitCode::SUCCESS;
    }
    let host = arg(&args, "--host").unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = arg(&args, "--port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let key = arg(&args, "--api-key").unwrap_or_default();
    let model = arg(&args, "-m").unwrap_or_default();
    if let Some(log) = &control.starts_log {
        let line = format!("{} {port}\n", std::process::id());
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .and_then(|mut f| f.write_all(line.as_bytes()));
    }
    match control.fail.as_deref() {
        Some("oom") => {
            eprintln!(
                "ggml_backend_cuda_buffer_type_alloc_buffer: allocating 20480.00 MiB on device 0: cudaMalloc failed: out of memory\n\
                 E llama_model_load: error loading model: unable to allocate CUDA0 buffer\n\
                 E srv  llama_server: exiting due to model loading error"
            );
            return ExitCode::from(1);
        }
        Some("exit") => return ExitCode::from(3),
        _ => {}
    }
    if control.fail.as_deref() == Some("bad_model") || !Path::new(&model).is_file() {
        eprintln!(
            "E llama_model_load: error loading model: llama_model_loader: failed to load model from {model}\n\
             E srv    load_model: failed to load model, '{model}'\n\
             E srv  llama_server: exiting due to model loading error"
        );
        return ExitCode::from(1);
    }
    if let Some(marker) = &control.bind_fail_once
        && !marker.exists()
    {
        let _ = std::fs::write(marker, b"1");
        eprintln!(
            "E srv         start: couldn't bind HTTP server socket, hostname: {host}, port: {port}"
        );
        return ExitCode::from(1);
    }
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "E srv start: couldn't bind HTTP server socket, hostname: {host}, port: {port}: {e}"
            );
            return ExitCode::from(1);
        }
    };
    eprintln!("fake: listening on {host}:{port}");
    let ready_at = Instant::now() + Duration::from_millis(control.start_delay_ms);
    if let Some(ms) = control.crash_after_ms {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            eprintln!("fake: crashing");
            std::process::abort();
        });
    }
    let dims = control.dims.unwrap_or(8);
    let model_name = Path::new(&model)
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    for stream in listener.incoming().flatten() {
        let key = key.clone();
        let model_name = model_name.clone();
        std::thread::spawn(move || handle(stream, &key, ready_at, dims, &model_name));
    }
    ExitCode::SUCCESS
}
