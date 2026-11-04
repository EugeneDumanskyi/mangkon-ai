//! Test helpers (feature `testkit`, dev-dependencies only): a fake Hugging Face over plain HTTP
//! that serves byte ranges and can redirect to a second host name, drop the connection, send
//! wrong bytes or ignore `Range`; a resolver that points test host names at it and records
//! every name it is asked for; a tiny catalog for those files; a stand-in `llama-server`
//! ([`llama`]). Never in a shipped build.

pub mod llama;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::download::{DownloadConfig, HostResolver, TlsRoots, sha256_hex};
use crate::{Benchmark, Catalog, Embedding, License, Model, ModelFile, Role, SCHEMA_VERSION, Tier};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The test origin host (stands for `huggingface.co`).
pub const ORIGIN_HOST: &str = "hf.test";
/// The test CDN host the origin redirects to (stands for `us.aws.cdn.hf.co`).
pub const CDN_HOST: &str = "us.cdn.hf.test";

/// One request the fake server got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub host: String,
    pub path: String,
    pub range: Option<String>,
}

/// How the fake server misbehaves; changeable while it runs.
#[derive(Clone, Debug, Default)]
pub struct Faults {
    /// Origin requests get a 302 to this host (same port), path `/cdn<path>`.
    pub redirect_to: Option<String>,
    /// The next body is cut after this many bytes (then cleared).
    pub drop_after: Option<u64>,
    /// Serve the file with one byte changed.
    pub corrupt: bool,
    /// Answer `200` with the whole file to a `Range` request.
    pub ignore_range: bool,
    /// Answer every request with this status and no body.
    pub status: Option<u16>,
    /// Sleep this long between 64 KiB pieces of a body.
    pub throttle: Option<Duration>,
}

struct Shared {
    files: HashMap<String, Vec<u8>>,
    faults: Mutex<Faults>,
    seen: Mutex<Vec<Seen>>,
}

/// A fake Hugging Face on a local port.
pub struct FakeHf {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) fn read_head(r: &mut impl BufRead) -> Option<(String, Vec<(String, String)>)> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let path = line.split_whitespace().nth(1)?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    Some((path, headers))
}

fn handle(stream: TcpStream, port: u16, shared: &Shared) {
    let mut r = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut w = stream;
    let Some((path, headers)) = read_head(&mut r) else {
        return;
    };
    let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let host = header("host")
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();
    let range = header("range");
    lock(&shared.seen).push(Seen {
        host: host.clone(),
        path: path.clone(),
        range: range.clone(),
    });
    let faults = lock(&shared.faults).clone();
    let reply = |w: &mut TcpStream, status: u16, extra: &[(String, String)], len: usize| {
        let mut head =
            format!("HTTP/1.1 {status} X\r\nContent-Length: {len}\r\nConnection: close\r\n");
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        w.write_all(head.as_bytes())
    };
    if let Some(s) = faults.status {
        let _ = reply(&mut w, s, &[], 0);
        return;
    }
    let file_path = match path.strip_prefix("/cdn") {
        Some(rest) => rest.to_string(),
        None => {
            if let Some(to) = &faults.redirect_to {
                let loc = format!("http://{to}:{port}/cdn{path}");
                let _ = reply(&mut w, 302, &[("Location".into(), loc)], 0);
                return;
            }
            path.clone()
        }
    };
    let Some(data) = shared.files.get(&file_path) else {
        let _ = reply(&mut w, 404, &[], 0);
        return;
    };
    let mut data = data.clone();
    if faults.corrupt && !data.is_empty() {
        let mid = data.len() / 2;
        data[mid] ^= 0xff;
    }
    let total = data.len();
    let start = range
        .as_deref()
        .filter(|_| !faults.ignore_range)
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<usize>().ok());
    let (status, body, extra) = match start {
        Some(s) if s >= total => {
            let _ = reply(
                &mut w,
                416,
                &[("Content-Range".into(), format!("bytes */{total}"))],
                0,
            );
            return;
        }
        Some(s) => (
            206,
            &data[s..],
            vec![(
                "Content-Range".to_string(),
                format!("bytes {s}-{}/{total}", total - 1),
            )],
        ),
        None => (200, &data[..], Vec::new()),
    };
    let mut extra = extra;
    extra.push(("Accept-Ranges".into(), "bytes".into()));
    if reply(&mut w, status, &extra, body.len()).is_err() {
        return;
    }
    let cut = faults.drop_after.map(|n| n as usize);
    if cut.is_some() {
        lock(&shared.faults).drop_after = None;
    }
    let send = cut.map_or(body.len(), |c| c.min(body.len()));
    for piece in body[..send].chunks(64 * 1024) {
        if w.write_all(piece).is_err() {
            return;
        }
        if let Some(t) = faults.throttle {
            std::thread::sleep(t);
        }
    }
    let _ = w.flush();
    if cut.is_some() {
        let _ = w.shutdown(std::net::Shutdown::Both);
    }
}

