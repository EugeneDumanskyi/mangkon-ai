//! Which AI each stage uses and whether it can run now (M5c).
//!
//! - The AI choice is app-level, per role: a local model or an endpoint (`app_setting`
//!   `ai.extraction`, `ai.embeddings` = `local:<id>` | `endpoint:<id>`). Unset, it is the
//!   installed recommended model of the role, else the first installed one (INV-MM-04).
//! - [`Workspace::ai_readiness`]: Ready with where data would go, or Blocked with a reason
//!   (US-09.7 AC3, US-09.5 AC3); [`Workspace::refresh_ai_blocks`] keeps the project's AI jobs
//!   Blocked or running to match (INV-PR-09).
//! - [`Workspace::ai_client`]: the entry point of the AI stages: the local runtime started on
//!   demand (US-09.5 AC1), or an endpoint, always through the egress gate (ADR-0009).
//! - [`Workspace::try_local_model`]: "Try": fixed test texts to an installed model
//!   (INV-EG-08).
//! - [`Activity`]: where data goes now, for the status bar (INV-EG-06, NFR-SEC-05).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use mk_egress::{
    BaseUrl, DestinationClass, ModelClient, Purpose, SecretString, Sending, TestTarget,
    embeddings_test, endpoint_test,
};
use mk_jobs::Stage;
use mk_models::Role;
use mk_models::runtime::Lease;
use mk_store::StoreError;

use crate::egress::{Access, ProjectEgress};
use crate::models::ModelError;
use crate::project::Project;
use crate::workspace::Workspace;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One call on its way: host, class and what for (US-09.8 AC1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiActivity {
    pub host: String,
    pub class: DestinationClass,
    /// `extraction`, `embeddings`, `search_query` or `test`.
    pub purpose: &'static str,
}

impl AiActivity {
    pub(crate) fn of(s: &Sending) -> AiActivity {
        AiActivity {
            host: s.host.clone(),
            class: s.class,
            purpose: s.purpose.as_str(),
        }
    }

    pub(crate) fn test(host: &str, class: DestinationClass) -> AiActivity {
        AiActivity {
            host: host.to_string(),
            class,
            purpose: "test",
        }
    }

    /// The status bar text (US-09.8 AC1).
    pub fn label(&self) -> String {
        match self.class {
            DestinationClass::ThisMachine => "Data sent to this machine".into(),
            c => format!("Data sent to {} ({})", self.host, c.label()),
        }
    }
}

type ActivityListener = Arc<dyn Fn(Option<AiActivity>) + Send + Sync>;

/// The calls in flight. Each is published before it is sent and cleared when it ends.
#[derive(Default)]
pub(crate) struct Activity {
    calls: Mutex<Vec<AiActivity>>,
    listener: Mutex<Option<ActivityListener>>,
}

impl Activity {
    pub(crate) fn begin(&self, a: AiActivity) {
        lock(&self.calls).push(a.clone());
        self.publish(Some(a));
    }

    pub(crate) fn end(&self, a: &AiActivity) {
        let now = {
            let mut calls = lock(&self.calls);
            if let Some(i) = calls.iter().position(|c| c == a) {
                calls.remove(i);
            }
            calls.last().cloned()
        };
        self.publish(now);
    }

    pub(crate) fn current(&self) -> Option<AiActivity> {
        lock(&self.calls).last().cloned()
    }

    fn publish(&self, now: Option<AiActivity>) {
        let f = lock(&self.listener).clone();
        if let Some(f) = f {
            f(now);
        }
    }
}

/// What a role uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiChoice {
    /// An installed catalog model, run by the local runtime.
    Local(String),
    /// An app-level endpoint, by id.
    Endpoint(String),
}

impl AiChoice {
    fn stored(&self) -> String {
        match self {
            AiChoice::Local(id) => format!("local:{id}"),
            AiChoice::Endpoint(id) => format!("endpoint:{id}"),
        }
    }

    fn parse(s: &str) -> Option<AiChoice> {
        match s.split_once(':')? {
            ("local", id) if !id.is_empty() => Some(AiChoice::Local(id.into())),
            ("endpoint", id) if !id.is_empty() => Some(AiChoice::Endpoint(id.into())),
            _ => None,
        }
    }
}

/// A role's choice as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiChoiceView {
    pub role: Role,
    pub choice: Option<AiChoice>,
    /// Picked by the user, not the default.
    pub explicit: bool,
    /// The model or endpoint name.
    pub name: Option<String>,
}

/// US-09.7 AC3.
pub const NO_MODEL_ALLOWED: &str = "No model allowed for this project";

/// Whether a role's AI can run for a project now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiReadiness {
    Ready {
        /// What the status bar would say.
        destination: String,
        class: DestinationClass,
        /// The model or endpoint name.
        name: String,
    },
    Blocked {
        /// `no_model`, `needs_reconfirmation`, `runtime_failed`, `not_installed`.
        code: &'static str,
        reason: String,
    },
}

