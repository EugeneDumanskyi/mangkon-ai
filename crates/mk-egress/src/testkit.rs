//! Test helpers (feature `testkit`, dev-dependencies only): a fake OpenAI-compatible server
//! over plain HTTP or TLS, a scripted resolver, in-memory ports. Never in a shipped build.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{EgressError, ErrorCode};
use crate::gate::{EgressStore, EndpointConfig, Endpoints};
use crate::policy::{DestinationClass, Permission, Purpose};
use crate::resolve::Resolve;
use crate::secrets::{SecretStore, SecretStoreKind, SessionStore};
use secrecy::SecretString;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One request the fake server got.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// The TLS server name the client sent, for a TLS server.
    pub sni: Option<String>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// How the fake server answers.
#[derive(Clone, Debug)]
pub enum Behavior {
    /// An OpenAI-compatible API under `/v1`.
    OpenAi {
        model: String,
        /// Follows `response_format: json_schema`; otherwise ignores it and answers in words.
        structured: bool,
        /// Serves `/v1/embeddings`.
        embeddings: bool,
        /// The bearer key it wants; `None` takes any.
        key: Option<String>,
    },
    /// Every request gets this status and body.
    Fixed { status: u16, body: String },
    /// Every request gets a 302 to this URL.
    Redirect(String),
}

impl Behavior {
    pub fn openai(model: &str) -> Behavior {
        Behavior::OpenAi {
            model: model.into(),
            structured: true,
            embeddings: true,
            key: None,
        }
    }
}

fn answer(b: &Behavior, req: &Seen) -> (u16, Vec<(String, String)>, String) {
    let json_reply = |status: u16, v: Value| {
        (
            status,
            vec![("Content-Type".to_string(), "application/json".to_string())],
            v.to_string(),
        )
    };
    match b {
        Behavior::Fixed { status, body } => (*status, Vec::new(), body.clone()),
        Behavior::Redirect(to) => (302, vec![("Location".into(), to.clone())], String::new()),
        Behavior::OpenAi {
            model,
            structured,
            embeddings,
            key,
        } => {
            if let Some(k) = key
                && req.header("authorization") != Some(&format!("Bearer {k}"))
            {
                return json_reply(
                    401,
                    json!({"error": {"message": "Incorrect API key provided", "code": "invalid_api_key"}}),
                );
            }
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            let asked = body["model"].as_str().unwrap_or("");
            let model_missing = || {
                json_reply(
                    404,
                    json!({"error": {"message": format!("The model `{asked}` does not exist"), "code": "model_not_found"}}),
                )
            };
            match (req.method.as_str(), req.path.as_str()) {
                ("POST", "/v1/chat/completions") => {
                    if asked != model {
                        return model_missing();
                    }
                    let content = if !body["response_format"].is_null() && *structured {
                        r#"{"greeting":"Hello","words":1}"#.to_string()
                    } else {
                        "Hello".to_string()
                    };
                    json_reply(
                        200,
                        json!({
                            "id": "chatcmpl-test", "object": "chat.completion", "model": model,
                            "choices": [{"index": 0, "finish_reason": "stop",
                                         "message": {"role": "assistant", "content": content}}],
                            "usage": {"prompt_tokens": 10, "completion_tokens": 2}
                        }),
                    )
                }
                ("POST", "/v1/embeddings") if *embeddings => {
                    if asked != model {
                        return model_missing();
                    }
                    let n = body["input"].as_array().map_or(1, Vec::len);
                    let data: Vec<Value> = (0..n)
                        .map(|i| json!({"object": "embedding", "index": i, "embedding": [0.1, 0.2, 0.3]}))
                        .collect();
                    json_reply(200, json!({"object": "list", "model": model, "data": data}))
                }
                _ => (404, Vec::new(), "404 page not found".into()),
            }
        }
    }
}

/// Reads one HTTP/1.1 request; `None` at the end of the connection.
/// Method, path, headers and body of one request.
type Request = (String, String, Vec<(String, String)>, String);

fn read_request(r: &mut impl BufRead) -> Option<Request> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
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
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if k.eq_ignore_ascii_case("content-length") {
                length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; length];
    r.read_exact(&mut body).ok()?;
    Some((
        method,
        path,
        headers,
        String::from_utf8_lossy(&body).into_owned(),
    ))
}

