//! A stand-in `llama-server` (the `mk-fake-llama-server` binary, and `mk-core`'s copy of it):
//! answers `--version` and `--list-devices`, then listens where it is told and serves
//! `/health`, `/v1/chat/completions` and `/v1/embeddings`, checking `--api-key` as the real
//! one does. What it does wrong comes from a JSON control file named by
//! [`CONTROL_ENV`], read at every start, so a test can change it between restarts. Extraction
//! replies follow rules per model file ([`FakeExtraction`]), so a test can run two "models".

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Rules for extraction replies, by model file name; `*` for any other.
    pub extraction: HashMap<String, FakeExtraction>,
    /// Every chat request's body is appended here, one JSON line, with the model file name.
    pub requests_log: Option<PathBuf>,
    /// The most chat requests in flight at once is written here (NFR-PERF-25).
    pub in_flight_log: Option<PathBuf>,
    /// Rules for embedding replies, by model file name; `*` for any other.
    pub embeddings: HashMap<String, FakeEmbeddings>,
}

/// How a fake model answers `/v1/embeddings`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct FakeEmbeddings {
    /// Every reply takes this long.
    pub delay_ms: u64,
    /// A request with a text holding `hang_text` takes `hang_ms`.
    pub hang_text: Option<String>,
    pub hang_ms: u64,
    /// A request with a text holding this answers HTTP 500.
    pub error_text: Option<String>,
    /// One vector too few in every reply.
    pub short_reply: bool,
}

/// A fake model's vector for `text`: its words hashed into the dimensions, with a sign, so texts
/// that share words are similar and two models disagree.
pub fn fake_embedding(model: &str, text: &str, dims: usize) -> Vec<f32> {
    let dims = dims.max(1);
    let mut out = vec![0.0f32; dims];
    let mut hash_of = |word: &str, weight: f32| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in model.bytes().chain(word.bytes()) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        for i in 0..4 {
            let bits = h.rotate_left(i * 16);
            let sign = if bits & (1 << 40) == 0 { 1.0 } else { -1.0 };
            out[(bits % dims as u64) as usize] += sign * weight;
        }
    };
    hash_of("", 0.01);
    for word in text.split_whitespace() {
        let word: String = word
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        hash_of(&word, 1.0);
    }
    out
}

/// One name a fake model finds.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct FakeName {
    pub name: String,
    /// `person` or `org`; `person` when empty.
    #[serde(rename = "type")]
    pub kind: String,
    /// 0.9 when unset.
    pub confidence: Option<f64>,
    /// What it quotes; the name itself when unset.
    pub evidence: Option<String>,
    /// Reply with it only if the text holds this; the name itself when unset.
    pub when: Option<String>,
}

/// How a fake model answers a request with the `mentions` schema.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct FakeExtraction {
    pub names: Vec<FakeName>,
    /// Bad JSON on the first try of every text; the retry is right.
    pub bad_first: bool,
    /// Bad JSON on every try.
    pub bad_always: bool,
    /// Every reply takes this long.
    pub delay_ms: u64,
    /// A text holding `hang_text` takes `hang_ms`.
    pub hang_text: Option<String>,
    pub hang_ms: u64,
    /// A text holding this answers HTTP 500.
    pub error_text: Option<String>,
}

impl FakeExtraction {
    /// The fake's reply to `text`; `None` for an HTTP error.
    fn reply(&self, text: &str, retry: bool) -> Option<String> {
        if self.error_text.as_deref().is_some_and(|e| text.contains(e)) {
            return None;
        }
        if self.bad_always || (self.bad_first && !retry) {
            return Some("{\"mentions\":[{\"name\":".to_string());
        }
        let mentions: Vec<serde_json::Value> = self
            .names
            .iter()
            .filter(|n| text.contains(n.when.as_deref().unwrap_or(&n.name)))
            .map(|n| {
                serde_json::json!({
                    "name": n.name,
                    "type": if n.kind.is_empty() { "person" } else { n.kind.as_str() },
                    "evidence": n.evidence.as_deref().unwrap_or(&n.name),
                    "confidence": n.confidence.unwrap_or(0.9),
                })
            })
            .collect();
        Some(serde_json::json!({ "mentions": mentions }).to_string())
    }
}

