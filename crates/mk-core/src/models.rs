//! Local models (BC-12, M5b): the hardware check (US-09.1, US-09.2), the catalog with each
//! model's state (US-09.3) and downloads (US-09.4) on [`Workspace`]. One download runs at a
//! time (ASSUMPTION), on its own thread; its state is kept in `app.db` on every change of
//! state and its byte count about once a second, so a quit leaves it Interrupted (domain §7.7).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mk_models::download::{
    self, Control, DownloadConfig, Downloader, Outcome, Progress, bytes_on_disk, file_path,
    remaining_bytes,
};
use mk_models::hardware::{
    HardwareProbe, HardwareProfile, OVERRIDE_WARNING, SystemProbe, TOO_WEAK, Verdict, verdict,
};
use mk_models::state::{DownloadState, Event, can_start, disk_needed};
use mk_models::{Catalog, Model, Role};
use mk_store::AppStore;

use crate::workspace::Workspace;

/// The `app_setting` key of the US-09.2 AC3 override.
const OVERRIDE_KEY: &str = "local_models_override";
/// How often a running download's byte count is written to `app.db`.
const PERSIST_EVERY: Duration = Duration::from_secs(1);
/// How often progress is reported to [`Workspace::on_models_changed`] at most.
pub const MODELS_CHANGED_EVERY: Duration = Duration::from_millis(500);

/// Where local models live and how the machine and the network are reached.
#[derive(Clone)]
pub struct ModelsConfig {
    /// `None`: `<app data>/models`.
    pub models_dir: Option<PathBuf>,
    pub catalog: Catalog,
    pub probe: Arc<dyn HardwareProbe>,
    pub download: DownloadConfig,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        ModelsConfig {
            models_dir: None,
            catalog: Catalog::shipped(),
            probe: Arc::new(SystemProbe),
            download: DownloadConfig::default(),
        }
    }
}

/// A model call that failed, in plain words with a stable code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelError {
    pub code: &'static str,
    pub message: String,
}

impl ModelError {
    fn new(code: &'static str, message: impl Into<String>) -> ModelError {
        ModelError {
            code,
            message: message.into(),
        }
    }

    fn store(e: mk_store::StoreError) -> ModelError {
        ModelError::new("store", format!("The app database failed: {e}"))
    }

    fn io(e: std::io::Error) -> ModelError {
        ModelError::new("io", format!("The models folder could not be changed: {e}"))
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

/// The hardware check and what it allows (US-09.1, US-09.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareView {
    pub profile: HardwareProfile,
    pub verdict: Verdict,
    /// GPU and memory meet the minimum (free disk is a note; each download checks its own
    /// size, decided 2026-11-04).
    pub hardware_meets: bool,
    /// US-09.2 AC3 (Q7).
    pub local_override: bool,
    /// Local downloads are offered (INV-MM-03).
    pub local_allowed: bool,
    /// US-09.2 AC1 text when local downloads are not offered.
    pub too_weak: Option<&'static str>,
    /// The Q7 warning shown with the override.
    pub override_warning: &'static str,
}

/// One catalog model and where its download stands (US-09.3, US-09.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelView {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub quant: String,
    /// Size on disk.
    pub bytes: u64,
    pub vram_bytes: u64,
    pub ram_bytes: u64,
    pub license_spdx: String,
    pub license_url: String,
    /// The tier this machine fits recommends it (US-09.3 AC1).
    pub recommended: bool,
    /// Its measured VRAM and RAM fit this machine.
    pub fits: bool,
    /// The free disk its download needs (what is left + 2 GB, INV-MM-02), when the disk has
    /// less; `None` when it fits, is installed, or the free space is unknown.
    pub disk_needed: Option<u64>,
    pub state: DownloadState,
    pub bytes_done: u64,
    pub speed_bps: u64,
    pub eta_s: Option<u64>,
    /// The last failure (an interruption, or a checksum mismatch that left it Available).
    pub error: Option<ModelError>,
}

struct Running {
    id: String,
    control: Arc<Control>,
}

type Listener = Arc<dyn Fn() + Send + Sync>;