fn serve<S: Read + Write>(
    stream: S,
    sni: Option<String>,
    behavior: &Behavior,
    seen: &Mutex<Vec<Seen>>,
) {
    let mut r = BufReader::new(stream);
    while let Some((method, path, headers, body)) = read_request(&mut r) {
        let req = Seen {
            method,
            path,
            headers,
            body,
            sni: sni.clone(),
        };
        let (status, extra, out) = answer(behavior, &req);
        lock(seen).push(req);
        let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", out.len());
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        let w = r.get_mut();
        if w.write_all(head.as_bytes()).is_err() || w.write_all(out.as_bytes()).is_err() {
            return;
        }
        let _ = w.flush();
    }
}

/// A TLS certificate for the fake server and the test CA that signed it.
pub struct TestCert {
    /// The CA certificate (DER), to trust in [`crate::TlsRoots::Only`].
    pub ca_der: Vec<u8>,
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl TestCert {
    /// A test CA and a server certificate for `names`.
    pub fn new(names: &[&str]) -> TestCert {
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
        let ca_key = KeyPair::generate().expect("CA key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA cert");
        let issuer = Issuer::new(ca_params, ca_key);
        let key = KeyPair::generate().expect("server key");
        let params =
            CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .expect("server params");
        let cert = params.signed_by(&key, &issuer).expect("server cert");
        TestCert {
            ca_der: ca_cert.der().to_vec(),
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }
}

/// A fake endpoint on a local port, one thread per connection, keep-alive.
pub struct FakeServer {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    /// Plain HTTP on `ip` (`127.0.0.1` in most tests).
    pub fn start(ip: IpAddr, behavior: Behavior) -> FakeServer {
        Self::start_inner(ip, behavior, None)
    }

    /// HTTPS on `ip` with `cert`.
    pub fn start_tls(ip: IpAddr, behavior: Behavior, cert: &TestCert) -> FakeServer {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert.cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.key_der.clone())),
            )
            .expect("server config");
        Self::start_inner(ip, behavior, Some(Arc::new(config)))
    }

    fn start_inner(
        ip: IpAddr,
        behavior: Behavior,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> FakeServer {
        let listener = TcpListener::bind((ip, 0)).expect("bind the fake server");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (seen, stop) = (seen.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                            let (behavior, seen, tls) =
                                (behavior.clone(), seen.clone(), tls.clone());
                            std::thread::spawn(move || match tls {
                                None => serve(stream, None, &behavior, &seen),
                                Some(cfg) => {
                                    let Ok(conn) = rustls::ServerConnection::new(cfg) else {
                                        return;
                                    };
                                    let mut tls = rustls::StreamOwned::new(conn, stream);
                                    // Finish the handshake to learn the server name.
                                    while tls.conn.is_handshaking() {
                                        if tls.conn.complete_io(&mut tls.sock).is_err() {
                                            return;
                                        }
                                    }
                                    let sni = tls.conn.server_name().map(str::to_string);
                                    serve(tls, sni, &behavior, &seen);
                                }
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        FakeServer {
            addr,
            seen,
            stop,
            thread: Some(thread),
        }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Every request so far.
    pub fn seen(&self) -> Vec<Seen> {
        lock(&self.seen).clone()
    }

    pub fn count(&self) -> usize {
        lock(&self.seen).len()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// This machine's address on a private network, if it has one: the source address the OS
/// would use towards a documentation address (no packet is sent). Lets a test serve on an
/// address the gate classifies as Local network.
pub fn private_lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect(("192.0.2.1", 9)).ok()?;
    let ip = s.local_addr().ok()?.ip();
    (crate::policy::classify(ip) == Ok(DestinationClass::LocalNetwork)).then_some(ip)
}

/// A resolver that answers from a script: per host, a queue of answers; the last answer
/// repeats. Counts its calls.
#[derive(Default)]
pub struct ScriptedResolver {
    answers: Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedResolver {
    pub fn new() -> ScriptedResolver {
        ScriptedResolver::default()
    }

    /// `host` resolves to `ips` (after the answers set before it).
    pub fn then(&self, host: &str, ips: &[&str]) -> &ScriptedResolver {
        lock(&self.answers)
            .entry(host.to_string())
            .or_default()
            .push_back(ips.iter().map(|s| s.parse().expect("address")).collect());
        self
    }

    pub fn then_ips(&self, host: &str, ips: Vec<IpAddr>) -> &ScriptedResolver {
        lock(&self.answers)
            .entry(host.to_string())
            .or_default()
            .push_back(ips);
        self
    }

    pub fn calls(&self) -> usize {
        lock(&self.calls).len()
    }
}

impl Resolve for ScriptedResolver {
    fn resolve(&self, host: &str, _port: u16) -> Result<Vec<IpAddr>, EgressError> {
        lock(&self.calls).push(host.to_string());
        let mut answers = lock(&self.answers);
        let q = answers.get_mut(host).ok_or_else(|| {
            EgressError::new(ErrorCode::HostNotFound, format!("Cannot find host {host}"))
        })?;
        let ips = if q.len() > 1 {
            q.pop_front().unwrap_or_default()
        } else {
            q.front().cloned().unwrap_or_default()
        };
        Ok(ips)
    }
}

/// A stand-in for the OS keychain: kind [`SecretStoreKind::Keyring`], keys in memory, and the
/// accounts it holds can be listed (to check a key went there and nowhere else).
#[derive(Default)]
pub struct FakeKeyring {
    inner: SessionStore,
    accounts: Mutex<Vec<String>>,
}

impl FakeKeyring {
    pub fn new() -> FakeKeyring {
        FakeKeyring::default()
    }

    /// The accounts holding a key now.
    pub fn accounts(&self) -> Vec<String> {
        lock(&self.accounts).clone()
    }
}

impl SecretStore for FakeKeyring {
    fn kind(&self) -> SecretStoreKind {
        SecretStoreKind::Keyring
    }

    fn set(&self, account: &str, key: &SecretString) -> Result<(), EgressError> {
        let mut a = lock(&self.accounts);
        if !a.iter().any(|x| x == account) {
            a.push(account.to_string());
        }
        self.inner.set(account, key)
    }

    fn get(&self, account: &str) -> Result<Option<SecretString>, EgressError> {
        self.inner.get(account)
    }

    fn delete(&self, account: &str) -> Result<(), EgressError> {
        lock(&self.accounts).retain(|x| x != account);
        self.inner.delete(account)
    }
}

/// Endpoints in memory.
#[derive(Default)]
pub struct MemoryEndpoints {
    pub map: Mutex<HashMap<String, EndpointConfig>>,
}

impl MemoryEndpoints {
    pub fn with(endpoints: Vec<EndpointConfig>) -> MemoryEndpoints {
        MemoryEndpoints {
            map: Mutex::new(endpoints.into_iter().map(|e| (e.id.clone(), e)).collect()),
        }
    }
}

impl Endpoints for MemoryEndpoints {
    fn endpoint(&self, id: &str) -> Result<Option<EndpointConfig>, EgressError> {
        Ok(lock(&self.map).get(id).cloned())
    }
}

/// A project's permissions and records in memory.
#[derive(Default)]
pub struct MemoryEgressStore {
    pub permissions: Mutex<HashMap<String, Permission>>,
    /// `(host, class, purpose)` in call order.
    pub records: Mutex<Vec<(String, DestinationClass, Purpose)>>,
}

impl MemoryEgressStore {
    pub fn records(&self) -> Vec<(String, DestinationClass, Purpose)> {
        lock(&self.records).clone()
    }

    pub fn set(&self, endpoint_id: &str, p: Permission) {
        lock(&self.permissions).insert(endpoint_id.to_string(), p);
    }

    pub fn get(&self, endpoint_id: &str) -> Permission {
        lock(&self.permissions)
            .get(endpoint_id)
            .cloned()
            .unwrap_or(Permission::NotAllowed)
    }
}

impl EgressStore for MemoryEgressStore {
    fn permission(&self, endpoint_id: &str) -> Result<Permission, EgressError> {
        Ok(self.get(endpoint_id))
    }

    fn needs_reconfirmation(&self, endpoint_id: &str, reason: &str) -> Result<(), EgressError> {
        let mut p = lock(&self.permissions);
        if let Some(x) = p.get_mut(endpoint_id) {
            *x = Permission::NeedsReconfirmation {
                reason: reason.to_string(),
            };
        }
        Ok(())
    }

    fn record(
        &self,
        host: &str,
        class: DestinationClass,
        purpose: Purpose,
    ) -> Result<(), EgressError> {
        lock(&self.records).push((host.to_string(), class, purpose));
        Ok(())
    }
}
