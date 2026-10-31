//! M3c services: the documents list (US-02.8), retry (US-02.7 AC3), removing files and roots
//! (US-01.5), settings (US-01.6), relocating roots (D12) and the progress panel (US-03.1).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use mk_store::{CardFlagRow, DocumentCursor, DocumentFilter, DocumentRow, RemoveOutcome};
use mk_workspace::{ProjectSettings, Reprocess, RootKind};

use crate::project::{Project, ProjectError};

/// One page of the documents list and the cursor of the page after it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocumentsPage {
    pub rows: Vec<DocumentRow>,
    /// `None` on the last page.
    pub next: Option<DocumentCursor>,
}

/// A source root and whether its folder or file is there now (a missing one offers "Locate…").
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SourceRootInfo {
    pub id: i64,
    pub path: String,
    pub kind: RootKind,
    pub present: bool,
    /// Paths removed from it (US-01.5).
    pub excluded: usize,
}

/// The ETA as the panel shows it (US-03.1 AC2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eta {
    /// Fewer than [`mk_jobs::ETA_MIN_DONE`] files finished in this run.
    Estimating,
    Left(Duration),
    /// Nothing is running.
    None,
}

/// The progress panel (US-03.1 AC1). Stage A and the identifier re-run: Stages B and C come
/// in M6.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProgressView {
    /// `"identifiers"` while identifiers are found again from stored text (M4b), `"A"` while
    /// Stage A work is open; `None` when the queue is empty.
    pub stage: Option<&'static str>,
    /// Files finished (done or failed) in this run, and those plus the files still to do. In
    /// the identifier re-run: contents done and to do.
    pub files_done: u64,
    pub files_total: u64,
    /// The files in a worker now.
    pub current_files: Vec<String>,
    pub eta: Eta,
    pub paused: bool,
    /// A re-scan or Stage A run is going on.
    pub running: bool,
}

/// Contents to retry: Failed and Not processed.
const RETRYABLE: [&str; 2] = ["failed", "not_processed"];

impl Project {
    /// The project name, if it has one (projects made before M3c have none).
    pub fn name(&self) -> Result<Option<String>, ProjectError> {
        Ok(self.store().meta("name")?)
    }

    /// One page of the documents list in path order (US-02.8), after `after`.
    pub fn documents_page(
        &self,
        filter: &DocumentFilter,
        after: Option<&DocumentCursor>,
        limit: usize,
    ) -> Result<DocumentsPage, ProjectError> {
        let limit = limit.max(1);
        let rows = self.store().documents_page(filter, after, limit)?;
        let next = (rows.len() == limit)
            .then(|| rows.last().map(DocumentRow::cursor))
            .flatten();
        Ok(DocumentsPage { rows, next })
    }

    pub fn documents_count(&self, filter: &DocumentFilter) -> Result<u64, ProjectError> {
        Ok(self.store().documents_count(filter)?)
    }

    /// Paths per file type, for the type filter (US-02.8 AC2).
    pub fn document_types(&self) -> Result<BTreeMap<String, u64>, ProjectError> {
        Ok(self.store().document_type_counts()?)
    }

    /// Queues `contents` again, those that failed or were not processed (US-02.7 AC3). Only
    /// those files run; the caller starts [`Project::ingest`]. Returns how many were queued.
    pub fn retry(&self, contents: &[i64]) -> Result<usize, ProjectError> {
        Ok(self.store().retry_contents(contents.to_vec())?)
    }