/// The models part of a [`Workspace`]; shared with the download thread.
pub(crate) struct ModelParts {
    app: Arc<AppStore>,
    dir: PathBuf,
    catalog: Catalog,
    probe: Arc<dyn HardwareProbe>,
    downloader: Downloader,
    hardware: Mutex<Option<HardwareProfile>>,
    running: Mutex<Option<Running>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    live: Mutex<HashMap<String, Progress>>,
    /// Failures of models whose row is gone (a checksum mismatch leaves them Available).
    errors: Mutex<HashMap<String, ModelError>>,
    quitting: AtomicBool,
    listener: Mutex<Option<Listener>>,
    last_notice: Mutex<Option<Instant>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ModelParts {
    pub(crate) fn new(app: Arc<AppStore>, app_dir: &Path, cfg: ModelsConfig) -> Arc<ModelParts> {
        let hosts = cfg.catalog.download_hosts.clone();
        Arc::new(ModelParts {
            app,
            dir: cfg.models_dir.unwrap_or_else(|| app_dir.join("models")),
            catalog: cfg.catalog,
            probe: cfg.probe,
            downloader: Downloader::new(cfg.download, hosts),
            hardware: Mutex::new(None),
            running: Mutex::new(None),
            thread: Mutex::new(None),
            live: Mutex::new(HashMap::new()),
            errors: Mutex::new(HashMap::new()),
            quitting: AtomicBool::new(false),
            listener: Mutex::new(None),
            last_notice: Mutex::new(None),
        })
    }

    fn notify(&self, force: bool) {
        {
            let mut last = lock(&self.last_notice);
            if !force && last.is_some_and(|t| t.elapsed() < MODELS_CHANGED_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
        let listener = lock(&self.listener).clone();
        if let Some(f) = listener {
            f();
        }
    }

    fn profile(&self) -> HardwareProfile {
        let mut cached = lock(&self.hardware);
        cached
            .get_or_insert_with(|| self.probe.probe(&self.dir))
            .clone()
    }

    fn local_override(&self) -> Result<bool, ModelError> {
        Ok(self
            .app
            .setting(OVERRIDE_KEY)
            .map_err(ModelError::store)?
            .as_deref()
            == Some("1"))
    }

    fn model(&self, id: &str) -> Result<&Model, ModelError> {
        self.catalog.model(id).ok_or_else(|| {
            ModelError::new("model_not_found", format!("No model {id} in the catalog"))
        })
    }

    /// The stored state of each model with a row, as of now.
    fn stored(&self) -> Result<HashMap<String, (DownloadState, Option<ModelError>)>, ModelError> {
        let rows = self.app.model_downloads().map_err(ModelError::store)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let state = DownloadState::parse(&r.state)?;
                let error = match (r.error_code, r.error_message) {
                    (Some(c), Some(m)) => Some(ModelError {
                        code: code_str(&c),
                        message: m,
                    }),
                    _ => None,
                };
                Some((r.model_id, (state, error)))
            })
            .collect())
    }

    fn state_of(&self, model: &Model) -> Result<DownloadState, ModelError> {
        if lock(&self.running)
            .as_ref()
            .is_some_and(|r| r.id == model.id)
        {
            return Ok(DownloadState::Downloading);
        }
        let state = self
            .stored()?
            .remove(&model.id)
            .map_or(DownloadState::Available, |(s, _)| s);
        Ok(self.checked(model, state))
    }

    /// An installed model whose files are gone or of the wrong size is Available again.
    fn checked(&self, model: &Model, state: DownloadState) -> DownloadState {
        if state == DownloadState::Installed
            && !model.files.iter().all(|f| {
                std::fs::metadata(file_path(&self.dir, model, f)).is_ok_and(|m| m.len() == f.bytes)
            })
        {
            return DownloadState::Available;
        }
        state
    }

