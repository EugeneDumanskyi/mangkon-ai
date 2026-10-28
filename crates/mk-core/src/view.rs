//! Reads the desktop and the CLI share (M2d): the connections list, the occurrences of one
//! connection, and the source text around one occurrence with the hit already split out
//! (US-07.1, US-07.3 AC2, US-08.1).
//!
//! Spans count Unicode characters (INV-CO-04). They are applied here, on the Rust side, so the
//! UI never has to map them onto its own string units.

use mk_corpus::IngestionState;
use mk_graph::{ConnectionRow, RankKey};
use mk_kernel::{Column, Locator, Span};
use mk_store::{FailedFile, OccurrenceRow, WindowSegment};

use crate::project::{Project, ProjectError};

/// Segments shown before and after the hit in the source view.
pub const CONTEXT_SEGMENTS: usize = 20;

/// The most occurrences [`Project::occurrences`] returns for one connection.
pub const MAX_OCCURRENCES: usize = 1_000;

/// Content counts for the progress panel (US-03.1 AC1): files, not work items.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct IngestCounts {
    /// Every file still in the project: everything below plus not-processed ones.
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    /// Waiting for Stage A.
    pub queued: u64,
    /// In a worker now.
    pub processing: u64,
}

/// One page of the connections list and the cursor of the page after it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectionsPage {
    pub rows: Vec<ConnectionRow>,
    /// `None` on the last page.
    pub next: Option<RankKey>,
}

/// A text with one marked part (the hit). `hit` is empty for a text without one, and then the
/// whole text is in `before`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Marked {
    pub before: String,
    pub hit: String,
    pub after: String,
}

impl Marked {
    pub fn plain(text: impl Into<String>) -> Self {
        Marked {
            before: text.into(),
            ..Marked::default()
        }
    }

    /// Splits `text` at a character span (INV-CO-04). A span past the end is clamped.
    pub fn split(text: &str, start: usize, end: usize) -> Self {
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
        let (s, e) = (byte(start), byte(end.max(start)));
        Marked {
            before: text[..s].to_string(),
            hit: text[s..e].to_string(),
            after: text[e..].to_string(),
        }
    }

    pub fn has_hit(&self) -> bool {
        !self.hit.is_empty()
    }
}

/// One line of a text file in the source view.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContextLine {
    /// 1-based line number in the file.
    pub number: u32,
    pub text: Marked,
}

/// One CSV row in the source view.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContextRow {
    /// 0-based record number; a header is row 0 (US-02.4).
    pub row: u64,
    pub cells: Vec<Marked>,
    /// The row has the wrong number of cells (INV-CO-10).
    pub malformed: bool,
}