/// What every connection of one fake process shares.
struct Shared {
    control: Control,
    key: String,
    ready_at: Instant,
    dims: usize,
    model: String,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl Shared {
    fn rules(&self) -> Option<&FakeExtraction> {
        self.control
            .extraction
            .get(&self.model)
            .or_else(|| self.control.extraction.get("*"))
    }

    fn embedding_rules(&self) -> FakeEmbeddings {
        self.control
            .embeddings
            .get(&self.model)
            .or_else(|| self.control.embeddings.get("*"))
            .cloned()
            .unwrap_or_default()
    }

    fn log_request(&self, body: &serde_json::Value) {
        if let Some(log) = &self.control.requests_log {
            let line = serde_json::json!({ "model": self.model, "body": body }).to_string() + "\n";
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .and_then(|mut f| f.write_all(line.as_bytes()));
        }
    }

    /// One chat request starts; the guard ends it.
    fn enter(self: &Arc<Self>) -> InFlight {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        let max = self.max_in_flight.fetch_max(now, Ordering::SeqCst).max(now);
        if let Some(log) = &self.control.in_flight_log {
            let _ = std::fs::write(log, max.to_string());
        }
        InFlight(self.clone())
    }
}

struct InFlight(Arc<Shared>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The extraction reply to a chat request with the `mentions` schema, if it is one: `Ok(body)`,
/// or `Err(())` for an HTTP error.
fn extraction_reply(shared: &Shared, body: &serde_json::Value) -> Option<Result<String, ()>> {
    if body["response_format"]["json_schema"]["name"] != "mentions" {
        return None;
    }
    let rules = shared.rules().cloned().unwrap_or_default();
    let messages = body["messages"].as_array()?;
    let text = messages.get(1)?["content"].as_str().unwrap_or_default();
    if rules.delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(rules.delay_ms));
    }
    if rules.hang_text.as_deref().is_some_and(|h| text.contains(h)) {
        std::thread::sleep(Duration::from_millis(rules.hang_ms));
    }
    Some(rules.reply(text, messages.len() > 2).ok_or(()))
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

fn handle(stream: TcpStream, shared: &Arc<Shared>) {
    let (key, ready_at, dims, model) = (
        shared.key.as_str(),
        shared.ready_at,
        shared.dims,
        shared.model.as_str(),
    );
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
            let _running = shared.enter();
            shared.log_request(&body);
            let content = match extraction_reply(shared, &body) {
                Some(Ok(c)) => c,
                Some(Err(())) => {
                    reply(
                        &mut w,
                        500,
                        r#"{"error":{"code":500,"message":"Context size has been exceeded"}}"#,
                    );
                    return;
                }
                None if body.get("response_format").is_some() => {
                    r#"{"greeting":"Hello","words":1}"#.to_string()
                }
                None => "Hello".to_string(),
            };
            let out = serde_json::json!({
                "model": model,
                "choices": [{"index": 0, "message": {"role": "assistant", "content": content}}],
            });
            reply(&mut w, 200, &out.to_string());
        }
        "/v1/embeddings" => {
            let _running = shared.enter();
            shared.log_request(&body);
            let rules = shared.embedding_rules();
            let inputs: Vec<&str> = match &body["input"] {
                serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
                v => v.as_str().into_iter().collect(),
            };
            let holds = |t: &Option<String>| {
                t.as_deref()
                    .is_some_and(|t| inputs.iter().any(|i| i.contains(t)))
            };
            if rules.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(rules.delay_ms));
            }
            if holds(&rules.hang_text) {
                std::thread::sleep(Duration::from_millis(rules.hang_ms));
            }
            if holds(&rules.error_text) {
                reply(
                    &mut w,
                    500,
                    r#"{"error":{"code":500,"message":"input is too large"}}"#,
                );
                return;
            }
            let sent = inputs.len() - usize::from(rules.short_reply && !inputs.is_empty());
            let data: Vec<_> = inputs[..sent]
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    serde_json::json!({"index": i, "embedding": fake_embedding(model, t, dims)})
                })
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
    let shared = Arc::new(Shared {
        control,
        key,
        ready_at,
        dims,
        model: model_name,
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    });
    for stream in listener.incoming().flatten() {
        let shared = shared.clone();
        std::thread::spawn(move || handle(stream, &shared));
    }
    ExitCode::SUCCESS
}
