//! External endpoints and the egress gate (M5a, BC-11, ADR-0009, ADR-0012).
//!
//! - App level ([`Workspace`]): endpoints in `app.db`, their keys in the secret store only
//!   (US-09.6 AC1, INV-EG-07), the endpoint test with fixed texts (US-09.6 AC2/AC3, INV-EG-08).
//! - Project level: the permission list with each endpoint's live class, allow (prepare, then
//!   confirm the exact host and class, US-09.7 AC2), revoke, the egress records (US-09.8 AC2),
//!   and [`Workspace::model_client`], the only way to a model client for a project.

use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mk_egress::policy::check;
use mk_egress::{
    BadUrl, BaseUrl, Destination, DestinationClass, EgressError, EgressStore, EndpointConfig,
    Endpoints, ErrorCode, ExposeSecret, Gate, GateError, ModelClient, Origin, Permission, Purpose,
    ReconfirmReason, Resolve, SESSION_ONLY_NOTICE, SecretStore, SecretStoreKind, SecretString,
    SystemResolver, TestReport, TestTarget, TlsRoots, Verdict, account_for, endpoint_test,
    resolve_origin,
};
use mk_store::{AppStore, EndpointRow, EndpointTestRow, Store, StoreError};

use crate::project::Project;
use crate::workspace::Workspace;

/// An endpoint call or change that failed, in plain words with a stable code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointError {
    pub code: &'static str,
    pub message: String,
}

impl EndpointError {
    fn new(code: &'static str, message: impl Into<String>) -> EndpointError {
        EndpointError {
            code,
            message: message.into(),
        }
    }

    fn not_found(id: &str) -> EndpointError {
        EndpointError::new("endpoint_not_found", format!("No endpoint {id}"))
    }
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EndpointError {}

impl From<StoreError> for EndpointError {
    fn from(e: StoreError) -> Self {
        EndpointError::new("store", e.to_string())
    }
}

impl From<EgressError> for EndpointError {
    fn from(e: EgressError) -> Self {
        EndpointError::new(e.code.as_str(), e.message)
    }
}

impl From<BadUrl> for EndpointError {
    fn from(e: BadUrl) -> Self {
        EndpointError::new("bad_url", e.0)
    }
}

/// The secret store, picked on first use: the OS keychain if it answers, else this session
/// only (ADR-0012). Lazy, so opening the app (or a test) never touches the keychain.
pub(crate) struct LazySecrets {
    preset: Option<Arc<dyn SecretStore>>,
    picked: OnceLock<Arc<dyn SecretStore>>,
}

impl LazySecrets {
    fn store(&self) -> &Arc<dyn SecretStore> {
        self.picked.get_or_init(|| match &self.preset {
            Some(s) => s.clone(),
            None => Arc::from(mk_egress::system_secret_store()),
        })
    }
}

impl SecretStore for LazySecrets {
    fn kind(&self) -> SecretStoreKind {
        self.store().kind()
    }

    fn set(&self, account: &str, key: &SecretString) -> Result<(), EgressError> {
        self.store().set(account, key)
    }

    fn get(&self, account: &str) -> Result<Option<SecretString>, EgressError> {
        self.store().get(account)
    }