impl FakeHf {
    /// Serves `files` (path → bytes) on 127.0.0.1.
    pub fn start(files: HashMap<String, Vec<u8>>) -> FakeHf {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind the fake server");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("address");
        let shared = Arc::new(Shared {
            files,
            faults: Mutex::new(Faults::default()),
            seen: Mutex::new(Vec::new()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (shared.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                            let shared = shared.clone();
                            let port = addr.port();
                            std::thread::spawn(move || handle(stream, port, &shared));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        FakeHf {
            addr,
            shared,
            stop,
            thread: Some(thread),
        }
    }

    /// Serves every file of `model` as `/{repo}/resolve/{rev}/{name}`.
    pub fn serving(models: &[(&Model, &[Vec<u8>])]) -> FakeHf {
        let mut files = HashMap::new();
        for (m, bodies) in models {
            for (f, body) in m.files.iter().zip(bodies.iter()) {
                files.insert(
                    format!("/{}/resolve/{}/{}", m.hf_repo, m.hf_revision, f.name),
                    body.clone(),
                );
            }
        }
        FakeHf::start(files)
    }

    pub fn faults(&self, f: impl FnOnce(&mut Faults)) {
        f(&mut lock(&self.shared.faults));
    }

    pub fn seen(&self) -> Vec<Seen> {
        lock(&self.shared.seen).clone()
    }

    /// The origin the downloader should start at: `http://hf.test:<port>`.
    pub fn origin(&self) -> String {
        format!("http://{ORIGIN_HOST}:{}", self.addr.port())
    }

    /// A download config for this server: the test hosts resolve to it, plain HTTP allowed,
    /// short stall timeout.
    pub fn config(&self, resolver: Arc<NamedResolver>) -> DownloadConfig {
        DownloadConfig {
            origin: self.origin(),
            resolver,
            roots: TlsRoots::Only(Vec::new()),
            allow_http: true,
            read_timeout: Duration::from_secs(5),
            disk_full_after: None,
        }
    }
}

impl Drop for FakeHf {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Resolves the test host names (and nothing else) to 127.0.0.1, and records every name it
/// is asked for.
#[derive(Debug, Default)]
pub struct NamedResolver {
    names: Vec<String>,
    asked: Mutex<Vec<String>>,
}

impl NamedResolver {
    pub fn new(names: &[&str]) -> Arc<NamedResolver> {
        Arc::new(NamedResolver {
            names: names.iter().map(|n| n.to_string()).collect(),
            asked: Mutex::new(Vec::new()),
        })
    }

    /// Every host asked for, in order.
    pub fn asked(&self) -> Vec<String> {
        lock(&self.asked).clone()
    }
}

impl HostResolver for NamedResolver {
    fn resolve(&self, host: &str, _port: u16) -> std::io::Result<Vec<IpAddr>> {
        lock(&self.asked).push(host.to_string());
        if self.names.iter().any(|n| n.eq_ignore_ascii_case(host)) {
            Ok(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no test address for {host}"),
            ))
        }
    }
}

/// Deterministic bytes for a test file.
pub fn test_bytes(len: usize, seed: u8) -> Vec<u8> {
    let mut x = u32::from(seed).wrapping_mul(2_654_435_761) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

/// A catalog model whose files are `bodies`, with their real sizes and hashes.
pub fn test_model(id: &str, role: Role, bodies: &[Vec<u8>]) -> Model {
    Model {
        id: id.into(),
        role,
        name: format!("Test {id}"),
        hf_repo: format!("test/{id}"),
        hf_revision: "0123456789abcdef0123456789abcdef01234567".into(),
        files: bodies
            .iter()
            .enumerate()
            .map(|(i, b)| ModelFile {
                name: if bodies.len() == 1 {
                    format!("{id}.gguf")
                } else {
                    format!("parts/{id}-{:05}-of-{:05}.gguf", i + 1, bodies.len())
                },
                bytes: b.len() as u64,
                sha256: sha256_hex(b),
            })
            .collect(),
        quant: "Q8_0".into(),
        license: License {
            spdx: "Apache-2.0".into(),
            url: "https://www.apache.org/licenses/LICENSE-2.0".into(),
        },
        context: 8192,
        vram_bytes: 1_000_000_000,
        ram_bytes: 2_000_000_000,
        min_llama_build: 10863,
        embedding: (role == Role::Embeddings).then(|| Embedding {
            dims: 768,
            query_prefix: String::new(),
            document_prefix: String::new(),
            binary_recall_at_20: 0.95,
        }),
        benchmark: Benchmark {
            source: "test".into(),
            machine: "test".into(),
            llama_build: 10863,
            precision: None,
            recall: None,
            segments_per_s: None,
            windows_per_s: None,
        },
    }
}

/// A catalog of `models` whose download hosts are the test hosts, with one tier
/// recommending the first extraction and the first embeddings model.
pub fn test_catalog(models: Vec<Model>) -> Catalog {
    let first = |r: Role| models.iter().find(|m| m.role == r).map(|m| m.id.clone());
    Catalog {
        schema_version: SCHEMA_VERSION,
        catalog_version: 1,
        download_hosts: vec![ORIGIN_HOST.into(), format!("*.cdn.{ORIGIN_HOST}")],
        tiers: vec![Tier {
            id: "minimum".into(),
            name: "Minimum".into(),
            min_vram_bytes: 8_000_000_000,
            min_ram_bytes: 16_000_000_000,
            min_unified_bytes: Some(32_000_000_000),
            recommended_extraction: first(Role::Extraction),
            recommended_embeddings: first(Role::Embeddings).unwrap_or_default(),
        }],
        models,
    }
}
