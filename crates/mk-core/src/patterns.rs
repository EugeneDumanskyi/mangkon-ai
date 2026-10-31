//! M4c: custom patterns (US-04.7) and column mapping (E05).
//!
//! A pattern change is saved with the start of its re-run in one transaction; the caller then
//! starts [`Project::ingest`], which runs only the custom-pattern step on the stored text
//! (AC3, INV-ID-08). A mapping confirm is the decision and its application in one transaction
//! (US-05.2 AC1).

use std::collections::BTreeSet;

use mk_identifiers::CustomPattern;
use mk_mapping::Assignments;
use mk_search::Run;
use mk_store::MappingView;
use mk_workspace::{PatternSetting, ProjectSettings, SettingsError};

use crate::project::{Project, ProjectError};

/// A pattern with more stored matches than this "matches a lot" (US-04.7 AC4, ASSUMPTION M4c).
pub const TOO_BROAD: u64 = 10_000;

/// The most characters of a sample [`Project::test_pattern`] reads.
pub const MAX_SAMPLE_CHARS: usize = 20_000;

/// A saved custom pattern with its stored matches (US-04.7 AC4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CustomPatternInfo {
    pub name: String,
    pub expression: String,
    pub match_count: u64,
    /// More than [`TOO_BROAD`] matches: "This pattern matches a lot. It may be too broad."
    pub too_broad: bool,
}

/// What a pattern finds in a sample text (US-04.7 AC1): the sample in runs, the matches
/// marked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatternTest {
    pub runs: Vec<Run>,
    pub matches: usize,
}

impl Project {
    /// The project's custom patterns in the order added, with their stored matches.
    pub fn custom_patterns(&self) -> Result<Vec<CustomPatternInfo>, ProjectError> {
        Ok(self
            .store()
            .pattern_counts()?
            .into_iter()
            .map(|(name, expression, match_count)| CustomPatternInfo {
                name,
                expression,
                match_count,
                too_broad: match_count > TOO_BROAD,
            })
            .collect())
    }

    /// Adds a pattern (US-04.7 AC1–AC3). Names are unique ignoring case (INV-ID-07); look-around
    /// and backreferences are refused with a plain reason (AC2).
    pub fn add_pattern(&self, name: &str, expression: &str) -> Result<(), ProjectError> {
        let mut patterns = self.settings()?.custom_patterns;
        patterns.push(PatternSetting::new(name, expression));
        self.save_patterns(patterns)
    }