    fn delete(&self, account: &str) -> Result<(), EgressError> {
        self.store().delete(account)
    }
}

/// How the workspace reaches the network. The default is the app's: the OS resolver, the OS
/// secret store and the OS trust store. Tests pass fakes.
pub struct EgressConfig {
    pub resolver: Arc<dyn Resolve>,
    /// `None`: the OS keychain, or a session store when there is none.
    pub secrets: Option<Arc<dyn SecretStore>>,
    pub roots: TlsRoots,
    /// How long one model call may take.
    pub call_timeout: Duration,
}

impl Default for EgressConfig {
    fn default() -> Self {
        EgressConfig {
            resolver: Arc::new(SystemResolver::default()),
            secrets: None,
            roots: TlsRoots::Platform,
            call_timeout: mk_egress::CALL_TIMEOUT,
        }
    }
}

/// The app's endpoints for the gate, read from `app.db` on every call.
struct AppEndpoints(Arc<AppStore>);

fn config_of(row: EndpointRow) -> Result<EndpointConfig, EgressError> {
    Ok(EndpointConfig {
        base_url: BaseUrl::parse(&row.base_url)
            .map_err(|e| EgressError::new(ErrorCode::BadUrl, e.0))?,
        id: row.id,
        model: row.model_name,
        secret_ref: row.secret_ref,
    })
}

fn store_error(e: StoreError) -> EgressError {
    EgressError::new(ErrorCode::Store, e.to_string())
}

impl Endpoints for AppEndpoints {
    fn endpoint(&self, id: &str) -> Result<Option<EndpointConfig>, EgressError> {
        self.0
            .endpoint(id)
            .map_err(store_error)?
            .map(config_of)
            .transpose()
    }
}

/// The project's permissions and records for the gate.
struct ProjectEgress<'a>(&'a Store);

fn permission_of(row: Option<mk_store::PermissionRow>) -> Permission {
    let Some(row) = row else {
        return Permission::NotAllowed;
    };
    let origin = Origin::parse_key(&row.confirmed_origin);
    let class = DestinationClass::parse(&row.confirmed_class);
    match (row.state.as_str(), origin, class) {
        ("allowed", Some(origin), Some(class)) => Permission::Allowed {
            origin,
            class,
            at_ms: 0,
        },
        ("allowed", ..) => Permission::NeedsReconfirmation {
            reason: "The stored permission could not be read".into(),
        },
        _ => Permission::NeedsReconfirmation {
            reason: row
                .reason
                .unwrap_or_else(|| "Confirm this endpoint again".into()),
        },
    }
}

impl EgressStore for ProjectEgress<'_> {
    fn permission(&self, endpoint_id: &str) -> Result<Permission, EgressError> {
        Ok(permission_of(
            self.0
                .endpoint_permission(endpoint_id)
                .map_err(store_error)?,
        ))
    }

    fn needs_reconfirmation(&self, endpoint_id: &str, reason: &str) -> Result<(), EgressError> {
        self.0
            .permission_needs_reconfirmation(endpoint_id, reason)
            .map(|_| ())
            .map_err(store_error)
    }

    fn record(
        &self,
        host: &str,
        class: DestinationClass,
        purpose: Purpose,
    ) -> Result<(), EgressError> {
        self.0
            .record_egress(host, class.as_str(), purpose.as_str())
            .map_err(store_error)
    }
}

/// The workspace's egress side: the gate and what it is made of.
pub(crate) struct EgressParts {
    pub(crate) resolver: Arc<dyn Resolve>,
    pub(crate) secrets: Arc<LazySecrets>,
    pub(crate) roots: TlsRoots,
    pub(crate) gate: Gate,
}

impl EgressParts {
    pub(crate) fn new(app: Arc<AppStore>, config: EgressConfig) -> EgressParts {
        let secrets = Arc::new(LazySecrets {
            preset: config.secrets,
            picked: OnceLock::new(),
        });
        let gate = Gate::new(
            config.resolver.clone(),
            Arc::new(AppEndpoints(app)),
            secrets.clone(),
            config.roots.clone(),
        )
        .with_timeout(config.call_timeout);
        EgressParts {
            resolver: config.resolver,
            secrets,
            roots: config.roots,
            gate,
        }
    }
}

/// Where an endpoint's host points now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationInfo {
    /// `scheme://host:port`, what a permission is confirmed for.
    pub origin: String,
    pub host: String,
    pub port: u16,
    pub class: DestinationClass,
    pub addresses: Vec<IpAddr>,
    /// False for plain `http`: the key and the data go unencrypted.
    pub encrypted: bool,
}

/// Whether an endpoint's key is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// The endpoint has no key.
    None,
    Stored,
    /// It had one, but the secret store has none now (a session-only key after a restart).
    Missing,
}

