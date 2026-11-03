//! The HTTP client, pinned to the addresses the gate checked (ADR-0009 step 5, spike S5).
//!
//! ureq's resolver is replaced by one that returns only the checked addresses and never asks
//! DNS, so a DNS change between the check and the send cannot move the request. TLS still
//! verifies the certificate against the URL's **host name** and sends it as SNI, because ureq
//! takes both from the URI, not from the address. Redirects are never followed (a redirect
//! would leave the checked host) and proxies are off (a proxy would break the pin).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use ureq::Agent;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::error::{EgressError, ErrorCode};
use crate::policy::Origin;

/// Where TLS trust comes from.
#[derive(Clone, Debug)]
pub enum TlsRoots {
    /// The OS trust store, through `rustls-platform-verifier` (the app).
    Platform,
    /// These DER certificates only (tests with a test CA).
    Only(Vec<Vec<u8>>),
}

/// How long a connection may take to open.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The largest reply body read (an embeddings batch is the biggest).
const MAX_BODY: u64 = 64 * 1024 * 1024;

/// The resolver of a pinned agent: the checked addresses for the checked host, and nothing for
/// any other host.
#[derive(Debug)]
struct Pinned {
    host: String,
    addrs: Vec<SocketAddr>,
}

impl Resolver for Pinned {
    fn resolve(
        &self,
        uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let host = uri.host().unwrap_or("").trim_matches(['[', ']']);
        if !host.eq_ignore_ascii_case(&self.host) {
            return Err(ureq::Error::HostNotFound);
        }
        let mut out = self.empty();
        for a in self.addrs.iter().take(16) {
            out.push(*a);
        }
        if out.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(out)
    }
}

/// An agent that can reach only `origin` at `ips`.
pub(crate) fn pinned_agent(
    origin: &Origin,
    ips: &[IpAddr],
    roots: &TlsRoots,
    timeout: Duration,
) -> Agent {
    let roots = match roots {
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
        .tls_config(tls)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(timeout))
        .user_agent(concat!("mangkon-ai/", env!("CARGO_PKG_VERSION")))
        .build();
    let addrs = ips
        .iter()
        .map(|ip| SocketAddr::new(*ip, origin.port))
        .collect();
    Agent::with_parts(
        config,
        DefaultConnector::default(),
        Pinned {
            host: origin.host.clone(),
            addrs,
        },
    )
}

/// One HTTP reply, read in full.
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    pub ms: u64,
}

/// Whether `e` (or what it wraps) is a rustls error, and if so whether it is about the
/// certificate.
fn tls_failure(e: &ureq::Error) -> Option<bool> {
    fn is_cert(e: &rustls::Error) -> bool {
        matches!(
            e,
            rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented
        )
    }
    match e {
        ureq::Error::Rustls(r) => Some(is_cert(r)),
        ureq::Error::Tls(_) => Some(false),
        ureq::Error::Io(io) => io
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
            .map(is_cert),
        _ => None,
    }
}

/// A transport error in plain words (US-09.6 AC2).
pub(crate) fn transport_error(e: &ureq::Error, origin: &Origin) -> EgressError {
    if let Some(cert) = tls_failure(e) {
        return if cert {
            EgressError::new(
                ErrorCode::BadCertificate,
                format!("The certificate of {} is not valid", origin.host),
            )
        } else {
            EgressError::new(
                ErrorCode::BadCertificate,
                format!("A secure connection to {} failed", origin.host),
            )
        };
    }
    match e {
        ureq::Error::Timeout(_) => EgressError::new(
            ErrorCode::Timeout,
            format!("{} did not answer in time", origin.authority()),
        ),
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => EgressError::new(
            ErrorCode::Redirect,
            "The endpoint answered with a redirect, which is never followed. Enter the address \
             it redirects to.",
        ),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => EgressError::new(
            ErrorCode::Timeout,
            format!("{} did not answer in time", origin.authority()),
        ),
        _ => EgressError::new(
            ErrorCode::CannotReach,
            format!("Cannot reach {}", origin.authority()),
        ),
    }
}

/// `POST url` with a JSON body and the key as a bearer token, read at this moment.
pub(crate) fn post_json(
    agent: &Agent,
    origin: &Origin,
    url: &str,
    key: Option<&SecretString>,
    body: &Value,
) -> Result<Reply, EgressError> {
    let started = Instant::now();
    let mut req = agent.post(url);
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {}", k.expose_secret()));
    }
    let mut resp = req
        .send_json(body)
        .map_err(|e| transport_error(&e, origin))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .map_err(|e| transport_error(&e, origin))?;
    Ok(Reply {
        status,
        body,
        ms: started.elapsed().as_millis() as u64,
    })
}

/// The error text in an OpenAI-style error reply (`{"error": {"message": …}}` or
/// `{"error": "…"}`), or the start of the body.
fn reply_message(body: &str) -> String {
    let v: Option<Value> = serde_json::from_str(body).ok();
    let msg = v
        .as_ref()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .or_else(|| v["detail"].as_str())
        })
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string());
    msg.chars().take(200).collect()
}

fn says_model_missing(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    b.contains("model_not_found")
        || (b.contains("model")
            && (b.contains("not found") || b.contains("does not exist") || b.contains("no such")))
}

/// A reply status other than 2xx in plain words (US-09.6 AC2). The server's own words are
/// kept for other errors, with the key cut out in case the server echoes it.
pub(crate) fn status_error(
    status: u16,
    body: &str,
    model: &str,
    key: Option<&SecretString>,
) -> EgressError {
    match status {
        401 | 403 => EgressError::new(ErrorCode::KeyRefused, "The endpoint refused the API key"),
        404 | 400 | 422 if says_model_missing(body) => EgressError::new(
            ErrorCode::ModelNotFound,
            format!("Model '{model}' not found"),
        ),
        404 | 405 => EgressError::new(ErrorCode::NotOpenAi, "No OpenAI-compatible API at this URL"),
        300..=399 => EgressError::new(
            ErrorCode::Redirect,
            "The endpoint answered with a redirect, which is never followed. Enter the address \
             it redirects to.",
        ),
        _ => {
            let mut msg = reply_message(body);
            if let Some(k) = key {
                let k = k.expose_secret();
                if !k.is_empty() {
                    msg = msg.replace(k, "[key]");
                }
            }
            EgressError::new(
                ErrorCode::HttpStatus,
                format!("The endpoint answered HTTP {status}: {msg}"),
            )
        }
    }
}
