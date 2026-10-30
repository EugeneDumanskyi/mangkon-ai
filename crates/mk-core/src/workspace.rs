//! The app's projects (US-01.1, US-01.2, domain §5.2): create, open, the recent list, locate and
//! forget, over the registry in `app.db`. [`Project::open`] stays as it is for the CLI and tests,
//! which name a folder directly.

use std::fmt;
use std::path::{Path, PathBuf};

use mk_store::{AppStore, StoreError};
use mk_workspace::{NameError, ProjectLayout, ProjectName, RegistryEntry};

use crate::project::{OpenSummary, Project, ProjectError, project_exists};

/// Why a project could not be created or opened, in plain words (NFR-USE-05).
#[derive(Debug)]
pub enum WorkspaceError {
    Name(NameError),
    /// INV-WS-01, US-01.1 AC2.
    NameTaken,
    /// INV-WS-02, US-01.1 AC3.
    NotWritable(PathBuf),
    /// `<location>/<name>` exists already.
    FolderExists(PathBuf),
    /// US-01.2 AC2: the folder is gone; the UI offers Locate… and Remove from list.
    NotFound(PathBuf),
    NotAProject(PathBuf),
    /// US-01.2 AC3: made by a newer app, not opened or changed.
    TooNew {
        needs: Option<String>,
    },
    /// US-01.2 AC4: the migration failed and rolled back; the backup is kept.
    MigrationFailed {
        backup: Option<PathBuf>,
        reason: String,
    },
    /// No registry entry with that id.
    NoEntry(i64),
    Project(ProjectError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceError::Name(e) => e.fmt(f),
            WorkspaceError::NameTaken => f.write_str("A project with this name already exists"),
            WorkspaceError::NotWritable(_) => f.write_str("Cannot write to this folder"),
            WorkspaceError::FolderExists(p) => {
                write!(f, "A folder named {} already exists there", p.display())
            }
            WorkspaceError::NotFound(p) => write!(f, "Project not found at {}", p.display()),
            WorkspaceError::NotAProject(p) => {
                write!(f, "{} is not a Mangkon AI project", p.display())
            }
            WorkspaceError::TooNew { needs } => {
                match needs {
                    Some(v) => write!(f, "This project needs Mangkon AI {v} or newer")?,
                    None => f.write_str("This project needs a newer version of Mangkon AI")?,
                }
                f.write_str(". It was not opened or changed.")
            }
            WorkspaceError::MigrationFailed { backup, reason } => {
                f.write_str("The project could not be upgraded and was not changed")?;
                if let Some(b) = backup {
                    write!(f, "; a backup is kept at {}", b.display())?;
                }
                write!(f, " ({reason})")
            }
            WorkspaceError::NoEntry(id) => write!(f, "No project {id} in the list"),
            WorkspaceError::Project(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<ProjectError> for WorkspaceError {
    fn from(e: ProjectError) -> Self {
        match e {
            ProjectError::Store(e) => e.into(),
            e => WorkspaceError::Project(e),
        }
    }
}

impl From<StoreError> for WorkspaceError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::TooNew { app_version, .. } => WorkspaceError::TooNew { needs: app_version },
            StoreError::MigrationFailed { backup, reason, .. } => {
                WorkspaceError::MigrationFailed { backup, reason }
            }
            StoreError::NameTaken(_) => WorkspaceError::NameTaken,
            e => WorkspaceError::Project(ProjectError::Store(e)),
        }
    }
}

/// A recent project and whether its folder still holds it (US-01.2 AC1–AC2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecentProject {
    pub entry: RegistryEntry,
    pub present: bool,
}

/// The app's projects: the registry in `<app data>/app.db`.
pub struct Workspace {
    app: AppStore,
    dir: PathBuf,
}

/// Whether the app can create files in `dir`: it creates and removes a probe file
/// (INV-WS-02). Permission bits alone do not tell (read-only mounts, ACLs).
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".mangkon-write-check-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => std::fs::remove_file(&probe).is_ok(),
        Err(_) => false,
    }
}

fn path_string(p: &Path) -> Result<String, WorkspaceError> {
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| WorkspaceError::NotAProject(p.to_path_buf()))
}

impl Workspace {
    /// Opens the registry in `dir` (the app's data folder; tests pass a temp folder).
    pub fn open(dir: &Path) -> Result<Workspace, WorkspaceError> {
        Ok(Workspace {
            app: AppStore::open(dir)?,
            dir: dir.to_path_buf(),
        })
    }

    /// Opens the registry in [`crate::app_data_dir`].
    pub fn open_default() -> Result<Workspace, WorkspaceError> {
        let dir = crate::app_data_dir().ok_or_else(|| {
            WorkspaceError::Project(ProjectError::Invalid(
                "The system gives no folder for app data".into(),
            ))
        })?;
        Self::open(&dir)
    }

    /// The app's data folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Recent projects, newest first, each with whether its folder still holds it.
    pub fn recent(&self) -> Result<Vec<RecentProject>, WorkspaceError> {
        Ok(self
            .app
            .recent()?
            .into_iter()
            .map(|entry| RecentProject {
                present: project_exists(Path::new(&entry.path)),
                entry,
            })
            .collect())
    }