    /// Starts or resumes `id` on the download thread (domain §7.7 guard).
    fn begin(self: &Arc<Self>, id: &str) -> Result<(), ModelError> {
        let model = self.model(id)?.clone();
        let mut running = lock(&self.running);
        if let Some(r) = running.as_ref() {
            return Err(if r.id == model.id {
                ModelError::new("invalid_state", "This model is downloading already.")
            } else {
                ModelError::new(
                    "busy",
                    "Another model is downloading. Pause it or wait until it ends.",
                )
            });
        }
        let state = {
            let stored = self.stored()?;
            let s = stored
                .get(&model.id)
                .map_or(DownloadState::Available, |(s, _)| *s);
            self.checked(&model, s)
        };
        let event = if state == DownloadState::Available {
            Event::Start
        } else {
            Event::Resume
        };
        let next = state.transition(event).ok_or_else(|| {
            ModelError::new(
                "invalid_state",
                match state {
                    DownloadState::Installed => "This model is installed already.",
                    _ => "This download cannot start now.",
                },
            )
        })?;
        let profile = self.profile();
        let meets = verdict(&profile, &self.catalog).meets;
        let free = self.probe.free_disk(&self.dir);
        if let Some(h) = lock(&self.hardware).as_mut() {
            h.free_disk_bytes = free;
        }
        if let Err(refusal) = can_start(
            meets,
            self.local_override()?,
            free,
            remaining_bytes(&self.dir, &model),
        ) {
            return Err(ModelError::new(refusal.code(), refusal.message()));
        }
        lock(&self.errors).remove(&model.id);
        self.app
            .set_model_download(
                &model.id,
                next.as_str(),
                bytes_on_disk(&self.dir, &model),
                model.bytes(),
                None,
            )
            .map_err(ModelError::store)?;
        let control = Control::new();
        *running = Some(Running {
            id: model.id.clone(),
            control: control.clone(),
        });
        drop(running);
        // The previous thread has ended (it clears `running` last); reap it.
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
        let parts = self.clone();
        let handle = std::thread::Builder::new()
            .name(format!("model-download-{}", model.id))
            .spawn(move || parts.run(model, control))
            .map_err(ModelError::io)?;
        *lock(&self.thread) = Some(handle);
        self.notify(true);
        Ok(())
    }

    /// The download thread.
    fn run(&self, model: Model, control: Arc<Control>) {
        let id = model.id.clone();
        let verified: HashSet<(String, u64)> = self
            .app
            .model_files_verified(&id)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let sizes: HashMap<&str, u64> = model
            .files
            .iter()
            .map(|f| (f.name.as_str(), f.bytes))
            .collect();
        let is_verified = |name: &str| {
            sizes
                .get(name)
                .is_some_and(|b| verified.contains(&(name.to_string(), *b)))
        };
        let mut persisted = Instant::now();
        let result = self.downloader.download(
            &model,
            &self.dir,
            &is_verified,
            &control,
            &mut |ev| match ev {
                download::Event::Progress(p) => {
                    lock(&self.live).insert(id.clone(), p);
                    if persisted.elapsed() >= PERSIST_EVERY {
                        persisted = Instant::now();
                        let _ = self.app.set_model_bytes(&id, p.bytes_done);
                    }
                    self.notify(false);
                }
                download::Event::Verifying => {
                    let _ = self.app.set_model_download(
                        &id,
                        DownloadState::Verifying.as_str(),
                        model.bytes(),
                        model.bytes(),
                        None,
                    );
                    self.notify(true);
                }
                download::Event::FileVerified(name) => {
                    if let Some(b) = sizes.get(name.as_str()) {
                        let _ = self.app.model_file_verified(&id, &name, *b);
                    }
                }
            },
        );
        let (state, error) = match result {
            Ok(Outcome::Installed) => (DownloadState::Installed, None),
            Ok(Outcome::Paused) if self.quitting.load(Ordering::SeqCst) => {
                (DownloadState::Interrupted, None)
            }
            Ok(Outcome::Paused) => (DownloadState::Paused, None),
            Ok(Outcome::Cancelled) => (DownloadState::Available, None),
            Err(e) => (
                e.state_after(),
                Some(ModelError {
                    code: e.code.as_str(),
                    message: e.message,
                }),
            ),
        };
        let mut running = lock(&self.running);
        let err = error.as_ref().map(|e| (e.code, e.message.as_str()));
        let _ = self.app.set_model_download(
            &id,
            state.as_str(),
            bytes_on_disk(&self.dir, &model),
            model.bytes(),
            err,
        );
        if state == DownloadState::Available
            && let Some(e) = error
        {
            lock(&self.errors).insert(id.clone(), e);
        }
        lock(&self.live).remove(&id);
        *running = None;
        drop(running);
        if state == DownloadState::Installed || state == DownloadState::Available {
            self.refresh_free_disk();
        }
        self.notify(true);
    }