/// Why an AI client could not be made, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiError {
    pub code: String,
    pub message: String,
    /// The project's AI jobs are Blocked with this reason (not a failure of one item).
    pub blocked: bool,
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AiError {}

/// A client for one role and project: the gate's [`ModelClient`], and, for the local model,
/// the lease that keeps it Ready (Busy while held).
pub struct AiClient<'a> {
    pub client: ModelClient<'a>,
    lease: Option<Lease>,
}

impl AiClient<'_> {
    /// A call succeeded: the local runtime's restart count starts again (INV-MM-06).
    pub fn succeeded(&self) {
        if let Some(l) = &self.lease {
            l.succeeded();
        }
    }

    /// The local runtime's port, when this is the local model.
    pub fn local_port(&self) -> Option<u16> {
        self.lease.as_ref().map(Lease::port)
    }
}

/// What "Try" found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TryReport {
    pub model_id: String,
    pub role: Role,
    pub ok: bool,
    /// One line for the UI.
    pub summary: String,
    /// `127.0.0.1:<port>`, when it started.
    pub bound: Option<String>,
    /// From the call to Ready (0 when it was Ready already).
    pub start_ms: u64,
    pub response_ms: Option<u64>,
    /// Extraction models.
    pub structured_output: Option<bool>,
    /// Embedding models.
    pub dimensions: Option<usize>,
}

fn key(role: Role) -> &'static str {
    match role {
        Role::Extraction => "ai.extraction",
        Role::Embeddings => "ai.embeddings",
    }
}

/// The job stages a role serves (domain §5.1).
pub fn stages_of(role: Role) -> &'static [Stage] {
    match role {
        Role::Extraction => &[Stage::B, Stage::C],
        Role::Embeddings => &[Stage::Embeddings],
    }
}

fn purpose_of(role: Role) -> Purpose {
    match role {
        Role::Extraction => Purpose::Extraction,
        Role::Embeddings => Purpose::Embeddings,
    }
}

impl Workspace {
    /// The AI a role uses now: the user's choice, else the default (INV-MM-04).
    pub fn ai_choice(&self, role: Role) -> Result<AiChoiceView, ModelError> {
        let stored = self
            .app()
            .setting(key(role))
            .map_err(ModelError::store)?
            .and_then(|s| AiChoice::parse(&s));
        let (choice, explicit) = match stored {
            Some(c) => (Some(c), true),
            None => {
                let installed = self.models.installed(role);
                let pick = self
                    .models
                    .recommended(role)
                    .filter(|r| installed.contains(r))
                    .or_else(|| installed.first().cloned());
                (pick.map(AiChoice::Local), false)
            }
        };
        let name = match &choice {
            Some(AiChoice::Local(id)) => Some(self.models.name_of(id)),
            Some(AiChoice::Endpoint(id)) => self.app().endpoint(id).ok().flatten().map(|e| e.name),
            None => None,
        };
        Ok(AiChoiceView {
            role,
            choice,
            explicit,
            name,
        })
    }

    /// "Use for AI": the model or endpoint a role uses from now on; `None` goes back to the
    /// default. A local model must be of the role (INV-MM-04).
    pub fn set_ai_choice(
        &self,
        role: Role,
        choice: Option<AiChoice>,
    ) -> Result<AiChoiceView, ModelError> {
        match &choice {
            Some(AiChoice::Local(id)) => {
                let r = self.models.role_of(id).ok_or_else(|| {
                    ModelError::new("model_not_found", format!("No model {id} in the catalog"))
                })?;
                if r != role {
                    return Err(ModelError::new(
                        "wrong_role",
                        format!("{} cannot be used for this", self.models.name_of(id)),
                    ));
                }
            }
            Some(AiChoice::Endpoint(id))
                if self
                    .app()
                    .endpoint(id)
                    .map_err(ModelError::store)?
                    .is_none() =>
            {
                return Err(ModelError::new(
                    "endpoint_not_found",
                    format!("No endpoint {id}"),
                ));
            }
            Some(AiChoice::Endpoint(_)) | None => {}
        }
        let stored = choice.as_ref().map(AiChoice::stored);
        self.app()
            .set_setting(key(role), stored.as_deref())
            .map_err(ModelError::store)?;
        // A local model switched away from is stopped (domain §7.8 "model switched").
        if let Some((running, ..)) = self.runtime.state_of(role)
            && choice != Some(AiChoice::Local(running))
        {
            self.runtime.stop(role);
        }
        self.ai_choice(role)
    }

