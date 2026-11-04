//! First run and the demo project (US-14.4).
//!
//! - [`Workspace::first_run`]: shown while `app_setting` `first_run_done` is unset and no
//!   project is in the list; it also says where the demo files are, if found.
//! - The demo files are TD-1 (`testdata/td1`, made by `corpus-gen`, docs/07-test-corpus.md).
//!   They come with the source code, not the app, so they are looked for above the running
//!   binary and the current folder; else the user picks the folder.
//! - [`Workspace::open_demo`]: the "Demo project" in `<app data>/demo-project`, with TD-1's
//!   `synthetic/` and `public/` folders as its sources. The caller starts the ingest.

use std::path::{Path, PathBuf};

use mk_workspace::{ProjectName, RegistryEntry};

use crate::models::ModelError;
use crate::project::{OpenSummary, Project, ProjectError};
use crate::workspace::{Workspace, WorkspaceError};

/// The `app_setting` key set when the first-run screen is done or skipped.
pub(crate) const DONE_KEY: &str = "first_run_done";

/// The demo project's folder under the app's data folder.
pub const DEMO_FOLDER: &str = "demo-project";

/// The demo project's name (a number is added if a project has it already).
pub const DEMO_NAME: &str = "Demo project";

/// TD-1's source folders, in the order they are added.
const DEMO_SOURCES: [&str; 2] = ["synthetic", "public"];

/// What the first-run screen needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstRun {
    /// Show the first-run screen (US-14.4 AC1).
    pub show: bool,
    /// TD-1, when found next to the app or the current folder (US-14.4 AC2).
    pub demo: Option<PathBuf>,
}

/// Whether `dir` is TD-1: it has a `ground-truth.json` written by `corpus-gen`.
pub fn is_demo_corpus(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("ground-truth.json")) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .is_some_and(|v| v["generator"]["name"] == "corpus-gen")
}

/// TD-1 in `dir`: `dir` itself, `dir/td1` or `dir/testdata/td1`.
pub fn demo_corpus_in(dir: &Path) -> Option<PathBuf> {
    [
        dir.to_path_buf(),
        dir.join("td1"),
        dir.join("testdata").join("td1"),
    ]
    .into_iter()
    .find(|d| is_demo_corpus(d))
}

/// TD-1 above any of `starts` (each start and its parent folders), the first found.
pub fn find_demo_corpus(starts: &[PathBuf]) -> Option<PathBuf> {
    starts
        .iter()
        .flat_map(|s| s.ancestors())
        .find_map(demo_corpus_in)
        .and_then(|d| std::fs::canonicalize(d).ok())
}

/// Where to look for TD-1: the running binary's folder and the current folder.
fn demo_starts() -> Vec<PathBuf> {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    exe.into_iter()
        .chain(std::env::current_dir().ok())
        .collect()
}

impl Workspace {
    /// Whether to show the first-run screen, and where the demo files are.
    pub fn first_run(&self) -> Result<FirstRun, WorkspaceError> {
        let done = self.app().setting(DONE_KEY)?.is_some();
        let show = !done && self.recent()?.is_empty();
        Ok(FirstRun {
            show,
            demo: find_demo_corpus(&demo_starts()),
        })
    }

    /// The first-run screen is done or skipped; it is not shown again.
    pub fn finish_first_run(&self) -> Result<(), ModelError> {
        self.app()
            .set_setting(DONE_KEY, Some("1"))
            .map_err(ModelError::store)
    }

    /// "Open demo project" (US-14.4 AC2): opens the demo project, or creates it in
    /// `<app data>/demo-project`, with TD-1's folders under `corpus` (TD-1, `td1/` or
    /// `testdata/td1/` in it) as sources. The first-run screen is done after this.
    pub fn open_demo(
        &self,
        corpus: &Path,
    ) -> Result<(Project, OpenSummary, RegistryEntry), WorkspaceError> {
        let corpus = demo_corpus_in(corpus).ok_or_else(|| {
            WorkspaceError::Project(ProjectError::Invalid(format!(
                "{} does not hold the demo files (testdata/td1 from the source code)",
                corpus.display()
            )))
        })?;
        let dir = self.dir().join(DEMO_FOLDER);
        if !dir.exists() {
            // Created, closed and opened again like any other project.
            drop(self.create_demo(&dir)?);
        }
        let (project, summary, entry) = self.open_project(&dir)?;
        for name in DEMO_SOURCES {
            let src = corpus.join(name);
            if src.is_dir() {
                project.add_source_root(&src)?;
            }
        }
        self.app().set_setting(DONE_KEY, Some("1"))?;
        Ok((project, summary, entry))
    }

    fn create_demo(&self, dir: &Path) -> Result<(Project, RegistryEntry), WorkspaceError> {
        std::fs::create_dir_all(self.dir())
            .map_err(|_| WorkspaceError::NotWritable(self.dir().to_path_buf()))?;
        for n in 1..=9 {
            let text = match n {
                1 => DEMO_NAME.to_string(),
                n => format!("{DEMO_NAME} {n}"),
            };
            let name = ProjectName::parse(&text).map_err(WorkspaceError::Name)?;
            if !self.app().name_taken(&name)? {
                return self.create_in(&name, dir);
            }
        }
        Err(WorkspaceError::NameTaken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(dir: &Path, generator: &str) {
        std::fs::create_dir_all(dir.join("synthetic")).unwrap();
        std::fs::write(
            dir.join("ground-truth.json"),
            format!(r#"{{"schema_version":1,"generator":{{"name":"{generator}"}}}}"#),
        )
        .unwrap();
    }

    /// US-14.4 AC2: TD-1 is found above the binary or the current folder.
    #[test]
    fn us_14_4_demo_detection() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        corpus(&repo.join("testdata/td1"), "corpus-gen");
        let bin = repo.join("target/debug");
        std::fs::create_dir_all(&bin).unwrap();
        let found = find_demo_corpus(&[bin]).unwrap();
        assert_eq!(
            found,
            std::fs::canonicalize(repo.join("testdata/td1")).unwrap()
        );
        assert_eq!(demo_corpus_in(&repo), Some(repo.join("testdata/td1")));

        let other = t.path().join("other");
        corpus(&other, "something-else");
        assert_eq!(find_demo_corpus(std::slice::from_ref(&other)), None);
        assert!(!is_demo_corpus(&other.join("synthetic")));
    }
}