    fn refresh_free_disk(&self) {
        let free = self.probe.free_disk(&self.dir);
        if let Some(h) = lock(&self.hardware).as_mut() {
            h.free_disk_bytes = free;
        }
    }

    /// Stops the running download at app quit: it is left Interrupted (domain §7.7).
    pub(crate) fn shut_down(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        if let Some(r) = lock(&self.running).as_ref() {
            r.control.pause();
        }
        if let Some(t) = lock(&self.thread).take() {
            let _ = t.join();
        }
    }
}

/// The stored error code as one of the known codes.
fn code_str(c: &str) -> &'static str {
    [
        "download_host_not_allowed",
        "network",
        "disk_full",
        "corrupted",
        "io",
    ]
    .into_iter()
    .find(|k| *k == c)
    .unwrap_or("error")
}

impl Workspace {
    /// The hardware check (US-09.1): the machine is read once per session, then cached.
    pub fn hardware(&self) -> Result<HardwareView, ModelError> {
        let profile = self.models.profile();
        self.hardware_view(profile)
    }

    /// "Check hardware": reads the machine again.
    pub fn recheck_hardware(&self) -> Result<HardwareView, ModelError> {
        *lock(&self.models.hardware) = None;
        self.hardware()
    }

    fn hardware_view(&self, profile: HardwareProfile) -> Result<HardwareView, ModelError> {
        let catalog = &self.models.catalog;
        let v = verdict(&profile, catalog);
        let meets = v.meets;
        let local_override = self.models.local_override()?;
        let local_allowed = meets || local_override;
        Ok(HardwareView {
            verdict: v,
            profile,
            hardware_meets: meets,
            local_override,
            local_allowed,
            too_weak: (!local_allowed).then_some(TOO_WEAK),
            override_warning: OVERRIDE_WARNING,
        })
    }

    /// US-09.2 AC3 (Q7): allow local models on a machine below the minimum.
    pub fn set_local_override(&self, on: bool) -> Result<HardwareView, ModelError> {
        self.models
            .app
            .set_setting(OVERRIDE_KEY, on.then_some("1"))
            .map_err(ModelError::store)?;
        self.models.notify(true);
        self.hardware()
    }

    /// The folder models are downloaded into.
    pub fn models_dir(&self) -> &Path {
        &self.models.dir
    }

    /// The shipped catalog (US-09.3 AC2).
    pub fn catalog(&self) -> &Catalog {
        &self.models.catalog
    }

    /// The catalog with each model's state and progress (US-09.3, US-09.4 AC2).
    pub fn model_catalog(&self) -> Result<Vec<ModelView>, ModelError> {
        let parts = &self.models;
        let profile = parts.profile();
        let v = verdict(&profile, &parts.catalog);
        let tier = v
            .tier
            .as_deref()
            .and_then(|t| parts.catalog.tiers.iter().find(|x| x.id == t))
            .or_else(|| {
                parts
                    .local_override()
                    .unwrap_or(false)
                    .then(|| parts.catalog.tiers.first())
                    .flatten()
            });
        let mut stored = parts.stored()?;
        let running = lock(&parts.running).as_ref().map(|r| r.id.clone());
        let live = lock(&parts.live).clone();
        let errors = lock(&parts.errors).clone();
        Ok(parts
            .catalog
            .models
            .iter()
            .map(|m| {
                let (state, error) = match stored.remove(&m.id) {
                    Some((s, e)) => (parts.checked(m, s), e),
                    None => (DownloadState::Available, errors.get(&m.id).cloned()),
                };
                let state = if running.as_deref() == Some(m.id.as_str())
                    && state != DownloadState::Verifying
                {
                    DownloadState::Downloading
                } else {
                    state
                };
                let progress = live.get(&m.id).copied();
                let bytes_done = match state {
                    DownloadState::Installed => m.bytes(),
                    DownloadState::Available => 0,
                    _ => progress.map_or_else(|| bytes_on_disk(&parts.dir, m), |p| p.bytes_done),
                };
                ModelView {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    role: m.role,
                    quant: m.quant.clone(),
                    bytes: m.bytes(),
                    vram_bytes: m.vram_bytes,
                    ram_bytes: m.ram_bytes,
                    license_spdx: m.license.spdx.clone(),
                    license_url: m.license.url.clone(),
                    recommended: tier.is_some_and(|t| {
                        t.recommended_extraction.as_deref() == Some(m.id.as_str())
                            || t.recommended_embeddings == m.id
                    }),
                    fits: profile.fits_model(m),
                    disk_needed: (state != DownloadState::Installed)
                        .then(|| disk_needed(m.bytes().saturating_sub(bytes_done)))
                        .filter(|need| profile.free_disk_bytes.is_some_and(|f| f < *need)),
                    state,
                    bytes_done,
                    speed_bps: progress.map_or(0, |p| p.speed_bps),
                    eta_s: progress.and_then(|p| p.eta_s),
                    error,
                }
            })
            .collect())
    }