    /// Whether `role` can run for `project` now, and where its data would go.
    pub fn ai_readiness(&self, project: &Project, role: Role) -> AiReadiness {
        let blocked = |code, reason: String| AiReadiness::Blocked { code, reason };
        let view = match self.ai_choice(role) {
            Ok(v) => v,
            Err(e) => return blocked("no_model", e.message),
        };
        match view.choice {
            None => blocked("no_model", NO_MODEL_ALLOWED.into()),
            Some(AiChoice::Local(id)) => {
                let name = self.models.name_of(&id);
                if !self.models.is_installed(&id) {
                    return blocked(
                        "not_installed",
                        format!("{name} is not installed. Download it in AI models."),
                    );
                }
                if let Some((running, state, failure)) = self.runtime.state_of(role)
                    && running == id
                    && state == mk_models::runtime::RuntimeState::Failed
                {
                    let reason =
                        failure.map_or_else(|| "llama-server failed".into(), |f| f.message);
                    return blocked("runtime_failed", reason);
                }
                if let Some((_, reason)) = self.runtime.binary().error {
                    return blocked("runtime_failed", reason);
                }
                AiReadiness::Ready {
                    destination: "Data sent to this machine".into(),
                    class: DestinationClass::ThisMachine,
                    name,
                }
            }
            Some(AiChoice::Endpoint(id)) => {
                let list = match self.project_endpoints(project) {
                    Ok(l) => l,
                    Err(e) => return blocked("no_model", e.message),
                };
                let Some(e) = list.into_iter().find(|e| e.id == id) else {
                    return blocked(
                        "not_installed",
                        "The endpoint chosen for AI is not set up on this computer".into(),
                    );
                };
                let host = e
                    .destination
                    .as_ref()
                    .map_or_else(|_| e.base_url.clone(), |d| d.host.clone());
                let ready = |class: DestinationClass| AiReadiness::Ready {
                    destination: AiActivity::test(&host, class).label(),
                    class,
                    name: e.name.clone(),
                };
                match e.access {
                    Access::ThisMachine => ready(DestinationClass::ThisMachine),
                    Access::Allowed { class, .. } => {
                        ready(e.destination.as_ref().map_or(class, |d| d.class))
                    }
                    Access::NotAllowed => blocked("no_model", NO_MODEL_ALLOWED.into()),
                    Access::NeedsReconfirmation { reason } => blocked(
                        "needs_reconfirmation",
                        format!("Confirm the endpoint again: {reason}"),
                    ),
                }
            }
        }
    }

    /// INV-PR-09: the project's AI jobs are Blocked with the reason while their role cannot
    /// run, and run again when it can. Called on runtime, permission, choice and model
    /// changes. Returns whether a job changed.
    pub fn refresh_ai_blocks(&self, project: &Project) -> Result<bool, StoreError> {
        let mut changed = 0;
        for role in [Role::Extraction, Role::Embeddings] {
            changed += match self.ai_readiness(project, role) {
                AiReadiness::Blocked { reason, .. } => {
                    project.store().block_ai_jobs(stages_of(role), &reason)?
                }
                // Stage B and embeddings also wait while the choice is not the model of their run.
                AiReadiness::Ready { .. } if role == Role::Extraction => {
                    project.store().unblock_ai_jobs(&[Stage::C])?
                        + self.refresh_identity_blocks(project, role)?
                }
                AiReadiness::Ready { .. } => self.refresh_identity_blocks(project, role)?,
            };
        }
        project.note_ai_switch();
        Ok(changed > 0)
    }

