//! Model downloads (US-09.4, NFR-REL-08, NFR-SEC-02, NFR-SEC-11). This module holds the
//! crate's HTTP client (ADR-0009: only `mk-egress` and `mk-models` may have one). It sends no
//! project data: only GET requests for catalog files.
//!
//! - Every hop, redirects included, must be HTTPS to a host in the catalog's
//!   `download_hosts`. Redirects are followed by hand, at most [`MAX_REDIRECTS`]; a hop to any
//!   other host fails before anything is sent to it, and the client's resolver answers only for
//!   listed hosts, so not even a DNS question goes out for another.
//! - Each file is written to `<models>/<model id>/<name>.part`. A resume asks the catalog URL
//!   again (the CDN links it redirects to are signed and expire) with `Range: bytes=<len>-`
//!   and appends only on a `206` whose `Content-Range` starts there; anything else starts the
//!   file again from byte 0.
//! - SHA-256 runs over the bytes as they are written (on resume, over the bytes already on
//!   disk first). A file is renamed to its final name only when its hash matches the catalog
//!   (INV-MM-01); a mismatch deletes it ("Download corrupted. Please retry.").
//! - [`Control`] pauses or cancels between chunks; a stalled connection gives up after
//!   [`DownloadConfig::read_timeout`]. A full disk is an interruption that says so.

use std::collections::VecDeque;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use ureq::Agent;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::Url;

use crate::state::{CORRUPTED, DownloadState};
use crate::{HF_HOST, Model, ModelFile, host_allowed};

/// Redirects followed per request.
pub const MAX_REDIRECTS: usize = 5;
/// How long a connection may take to open.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one read may wait for bytes before the download counts as interrupted.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one name lookup may take.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);
/// The window the speed is measured over (US-09.4 AC2).
pub const SPEED_WINDOW: Duration = Duration::from_secs(5);
/// How often progress is reported.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const CHUNK: usize = 256 * 1024;

/// Where TLS trust comes from.
#[derive(Clone, Debug)]
pub enum TlsRoots {
    /// The OS trust store, through `rustls-platform-verifier` (the app).
    Platform,
    /// These DER certificates only (tests with a test CA).
    Only(Vec<Vec<u8>>),
}

/// Resolves a host for the download client (a port, so tests can point names at a local
/// server and record what was asked).
pub trait HostResolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>>;
}

/// The OS resolver, on a helper thread so a slow DNS gives up after [`RESOLVE_TIMEOUT`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
        let (tx, rx) = mpsc::sync_channel(1);
        let name = host.to_string();
        std::thread::Builder::new()
            .name("mk-models-dns".into())
            .spawn(move || {
                let r = (name.as_str(), port)
                    .to_socket_addrs()
                    .map(|it| it.map(|a| a.ip()).collect::<Vec<_>>());
                let _ = tx.send(r);
            })?;
        rx.recv_timeout(RESOLVE_TIMEOUT)
            .map_err(|_| io::Error::new(ErrorKind::TimedOut, "name lookup timed out"))?
    }
}

/// How downloads reach the network. The default is the app's.
#[derive(Clone)]
pub struct DownloadConfig {
    /// Where model URLs start: `https://huggingface.co`.
    pub origin: String,
    pub resolver: Arc<dyn HostResolver>,
    pub roots: TlsRoots,
    /// Tests only: allow `http://` and any port, for a local fake server.
    pub allow_http: bool,
    pub read_timeout: Duration,
    /// Tests only: writes fail as "disk full" after this many bytes in one run.
    pub disk_full_after: Option<u64>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        DownloadConfig {
            origin: format!("https://{HF_HOST}"),
            resolver: Arc::new(SystemResolver),
            roots: TlsRoots::Platform,
            allow_http: false,
            read_timeout: READ_TIMEOUT,
            disk_full_after: None,
        }
    }
}

impl fmt::Debug for DownloadConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadConfig")
            .field("origin", &self.origin)
            .field("allow_http", &self.allow_http)
            .finish_non_exhaustive()
    }
}

/// Pause and cancel, set from another thread and checked between chunks.
#[derive(Debug, Default)]
pub struct Control {
    pause: AtomicBool,
    cancel: AtomicBool,
}

impl Control {
    pub fn new() -> Arc<Control> {
        Arc::new(Control::default())
    }

