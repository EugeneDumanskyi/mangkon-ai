//! M4b, M4c: finding identifiers again from stored segments. The built-in step runs again when
//! the phone region changes (US-04.2 AC2) or a project's mentions were made with older rules
//! ([`mk_identifiers::RULES_VERSION`]); the custom-pattern step runs again when the patterns
//! change (US-04.7 AC3), and only that step (INV-ID-08). No file is read again and nothing is
//! OCR'd again: the same [`mk_worker::extract::segment_builtin`] and
//! [`mk_worker::extract::segment_custom`] the workers run read each stored segment.
//!
//! A re-run goes content by content, each in one transaction with its derived rebuild
//! (ADR-0005), and the store keeps a cursor past the last one, so a kill or a pause loses at most
//! one content's work and the next run goes on from there (INV-PR-04/05). It is not a job in
//! `mk-jobs`: its work items are files to read, and this reads none.

use mk_identifiers::{RULES_VERSION, Region, Rules};
use mk_kernel::{ContentHash, Locator};
use mk_parsers::{ParsedSegment, RowState};
use mk_store::{FoundFlag, FoundMention, RerunStep, SegmentFinds, StoredSegment};
use mk_workspace::PatternSetting;

use crate::project::{Project, ProjectError};

/// What a re-run step did ([`Project::rerun_identifiers`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RerunSummary {
    /// Contents whose identifiers were found again.
    pub contents: usize,
    pub mentions: usize,
    pub card_flags: usize,
    /// Pause, cancel or close came first: the rest is done by the next run.
    pub interrupted: bool,
}

impl RerunSummary {
    pub(crate) fn add(&mut self, other: RerunSummary) {
        self.contents += other.contents;
        self.mentions += other.mentions;
        self.card_flags += other.card_flags;
        self.interrupted = other.interrupted;
    }
}

/// What the stored custom mentions follow: a hash of the pattern set, in order. A project that
/// never had one follows the empty set.
pub(crate) fn patterns_stamp(patterns: &[PatternSetting]) -> String {
    let mut text = String::new();
    for p in patterns {
        text.push_str(&p.name);
        text.push('\u{1f}');
        text.push_str(&p.expression);
        text.push('\n');
    }
    ContentHash::of(text.as_bytes()).to_hex()
}

impl Project {
    /// The finder rules of this project: its phone region (US-04.2) and its custom patterns
    /// (US-04.7).
    pub(crate) fn identifier_rules(&self) -> Result<Rules, ProjectError> {
        let settings = self.store().settings()?;
        Ok(Rules {
            phone_region: settings.phone_region.as_deref().and_then(Region::parse),
            custom: settings.compiled_patterns(),
        })
    }

    /// What the stored mentions of `step` should follow now.
    fn step_target(&self, step: RerunStep) -> Result<String, ProjectError> {
        Ok(match step {
            RerunStep::Builtin => RULES_VERSION.to_string(),
            RerunStep::Custom => patterns_stamp(&self.store().settings()?.custom_patterns),
        })
    }

    fn step_pending(&self, step: RerunStep) -> Result<bool, ProjectError> {
        let (stamp, after) = self.store().rerun_state(step)?;
        let stamp = match (step, stamp) {
            (RerunStep::Custom, None) => Some(patterns_stamp(&[])),
            (_, stamp) => stamp,
        };
        Ok(after.is_some() || stamp != Some(self.step_target(step)?))
    }

    /// Whether stored identifiers are to be found again: a re-run of either step was started
    /// and not finished, or the mentions were made with older rules or other patterns.
    pub fn identifiers_pending(&self) -> Result<bool, ProjectError> {
        Ok(self.step_pending(RerunStep::Builtin)? || self.step_pending(RerunStep::Custom)?)
    }

    /// A project with nothing read yet follows the current rules as it is: no re-run needed.
    pub(crate) fn stamp_identifier_rules_if_empty(&self) -> Result<(), ProjectError> {
        if !self.store().rerun_contents(0, 1)?.is_empty() {
            return Ok(());
        }
        for step in [RerunStep::Builtin, RerunStep::Custom] {
            let (_, after) = self.store().rerun_state(step)?;
            if after.is_none() && self.step_pending(step)? {
                self.store()
                    .finish_rerun(step, None, &self.step_target(step)?)?;
            }
        }
        Ok(())
    }

