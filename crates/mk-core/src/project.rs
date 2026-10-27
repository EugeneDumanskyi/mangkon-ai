//! One open project: its store, source roots, re-scan and the Stage A lane.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use mk_corpus::fs::{FileTypes, Skipped};
use mk_corpus::{RescanPlan, Resolved, RootId};
use mk_graph::DecisionKind;
use mk_jobs::Trigger;
use mk_kernel::ContentHash;
use mk_parsers::Format;
use mk_store::{OpenReport, Recovery, RescanChange, RescanOutcome, Store, StoreError};
use mk_workspace::{ProjectLayout, RootKind};

use crate::events::{ConnectionsChanged, DEBOUNCE, Events};
use crate::stage_a;

#[derive(Debug)]
pub enum ProjectError {
    Store(StoreError),
    Io(PathBuf, std::io::Error),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Store(e) => e.fmt(f),
            ProjectError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<StoreError> for ProjectError {
    fn from(e: StoreError) -> Self {
        ProjectError::Store(e)
    }
}

/// What opening did: the schema steps (domain §7.9) and the restart recovery (ADR-0006).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenSummary {
    pub schema: OpenReport,
    pub recovered: Recovery,
}

/// What a re-scan found and did (US-01.3, US-01.4).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RescanSummary {
    /// Supported files found.
    pub walked: usize,
    /// Files read and hashed (new or changed size/time, or all with `full`).
    pub hashed: usize,
    /// Paths with nothing to do.
    pub unchanged: usize,
    pub outcome: RescanOutcome,
    /// Unsupported files and names that cannot be stored (INV-CO-11), with their root.
    pub skipped: Vec<(RootId, Skipped)>,
    /// Folders or files that could not be read.
    pub errors: Vec<(RootId, String, String)>,
}

/// What a Stage A run did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StageASummary {
    pub done: usize,
    pub failed: usize,
    /// Commits that changed connection rows.
    pub changed_commits: usize,
    /// For each committed item: time from "parse finished" to "commit done", which includes
    /// waiting for the writer (spike S2c, NFR-PERF-34).
    pub commit_latencies: Vec<Duration>,
}

/// An open project (ADR-0002: the shell and the CLI reach the core only through services like
/// this one).
pub struct Project {
    layout: ProjectLayout,
    store: Store,
    events: Events,
    workers: usize,
}

/// The file types with a parser (US-01.3 AC3).
pub fn file_types() -> FileTypes {
    FileTypes::new(Format::ALL.iter().map(|f| f.as_str()))
}

impl Project {
    /// Opens the project in `dir`, creating it if needed: schema check, backup and migrations
    /// (domain §7.9), then restart recovery (INV-PR-05/07), then the writer and readers run.
    pub fn open(dir: &Path) -> Result<(Project, OpenSummary), ProjectError> {
        let layout = ProjectLayout::new(dir);
        std::fs::create_dir_all(dir).map_err(|e| ProjectError::Io(dir.into(), e))?;
        let (store, schema) = Store::open(&layout.database(), &layout.backups())?;
        let recovered = store.recover_after_restart()?;
        let workers =
            mk_jobs::stage_a_limit(std::thread::available_parallelism().map_or(1, |n| n.get()));
        Ok((
            Project {
                layout,
                store,
                events: Events::start(DEBOUNCE),
                workers,
            },
            OpenSummary { schema, recovered },
        ))
    }

    pub fn layout(&self) -> &ProjectLayout {
        &self.layout
    }

    /// The store, for reads (and for tests and benchmarks that write through its API).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Overrides the Stage A lane width (default cores − 1, NFR-PERF-25).
    pub fn set_stage_a_workers(&mut self, n: usize) {
        self.workers = n.max(1);
    }

    /// `ConnectionsChanged` events, debounced 500 ms.
    pub fn subscribe(&self) -> Receiver<ConnectionsChanged> {
        self.events.subscribe()
    }

    /// Adds a folder or a single file as a source root. The path is stored absolute.
    pub fn add_source_root(&self, path: &Path) -> Result<i64, ProjectError> {
        let abs = std::fs::canonicalize(path).map_err(|e| ProjectError::Io(path.into(), e))?;
        let kind = if abs.is_file() {
            RootKind::File
        } else {
            RootKind::Folder
        };
        let s = abs.to_str().ok_or_else(|| {
            ProjectError::Io(
                abs.clone(),
                std::io::Error::other("path is not valid Unicode"),
            )
        })?;
        Ok(self.store.add_source_root(s, kind.as_str())?)
    }