    pub fn pause(&self) {
        self.pause.store(true, Ordering::SeqCst);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Progress of a whole model (all its files), US-09.4 AC2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub bytes_done: u64,
    pub total: u64,
    /// Over the last [`SPEED_WINDOW`]; 0 until measured.
    pub speed_bps: u64,
    pub eta_s: Option<u64>,
}

/// What a download reports while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Progress(Progress),
    /// All bytes are in; the last check runs.
    Verifying,
    /// A file matched its checksum and has its final name.
    FileVerified(String),
}

/// How a download that did not fail ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every file is on disk under its final name and matched the catalog.
    Installed,
    /// Stopped by [`Control::pause`]; the partial files are kept.
    Paused,
    /// Stopped by [`Control::cancel`]; the model's folder is deleted.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// A hop went to a host not in the catalog, or not over HTTPS.
    HostNotAllowed,
    /// Connection lost, stalled or refused, or a server error.
    Network,
    DiskFull,
    /// The bytes did not match the catalog; the file was deleted (US-09.4 AC4).
    Corrupted,
    /// Reading or writing the models folder failed.
    Io,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::HostNotAllowed => "download_host_not_allowed",
            ErrorCode::Network => "network",
            ErrorCode::DiskFull => "disk_full",
            ErrorCode::Corrupted => "corrupted",
            ErrorCode::Io => "io",
        }
    }
}

/// A download that failed, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadError {
    pub code: ErrorCode,
    pub message: String,
}

impl DownloadError {
    fn new(code: ErrorCode, message: impl Into<String>) -> DownloadError {
        DownloadError {
            code,
            message: message.into(),
        }
    }

    /// The state the model is in after this error (domain §7.7): a mismatch goes back to
    /// Available (the file is gone), anything else is an interruption that can resume.
    pub fn state_after(&self) -> DownloadState {
        match self.code {
            ErrorCode::Corrupted => DownloadState::Available,
            _ => DownloadState::Interrupted,
        }
    }
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DownloadError {}

fn is_disk_full(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::StorageFull | ErrorKind::QuotaExceeded)
        || matches!(
            e.raw_os_error(),
            Some(28) /* ENOSPC */ | Some(112) /* Windows */
        )
}

fn io_error(e: io::Error) -> DownloadError {
    if is_disk_full(&e) {
        DownloadError::new(
            ErrorCode::DiskFull,
            "Disk full: free some space and resume.",
        )
    } else {
        DownloadError::new(ErrorCode::Io, format!("Cannot write the model file: {e}"))
    }
}

fn lost() -> DownloadError {
    DownloadError::new(
        ErrorCode::Network,
        "The network connection was lost. Resume to continue.",
    )
}

fn corrupted() -> DownloadError {
    DownloadError::new(ErrorCode::Corrupted, CORRUPTED)
}

/// The folder of a model's files.
pub fn model_dir(models_dir: &Path, model: &Model) -> PathBuf {
    models_dir.join(&model.id)
}

/// The final path of a file.
pub fn file_path(models_dir: &Path, model: &Model, file: &ModelFile) -> PathBuf {
    model_dir(models_dir, model).join(&file.name)
}

/// The partial path of a file.
pub fn part_path(models_dir: &Path, model: &Model, file: &ModelFile) -> PathBuf {
    let mut p = file_path(models_dir, model, file).into_os_string();
    p.push(".part");
    PathBuf::from(p)
}