    /// A model client for `role` and `project` (the M6 stages' entry point). The local model
    /// is started on demand and held Ready by the returned lease (US-09.5 AC1). When it cannot
    /// run, the project's AI jobs are Blocked with the reason and Layer 1 goes on (US-09.5
    /// AC3, US-09.7 AC3, INV-PR-09).
    pub fn ai_client<'a>(
        &'a self,
        project: &'a Project,
        role: Role,
    ) -> Result<AiClient<'a>, AiError> {
        self.ai_client_for(project, role, purpose_of(role))
    }

    /// [`ai_client`](Self::ai_client) with the purpose its calls are recorded under: a meaning
    /// search's query goes as [`Purpose::SearchQuery`] (INV-SR-03).
    pub fn ai_client_for<'a>(
        &'a self,
        project: &'a Project,
        role: Role,
        purpose: Purpose,
    ) -> Result<AiClient<'a>, AiError> {
        let block = |code: &str, reason: String| {
            let _ = project.store().block_ai_jobs(stages_of(role), &reason);
            AiError {
                code: code.into(),
                message: reason,
                blocked: true,
            }
        };
        if let AiReadiness::Blocked { code, reason } = self.ai_readiness(project, role) {
            return Err(block(code, reason));
        }
        let choice = self
            .ai_choice(role)
            .map_err(|e| block("no_model", e.message))?
            .choice
            .ok_or_else(|| block("no_model", NO_MODEL_ALLOWED.into()))?;
        let (destination, lease) = match choice {
            AiChoice::Local(id) => {
                let spec = self
                    .models
                    .spec(&id)
                    .map_err(|e| block("not_installed", e.message))?;
                let lease = self
                    .runtime
                    .ensure_ready(&spec)
                    .map_err(|f| block("runtime_failed", f.message))?;
                (
                    mk_egress::Destination::LocalRuntime {
                        port: lease.port(),
                        model: id,
                        key: SecretString::from(lease.key().to_string()),
                    },
                    Some(lease),
                )
            }
            AiChoice::Endpoint(id) => (mk_egress::Destination::Endpoint(id), None),
        };
        let client = self
            .egress
            .gate
            .client(ProjectEgress(project.store()), destination, purpose)
            .map_err(|e| {
                if e.is_blocked() {
                    block(e.code(), crate::egress::refusal_text(&e))
                } else {
                    AiError {
                        code: e.code().into(),
                        message: e.to_string(),
                        blocked: false,
                    }
                }
            })?;
        Ok(AiClient { client, lease })
    }

    /// "Try" (US-09.5 AC1 by hand): starts the installed model and sends the fixed test texts
    /// to it on `127.0.0.1` with its key (INV-EG-08): chat and the structured-output probe for
    /// an extraction model, one embedding for an embedding model. No project is involved.
    pub fn try_local_model(&self, model_id: &str) -> Result<TryReport, ModelError> {
        let spec = self.models.spec(model_id)?;
        let name = self.models.name_of(model_id);
        let started = Instant::now();
        let mut report = TryReport {
            model_id: model_id.to_string(),
            role: spec.role,
            ok: false,
            summary: String::new(),
            bound: None,
            start_ms: 0,
            response_ms: None,
            structured_output: None,
            dimensions: None,
        };
        let lease = match self.runtime.ensure_ready(&spec) {
            Ok(l) => l,
            Err(f) => {
                report.summary = f.message;
                return Ok(report);
            }
        };
        report.start_ms = started.elapsed().as_millis() as u64;
        report.bound = Some(format!("127.0.0.1:{}", lease.port()));
        let base = BaseUrl::local_runtime(lease.port());
        let key = SecretString::from(lease.key().to_string());
        let target = TestTarget {
            base_url: &base,
            model: model_id,
            key: Some(&key),
        };
        let shown = AiActivity::test("127.0.0.1", DestinationClass::ThisMachine);
        let parts = &self.egress;
        parts.activity.begin(shown.clone());
        match spec.role {
            Role::Extraction => {
                let r = endpoint_test(parts.resolver.as_ref(), &parts.roots, target);
                report.ok = r.ok();
                report.summary = r.summary_as(&name);
                report.structured_output = r.structured_output;
                report.response_ms = r.chat.as_ref().ok().map(|c| c.response_ms);
            }
            Role::Embeddings => {
                match embeddings_test(parts.resolver.as_ref(), &parts.roots, target) {
                    Ok(e) => {
                        report.ok = true;
                        report.summary = format!(
                            "Connected: {name}, {} dimensions in {} ms",
                            e.dimensions, e.response_ms
                        );
                        report.dimensions = Some(e.dimensions);
                        report.response_ms = Some(e.response_ms);
                    }
                    Err(e) => report.summary = e.message,
                }
            }
        }
        parts.activity.end(&shown);
        if report.ok {
            lease.succeeded();
        }
        Ok(report)
    }

    /// Where data goes now, if a call is on its way.
    pub fn ai_activity(&self) -> Option<AiActivity> {
        self.egress.activity.current()
    }

    /// `f` runs (on the calling thread of each call) before every send with where it goes, and
    /// when calls end with what is still on its way (`None`: nothing).
    pub fn on_ai_activity(&self, f: impl Fn(Option<AiActivity>) + Send + Sync + 'static) {
        *lock(&self.egress.activity.listener) = Some(Arc::new(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_round_trip() {
        for c in [
            AiChoice::Local("a.b-c".into()),
            AiChoice::Endpoint("00ff".into()),
        ] {
            assert_eq!(AiChoice::parse(&c.stored()), Some(c));
        }
        assert_eq!(AiChoice::parse("local:"), None);
        assert_eq!(AiChoice::parse("cloud:x"), None);
    }

    /// US-09.8 AC1 texts.
    #[test]
    fn us_09_8_ac1_labels() {
        let a = |h: &str, c| AiActivity::test(h, c).label();
        assert_eq!(
            a("127.0.0.1", DestinationClass::ThisMachine),
            "Data sent to this machine"
        );
        assert_eq!(
            a("192.168.1.20", DestinationClass::LocalNetwork),
            "Data sent to 192.168.1.20 (local network)"
        );
        assert_eq!(
            a("api.openai.com", DestinationClass::Internet),
            "Data sent to api.openai.com (internet)"
        );
    }
}
