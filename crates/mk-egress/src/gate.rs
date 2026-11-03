//! The gate (ADR-0009): the only way to get a [`ModelClient`]. Every call through a client runs
//! all six steps again, so a revoked permission, an edited endpoint or a host that moved stops
//! the very next call:
//!
//! 1. look up the destination (the local runtime, always loopback, or an app-level endpoint);
//! 2. resolve the host to all its addresses and take the most exposed class (INV-EG-03);
//! 3. check the project's permission (INV-EG-04, domain §7.6);
//! 4. record the destination in the project **before** sending (INV-EG-05);
//! 5. send pinned to the checked addresses ([`crate::transport`]);
//! 6. publish the destination for the status bar (INV-EG-06; shown in M5c).

use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::SecretString;
use serde_json::Value;
use ureq::Agent;

use crate::error::{EgressError, ErrorCode};
use crate::openai::{
    ChatReply, ChatRequest, Embeddings, chat_body, embeddings_body, parse_chat, parse_embeddings,
};
use crate::policy::{
    BaseUrl, DestinationClass, Origin, Permission, Purpose, ReconfirmReason, Verdict, check,
};
use crate::resolve::{Resolve, resolve_origin};
use crate::secrets::SecretStore;
use crate::transport::{TlsRoots, pinned_agent, post_json, status_error};

/// An app-level endpoint as the gate needs it (domain §5.2 ENDPOINT).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointConfig {
    pub id: String,
    pub base_url: BaseUrl,
    pub model: String,
    /// The secret-store account of its key; `None` for no key.
    pub secret_ref: Option<String>,
}

/// The app's endpoints (`app.db`), read on every call so an edit or a delete counts at once.
pub trait Endpoints: Send + Sync {
    fn endpoint(&self, id: &str) -> Result<Option<EndpointConfig>, EgressError>;
}

/// The project's side of the gate (BC-11): its permissions and its egress records.
pub trait EgressStore: Sync {
    /// No stored permission is [`Permission::NotAllowed`] (INV-EG-01).
    fn permission(&self, endpoint_id: &str) -> Result<Permission, EgressError>;
    /// Moves a stored permission to "needs reconfirmation" (domain §7.6).
    fn needs_reconfirmation(&self, endpoint_id: &str, reason: &str) -> Result<(), EgressError>;
    /// Upserts the record for this destination and purpose: first and last sent (INV-EG-05).
    fn record(
        &self,
        host: &str,
        class: DestinationClass,
        purpose: Purpose,
    ) -> Result<(), EgressError>;
}

impl<T: EgressStore + ?Sized> EgressStore for &T {
    fn permission(&self, endpoint_id: &str) -> Result<Permission, EgressError> {
        (**self).permission(endpoint_id)
    }

    fn needs_reconfirmation(&self, endpoint_id: &str, reason: &str) -> Result<(), EgressError> {
        (**self).needs_reconfirmation(endpoint_id, reason)
    }

    fn record(
        &self,
        host: &str,
        class: DestinationClass,
        purpose: Purpose,
    ) -> Result<(), EgressError> {
        (**self).record(host, class, purpose)
    }
}

/// Where a call goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// The app's own `llama-server` on `127.0.0.1:port` (INV-MM-05). No permission needed.
    LocalRuntime { port: u16, model: String },
    /// An app-level endpoint, by id.
    Endpoint(String),
}

/// What is being sent where, published before each send (INV-EG-06).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sending {
    pub host: String,
    pub class: DestinationClass,
    pub purpose: Purpose,
}

/// Why the gate sent nothing, or why the call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateError {
    /// The endpoint is not allowed for this project (INV-EG-01). Nothing sent or recorded.
    NotAllowed {
        endpoint_id: String,
        host: String,
        class: DestinationClass,
    },
    /// A confirmed permission no longer holds; it is stored as such and the job is Blocked
    /// until the user acts (INV-PR-09). Nothing sent or recorded.
    NeedsReconfirmation {
        endpoint_id: String,
        reason: ReconfirmReason,
    },
    /// The check could not be made (DNS, store, key), or the call itself failed.
    Failed(EgressError),
}