    /// Finds every content's identifiers again from its stored segments, for each step that
    /// [`Project::identifiers_pending`]: the built-in step, then the custom-pattern step.
    /// Returns both summed. Waits for a running ingest to end first. A paused queue does
    /// nothing.
    pub fn rerun_identifiers(&self) -> Result<RerunSummary, ProjectError> {
        let _run = self.begin_run();
        let (mut builtin, custom) = self.reruns_locked()?;
        builtin.add(custom);
        Ok(builtin)
    }

    /// The built-in step, then (unless it was interrupted) the custom-pattern step.
    pub(crate) fn reruns_locked(&self) -> Result<(RerunSummary, RerunSummary), ProjectError> {
        let builtin = self.rerun_step_locked(RerunStep::Builtin)?;
        let custom = if builtin.interrupted {
            RerunSummary::default()
        } else {
            self.rerun_step_locked(RerunStep::Custom)?
        };
        Ok((builtin, custom))
    }

    fn rerun_step_locked(&self, step: RerunStep) -> Result<RerunSummary, ProjectError> {
        let mut summary = RerunSummary::default();
        let result = self.rerun_loop(step, &mut summary);
        *self
            .rerun_progress()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        result.map(|()| summary)
    }

    fn rerun_loop(&self, step: RerunStep, summary: &mut RerunSummary) -> Result<(), ProjectError> {
        'restart: loop {
            if !self.step_pending(step)? || self.is_paused()? {
                return Ok(());
            }
            let after = match self.store().rerun_state(step)?.1 {
                Some(after) => after,
                None => {
                    self.store().begin_rerun(step)?;
                    0
                }
            };
            // One read of the settings, so the stamp is the one these rules make; a change
            // saved while this runs starts it again (the cursor check in the store).
            let settings = self.store().settings()?;
            let target = match step {
                RerunStep::Builtin => RULES_VERSION.to_string(),
                RerunStep::Custom => patterns_stamp(&settings.custom_patterns),
            };
            let rules = Rules {
                phone_region: settings.phone_region.as_deref().and_then(Region::parse),
                custom: settings.compiled_patterns(),
            };
            let contents = self.store().rerun_contents(after, usize::MAX)?;
            let total = contents.len() as u64;
            let progress = self.rerun_progress();
            *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some((step, 0, total));
            let mut cursor = after;
            for (i, content) in contents.into_iter().enumerate() {
                if self.control().stopping() {
                    summary.interrupted = true;
                    return Ok(());
                }
                let Some(out) =
                    self.store()
                        .rerun(step, cursor, content, finder(step, rules.clone()))?
                else {
                    // Started again with other rules (a second region or pattern change).
                    continue 'restart;
                };
                cursor = content;
                summary.contents += 1;
                summary.mentions += out.mentions;
                summary.card_flags += out.card_flags;
                if out.connections_changed {
                    self.changed();
                }
                *progress.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((step, i as u64 + 1, total));
            }
            if self.store().finish_rerun(step, Some(cursor), &target)? {
                return Ok(());
            }
        }
    }
}

/// The finder of `step` for one content's stored segments, in document and segment order: a
/// CSV document's column names come back from its row 0 by the parser's rule, as the worker had
/// them.
fn finder(
    step: RerunStep,
    rules: Rules,
) -> impl FnMut(&StoredSegment) -> SegmentFinds + Send + 'static {
    let mut document = None;
    let mut columns: Vec<Option<String>> = Vec::new();
    move |stored| {
        if document != Some(stored.document_id) {
            document = Some(stored.document_id);
            columns = match stored.locator {
                Locator::CsvRow { row: 0 } => mk_kernel::csv_columns(&cells(stored)),
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
        let (mentions, flags) = match step {
            RerunStep::Builtin => mk_worker::extract::segment_builtin(&seg, 0, &columns, &rules),
            RerunStep::Custom => (
                mk_worker::extract::segment_custom(&seg, 0, &columns, &rules.custom),
                Vec::new(),
            ),
        };
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
