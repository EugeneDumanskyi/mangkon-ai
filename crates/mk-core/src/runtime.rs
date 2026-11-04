//! The local runtime service (M5c, BC-12, ADR-0015): the user's `llama-server` (path, build,
//! backend, or why it cannot be used), the settings path, "Check again" and "Retry", and the
//! state of each loaded model. The supervisor itself is in `mk-models`.

use std::path::{Path, PathBuf};

use mk_models::hardware::GpuVendor;
use mk_models::runtime::{
    Backend, BinaryInfo, INSTALL_GUIDE, MIN_BUILD, RuntimeConfig, SlotStatus, Supervisor,
};
use mk_store::AppStore;

use crate::models::ModelError;
use crate::workspace::Workspace;

/// The `app_setting` key of the `llama-server` path (ADR-0015).
pub(crate) const PATH_KEY: &str = "llama_server_path";

/// What the AI models screen shows about the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeView {
    pub binary: BinaryInfo,
    /// The path set in settings, as typed.
    pub settings_path: Option<PathBuf>,
    pub min_build: u32,
    pub install_guide: &'static str,
    /// ADR-0015: a GPU and a CPU-only build → which build to get instead.
    pub backend_note: Option<String>,
    /// Each model the supervisor started this session (none before the first AI work).
    pub models: Vec<SlotStatus>,
}

pub(crate) fn supervisor(app: &AppStore, cfg: RuntimeConfig) -> Supervisor {
    let s = Supervisor::new(cfg);
    s.set_settings_path(app.setting(PATH_KEY).ok().flatten().map(PathBuf::from));
    s
}

/// The ADR-0015 hint: a GPU is present but the build has no GPU backend.
pub fn backend_note(vendors: &[(GpuVendor, String)], backends: &[Backend]) -> Option<String> {
    if backends != [Backend::Cpu] {
        return None;
    }
    let (vendor, name) = vendors.iter().find(|(v, _)| *v != GpuVendor::Other)?;
    let pick = match vendor {
        GpuVendor::Nvidia => "a CUDA build (or the smaller Vulkan build)",
        GpuVendor::Amd | GpuVendor::Intel => "a Vulkan build",
        GpuVendor::Apple => "the macOS arm64 build (Metal)",
        GpuVendor::Other => return None,
    };
    Some(format!(
        "This machine has a GPU ({name}), but this llama-server build runs on the CPU only, \
         which is much slower. Install {pick}; see {INSTALL_GUIDE}."
    ))
}

impl Workspace {
    fn runtime_view(&self, binary: BinaryInfo) -> RuntimeView {
        let gpus: Vec<(GpuVendor, String)> = self
            .models
            .profile()
            .gpus
            .iter()
            .map(|g| (g.vendor, g.name.clone()))
            .collect();
        RuntimeView {
            backend_note: backend_note(&gpus, &binary.backends),
            binary,
            settings_path: self.runtime.settings_path(),
            min_build: MIN_BUILD,
            install_guide: INSTALL_GUIDE,
            models: self.runtime.status(),
        }
    }

    /// The runtime as last checked (checked now on first use).
    pub fn runtime_status(&self) -> RuntimeView {
        let b = self.runtime.binary();
        self.runtime_view(b)
    }

    /// "Check again": finds the binary and reads its build and devices again.
    pub fn recheck_runtime(&self) -> RuntimeView {
        let b = self.runtime.recheck();
        self.runtime_view(b)
    }

    /// "Retry" after a failure (domain §7.8): checks again and starts Failed models again; AI
    /// jobs follow through [`Workspace::refresh_ai_blocks`].
    pub fn retry_runtime(&self) -> RuntimeView {
        let b = self.runtime.retry();
        self.runtime_view(b)
    }

    /// "Choose…": the path of `llama-server` (ADR-0015: looked at before `PATH`); `None` clears
    /// it. A path that is not an executable file is refused.
    pub fn set_llama_server_path(&self, path: Option<&Path>) -> Result<RuntimeView, ModelError> {
        if let Some(p) = path
            && !Supervisor::path_is_executable(p)
        {
            return Err(ModelError::new(
                "not_executable",
                format!("{} is not a program that can be run", p.display()),
            ));
        }
        let text = path.map(|p| p.to_string_lossy().into_owned());
        self.app()
            .set_setting(PATH_KEY, text.as_deref())
            .map_err(ModelError::store)?;
        self.runtime.set_settings_path(path.map(Path::to_path_buf));
        Ok(self.retry_runtime())
    }

    /// `f` runs (on any thread) whenever a runtime changes state or the binary is checked.
    pub fn on_runtime_changed(&self, f: impl Fn() + Send + Sync + 'static) {
        self.runtime.on_change(f);
    }

    /// The supervisor (CLI and tests: waits, status).
    pub fn supervisor(&self) -> &Supervisor {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adr_0015_backend_note() {
        let nvidia = [(GpuVendor::Nvidia, "RTX 5070".to_string())];
        let note = backend_note(&nvidia, &[Backend::Cpu]).unwrap();
        assert!(note.contains("CUDA build"), "{note}");
        assert_eq!(backend_note(&nvidia, &[Backend::Cuda]), None);
        assert_eq!(backend_note(&[], &[Backend::Cpu]), None);
        assert_eq!(backend_note(&nvidia, &[]), None, "backend unknown: no note");
        let amd = [(GpuVendor::Amd, "Radeon".to_string())];
        assert!(
            backend_note(&amd, &[Backend::Cpu])
                .unwrap()
                .contains("Vulkan")
        );
    }
}