    /// Changes a pattern's expression. Its name stays: renaming is remove and add.
    pub fn edit_pattern(&self, name: &str, expression: &str) -> Result<(), ProjectError> {
        let mut patterns = self.settings()?.custom_patterns;
        let pattern = patterns
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| ProjectError::Invalid(format!("No pattern named \"{name}\"")))?;
        pattern.expression = expression.to_string();
        self.save_patterns(patterns)
    }

    /// Removes a pattern; its matches go with the re-run. Ignore decisions on them stay in the
    /// log (INV-CU-04).
    pub fn remove_pattern(&self, name: &str) -> Result<(), ProjectError> {
        let mut patterns = self.settings()?.custom_patterns;
        let before = patterns.len();
        patterns.retain(|p| p.name != name);
        if patterns.len() == before {
            return Err(ProjectError::Invalid(format!(
                "No pattern named \"{name}\""
            )));
        }
        self.save_patterns(patterns)
    }

    /// Checks the pattern list and saves it with the start of the custom-pattern re-run. An
    /// unchanged list saves nothing.
    fn save_patterns(&self, patterns: Vec<PatternSetting>) -> Result<(), ProjectError> {
        let old = self.settings()?;
        let new = ProjectSettings {
            custom_patterns: patterns,
            ..old.clone()
        }
        .normalized()
        .map_err(|e| match e {
            SettingsError::BadPattern(..) | SettingsError::DuplicatePattern(_) => {
                ProjectError::Pattern(e.to_string())
            }
            e => ProjectError::Invalid(e.to_string()),
        })?;
        if new.custom_patterns == old.custom_patterns {
            return Ok(());
        }
        self.store().set_custom_patterns(
            new.custom_patterns
                .into_iter()
                .map(|p| (p.name, p.expression))
                .collect(),
        )?;
        Ok(())
    }

    /// Runs `expression` on a sample text for the live test box (US-04.7 AC1/AC2): the sample
    /// (its first [`MAX_SAMPLE_CHARS`] characters) with the matches marked, or the plain reason
    /// it cannot be used. Nothing is saved.
    pub fn test_pattern(
        &self,
        expression: &str,
        sample: &str,
    ) -> Result<PatternTest, ProjectError> {
        test_pattern(expression, sample)
    }

    /// The mapping screen of the file at `ref_id` (US-05.1 AC1); `None` if the file is gone or
    /// was not read as a table.
    pub fn mapping_view(&self, ref_id: i64) -> Result<Option<MappingView>, ProjectError> {
        Ok(self.store().mapping_view(ref_id)?)
    }

    /// Confirms the column types of the file at `ref_id` and applies them (US-05.2 AC1):
    /// confirmed person and organization columns give "Name (from table)" entities (US-05.3).
    /// Every key must be one of the file's columns. Returns the decision's `seq`.
    pub fn confirm_mapping(
        &self,
        ref_id: i64,
        assignments: Assignments,
    ) -> Result<i64, ProjectError> {
        let view = self
            .mapping_view(ref_id)?
            .ok_or_else(|| ProjectError::Invalid("This file has no columns to map".into()))?;
        let keys: BTreeSet<&str> = view.columns.iter().map(|c| c.key.as_str()).collect();
        if let Some(key) = assignments.0.keys().find(|k| !keys.contains(k.as_str())) {
            return Err(ProjectError::Invalid(format!(
                "No column \"{key}\" in this file"
            )));
        }
        let (seq, changed) = self.store().confirm_mapping(ref_id, assignments)?;
        if changed {
            self.changed();
        }
        Ok(seq)
    }

    /// Confirms the mapping of `from_ref_id`, a file with the same headers, for the file at
    /// `ref_id` (US-05.2 AC2: "Use the same mapping as <file>").
    pub fn reuse_mapping(&self, ref_id: i64, from_ref_id: i64) -> Result<i64, ProjectError> {
        let view = self
            .mapping_view(ref_id)?
            .ok_or_else(|| ProjectError::Invalid("This file has no columns to map".into()))?;
        let candidate = view
            .reuse
            .into_iter()
            .find(|c| c.ref_id == from_ref_id)
            .ok_or_else(|| {
                ProjectError::Invalid("That file has no confirmed mapping for these headers".into())
            })?;
        self.confirm_mapping(ref_id, candidate.assignments)
    }
}

/// See [`Project::test_pattern`].
pub fn test_pattern(expression: &str, sample: &str) -> Result<PatternTest, ProjectError> {
    let pattern = CustomPattern::compile("test", expression)
        .map_err(|e| ProjectError::Pattern(e.to_string()))?;
    let sample: String = sample.chars().take(MAX_SAMPLE_CHARS).collect();
    let hits = mk_identifiers::find_custom(&sample, std::slice::from_ref(&pattern));
    let chars: Vec<char> = sample.chars().collect();
    let mut runs = Vec::new();
    let mut at = 0;
    let push = |runs: &mut Vec<Run>, from: usize, to: usize, hit: bool| {
        if from < to {
            runs.push(Run {
                text: chars[from..to].iter().collect(),
                hit,
            });
        }
    };
    for h in &hits {
        let (start, end) = (h.span.start(), h.span.end());
        if start < at {
            continue;
        }
        push(&mut runs, at, start, false);
        push(&mut runs, start, end, true);
        at = end;
    }
    push(&mut runs, at, chars.len(), false);
    Ok(PatternTest {
        runs,
        matches: hits.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-04.7 AC1: matches are marked live in the sample, character spans applied here.
    #[test]
    fn us_04_7_ac1_test_marks_matches() {
        let t = test_pattern(
            r"VPA-TND-\d{4}-\d{3}",
            "Größe VPA-TND-2024-017, VPA-TND-2024-018",
        )
        .unwrap();
        assert_eq!(t.matches, 2);
        let marked: Vec<(&str, bool)> = t.runs.iter().map(|r| (r.text.as_str(), r.hit)).collect();
        assert_eq!(
            marked,
            vec![
                ("Größe ", false),
                ("VPA-TND-2024-017", true),
                (", ", false),
                ("VPA-TND-2024-018", true),
            ]
        );
    }

    /// US-04.7 AC2: look-around is refused with a plain reason, as a pattern error.
    #[test]
    fn us_04_7_ac2_test_refuses_look_around() {
        let e = test_pattern(r"(?<=ref )\d+", "ref 12").unwrap_err();
        assert!(matches!(e, ProjectError::Pattern(_)), "{e:?}");
        assert!(e.to_string().contains("not supported"), "{e}");
    }
}
