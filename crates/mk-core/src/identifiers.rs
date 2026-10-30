//! M4b: finding identifiers again from stored segments, when the phone region changes
//! (US-04.2 AC2) or a project's mentions were made with older rules
//! ([`mk_identifiers::RULES_VERSION`]). No file is read again and nothing is OCR'd again: the
//! same [`mk_worker::extract::segment_identifiers`] the workers run reads each stored segment.
//!
//! The re-run goes content by content, each in one transaction with its derived rebuild
//! (ADR-0005), and the store keeps a cursor past the last one, so a kill or a pause loses at most
//! one content's work and the next run goes on from there (INV-PR-04/05). It is not a job in
//! `mk-jobs`: its work items are files to read, and this reads none.

use mk_identifiers::{RULES_VERSION, Region, Rules};
use mk_kernel::Locator;
use mk_parsers::{ParsedSegment, RowState};
use mk_store::{FoundFlag, FoundMention, SegmentFinds, StoredSegment};

use crate::project::{Project, ProjectError};

/// What [`Project::rerun_identifiers`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RerunSummary {
    /// Contents whose identifiers were found again.
    pub contents: usize,
    pub mentions: usize,
    pub card_flags: usize,
    /// Pause, cancel or close came first: the rest is done by the next run.
    pub interrupted: bool,
}

impl Project {
    /// The finder rules of this project: its phone region (US-04.2).
    pub(crate) fn identifier_rules(&self) -> Result<Rules, ProjectError> {
        Ok(Rules {
            phone_region: self
                .store()
                .settings()?
                .phone_region
                .as_deref()
                .and_then(Region::parse),
        })
    }

    /// Whether stored identifiers are to be found again: a re-run was started and not finished,
    /// or they were made with older rules.
    pub fn identifiers_pending(&self) -> Result<bool, ProjectError> {
        let (version, after) = self.store().identifier_state()?;
        Ok(after.is_some() || version != Some(RULES_VERSION))
    }

    /// A project with nothing read yet follows the current rules as it is: no re-run needed.
    pub(crate) fn stamp_identifier_rules_if_empty(&self) -> Result<(), ProjectError> {
        let (version, after) = self.store().identifier_state()?;
        if version != Some(RULES_VERSION)
            && after.is_none()
            && self.store().rerun_contents(0, 1)?.is_empty()
        {
            self.store().finish_identifier_rerun(None, RULES_VERSION)?;
        }
        Ok(())
    }

    /// Finds every content's identifiers again from its stored segments with the current rules,
    /// if [`Project::identifiers_pending`]. Waits for a running ingest to end first. A paused
    /// queue does nothing.
    pub fn rerun_identifiers(&self) -> Result<RerunSummary, ProjectError> {
        let _run = self.begin_run();
        self.rerun_identifiers_locked()
    }

    pub(crate) fn rerun_identifiers_locked(&self) -> Result<RerunSummary, ProjectError> {
        let mut summary = RerunSummary::default();
        let result = self.rerun_loop(&mut summary);
        *self
            .rerun_progress()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        result.map(|()| summary)
    }

    fn rerun_loop(&self, summary: &mut RerunSummary) -> Result<(), ProjectError> {
        'restart: loop {
            if !self.identifiers_pending()? || self.is_paused()? {
                return Ok(());
            }
            let after = match self.store().identifier_state()?.1 {
                Some(after) => after,
                None => {
                    self.store().begin_identifier_rerun()?;
                    0
                }
            };
            let rules = self.identifier_rules()?;
            let contents = self.store().rerun_contents(after, usize::MAX)?;
            let total = contents.len() as u64;
            let progress = self.rerun_progress();
            *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some((0, total));
            let mut cursor = after;
            for (i, content) in contents.into_iter().enumerate() {
                if self.control().stopping() {
                    summary.interrupted = true;
                    return Ok(());
                }
                let Some(out) =
                    self.store()
                        .rerun_identifiers(cursor, content, finder(rules.clone()))?
                else {
                    // Started again with other rules (a second region change).
                    continue 'restart;
                };
                cursor = content;
                summary.contents += 1;
                summary.mentions += out.mentions;
                summary.card_flags += out.card_flags;
                if out.connections_changed {
                    self.changed();
                }
                *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some((i as u64 + 1, total));
            }
            if self
                .store()
                .finish_identifier_rerun(Some(cursor), RULES_VERSION)?
            {
                return Ok(());
            }
        }
    }
}

/// The finder for one content's stored segments, in document and segment order: a CSV
/// document's column names come back from its row 0 by the parser's rule, as the worker had
/// them.
fn finder(rules: Rules) -> impl FnMut(&StoredSegment) -> SegmentFinds + Send + 'static {
    let mut document = None;
    let mut columns: Vec<Option<String>> = Vec::new();
    move |stored| {
        if document != Some(stored.document_id) {
            document = Some(stored.document_id);
            columns = match stored.locator {
                Locator::CsvRow { row: 0 } => mk_parsers::csv_columns(&cells(stored)),
                _ => Vec::new(),
            };
        }
        let seg = ParsedSegment {
            locator: stored.locator.clone(),
            text: stored.text.clone(),
            row_state: if stored.malformed {
                RowState::Malformed
            } else {
                RowState::Normal
            },
            cell_starts: stored.cell_starts.clone(),
            ocr: stored.ocr.clone(),
        };
        let (mentions, flags) = mk_worker::extract::segment_identifiers(&seg, 0, &columns, &rules);
        SegmentFinds {
            mentions: mentions
                .into_iter()
                .map(|m| FoundMention {
                    locator: m.locator,
                    raw: m.raw,
                    key: m.key,
                    confidence: m.confidence,
                })
                .collect(),
            card_flags: flags
                .into_iter()
                .map(|f| FoundFlag {
                    locator: f.locator,
                    last4: f.last4,
                })
                .collect(),
        }
    }
}

/// A stored CSV row's cell values: cells are joined by one `\t` (see [`ParsedSegment`]).
fn cells(seg: &StoredSegment) -> Vec<String> {
    let chars: Vec<char> = seg.text.chars().collect();
    let starts = &seg.cell_starts;
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let end = starts.get(i + 1).map_or(chars.len(), |&n| n as usize - 1);
            chars[(s as usize).min(end)..end].iter().collect()
        })
        .collect()
}
