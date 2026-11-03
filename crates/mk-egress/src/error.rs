//! Plain-words errors with stable codes (US-09.6 AC2, NFR-USE-05). The message never holds an
//! API key or the body that was sent.

use std::fmt;

/// A stable code the UI and tests can match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The base URL is not usable.
    BadUrl,
    /// DNS gave no address for the host.
    HostNotFound,
    /// The host resolves to `0.0.0.0` or `::`.
    UnspecifiedAddress,
    /// Nothing answered at the address and port.
    CannotReach,
    /// The call took too long.
    Timeout,
    /// TLS failed: the certificate is not valid for the host, or the handshake failed.
    BadCertificate,
    /// 401 or 403.
    KeyRefused,
    /// No OpenAI-compatible API at the URL (404, or a reply that is not one).
    NotOpenAi,
    /// The endpoint answered that the model does not exist.
    ModelNotFound,
    /// The endpoint answered with a redirect, which the gate never follows.
    Redirect,
    /// The endpoint has a key reference but the secret store has no key (session ended, or the
    /// keyring entry was removed).
    KeyMissing,
    /// The secret store failed.
    SecretStore,
    /// Any other HTTP error status.
    HttpStatus,
    /// The reply could not be read as the expected JSON.
    BadReply,
    /// The project store failed while checking or recording.
    Store,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadUrl => "bad_url",
            ErrorCode::HostNotFound => "host_not_found",
            ErrorCode::UnspecifiedAddress => "unspecified_address",
            ErrorCode::CannotReach => "cannot_reach",
            ErrorCode::Timeout => "timeout",
            ErrorCode::BadCertificate => "bad_certificate",
            ErrorCode::KeyRefused => "key_refused",
            ErrorCode::NotOpenAi => "not_openai",
            ErrorCode::ModelNotFound => "model_not_found",
            ErrorCode::Redirect => "redirect",
            ErrorCode::KeyMissing => "key_missing",
            ErrorCode::SecretStore => "secret_store",
            ErrorCode::HttpStatus => "http_status",
            ErrorCode::BadReply => "bad_reply",
            ErrorCode::Store => "store",
        }
    }
}

/// A failed call or check, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressError {
    pub code: ErrorCode,
    pub message: String,
}

impl EgressError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> EgressError {
        EgressError {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for EgressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EgressError {}