    /// Starts a download (US-09.4 AC1): refused below the minimum without the override
    /// (US-09.2 AC1) and when the disk lacks the space (INV-MM-02).
    pub fn start_download(&self, id: &str) -> Result<(), ModelError> {
        self.models.begin(id)
    }

    /// Resumes a paused or interrupted download from its partial files (US-09.4 AC3).
    pub fn resume_download(&self, id: &str) -> Result<(), ModelError> {
        self.models.begin(id)
    }

    /// Pauses the running download; its partial files are kept (US-09.4 AC2).
    pub fn pause_download(&self, id: &str) -> Result<(), ModelError> {
        match lock(&self.models.running).as_ref() {
            Some(r) if r.id == id => {
                r.control.pause();
                Ok(())
            }
            _ => Err(ModelError::new(
                "invalid_state",
                "This model is not downloading.",
            )),
        }
    }

    /// Cancels a download and deletes its partial files (US-09.4 AC2).
    pub fn cancel_download(&self, id: &str) -> Result<(), ModelError> {
        let parts = &self.models;
        let model = parts.model(id)?.clone();
        if let Some(r) = lock(&parts.running).as_ref()
            && r.id == id
        {
            r.control.cancel();
            return Ok(());
        }
        let state = parts.state_of(&model)?;
        if state.transition(Event::Cancel).is_none() {
            return Err(ModelError::new(
                "invalid_state",
                "This model has no download to cancel.",
            ));
        }
        download::remove_model(&parts.dir, &model).map_err(ModelError::io)?;
        parts
            .app
            .set_model_download(id, DownloadState::Available.as_str(), 0, 0, None)
            .map_err(ModelError::store)?;
        parts.refresh_free_disk();
        parts.notify(true);
        Ok(())
    }

    /// Deletes an installed model and reads the free space again (US-09.4 AC5). Returns the
    /// free space after the delete.
    pub fn delete_model(&self, id: &str) -> Result<Option<u64>, ModelError> {
        let parts = &self.models;
        let model = parts.model(id)?.clone();
        let state = parts.state_of(&model)?;
        if state.transition(Event::Delete).is_none() && state != DownloadState::Available {
            return Err(ModelError::new(
                "invalid_state",
                "Only an installed model can be deleted. Cancel the download instead.",
            ));
        }
        download::remove_model(&parts.dir, &model).map_err(ModelError::io)?;
        parts
            .app
            .set_model_download(id, DownloadState::Available.as_str(), 0, 0, None)
            .map_err(ModelError::store)?;
        lock(&parts.errors).remove(id);
        parts.refresh_free_disk();
        parts.notify(true);
        Ok(lock(&parts.hardware)
            .as_ref()
            .and_then(|h| h.free_disk_bytes)
            .or_else(|| parts.probe.free_disk(&parts.dir)))
    }

    /// The id of the model downloading now, if any.
    pub fn downloading(&self) -> Option<String> {
        lock(&self.models.running).as_ref().map(|r| r.id.clone())
    }

    /// Waits until the running download thread ends (CLI, tests).
    pub fn wait_download(&self) {
        let handle = lock(&self.models.thread).take();
        if let Some(t) = handle {
            let _ = t.join();
        }
    }

    /// `f` runs (on any thread) when the hardware view, the catalog states or a download's
    /// progress change; progress at most every [`MODELS_CHANGED_EVERY`].
    pub fn on_models_changed(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.models.listener) = Some(Arc::new(f));
    }
}