/// The last test of an endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointTestInfo {
    pub tested_at_ms: i64,
    pub ok: bool,
    pub message: String,
    pub response_ms: Option<i64>,
    pub reported_model: Option<String>,
}

/// An endpoint as the AI models screen shows it. Never the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub key: KeyState,
    pub structured_output: Option<bool>,
    pub last_test: Option<EndpointTestInfo>,
    /// Resolved now, `Err` with the plain reason when it could not be.
    pub destination: Result<DestinationInfo, String>,
}

/// What to do with the key on save.
pub enum KeyChange {
    Keep,
    Set(SecretString),
    Remove,
}

/// A new endpoint (`id: None`) or an edit.
pub struct EndpointInput {
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub key: KeyChange,
}

/// Where keys are kept now, and the plain-words notice when only for this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretStoreStatus {
    pub kind: SecretStoreKind,
    pub notice: Option<&'static str>,
}

/// A project's view of one endpoint (domain §7.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Resolves to this machine: no permission needed.
    ThisMachine,
    NotAllowed,
    Allowed {
        class: DestinationClass,
        origin: String,
        confirmed_at: String,
    },
    NeedsReconfirmation {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectEndpoint {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub destination: Result<DestinationInfo, String>,
    pub access: Access,
}

/// What the confirm dialog shows (US-09.7 AC2, INV-EG-02).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowPrompt {
    pub endpoint_id: String,
    pub name: String,
    pub destination: DestinationInfo,
    /// False for This machine: nothing to confirm.
    pub needs_confirmation: bool,
}

/// One destination that received this project's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressRecordInfo {
    pub host: String,
    pub class: DestinationClass,
    pub purpose: Purpose,
    pub first_sent_at: String,
    pub last_sent_at: String,
}