fn len_of(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

/// Bytes of the model already on disk (final files of the right size and partial files).
pub fn bytes_on_disk(models_dir: &Path, model: &Model) -> u64 {
    model
        .files
        .iter()
        .map(|f| {
            match len_of(&file_path(models_dir, model, f)) {
                Some(n) if n == f.bytes => n,
                _ => len_of(&part_path(models_dir, model, f)).unwrap_or(0),
            }
            .min(f.bytes)
        })
        .sum()
}

/// Bytes still to download.
pub fn remaining_bytes(models_dir: &Path, model: &Model) -> u64 {
    model
        .bytes()
        .saturating_sub(bytes_on_disk(models_dir, model))
}

/// Deletes the model's folder (cancel, delete). A folder that is not there is fine.
pub fn remove_model(models_dir: &Path, model: &Model) -> io::Result<()> {
    match std::fs::remove_dir_all(model_dir(models_dir, model)) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The speed and time left over a sliding window.
struct Meter {
    total: u64,
    done: u64,
    samples: VecDeque<(Instant, u64)>,
    last_emit: Option<Instant>,
}

impl Meter {
    fn new(total: u64, done: u64) -> Meter {
        Meter {
            total,
            done,
            samples: VecDeque::from([(Instant::now(), done)]),
            last_emit: None,
        }
    }

    fn add(&mut self, n: u64) {
        self.done += n;
        let now = Instant::now();
        if self
            .samples
            .back()
            .is_none_or(|(t, _)| now.duration_since(*t) >= Duration::from_millis(100))
        {
            self.samples.push_back((now, self.done));
        }
        while self.samples.len() > 2
            && self
                .samples
                .get(1)
                .is_some_and(|(t, _)| now.duration_since(*t) >= SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Rewinds `n` bytes (a file that starts again).
    fn rewind(&mut self, n: u64) {
        self.done = self.done.saturating_sub(n);
        self.samples.clear();
        self.samples.push_back((Instant::now(), self.done));
    }

    fn progress(&self) -> Progress {
        let speed = self.samples.front().map_or(0, |(t, b)| {
            let secs = t.elapsed().as_secs_f64();
            if secs >= 0.5 {
                (self.done.saturating_sub(*b) as f64 / secs) as u64
            } else {
                0
            }
        });
        let left = self.total.saturating_sub(self.done);
        Progress {
            bytes_done: self.done,
            total: self.total,
            speed_bps: speed,
            eta_s: (speed > 0).then(|| left.div_ceil(speed)),
        }
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_emit
            .is_none_or(|t| now.duration_since(t) >= PROGRESS_EVERY)
        {
            self.last_emit = Some(now);
            true
        } else {
            false
        }
    }
}

/// A writer that fails as "disk full" after a number of bytes (tests).
struct Sink {
    file: File,
    left: Option<u64>,
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.left {
            Some(0) => Err(io::Error::from(ErrorKind::StorageFull)),
            Some(left) => {
                let n = self.file.write(&buf[..buf.len().min(left as usize)])?;
                self.left = Some(left - n as u64);
                Ok(n)
            }
            None => self.file.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The client's resolver: listed hosts only, through the injected resolver.
struct ListedOnly {
    hosts: Vec<String>,
    inner: Arc<dyn HostResolver>,
}

impl fmt::Debug for ListedOnly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListedOnly")
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

impl Resolver for ListedOnly {
    fn resolve(
        &self,
        uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let host = uri.host().unwrap_or("");
        if !host_allowed(&self.hosts, host) {
            return Err(ureq::Error::HostNotFound);
        }
        let port = uri.port_u16().unwrap_or_else(|| {
            if uri.scheme_str() == Some("http") {
                80
            } else {
                443
            }
        });
        let ips = self
            .inner
            .resolve(host, port)
            .map_err(|_| ureq::Error::HostNotFound)?;
        let mut out = self.empty();
        for ip in ips.into_iter().take(16) {
            out.push(SocketAddr::new(ip, port));
        }
        if out.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(out)
    }
}

/// Downloads catalog models.
pub struct Downloader {
    cfg: DownloadConfig,
    hosts: Vec<String>,
    agent: Agent,
}

/// How one file ended, when it did not fail.
enum FileEnd {
    Done,
    Paused,
    Cancelled,
}

impl Downloader {
    /// A downloader that may reach only `hosts` (the catalog's `download_hosts`).
    pub fn new(cfg: DownloadConfig, hosts: Vec<String>) -> Downloader {
        let roots = match &cfg.roots {
            TlsRoots::Platform => RootCerts::PlatformVerifier,
            TlsRoots::Only(ders) => RootCerts::new_with_certs(
                &ders
                    .iter()
                    .map(|d| Certificate::from_der(d).to_owned())
                    .collect::<Vec<_>>(),
            ),
        };
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(roots)
            .use_sni(true)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .https_only(!cfg.allow_http)
            .tls_config(tls)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(cfg.read_timeout))
            .timeout_per_read(Some(cfg.read_timeout))
            .user_agent(concat!("mangkon-ai/", env!("CARGO_PKG_VERSION")))
            .build();
        let agent = Agent::with_parts(
            config,
            DefaultConnector::default(),
            ListedOnly {
                hosts: hosts.clone(),
                inner: cfg.resolver.clone(),
            },
        );
        Downloader { cfg, hosts, agent }
    }

    /// Checks one hop: HTTPS (port 443) to a listed host.
    fn check(&self, url: &Url) -> Result<(), DownloadError> {
        let host = url.host_str().unwrap_or("");
        let scheme_ok = url.scheme() == "https" || (self.cfg.allow_http && url.scheme() == "http");
        let port_ok = self.cfg.allow_http || url.port_or_known_default() == Some(443);
        let listed =
            matches!(url.host(), Some(url::Host::Domain(_))) && host_allowed(&self.hosts, host);
        if scheme_ok && port_ok && listed {
            Ok(())
        } else {
            Err(DownloadError::new(
                ErrorCode::HostNotAllowed,
                format!(
                    "The download was sent to {}, which is not a model download host in the \
                     catalog. Nothing was downloaded from it.",
                    if host.is_empty() { url.as_str() } else { host }
                ),
            ))
        }
    }

    /// `GET url` from byte `from`, following redirects to listed hosts.
    fn get(
        &self,
        start: &str,
        from: u64,
    ) -> Result<ureq::http::Response<ureq::Body>, DownloadError> {
        let mut url = Url::parse(start).map_err(|e| {
            DownloadError::new(ErrorCode::HostNotAllowed, format!("Bad URL {start}: {e}"))
        })?;
        for _ in 0..=MAX_REDIRECTS {
            self.check(&url)?;
            let host = url.host_str().unwrap_or("").to_string();
            let mut req = self.agent.get(url.as_str());
            if from > 0 {
                req = req.header("Range", &format!("bytes={from}-"));
            }
            let resp = req.call().map_err(|e| match e {
                ureq::Error::Timeout(_) => lost(),
                ureq::Error::Io(io) if is_disk_full(&io) => io_error(io),
                _ => DownloadError::new(
                    ErrorCode::Network,
                    format!("Cannot reach {host}. Check the network and resume."),
                ),
            })?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let loc = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        DownloadError::new(
                            ErrorCode::Network,
                            format!("{host} answered a redirect without a target"),
                        )
                    })?;
                url = url.join(loc).map_err(|e| {
                    DownloadError::new(
                        ErrorCode::HostNotAllowed,
                        format!("{host} redirected to a bad URL: {e}"),
                    )
                })?;
                continue;
            }
            return Ok(resp);
        }
        Err(DownloadError::new(
            ErrorCode::Network,
            format!("More than {MAX_REDIRECTS} redirects; the download stopped."),
        ))
    }

    /// Downloads every file of `model` that is not verified yet into
    /// `<models_dir>/<model id>/`. `verified(name)` says which files were checked before (they
    /// are not hashed again); `on` gets progress and each file that passes its check.
    pub fn download(
        &self,
        model: &Model,
        models_dir: &Path,
        verified: &dyn Fn(&str) -> bool,
        control: &Control,
        on: &mut dyn FnMut(Event),
    ) -> Result<Outcome, DownloadError> {
        let dir = model_dir(models_dir, model);
        std::fs::create_dir_all(&dir).map_err(io_error)?;
        let mut meter = Meter::new(model.bytes(), bytes_on_disk(models_dir, model));
        on(Event::Progress(meter.progress()));
        let last = model.files.len().saturating_sub(1);
        for (i, file) in model.files.iter().enumerate() {
            let fin = file_path(models_dir, model, file);
            if len_of(&fin) == Some(file.bytes) && verified(&file.name) {
                continue;
            }
            let part = part_path(models_dir, model, file);
            if let Some(parent) = part.parent() {
                std::fs::create_dir_all(parent).map_err(io_error)?;
            }
            if fin.exists() {
                // On disk under its final name but not recorded as checked (the app quit
                // between the rename and the record): check it again as a partial file.
                std::fs::rename(&fin, &part).map_err(io_error)?;
            }
            let url = model.url_at(&self.cfg.origin, file);
            let end = self.fetch(&url, &part, file, i == last, control, &mut meter, on);
            match end {
                Ok(FileEnd::Done) => {
                    std::fs::rename(&part, &fin).map_err(io_error)?;
                    on(Event::FileVerified(file.name.clone()));
                }
                Ok(FileEnd::Paused) => {
                    on(Event::Progress(meter.progress()));
                    return Ok(Outcome::Paused);
                }
                Ok(FileEnd::Cancelled) => {
                    remove_model(models_dir, model).map_err(io_error)?;
                    return Ok(Outcome::Cancelled);
                }
                Err(e) => {
                    if e.code == ErrorCode::Corrupted {
                        let _ = std::fs::remove_file(&part);
                    }
                    on(Event::Progress(meter.progress()));
                    return Err(e);
                }
            }
        }
        on(Event::Progress(meter.progress()));
        Ok(Outcome::Installed)
    }

    /// One file into `part`, resumed from what is there; checked against the catalog.
    #[allow(clippy::too_many_arguments)]
    fn fetch(
        &self,
        url: &str,
        part: &Path,
        file: &ModelFile,
        last: bool,
        control: &Control,
        meter: &mut Meter,
        on: &mut dyn FnMut(Event),
    ) -> Result<FileEnd, DownloadError> {
        let mut have = len_of(part).unwrap_or(0);
        if have > file.bytes {
            // Counted as `file.bytes` by the meter (`bytes_on_disk` caps it).
            std::fs::remove_file(part).map_err(io_error)?;
            meter.rewind(file.bytes);
            have = 0;
        }
        // The bytes already on disk go into the hash first.
        let mut hasher = Sha256::new();
        if have > 0 {
            let mut f = File::open(part).map_err(io_error)?;
            let mut buf = vec![0u8; CHUNK];
            let mut left = have;
            while left > 0 {
                if control.cancelled() {
                    return Ok(FileEnd::Cancelled);
                }
                let n = f
                    .read(&mut buf[..CHUNK.min(left as usize)])
                    .map_err(io_error)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                left -= n as u64;
            }
        }
        if have < file.bytes {
            let mut resp = self.get(url, have)?;
            let status = resp.status().as_u16();
            let mut restart = false;
            match status {
                206 if have > 0 => {
                    let range = resp
                        .headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if range != format!("bytes {have}-{}/{}", file.bytes - 1, file.bytes) {
                        restart = true;
                    }
                }
                200 => restart = have > 0,
                // The range is past the end, or the server ignored it: start again.
                206 | 416 => restart = true,
                s => {
                    return Err(DownloadError::new(
                        ErrorCode::Network,
                        format!("The download server answered HTTP {s}. Resume to try again."),
                    ));
                }
            }
            if restart {
                drop(resp);
                meter.rewind(have);
                have = 0;
                hasher = Sha256::new();
                resp = self.get(url, 0)?;
                if resp.status().as_u16() != 200 {
                    return Err(DownloadError::new(
                        ErrorCode::Network,
                        format!(
                            "The download server answered HTTP {}. Resume to try again.",
                            resp.status().as_u16()
                        ),
                    ));
                }
            }
            let expected = file.bytes - have;
            let length = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            if length.is_some_and(|n| n != expected) {
                let _ = std::fs::remove_file(part);
                meter.rewind(have);
                return Err(corrupted());
            }
            let f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(have == 0)
                .append(have > 0)
                .open(part)
                .map_err(io_error)?;
            let mut sink = Sink {
                file: f,
                left: self.cfg.disk_full_after,
            };
            let mut body = resp.body_mut().as_reader();
            let mut buf = vec![0u8; CHUNK];
            loop {
                if control.cancelled() {
                    return Ok(FileEnd::Cancelled);
                }
                if control.paused() {
                    sink.file.sync_data().map_err(io_error)?;
                    return Ok(FileEnd::Paused);
                }
                let n = match body.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => {
                        let _ = sink.file.sync_data();
                        return Err(lost());
                    }
                };
                if have + n as u64 > file.bytes {
                    let _ = std::fs::remove_file(part);
                    meter.rewind(have);
                    return Err(corrupted());
                }
                if let Err(e) = sink.write_all(&buf[..n]) {
                    let _ = sink.file.sync_data();
                    return Err(io_error(e));
                }
                hasher.update(&buf[..n]);
                have += n as u64;
                meter.add(n as u64);
                if meter.due() {
                    on(Event::Progress(meter.progress()));
                }
            }
            sink.file.sync_all().map_err(io_error)?;
            if have < file.bytes {
                return Err(lost());
            }
        }
        if last {
            on(Event::Verifying);
        }
        let digest = hex(&hasher.finalize());
        if digest != file.sha256 {
            meter.rewind(have);
            return Err(corrupted());
        }
        Ok(FileEnd::Done)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `bytes`, lowercase hex (tests and fixtures).
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
