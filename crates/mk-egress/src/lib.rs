//! BC-11 Egress policy (ADR-0009): the one gate project data leaves through, and API keys in
//! the OS secret store (ADR-0012).
//!
//! - [`policy`]: pure rules. Address classes exactly as US-09.7 AC4, the most exposed class
//!   wins (INV-EG-03), origins and base URLs, the permission check (INV-EG-04, domain §7.6).
//! - [`Gate`]: the only maker of a [`ModelClient`]. Each call looks up the destination, resolves
//!   all addresses, checks the project's permission, records the destination **before**
//!   sending (INV-EG-05) and sends pinned to the checked addresses with TLS checked against the
//!   host name (spike S5).
//! - [`endpoint_test`]: fixed test texts only, no project (INV-EG-08).
//! - [`SecretStore`]: the OS keychain ([`KeyringStore`]) or, with none, keys for this session
//!   only ([`SessionStore`]).
//!
//! Ports for the project store ([`EgressStore`]), the app's endpoints ([`Endpoints`]) and
//! name resolution ([`Resolve`]) are implemented by `mk-core` (ADR-0002 rule 2). Plain `std`
//! threads, no async runtime.

mod error;
mod gate;
mod openai;
pub mod policy;
mod probe;
mod resolve;
mod secrets;
mod transport;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use error::{EgressError, ErrorCode};
pub use gate::{
    CALL_TIMEOUT, Destination, EgressStore, EndpointConfig, Endpoints, Finished, Gate, GateError,
    ModelClient, Sending,
};
pub use openai::{ChatMessage, ChatReply, ChatRequest, Embeddings, ReplySchema};
pub use policy::{
    BadUrl, BaseUrl, DestinationClass, Origin, Permission, Purpose, ReconfirmReason, Scheme,
    Verdict, classify, most_exposed,
};
pub use probe::{
    ChatProbe, EmbeddingsProbe, NO_STRUCTURED_OUTPUT, TEST_EMBED_TEXT, TEST_TEXT, TEST_TIMEOUT,
    TestReport, TestTarget, embeddings_test, endpoint_test,
};
pub use resolve::{RESOLVE_TIMEOUT, Resolve, Resolved, SystemResolver, resolve_origin};
pub use secrecy::{ExposeSecret, SecretString};
pub use secrets::{
    KeyringStore, SERVICE, SESSION_ONLY_NOTICE, SecretStore, SecretStoreKind, SessionStore,
    account_for, system_secret_store,
};
pub use transport::TlsRoots;