impl GateError {
    /// True when the gate refused: the job is Blocked, not failed.
    pub fn is_blocked(&self) -> bool {
        !matches!(self, GateError::Failed(_))
    }

    pub fn code(&self) -> &'static str {
        match self {
            GateError::NotAllowed { .. } => "not_allowed",
            GateError::NeedsReconfirmation { .. } => "needs_reconfirmation",
            GateError::Failed(e) => e.code.as_str(),
        }
    }
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateError::NotAllowed { host, class, .. } => write!(
                f,
                "{host} ({}) is not allowed for this project",
                class.label()
            ),
            GateError::NeedsReconfirmation { reason, .. } => {
                write!(f, "Confirm this endpoint again for this project: {reason}")
            }
            GateError::Failed(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for GateError {}

impl From<EgressError> for GateError {
    fn from(e: EgressError) -> Self {
        GateError::Failed(e)
    }
}

/// How long one model call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(600);

type OnSend = Arc<dyn Fn(&Sending) + Send + Sync>;

/// The egress gate.
pub struct Gate {
    resolver: Arc<dyn Resolve>,
    endpoints: Arc<dyn Endpoints>,
    secrets: Arc<dyn SecretStore>,
    roots: TlsRoots,
    timeout: Duration,
    on_send: Option<OnSend>,
}

/// The outcome of steps 1–3: a destination this project may send to now.
struct Checked {
    base: BaseUrl,
    model: String,
    secret_ref: Option<String>,
    ips: Vec<IpAddr>,
    class: DestinationClass,
}

impl Gate {
    pub fn new(
        resolver: Arc<dyn Resolve>,
        endpoints: Arc<dyn Endpoints>,
        secrets: Arc<dyn SecretStore>,
        roots: TlsRoots,
    ) -> Gate {
        Gate {
            resolver,
            endpoints,
            secrets,
            roots,
            timeout: CALL_TIMEOUT,
            on_send: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Gate {
        self.timeout = timeout;
        self
    }

    /// Called with the destination before every send (the status bar, M5c).
    pub fn on_send(mut self, f: impl Fn(&Sending) + Send + Sync + 'static) -> Gate {
        self.on_send = Some(Arc::new(f));
        self
    }

    /// A client for `destination`, for one project and purpose. Fails at once when the project
    /// may not send there now; every call checks again.
    pub fn client<'a>(
        &'a self,
        store: impl EgressStore + 'a,
        destination: Destination,
        purpose: Purpose,
    ) -> Result<ModelClient<'a>, GateError> {
        self.check(&store, &destination)?;
        Ok(ModelClient {
            gate: self,
            store: Box::new(store),
            destination,
            purpose,
            agent: Mutex::new(None),
        })
    }

    /// Steps 1–3.
    fn check(
        &self,
        store: &dyn EgressStore,
        destination: &Destination,
    ) -> Result<Checked, GateError> {
        let (endpoint_id, base, model, secret_ref) = match destination {
            Destination::LocalRuntime { port, model } => {
                (None, BaseUrl::local_runtime(*port), model.clone(), None)
            }
            Destination::Endpoint(id) => match self.endpoints.endpoint(id)? {
                Some(e) => (Some(id.as_str()), e.base_url, e.model, e.secret_ref),
                None => {
                    if store.permission(id)? != Permission::NotAllowed {
                        let reason = ReconfirmReason::EndpointNotFound;
                        store.needs_reconfirmation(id, &reason.to_string())?;
                        return Err(GateError::NeedsReconfirmation {
                            endpoint_id: id.clone(),
                            reason,
                        });
                    }
                    return Err(GateError::Failed(EgressError::new(
                        ErrorCode::BadUrl,
                        "The endpoint is not set up on this computer",
                    )));
                }
            },
        };
        let resolved = resolve_origin(self.resolver.as_ref(), base.origin())?;
        let verdict = match endpoint_id {
            None if resolved.class == DestinationClass::ThisMachine => Verdict::Allow,
            // A local runtime is always on 127.0.0.1; anything else is a bug, never sent.
            None => {
                return Err(GateError::Failed(EgressError::new(
                    ErrorCode::BadUrl,
                    "The local model runtime must be on this machine",
                )));
            }
            Some(id) => check(base.origin(), resolved.class, &store.permission(id)?),
        };
        match verdict {
            Verdict::Allow => Ok(Checked {
                base,
                model,
                secret_ref,
                ips: resolved.ips,
                class: resolved.class,
            }),
            Verdict::NotAllowed => Err(GateError::NotAllowed {
                endpoint_id: endpoint_id.unwrap_or_default().to_string(),
                host: base.origin().host.clone(),
                class: resolved.class,
            }),
            Verdict::NeedsReconfirmation(reason) => {
                let id = endpoint_id.unwrap_or_default();
                if !matches!(reason, ReconfirmReason::Stored(_)) {
                    store.needs_reconfirmation(id, &reason.to_string())?;
                }
                Err(GateError::NeedsReconfirmation {
                    endpoint_id: id.to_string(),
                    reason,
                })
            }
        }
    }
}

/// A model client for one project, destination and purpose. Only [`Gate::client`] makes one
/// (ADR-0009 option 2: a missing check is a compile error).
pub struct ModelClient<'a> {
    gate: &'a Gate,
    store: Box<dyn EgressStore + 'a>,
    destination: Destination,
    purpose: Purpose,
    /// The pinned agent of the last call and the addresses it is pinned to. A new address set
    /// gets a new agent, so a pooled connection never outlives its check.
    agent: Mutex<Option<(Origin, Vec<IpAddr>, Agent)>>,
}