/// The source around one occurrence (US-08.1 AC3: text and CSV; pages come in M3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceBody {
    /// Lines around the hit. Only non-empty lines are stored, so numbers may skip.
    Text { lines: Vec<ContextLine> },
    /// Rows around the hit, with the header row when the file has one.
    Table {
        header: Option<Vec<String>>,
        rows: Vec<ContextRow>,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub struct SourceContext {
    pub occurrence: OccurrenceRow,
    pub body: SourceBody,
}

/// Cell `i` of a CSV row: its character range in the row's stored text (`\t`-joined).
fn cell_range(text_len: usize, starts: &[u32], i: usize) -> (usize, usize) {
    let start = starts[i] as usize;
    let end = starts
        .get(i + 1)
        .map_or(text_len, |next| (*next as usize).saturating_sub(1));
    (start, end.max(start))
}

/// The cells of a CSV row, the one holding `hit` (a span in the row's text) marked.
fn cells(seg: &WindowSegment, hit: Option<Span>) -> Vec<Marked> {
    let chars: Vec<char> = seg.text.chars().collect();
    let starts: &[u32] = if seg.cell_starts.is_empty() {
        &[0]
    } else {
        &seg.cell_starts
    };
    (0..starts.len())
        .map(|i| {
            let (s, e) = cell_range(chars.len(), starts, i);
            let cell: String = chars[s.min(chars.len())..e.min(chars.len())]
                .iter()
                .collect();
            match hit {
                Some(h) if h.start() >= s && h.start() < e.max(s + 1) => {
                    Marked::split(&cell, h.start() - s, h.end().min(e) - s)
                }
                _ => Marked::plain(cell),
            }
        })
        .collect()
}

impl Project {
    /// Content counts for the progress panel (US-03.1 AC1).
    pub fn counts(&self) -> Result<IngestCounts, ProjectError> {
        let p = self.store().progress()?;
        let n = |s| p.contents_in(s);
        let c = IngestCounts {
            done: n(IngestionState::Done),
            failed: n(IngestionState::Failed),
            queued: n(IngestionState::Queued),
            processing: n(IngestionState::Processing),
            total: 0,
        };
        Ok(IngestCounts {
            total: c.done + c.failed + c.queued + c.processing + n(IngestionState::NotProcessed),
            ..c
        })
    }

    /// Failed files with their plain reasons (NFR-USE-05), at most `limit`.
    pub fn failed_files(&self, limit: usize) -> Result<Vec<FailedFile>, ProjectError> {
        Ok(self.store().failed_files(limit)?)
    }

    /// Visible connections (not ignored, US-07.4), for the list length.
    pub fn connection_count(&self) -> Result<u64, ProjectError> {
        Ok(self.store().connection_count(false)?)
    }

    /// One page of visible connections in ranking order (domain §9.2), after `after`.
    pub fn connections_page(
        &self,
        after: Option<&RankKey>,
        limit: usize,
    ) -> Result<ConnectionsPage, ProjectError> {
        let limit = limit.max(1);
        let rows = self.store().connections_page(after, limit, false)?;
        let next = (rows.len() == limit)
            .then(|| rows.last().map(ConnectionRow::rank_key))
            .flatten();
        Ok(ConnectionsPage { rows, next })
    }

    /// The counted mentions of a connection's key, each with its locator and file (US-07.3
    /// AC2), at most [`MAX_OCCURRENCES`].
    pub fn occurrences(&self, key: &str) -> Result<Vec<OccurrenceRow>, ProjectError> {
        Ok(self.store().mentions_of(key, MAX_OCCURRENCES)?)
    }

    /// The source text around one mention, the hit split out (US-07.3 AC2, US-08.1 AC3).
    /// `None` if the mention is gone (a re-scan replaced its content).
    pub fn source_context(&self, mention: i64) -> Result<Option<SourceContext>, ProjectError> {
        let Some(occurrence) = self.store().mention(mention)? else {
            return Ok(None);
        };
        let Some(window) = self.store().segment_window(
            occurrence.segment_id,
            CONTEXT_SEGMENTS,
            CONTEXT_SEGMENTS,
        )?
        else {
            return Ok(None);
        };
        let hit_span = occurrence.locator.span();
        let is_hit = |s: &WindowSegment| s.id == occurrence.segment_id;
        let body = match &occurrence.locator {
            Locator::CsvCell { column, .. } => {
                let header = matches!(column, Column::Name(_))
                    .then(|| cells(&window.first, None))
                    .map(|cells| cells.into_iter().map(|c| c.before).collect());
                let rows = window
                    .segments
                    .iter()
                    .filter(|s| header.is_none() || s.id != window.first.id)
                    .map(|s| ContextRow {
                        row: match s.locator {
                            Locator::CsvRow { row } => row,
                            _ => s.ordinal as u64,
                        },
                        cells: cells(s, hit_span.filter(|_| is_hit(s))),
                        malformed: s.malformed,
                    })
                    .collect();
                SourceBody::Table { header, rows }
            }
            _ => SourceBody::Text {
                lines: window
                    .segments
                    .iter()
                    .map(|s| ContextLine {
                        number: match s.locator {
                            Locator::TextLine { line, .. } => line.get(),
                            _ => s.ordinal as u32 + 1,
                        },
                        text: match hit_span.filter(|_| is_hit(s)) {
                            Some(h) => Marked::split(&s.text, h.start(), h.end()),
                            None => Marked::plain(s.text.clone()),
                        },
                    })
                    .collect(),
            },
        };
        Ok(Some(SourceContext { occurrence, body }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INV-CO-04: spans count characters, not bytes.
    #[test]
    fn split_counts_characters() {
        let m = Marked::split("Zoë → a@x.example!", 6, 17);
        assert_eq!(
            (m.before.as_str(), m.hit.as_str(), m.after.as_str()),
            ("Zoë → ", "a@x.example", "!")
        );
        assert!(!Marked::plain("x").has_hit());
        let past = Marked::split("ab", 1, 9);
        assert_eq!((past.before.as_str(), past.hit.as_str()), ("a", "b"));
    }

    #[test]
    fn cells_split_on_starts_and_mark_the_hit_cell() {
        let seg = WindowSegment {
            id: 1,
            ordinal: 1,
            locator: Locator::CsvRow { row: 1 },
            text: "Zoë\tz@x.example\t3".into(),
            malformed: false,
            cell_starts: vec![0, 4, 16],
        };
        let hit = Span::new(4, 15, 17).unwrap();
        let got = cells(&seg, Some(hit));
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], Marked::plain("Zoë"));
        assert_eq!(got[1].hit, "z@x.example");
        assert_eq!(got[2], Marked::plain("3"));
    }
}