/// 128 random bits in hex: an endpoint id never matches one made on another machine.
fn new_id() -> Result<String, EndpointError> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b)
        .map_err(|e| EndpointError::new("internal", format!("no random source: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn destination_of(resolver: &dyn Resolve, base: &BaseUrl) -> Result<DestinationInfo, EgressError> {
    let r = resolve_origin(resolver, base.origin())?;
    let o = base.origin();
    Ok(DestinationInfo {
        origin: o.key(),
        host: o.host.clone(),
        port: o.port,
        class: r.class,
        addresses: r.ips,
        encrypted: o.encrypted(),
    })
}

fn test_info(t: EndpointTestRow) -> EndpointTestInfo {
    EndpointTestInfo {
        tested_at_ms: t.tested_at,
        ok: t.ok,
        message: t.message,
        response_ms: t.response_ms,
        reported_model: t.reported_model,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Resolves every row at once, each lookup bounded by the resolver's timeout.
fn resolve_all(
    resolver: &dyn Resolve,
    rows: &[EndpointRow],
) -> Vec<Result<DestinationInfo, String>> {
    std::thread::scope(|s| {
        let handles: Vec<_> = rows
            .iter()
            .map(|row| {
                s.spawn(move || {
                    let base = BaseUrl::parse(&row.base_url).map_err(|e| e.0)?;
                    destination_of(resolver, &base).map_err(|e| e.message)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("lookup failed".into())))
            .collect()
    })
}

/// Tests the endpoint at `base_url` with fixed texts, without saving anything (the CLI suite
/// runner, NFR-COMP-05). No project is involved (INV-EG-08).
pub fn test_endpoint_at(
    base_url: &str,
    model: &str,
    key: Option<SecretString>,
) -> Result<TestReport, EndpointError> {
    let base = BaseUrl::parse(base_url)?;
    Ok(endpoint_test(
        &SystemResolver::default(),
        &TlsRoots::Platform,
        TestTarget {
            base_url: &base,
            model,
            key: key.as_ref(),
        },
    ))
}

impl Workspace {
    fn egress_parts(&self) -> &EgressParts {
        &self.egress
    }

    /// Where keys go now (ADR-0012). Asks the OS keychain on first use.
    pub fn secret_store_status(&self) -> SecretStoreStatus {
        let kind = self.egress_parts().secrets.kind();
        SecretStoreStatus {
            kind,
            notice: (kind == SecretStoreKind::SessionOnly).then_some(SESSION_ONLY_NOTICE),
        }
    }

    fn key_state(&self, row: &EndpointRow) -> KeyState {
        match &row.secret_ref {
            None => KeyState::None,
            Some(r) => match self.egress_parts().secrets.get(r) {
                Ok(Some(_)) => KeyState::Stored,
                _ => KeyState::Missing,
            },
        }
    }

    fn info(&self, row: EndpointRow, destination: Result<DestinationInfo, String>) -> EndpointInfo {
        let last_test = self
            .app()
            .last_endpoint_test(&row.id)
            .ok()
            .flatten()
            .map(test_info);
        EndpointInfo {
            key: self.key_state(&row),
            id: row.id,
            name: row.name,
            base_url: row.base_url,
            model: row.model_name,
            structured_output: row.structured_output,
            last_test,
            destination,
        }
    }

    /// Every endpoint with its live destination class (US-09.7 AC4), looked up in parallel.
    pub fn endpoints(&self) -> Result<Vec<EndpointInfo>, EndpointError> {
        let rows = self.app().endpoints()?;
        let dests = resolve_all(self.egress_parts().resolver.as_ref(), &rows);
        Ok(rows
            .into_iter()
            .zip(dests)
            .map(|(row, d)| self.info(row, d))
            .collect())
    }

    pub fn endpoint(&self, id: &str) -> Result<EndpointInfo, EndpointError> {
        let row = self
            .app()
            .endpoint(id)?
            .ok_or_else(|| EndpointError::not_found(id))?;
        let d = resolve_all(
            self.egress_parts().resolver.as_ref(),
            std::slice::from_ref(&row),
        )
        .pop()
        .unwrap_or_else(|| Err("lookup failed".into()));
        Ok(self.info(row, d))
    }

    /// Where `base_url` points now: its host, class and addresses (shown in the form).
    pub fn resolve_destination(&self, base_url: &str) -> Result<DestinationInfo, EndpointError> {
        let base = BaseUrl::parse(base_url)?;
        Ok(destination_of(
            self.egress_parts().resolver.as_ref(),
            &base,
        )?)
    }

    /// Adds or edits an endpoint (US-09.6 AC1). The key goes to the secret store and only its
    /// reference to `app.db` (INV-EG-07).
    pub fn save_endpoint(&self, input: EndpointInput) -> Result<EndpointInfo, EndpointError> {
        let base = BaseUrl::parse(&input.base_url)?;
        let model = input.model.trim();
        if model.is_empty() {
            return Err(EndpointError::new("bad_model", "Enter the model name"));
        }
        let name = match input.name.trim() {
            "" => base.origin().host.clone(),
            n => n.chars().take(80).collect(),
        };
        let secrets = &self.egress_parts().secrets;
        let id = match &input.id {
            Some(id) => id.clone(),
            None => new_id()?,
        };
        let account = account_for(&id);
        let existing = match &input.id {
            Some(id) => Some(
                self.app()
                    .endpoint(id)?
                    .ok_or_else(|| EndpointError::not_found(id))?,
            ),
            None => None,
        };
        let secret_ref = match &input.key {
            KeyChange::Set(k) if !k.expose_secret().is_empty() => {
                secrets.set(&account, k)?;
                Some(account.clone())
            }
            KeyChange::Set(_) | KeyChange::Remove => {
                secrets.delete(&account)?;
                None
            }
            KeyChange::Keep => existing.as_ref().and_then(|e| e.secret_ref.clone()),
        };
        let saved = match existing {
            Some(_) => self
                .app()
                .update_endpoint(&id, &name, base.as_str(), model, secret_ref.as_deref())
                .map(|_| ()),
            None => {
                let now = now_ms();
                self.app().insert_endpoint(&EndpointRow {
                    id: id.clone(),
                    name,
                    base_url: base.as_str().to_string(),
                    model_name: model.to_string(),
                    secret_ref: secret_ref.clone(),
                    structured_output: None,
                    created_at: now,
                    updated_at: now,
                })
            }
        };
        if let Err(e) = saved {
            if input.id.is_none() && secret_ref.is_some() {
                let _ = secrets.delete(&account);
            }
            return Err(e.into());
        }
        self.endpoint(&id)
    }

    /// Deletes an endpoint and its key (ADR-0012). Projects that allowed it fall to "needs
    /// reconfirmation" on their next call (domain §7.6).
    pub fn delete_endpoint(&self, id: &str) -> Result<(), EndpointError> {
        let row = self
            .app()
            .endpoint(id)?
            .ok_or_else(|| EndpointError::not_found(id))?;
        if let Some(r) = &row.secret_ref {
            self.egress_parts().secrets.delete(r)?;
        }
        self.app().delete_endpoint(id)?;
        Ok(())
    }

    /// Tests a saved endpoint with fixed texts (US-09.6 AC2/AC3, INV-EG-08) and stores the
    /// result and the structured-output flag. Takes no project.
    pub fn test_endpoint(&self, id: &str) -> Result<TestReport, EndpointError> {
        let row = self
            .app()
            .endpoint(id)?
            .ok_or_else(|| EndpointError::not_found(id))?;
        let base = BaseUrl::parse(&row.base_url)?;
        let key = match &row.secret_ref {
            Some(r) => Some(self.egress_parts().secrets.get(r)?.ok_or_else(|| {
                EndpointError::new(
                    ErrorCode::KeyMissing.as_str(),
                    "The API key for this endpoint is missing. Enter it again.",
                )
            })?),
            None => None,
        };
        let parts = self.egress_parts();
        let report = endpoint_test(
            parts.resolver.as_ref(),
            &parts.roots,
            TestTarget {
                base_url: &base,
                model: &row.model_name,
                key: key.as_ref(),
            },
        );
        drop(key);
        let (ms, model) = match &report.chat {
            Ok(c) => (Some(c.response_ms as i64), c.reported_model.clone()),
            Err(_) => (None, None),
        };
        self.app().record_endpoint_test(
            &EndpointTestRow {
                endpoint_id: id.to_string(),
                tested_at: now_ms(),
                ok: report.ok(),
                message: report.summary(),
                response_ms: ms,
                reported_model: model,
            },
            report.structured_output,
        )?;
        Ok(report)
    }

    /// The project's view of every endpoint: its live class and its permission, with the
    /// check applied to the live class, so a host that moved shows "needs reconfirmation"
    /// before any call (shown only; the stored state changes on the next call).
    pub fn project_endpoints(
        &self,
        project: &Project,
    ) -> Result<Vec<ProjectEndpoint>, EndpointError> {
        let rows = self.app().endpoints()?;
        let dests = resolve_all(self.egress_parts().resolver.as_ref(), &rows);
        let mut out = Vec::with_capacity(rows.len());
        for (row, dest) in rows.into_iter().zip(dests) {
            let stored = project.store().endpoint_permission(&row.id)?;
            let confirmed_at = stored.as_ref().map(|p| p.confirmed_at.clone());
            let permission = permission_of(stored);
            let access = match (&dest, &permission) {
                (Ok(d), _) if d.class == DestinationClass::ThisMachine => Access::ThisMachine,
                (_, Permission::NotAllowed) => Access::NotAllowed,
                (_, Permission::NeedsReconfirmation { reason }) => Access::NeedsReconfirmation {
                    reason: reason.clone(),
                },
                (Ok(d), Permission::Allowed { origin, class, .. }) => {
                    let now = Origin::parse_key(&d.origin);
                    match now.map(|o| check(&o, d.class, &permission)) {
                        Some(Verdict::NeedsReconfirmation(r)) => Access::NeedsReconfirmation {
                            reason: r.to_string(),
                        },
                        _ => Access::Allowed {
                            class: *class,
                            origin: origin.key(),
                            confirmed_at: confirmed_at.unwrap_or_default(),
                        },
                    }
                }
                // The host does not resolve now: show the stored permission.
                (Err(_), Permission::Allowed { origin, class, .. }) => Access::Allowed {
                    class: *class,
                    origin: origin.key(),
                    confirmed_at: confirmed_at.unwrap_or_default(),
                },
            };
            out.push(ProjectEndpoint {
                id: row.id,
                name: row.name,
                base_url: row.base_url,
                model: row.model_name,
                destination: dest,
                access,
            });
        }
        Ok(out)
    }

    /// Step one of "Allow for this project…" (US-09.7 AC2): what the confirm dialog shows,
    /// resolved now. Nothing is stored.
    pub fn prepare_allow(&self, id: &str) -> Result<AllowPrompt, EndpointError> {
        let row = self
            .app()
            .endpoint(id)?
            .ok_or_else(|| EndpointError::not_found(id))?;
        let base = BaseUrl::parse(&row.base_url)?;
        let destination = destination_of(self.egress_parts().resolver.as_ref(), &base)?;
        Ok(AllowPrompt {
            endpoint_id: row.id,
            name: row.name,
            needs_confirmation: destination.class != DestinationClass::ThisMachine,
            destination,
        })
    }

    /// Step two: the user confirmed `origin` and `class` exactly as shown (INV-EG-02). The
    /// host is resolved again; the permission is stored only if it still is that origin and
    /// class, so a change between the dialog and the click is never confirmed by accident.
    pub fn confirm_allow(
        &self,
        project: &Project,
        id: &str,
        origin: &str,
        class: DestinationClass,
    ) -> Result<(), EndpointError> {
        let now = self.prepare_allow(id)?;
        if now.destination.origin != origin || now.destination.class != class {
            return Err(EndpointError::new(
                "destination_changed",
                format!(
                    "The destination changed: it is now {} ({}). Check it and allow again.",
                    now.destination.origin,
                    now.destination.class.label()
                ),
            ));
        }
        if class == DestinationClass::ThisMachine {
            return Ok(());
        }
        project.store().allow_endpoint(id, origin, class.as_str())?;
        Ok(())
    }

    /// Revokes the project's permission for an endpoint (domain §7.6).
    pub fn revoke_endpoint(&self, project: &Project, id: &str) -> Result<(), EndpointError> {
        project.store().revoke_endpoint(id)?;
        Ok(())
    }

    /// A model client for `project` (ADR-0009): the gate checks the project's permission now
    /// and again on every call, and records each destination before sending.
    pub fn model_client<'a>(
        &'a self,
        project: &'a Project,
        destination: Destination,
        purpose: Purpose,
    ) -> Result<ModelClient<'a>, GateError> {
        self.egress_parts()
            .gate
            .client(ProjectEgress(project.store()), destination, purpose)
    }
}

impl Project {
    /// Every destination that received this project's data, the most recent first
    /// (US-09.8 AC2).
    pub fn egress_records(&self) -> Result<Vec<EgressRecordInfo>, EndpointError> {
        Ok(self
            .store()
            .egress_records()?
            .into_iter()
            .filter_map(|r| {
                Some(EgressRecordInfo {
                    class: DestinationClass::parse(&r.class)?,
                    purpose: Purpose::parse(&r.purpose)?,
                    host: r.host,
                    first_sent_at: r.first_sent_at,
                    last_sent_at: r.last_sent_at,
                })
            })
            .collect())
    }
}

/// Plain words for a gate refusal in the UI (ReconfirmReason included).
pub fn refusal_text(e: &GateError) -> String {
    match e {
        GateError::NeedsReconfirmation {
            reason: ReconfirmReason::Stored(r),
            ..
        } => format!("Confirm this endpoint again for this project: {r}"),
        e => e.to_string(),
    }
}