impl ModelClient<'_> {
    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    fn agent(&self, c: &Checked) -> Agent {
        let mut slot = self.agent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((o, ips, a)) = slot.as_ref()
            && o == c.base.origin()
            && ips == &c.ips
        {
            return a.clone();
        }
        let a = pinned_agent(c.base.origin(), &c.ips, &self.gate.roots, self.gate.timeout);
        *slot = Some((c.base.origin().clone(), c.ips.clone(), a.clone()));
        a
    }

    /// Steps 1–6 and the send. Returns the reply body, its time and the model name used.
    fn send(
        &self,
        path: &str,
        body: impl FnOnce(&str) -> Value,
    ) -> Result<(String, u64), GateError> {
        let c = self.gate.check(self.store.as_ref(), &self.destination)?;
        // The key is read now, from the secret store, and dropped after the call (ADR-0012).
        let key: Option<SecretString> = match &c.secret_ref {
            Some(r) => Some(self.gate.secrets.get(r)?.ok_or_else(|| {
                EgressError::new(
                    ErrorCode::KeyMissing,
                    "The API key for this endpoint is missing. Enter it again in AI models.",
                )
            })?),
            None => None,
        };
        let host = c.base.origin().host.clone();
        self.store.record(&host, c.class, self.purpose)?;
        if let Some(f) = &self.gate.on_send {
            f(&Sending {
                host,
                class: c.class,
                purpose: self.purpose,
            });
        }
        let agent = self.agent(&c);
        let reply = post_json(
            &agent,
            c.base.origin(),
            &c.base.join(path),
            key.as_ref(),
            &body(&c.model),
        )?;
        if !(200..300).contains(&reply.status) {
            return Err(status_error(reply.status, &reply.body, &c.model, key.as_ref()).into());
        }
        Ok((reply.body, reply.ms))
    }

    /// One chat completion.
    pub fn chat(&self, req: &ChatRequest) -> Result<ChatReply, GateError> {
        let (body, ms) = self.send("chat/completions", |model| chat_body(model, req))?;
        Ok(parse_chat(&body, ms)?)
    }

    /// Embeddings for `inputs`, in input order.
    pub fn embeddings(&self, inputs: &[String]) -> Result<Embeddings, GateError> {
        let (body, ms) = self.send("embeddings", |model| embeddings_body(model, inputs))?;
        Ok(parse_embeddings(&body, inputs.len(), ms)?)
    }
}