    /// Queues every failed and not-processed file again.
    pub fn retry_all(&self) -> Result<usize, ProjectError> {
        let mut ids = Vec::new();
        for status in RETRYABLE {
            let filter = DocumentFilter {
                status: Some(status.into()),
                file_type: None,
            };
            let mut after = None;
            loop {
                let page = self.documents_page(&filter, after.as_ref(), 1_000)?;
                ids.extend(page.rows.iter().map(|r| r.content_id));
                match page.next {
                    Some(n) => after = Some(n),
                    None => break,
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        self.retry(&ids)
    }

    /// How many connections lose evidence if these paths are removed (US-01.5 AC2).
    pub fn removal_impact(&self, refs: &[i64]) -> Result<u64, ProjectError> {
        Ok(self.store().removal_impact(refs)?)
    }

    /// Removes paths from the project (US-01.5 AC1): their data goes when no other path has the
    /// same content (INV-CO-08); connections are rebuilt; the files on disk are not touched.
    /// A file being read now is refused: pause first.
    pub fn remove_files(&self, refs: &[i64]) -> Result<RemoveOutcome, ProjectError> {
        let out = self.store().remove_file_refs(refs.to_vec())?;
        if out.connections_changed {
            self.changed();
        }
        Ok(out)
    }

    /// Removes a source root with every path in it.
    pub fn remove_root(&self, root: i64) -> Result<RemoveOutcome, ProjectError> {
        let out = self.store().remove_root(root)?;
        if out.connections_changed {
            self.changed();
        }
        Ok(out)
    }

    /// Source roots, each with whether it is there now.
    pub fn source_roots(&self) -> Result<Vec<SourceRootInfo>, ProjectError> {
        self.store()
            .source_roots()?
            .into_iter()
            .map(|r| {
                let kind = RootKind::from_name(&r.kind).unwrap_or(RootKind::Folder);
                let md = std::fs::metadata(&r.path);
                let present = md.is_ok_and(|m| match kind {
                    RootKind::Folder => m.is_dir(),
                    RootKind::File => m.is_file(),
                });
                Ok(SourceRootInfo {
                    excluded: self.store().exclusions(r.id)?.len(),
                    id: r.id,
                    path: r.path,
                    kind,
                    present,
                })
            })
            .collect()
    }

    /// Points a source root at the folder or file it was moved to (D12). The caller re-scans:
    /// files with the same size and time are unchanged, others are hashed, and the same hash is
    /// found again with nothing redone.
    pub fn relocate_root(&self, root: i64, to: &Path) -> Result<(), ProjectError> {
        let abs = std::fs::canonicalize(to).map_err(|e| ProjectError::Io(to.into(), e))?;
        let current = self
            .store()
            .source_roots()?
            .into_iter()
            .find(|r| r.id == root)
            .ok_or_else(|| ProjectError::Invalid(format!("No source {root} in this project")))?;
        let want_file = current.kind == RootKind::File.as_str();
        if abs.is_file() != want_file {
            return Err(ProjectError::Invalid(if want_file {
                "Pick the file itself, not a folder".into()
            } else {
                "Pick a folder, not a file".into()
            }));
        }
        let s = abs
            .to_str()
            .ok_or_else(|| ProjectError::Invalid("The path is not valid Unicode".into()))?;
        Ok(self.store().relocate_root(root, s)?)
    }

    /// The project's settings (US-01.6).
    pub fn settings(&self) -> Result<ProjectSettings, ProjectError> {
        Ok(self.store().settings()?)
    }

    /// Checks and saves settings (US-01.6). New OCR languages and a new phone region apply to
    /// files read from now on; the returned [`Reprocess`] says what already read would change,
    /// for "Re-process affected files now?" ([`Project::reprocess`], INV-WS-07). A stop-list
    /// change applies at once: the domains added or dropped get their connections rebuilt in
    /// the same transaction (US-04.3 AC3, INV-LK-03).
    pub fn save_settings(&self, settings: ProjectSettings) -> Result<Reprocess, ProjectError> {
        let old = self.settings()?;
        // Custom patterns are saved on their own (`Project::add_pattern` and the rest).
        let new = ProjectSettings {
            custom_patterns: old.custom_patterns.clone(),
            ..settings
        }
        .normalized()
        .map_err(|e| ProjectError::Invalid(e.to_string()))?;
        if let Some(r) = &new.phone_region
            && mk_identifiers::Region::parse(r).is_none()
        {
            return Err(ProjectError::Invalid(format!(
                "Not a known phone region: {r} (use a country code, such as DE)"
            )));
        }
        let reprocess = ProjectSettings::affected_by(&old, &new);
        if self.store().set_settings(new)? {
            self.changed();
        }
        Ok(reprocess)
    }

    /// Re-does what a settings change affects (US-01.6 AC3, INV-WS-07): with `ocr`, queues the
    /// files with OCR'd pages and the files that failed for a missing OCR language; with
    /// `identifiers`, starts finding every content's identifiers again from its stored text
    /// (US-04.2 AC2). Returns how many files were queued. The caller starts
    /// [`Project::ingest`], which runs both.
    pub fn reprocess(&self, what: Reprocess) -> Result<usize, ProjectError> {
        let mut n = 0;
        if what.ocr {
            n += self.store().requeue_for_ocr()?;
        }
        if what.identifiers {
            self.store().begin_identifier_rerun()?;
        }
        Ok(n)
    }

    /// A document's card-like numbers as masked text with their places, in reading order
    /// (US-04.6 AC1). Only the last four digits are stored (NFR-SEC-14).
    pub fn card_flags(&self, document: i64) -> Result<Vec<CardFlagRow>, ProjectError> {
        Ok(self.store().card_flags(document)?)
    }

    /// The progress panel (US-03.1): files done and total in this run, the files in a worker
    /// now, and the ETA once 5 files are done (bytes left at this run's rate).
    pub fn progress_view(&self) -> Result<ProgressView, ProjectError> {
        let started = self.run_started();
        let d = self.store().progress_detail(started.map(|(_, ms)| ms))?;
        let running = self.is_running();
        let rerun = *self
            .rerun_progress()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((step, done, total)) = rerun {
            return Ok(ProgressView {
                stage: Some(match step {
                    mk_store::RerunStep::Builtin => "identifiers",
                    mk_store::RerunStep::Custom => "custom patterns",
                }),
                files_done: done,
                files_total: total,
                current_files: Vec::new(),
                eta: Eta::None,
                paused: d.paused,
                running,
            });
        }
        let eta = match started {
            Some((at, _)) if running && !d.paused && d.open > 0 => {
                match mk_jobs::estimate(d.finished, d.finished_bytes, d.open_bytes, at.elapsed()) {
                    Some(left) => Eta::Left(left),
                    None => Eta::Estimating,
                }
            }
            _ => Eta::None,
        };
        Ok(ProgressView {
            stage: (d.open > 0).then_some("A"),
            files_done: d.finished,
            files_total: d.finished + d.open,
            current_files: self.current_files(),
            eta,
            paused: d.paused,
            running,
        })
    }
}