    /// Creates the project `name` in `<location>/<folder name>` and opens it (US-01.1 AC1):
    /// the name must be free (INV-WS-01), the location writable (INV-WS-02) and the folder new.
    pub fn create(
        &self,
        name: &str,
        location: &Path,
    ) -> Result<(Project, RegistryEntry), WorkspaceError> {
        let name = ProjectName::parse(name).map_err(WorkspaceError::Name)?;
        if self.app.name_taken(&name)? {
            return Err(WorkspaceError::NameTaken);
        }
        if !location.is_dir() || !writable(location) {
            return Err(WorkspaceError::NotWritable(location.to_path_buf()));
        }
        let location = std::fs::canonicalize(location)
            .map_err(|_| WorkspaceError::NotWritable(location.to_path_buf()))?;
        let dir = location.join(name.folder_name());
        if dir.exists() {
            return Err(WorkspaceError::FolderExists(dir));
        }
        let path = path_string(&dir)?;
        std::fs::create_dir(&dir).map_err(|_| WorkspaceError::NotWritable(location.clone()))?;
        let created = (|| {
            let (project, _) = Project::open(&dir)?;
            project
                .store()
                .stamp(Some(name.as_str()), env!("CARGO_PKG_VERSION"))
                .map_err(ProjectError::from)?;
            let entry = self.app.register(&name, &path)?;
            Ok((project, entry))
        })();
        if created.is_err() {
            // Only the folder made just now, with what this call wrote in it.
            let _ = std::fs::remove_dir_all(&dir);
        }
        created
    }

    /// Opens the project in `dir` (US-01.2): a recent one, or one picked with Open…. A folder
    /// not in the list is registered under the project's stored name (its folder name for a
    /// project made before names were stored).
    pub fn open_project(
        &self,
        dir: &Path,
    ) -> Result<(Project, OpenSummary, RegistryEntry), WorkspaceError> {
        if !dir.is_dir() {
            return Err(WorkspaceError::NotFound(dir.to_path_buf()));
        }
        let dir =
            std::fs::canonicalize(dir).map_err(|_| WorkspaceError::NotFound(dir.to_path_buf()))?;
        if !ProjectLayout::new(&dir).database().is_file() {
            return Err(WorkspaceError::NotAProject(dir));
        }
        let path = path_string(&dir)?;
        let entry = self.app.entry_at(&path)?;
        let (project, summary) = Project::open(&dir)?;
        let stored = project.name()?;
        let entry = match entry {
            Some(e) => {
                self.app.touch(e.id)?;
                e
            }
            None => {
                let fallback = dir
                    .file_name()
                    .map_or_else(|| "Project".into(), |n| n.to_string_lossy().into_owned());
                let name = ProjectName::parse(stored.as_deref().unwrap_or(&fallback))
                    .or_else(|_| ProjectName::parse("Project"))
                    .map_err(WorkspaceError::Name)?;
                self.app.register(&name, &path)?
            }
        };
        if stored.is_none() {
            project
                .store()
                .stamp(Some(&entry.name), env!("CARGO_PKG_VERSION"))
                .map_err(ProjectError::from)?;
        }
        let entry = self.app.entry(entry.id)?.unwrap_or(entry);
        Ok((project, summary, entry))
    }

    /// Opens a registered project by id. A folder that is gone gives
    /// [`WorkspaceError::NotFound`] (US-01.2 AC2).
    pub fn open_entry(
        &self,
        id: i64,
    ) -> Result<(Project, OpenSummary, RegistryEntry), WorkspaceError> {
        let entry = self.app.entry(id)?.ok_or(WorkspaceError::NoEntry(id))?;
        if !project_exists(Path::new(&entry.path)) {
            return Err(WorkspaceError::NotFound(entry.path.into()));
        }
        self.open_project(Path::new(&entry.path))
    }

    /// "Locate…" (US-01.2 AC2): the entry now points at `dir`, which must hold a project.
    pub fn locate(&self, id: i64, dir: &Path) -> Result<RegistryEntry, WorkspaceError> {
        self.app.entry(id)?.ok_or(WorkspaceError::NoEntry(id))?;
        if !project_exists(dir) {
            return Err(WorkspaceError::NotAProject(dir.to_path_buf()));
        }
        let dir =
            std::fs::canonicalize(dir).map_err(|_| WorkspaceError::NotFound(dir.to_path_buf()))?;
        self.app.relocate(id, &path_string(&dir)?)?;
        self.app.entry(id)?.ok_or(WorkspaceError::NoEntry(id))
    }

    /// "Remove from list" (US-01.2 AC2). The project folder is not touched.
    pub fn forget(&self, id: i64) -> Result<(), WorkspaceError> {
        Ok(self.app.remove(id)?)
    }

    /// The entry registered at `dir`, if any.
    pub fn entry_at(&self, dir: &Path) -> Result<Option<RegistryEntry>, WorkspaceError> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        Ok(self.app.entry_at(&path_string(&dir)?)?)
    }
}
