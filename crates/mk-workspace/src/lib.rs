//! BC-01 Workspace: project settings, source roots and lifecycle (domain §3).
//!
//! M2b: [`SourceRoot`] and the [`ProjectLayout`] of a project folder. M3c: [`ProjectName`],
//! [`ProjectSettings`] and the registry rules (domain §5.2, §6.1). `mk-store` keeps the registry
//! in `app.db` and the settings in the project; `mk-core` creates and opens projects.

mod registry;
mod settings;

use std::path::{Path, PathBuf};

pub use registry::{
    MAX_NAME, NameError, ProjectName, RegistryEntry, name_key, name_taken, newest_first,
};
pub use settings::{
    DEFAULT_OCR_LANGUAGE, DEFAULT_STOP_LIST, ProjectSettings, Reprocess, SettingsError,
};

/// Whether a source root is a folder (walked recursively) or one file (domain §5.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RootKind {
    Folder,
    File,
}

impl RootKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RootKind::Folder => "folder",
            RootKind::File => "file",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [RootKind::Folder, RootKind::File]
            .into_iter()
            .find(|k| k.as_str() == name)
    }
}

/// A folder or file the project reads from. Stored as an absolute path that can be relocated;
/// file references are relative to it (domain D12).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SourceRoot {
    pub id: i64,
    pub path: PathBuf,
    pub kind: RootKind,
}

/// Paths inside a project folder (architecture §6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProjectLayout {
    dir: PathBuf,
}

impl ProjectLayout {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The source of truth: SQLite, WAL (ADR-0004).
    pub fn database(&self) -> PathBuf {
        self.dir.join("project.db")
    }

    /// Written before every migration (INV-WS-04).
    pub fn backups(&self) -> PathBuf {
        self.dir.join("backups")
    }

    /// Derived vector indexes, can be deleted (ADR-0010).
    pub fn index(&self) -> PathBuf {
        self.dir.join("index")
    }

    /// Rendered pages, can be deleted (ADR-0011).
    pub fn cache(&self) -> PathBuf {
        self.dir.join("cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_follows_architecture_section_6() {
        let l = ProjectLayout::new("/p");
        assert_eq!(l.database(), Path::new("/p/project.db"));
        assert_eq!(l.backups(), Path::new("/p/backups"));
        assert_eq!(l.index(), Path::new("/p/index"));
        assert_eq!(l.cache(), Path::new("/p/cache"));
        assert_eq!(RootKind::from_name("file"), Some(RootKind::File));
    }
}