    /// Re-scans every source root (architecture §5.4): walk, compare size and time with the
    /// stored references, hash only new or changed files (all of them with `full`), then commit
    /// the outcome in one transaction. Unchanged files create no work (INV-PR-08).
    pub fn rescan(&self, full: bool) -> Result<RescanSummary, ProjectError> {
        let types = file_types();
        let roots = self.store.source_roots()?;
        let mut summary = RescanSummary::default();
        let mut walked = Vec::new();
        let mut paths: HashMap<RootId, PathBuf> = HashMap::new();
        for root in &roots {
            let path = PathBuf::from(&root.path);
            let w = mk_corpus::fs::walk(root.id, &path, &types);
            walked.extend(w.entries);
            summary
                .skipped
                .extend(w.skipped.into_iter().map(|s| (root.id, s)));
            summary
                .errors
                .extend(w.errors.into_iter().map(|(p, e)| (root.id, p, e)));
            paths.insert(root.id, path);
        }
        summary.walked = walked.len();
        let stored = self.store.stored_refs()?;
        let trigger = if stored.is_empty() {
            Trigger::Add
        } else {
            Trigger::Rescan
        };
        let root_ids: Vec<RootId> = roots.iter().map(|r| r.id).collect();
        let plan = mk_corpus::plan(&stored, &walked, &root_ids, full);
        let hashes = self.hash_planned(&plan, &paths);
        summary.hashed = hashes.len();
        let Resolved { changes, unchanged } = plan.resolve(&hashes);
        summary.unchanged = unchanged;
        if changes.is_empty() {
            return Ok(summary);
        }
        let changes = changes
            .into_iter()
            .map(|change| {
                let path = mk_corpus::fs::resolve(&paths[&change.root()], change.rel_path());
                let file_type = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(Format::from_extension)
                    .map_or_else(String::new, |f| f.as_str().to_string());
                RescanChange { change, file_type }
            })
            .collect();
        summary.outcome = self.store.apply_rescan(changes, trigger)?;
        if summary.outcome.connections_changed {
            self.events.changed();
        }
        Ok(summary)
    }

    /// Hashes the plan's new and changed paths on all cores. A file that cannot be read is left
    /// out (it stays as it was).
    fn hash_planned(
        &self,
        plan: &RescanPlan,
        roots: &HashMap<RootId, PathBuf>,
    ) -> HashMap<usize, ContentHash> {
        let work: Vec<(usize, PathBuf)> = plan
            .to_hash()
            .map(|(i, item)| {
                (
                    i,
                    mk_corpus::fs::resolve(&roots[&item.root], &item.rel_path),
                )
            })
            .collect();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let next = Mutex::new(work.into_iter());
        let out = Mutex::new(HashMap::new());
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let Some((i, path)) = next.lock().unwrap_or_else(|e| e.into_inner()).next()
                        else {
                            return;
                        };
                        if let Ok(h) = mk_corpus::fs::hash_file(&path) {
                            out.lock().unwrap_or_else(|e| e.into_inner()).insert(i, h);
                        }
                    }
                });
            }
        });
        out.into_inner().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs the Stage A lane until no item is pending: up to `cores − 1` threads take items in
    /// scheduler order (INV-PR-03), parse in process and send each result to the one writer,
    /// which commits it with its derived rebuild (ADR-0005). A failure fails only its item
    /// (INV-CO-09, US-02.7).
    pub fn run_stage_a(&self) -> Result<StageASummary, ProjectError> {
        let summary = Mutex::new(StageASummary::default());
        let fatal: Mutex<Option<StoreError>> = Mutex::new(None);
        loop {
            let pending = self.store.pending_items()?;
            if pending.is_empty() {
                break;
            }
            let order = mk_jobs::next_batch(&pending, 0, pending.len());
            let queue = Mutex::new(VecDeque::from(order));
            std::thread::scope(|s| {
                for _ in 0..self.workers.min(pending.len()) {
                    s.spawn(|| {
                        while fatal.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                            let Some(id) =
                                queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
                            else {
                                return;
                            };
                            if let Err(e) = self.run_item(id, &summary) {
                                fatal
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .get_or_insert(e);
                            }
                        }
                    });
                }
            });
            if let Some(e) = fatal.lock().unwrap_or_else(|e| e.into_inner()).take() {
                return Err(e.into());
            }
        }
        Ok(summary.into_inner().unwrap_or_else(|e| e.into_inner()))
    }

    fn run_item(&self, id: i64, summary: &Mutex<StageASummary>) -> Result<(), StoreError> {
        let Some(started) = self.store.start_item(id)? else {
            return Ok(());
        };
        let result = stage_a::process(&started);
        let lock = || summary.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(output) => {
                let parsed_at = Instant::now();
                match self.store.commit_stage_a(id, output) {
                    Ok(outcome) => {
                        let latency = parsed_at.elapsed();
                        if outcome.connections_changed {
                            self.events.changed();
                        }
                        let mut s = lock();
                        s.done += 1;
                        s.changed_commits += usize::from(outcome.connections_changed);
                        s.commit_latencies.push(latency);
                    }
                    // The commit rolled back. Record why on the item, so it does not come back.
                    Err(e @ StoreError::Invalid(_)) => {
                        self.store.fail_item(
                            id,
                            "internal_error",
                            &format!("Could not store the results: {e}"),
                        )?;
                        lock().failed += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(f) => {
                self.store.fail_item(id, f.code, &f.reason)?;
                lock().failed += 1;
            }
        }
        Ok(())
    }

    /// Ignores (or un-ignores) an identifier connection: appends the decision and rebuilds its
    /// anchor in one transaction (US-07.4, INV-CU-01). Returns the decision's `seq`.
    pub fn set_ignored(&self, key: &str, ignored: bool) -> Result<i64, ProjectError> {
        let kind = if ignored {
            DecisionKind::IgnoreIdentifier
        } else {
            DecisionKind::UnignoreIdentifier
        };
        let (seq, changed) = self.store.append_decision(
            kind,
            key,
            Some(if ignored { "shown" } else { "ignored" }),
            Some(if ignored { "ignored" } else { "shown" }),
        )?;
        if changed {
            self.events.changed();
        }
        Ok(seq)
    }
}
