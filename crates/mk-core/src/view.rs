//! Reads the desktop and the CLI share (M2d, M3a): the connections list, the occurrences of one
//! connection, and the source around one occurrence with the hit already split out, or a whole
//! document without one (US-07.1, US-07.3 AC2, US-08.1 AC3–AC6).
//!
//! Every body is plain text in parts; nothing is HTML or markup to render (US-08.4, gap A8).
//!
//! Spans count Unicode characters (INV-CO-04). They are applied here, on the Rust side, so the
//! UI never has to map them onto its own string units.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mk_corpus::IngestionState;
use mk_graph::SubjectKind;
use mk_graph::model::EntityRow;
use mk_kernel::{Column, Locator, Span};
use mk_parsers::Format;
use mk_store::{
    ConnectionQuery, DocumentInfo, FailedFile, LinkedIdentifiers, ListCursor, ListedConnection,
    OccurrenceRow, WindowSegment,
};

use crate::project::{Project, ProjectError};
use crate::search::SegmentHit;

/// Segments shown before and after the hit in the source view.
pub const CONTEXT_SEGMENTS: usize = 20;

/// An email with at most this many segments is shown whole; a longer one as a window.
pub const WHOLE_EMAIL_SEGMENTS: usize = 2_000;

/// The segments a view shows, and the document's first segment (a CSV header row, if any).
type Shown = (Vec<WindowSegment>, Option<WindowSegment>);

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

/// How many linked identifiers an entity row carries; the rest are only counted (US-07.7 AC2).
pub const LINKED_SHOWN: usize = 3;

/// One page of the connections list and the cursor of the page after it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectionsPage {
    pub rows: Vec<ListedConnection>,
    /// The entity rows of this page's entity connections, by name key: the display name and
    /// the "Name (from table)" mark (US-05.3 AC2, INV-ER-07).
    pub entities: BTreeMap<String, EntityRow>,
    /// The identifiers linked to this page's entity connections, by name key (US-07.7 AC2, D7).
    pub linked: BTreeMap<String, LinkedIdentifiers>,
    /// Those of this page's entity connections with a pending proposal (US-07.7 AC4, D6).
    pub unreviewed: BTreeSet<String>,
    /// `None` on the last page.
    pub next: Option<ListCursor>,
}

/// What the empty states and the "Show ignored" label need (US-07.4, US-07.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConnectionsSummary {
    /// Connections not ignored.
    pub visible: u64,
    pub ignored: u64,
    /// Files whose text is read (done) and files in the project.
    pub documents_done: u64,
    pub documents_total: u64,
}

/// A text with one marked part (the hit). `hit` is empty for a text without one, and then the
/// whole text is in `before`. A text with links also comes as [`Run`]s (US-08.4 AC3), so the UI
/// never maps offsets itself; `runs` is empty when it has none.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Marked {
    pub before: String,
    pub hit: String,
    pub after: String,
    pub runs: Vec<Run>,
}

/// A part of a text: in the hit or not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Piece {
    pub text: String,
    pub hit: bool,
}

/// A text with links, in order: plain pieces and links. A link holds the hit's pieces when the
/// hit lies in it (a `mailto:` URL with the address found in it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Run {
    Text(Piece),
    Link { url: String, pieces: Vec<Piece> },
}

impl Marked {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let len = text.chars().count();
        Self::split(&text, len, len)
    }

    /// Splits `text` at a character span (INV-CO-04). A span past the end is clamped.
    pub fn split(text: &str, start: usize, end: usize) -> Self {
        let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
        let (s, e) = (byte(start), byte(end.max(start)));
        Marked {
            before: text[..s].to_string(),
            hit: text[s..e].to_string(),
            after: text[e..].to_string(),
            runs: runs(text, (s, e)),
        }
    }

    pub fn has_hit(&self) -> bool {
        !self.hit.is_empty()
    }
}

/// The pieces of `text[from..to]`, split where the hit (byte range) starts and ends.
fn pieces(text: &str, (from, to): (usize, usize), (hs, he): (usize, usize)) -> Vec<Piece> {
    let (a, b) = (hs.clamp(from, to), he.clamp(from, to));
    [(from, a, false), (a, b, true), (b, to, false)]
        .into_iter()
        .filter(|(x, y, _)| x < y)
        .map(|(x, y, hit)| Piece {
            text: text[x..y].to_string(),
            hit,
        })
        .collect()
}

/// `text` as runs around its links; empty when it has none.
fn runs(text: &str, hit: (usize, usize)) -> Vec<Run> {
    let urls = crate::links::find_urls(text);
    if urls.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = 0;
    for (s, e) in urls {
        out.extend(pieces(text, (at, s), hit).into_iter().map(Run::Text));
        out.push(Run::Link {
            url: text[s..e].to_string(),
            pieces: pieces(text, (s, e), hit),
        });
        at = e;
    }
    out.extend(
        pieces(text, (at, text.len()), hit)
            .into_iter()
            .map(Run::Text),
    );
    out
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

/// One row of the JSON outline: a key or index on the way to a value, or a value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JsonRow {
    /// 0 for the root's members.
    pub depth: u32,
    /// The key, or `[n]` for an array index.
    pub label: String,
    /// `None` for an object or array on the way.
    pub value: Option<Marked>,
}

/// One header of an email.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmailField {
    pub name: String,
    pub value: Marked,
}

/// One body part of an email, as lines.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmailPart {
    /// The IMAP part number (US-02.6 AC3).
    pub part: String,
    pub lines: Vec<Marked>,
}

/// A document to link to: an attachment, or the email an attachment came in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocumentLink {
    pub document_id: i64,
    /// What to call it: an email's subject, else its position (`part 2: notes.txt`).
    pub label: String,
}

/// How a page's text was read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageKind {
    /// From the PDF's text layer.
    Text,
    /// By OCR (US-02.2).
    Ocr,
}

/// The source around one occurrence, or the start of a document (US-08.1 AC1–AC6).
#[derive(Clone, PartialEq, Debug)]
pub enum SourceBody {
    /// TXT/MD lines around the hit. Only non-empty lines are stored, so numbers may skip.
    Text { lines: Vec<ContextLine> },
    /// CSV rows around the hit, with the header row when the file has one.
    Table {
        header: Option<Vec<String>>,
        rows: Vec<ContextRow>,
    },
    /// DOCX or HTML text blocks around the hit (extracted text, gap A8).
    Paragraphs { paragraphs: Vec<Marked> },
    /// JSON values around the hit as an outline, the path to each one shown.
    Json { rows: Vec<JsonRow> },
    /// An email: its headers, body parts and attachments.
    Email {
        headers: Vec<EmailField>,
        parts: Vec<EmailPart>,
        attachments: Vec<DocumentLink>,
    },
    /// One page of a PDF or image: its stored text as lines with the hit, shown beside the page
    /// image ([`Project::page_view`]) or instead of it (US-08.3, ADR-0011).
    Page {
        /// 1-based.
        page: u32,
        /// The last page with stored text; the page view gives the file's page count.
        pages: u32,
        /// `None` for a page with no text.
        kind: Option<PageKind>,
        lines: Vec<Marked>,
        /// OCR pages: the mean word confidence, 0 to 1 (US-02.2 AC3).
        ocr_quality: Option<f32>,
    },
}

/// Where the shown mention is among the mentions of its value in its document (US-08.2 AC1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    /// 1-based.
    pub index: u32,
    pub count: u32,
    /// The mentions before and after it, if any.
    pub prev: Option<i64>,
    pub next: Option<i64>,
}

/// The original file, as a stat sees it when the view opens (US-08.5). The view always comes
/// from the stored text (INV-CO-04); this only says whether the file is still there.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Original {
    Present {
        path: String,
    },
    /// Moved or deleted: its last known path.
    Missing {
        path: String,
    },
    /// There, but its size or modified time is not what was read (ASSUMPTION: enough to say
    /// "changed" without hashing a large file on every view; a page view also checks the hash).
    Changed {
        path: String,
    },
}

impl Original {
    /// The file a document came from, by stat. `None` when the store knows no path for it.
    pub fn of(document: &DocumentInfo) -> Option<Original> {
        let known = document.known.as_ref()?;
        let path = known.path.clone();
        if !known.present {
            return Some(Original::Missing { path });
        }
        Some(match std::fs::metadata(&path) {
            Ok(md) if md.is_file() => {
                let now = mk_corpus::fs::meta(&md);
                if now.size == known.size && now.mtime_ns == known.mtime_ns {
                    Original::Present { path }
                } else {
                    Original::Changed { path }
                }
            }
            _ => Original::Missing { path },
        })
    }

    pub fn path(&self) -> &str {
        match self {
            Original::Present { path }
            | Original::Missing { path }
            | Original::Changed { path } => path,
        }
    }
}

/// The banner text for a missing original (US-08.5 AC1).
pub fn not_found_note(path: &str) -> String {
    format!("Original file not found at {path}")
}

/// The note for an original that changed since it was read.
pub const CHANGED_NOTE: &str = "The file changed since it was read: showing the stored text";

#[derive(Clone, PartialEq, Debug)]
pub struct SourceContext {
    /// The mention shown, if the view was opened at one.
    pub occurrence: Option<OccurrenceRow>,
    /// Its place among its value's mentions in this document; `None` for a document view.
    pub position: Option<Position>,
    /// The place marked: the mention's or the search hit's span in its segment; `None` for a
    /// document view. A page view places its boxes from it.
    pub hit: Option<SegmentHit>,
    /// The original file's state; `None` when no path is known.
    pub original: Option<Original>,
    pub document: DocumentInfo,
    /// The email this document is an attachment of (US-08.1 AC4).
    pub parent: Option<DocumentLink>,
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
        self.connection_count_for(&ConnectionQuery::default())
    }

    /// Connections that match `query` (US-07.1 AC3, US-07.2).
    pub fn connection_count_for(&self, query: &ConnectionQuery) -> Result<u64, ProjectError> {
        Ok(self.store().connection_count(query)?)
    }

    /// Counts for the empty states and the "Show ignored" label (US-07.4, US-07.6).
    pub fn connections_summary(&self) -> Result<ConnectionsSummary, ProjectError> {
        let counts = self.counts()?;
        Ok(ConnectionsSummary {
            visible: self.store().connection_count(&ConnectionQuery::default())?,
            ignored: self.store().ignored_count()?,
            documents_done: counts.done,
            documents_total: counts.total,
        })
    }

    /// One page of connections that match `query`, in its order (default: domain §9.2), after
    /// `after`.
    pub fn connections_page(
        &self,
        query: &ConnectionQuery,
        after: Option<&ListCursor>,
        limit: usize,
    ) -> Result<ConnectionsPage, ProjectError> {
        let limit = limit.max(1);
        let rows = self.store().connections_page(query, after, limit)?;
        let next = (rows.len() == limit)
            .then(|| rows.last().map(ListedConnection::cursor))
            .flatten();
        let entity_keys: Vec<String> = rows
            .iter()
            .filter(|r| r.row.subject_kind == SubjectKind::Entity)
            .map(|r| r.row.subject_key.clone())
            .collect();
        let entities = self.store().entities(&entity_keys)?;
        let linked =
            self.store()
                .linked_identifiers(&entity_keys, LINKED_SHOWN, query.include_low)?;
        let unreviewed = self.store().unreviewed_keys(&entity_keys)?;
        Ok(ConnectionsPage {
            rows,
            entities,
            linked,
            unreviewed,
            next,
        })
    }

    /// The counted mentions of a connection's key, each with its locator and file (US-07.3
    /// AC2), at most [`MAX_OCCURRENCES`].
    pub fn occurrences(&self, key: &str) -> Result<Vec<OccurrenceRow>, ProjectError> {
        self.occurrences_with(key, false)
    }

    /// [`occurrences`](Self::occurrences), with a person's or organization's mentions under 0.5
    /// when "Show low confidence" is on (US-10.1 AC3).
    pub fn occurrences_with(
        &self,
        key: &str,
        include_low: bool,
    ) -> Result<Vec<OccurrenceRow>, ProjectError> {
        Ok(self
            .store()
            .mentions_of(key, MAX_OCCURRENCES, include_low)?)
    }

    /// The source around one mention, the hit split out (US-07.3 AC2, US-08.1 AC3–AC6).
    /// `None` if the mention is gone (a re-scan replaced its content).
    pub fn source_context(&self, mention: i64) -> Result<Option<SourceContext>, ProjectError> {
        let Some(occurrence) = self.store().mention(mention)? else {
            return Ok(None);
        };
        let Some(document) = self.store().document_info(occurrence.document_id)? else {
            return Ok(None);
        };
        let hit = occurrence
            .locator
            .span()
            .map(|span| (occurrence.segment_id, span));
        let Some(body) = self.body(&document, hit, Some(&occurrence.locator))? else {
            return Ok(None);
        };
        Ok(Some(SourceContext {
            parent: self.parent_link(&document)?,
            position: Some(self.position(&occurrence)?),
            hit: hit.map(|(segment_id, span)| SegmentHit { segment_id, span }),
            occurrence: Some(occurrence),
            original: Original::of(&document),
            document,
            body,
        }))
    }

    /// Where `occurrence` is among the counted mentions of its value in its document (US-08.2
    /// AC1). A mention that no longer counts is shown as the only one. A low-confidence one was
    /// opened with those on, so it moves among them too.
    fn position(&self, occurrence: &OccurrenceRow) -> Result<Position, ProjectError> {
        let ids = self.store().mentions_in_document(
            &occurrence.key,
            occurrence.document_id,
            occurrence.confidence < mk_graph::LOW_CONFIDENCE,
        )?;
        let Some(i) = ids.iter().position(|id| *id == occurrence.mention_id) else {
            return Ok(Position {
                index: 1,
                count: 1,
                prev: None,
                next: None,
            });
        };
        Ok(Position {
            index: i as u32 + 1,
            count: ids.len() as u32,
            prev: i.checked_sub(1).map(|p| ids[p]),
            next: ids.get(i + 1).copied(),
        })
    }

    /// The original file of a document, to open or show in its folder (US-08.6): an
    /// attachment's is its email file. `None` if the document is gone or its file is not there.
    pub fn original_path(&self, document: i64) -> Result<Option<PathBuf>, ProjectError> {
        let Some(doc) = self.store().document_info(document)? else {
            return Ok(None);
        };
        Ok(match Original::of(&doc) {
            Some(Original::Present { path } | Original::Changed { path }) => Some(path.into()),
            _ => None,
        })
    }

    /// A document from its start, without a hit: an attachment, or the email it came in
    /// (US-08.1 AC4). `None` if it is gone.
    pub fn document_view(&self, document: i64) -> Result<Option<SourceContext>, ProjectError> {
        self.document_page(document, 1)
    }

    /// A document at page `page` (1-based) when it has pages, else from its start. `None` if
    /// it is gone.
    pub fn document_page(
        &self,
        document: i64,
        page: u32,
    ) -> Result<Option<SourceContext>, ProjectError> {
        let Some(document) = self.store().document_info(document)? else {
            return Ok(None);
        };
        let body = if Format::from_name(&document.kind).is_some_and(|f| f.is_paged()) {
            Some(self.page_body(&document, page.max(1), None)?)
        } else {
            self.body(&document, None, None)?
        };
        let Some(body) = body else {
            return Ok(None);
        };
        Ok(Some(SourceContext {
            occurrence: None,
            position: None,
            hit: None,
            original: Original::of(&document),
            parent: self.parent_link(&document)?,
            document,
            body,
        }))
    }

    pub(crate) fn parent_link(
        &self,
        document: &DocumentInfo,
    ) -> Result<Option<DocumentLink>, ProjectError> {
        let Some(parent) = document.parent else {
            return Ok(None);
        };
        self.store()
            .document_info(parent)?
            .map(|p| self.link(p.id, &p.position))
            .transpose()
    }

    /// A link to a document: an email by its subject, anything else by its position.
    fn link(&self, document_id: i64, position: &str) -> Result<DocumentLink, ProjectError> {
        let subject = self
            .store()
            .document_segments(document_id, 16)?
            .into_iter()
            .find_map(|s| match s.locator {
                Locator::EmailHeader { name, .. } if name.eq_ignore_ascii_case("Subject") => {
                    Some(s.text)
                }
                _ => None,
            });
        Ok(DocumentLink {
            document_id,
            label: subject.unwrap_or_else(|| position.to_string()),
        })
    }

    /// The segments to show: around the hit, or from the start; a whole email when it is
    /// small. `None` if the segment is gone.
    fn shown(
        &self,
        document: &DocumentInfo,
        hit: Option<(i64, Span)>,
        whole: bool,
    ) -> Result<Option<Shown>, ProjectError> {
        let store = self.store();
        if whole && document.segments <= WHOLE_EMAIL_SEGMENTS as u64 {
            return Ok(Some((
                store.document_segments(document.id, WHOLE_EMAIL_SEGMENTS)?,
                None,
            )));
        }
        match hit {
            Some((segment, _)) => Ok(store
                .segment_window(segment, CONTEXT_SEGMENTS, CONTEXT_SEGMENTS)?
                .map(|w| (w.segments, Some(w.first)))),
            None => {
                let segs = store.document_segments(document.id, 2 * CONTEXT_SEGMENTS + 1)?;
                let first = segs.first().cloned();
                Ok(Some((segs, first)))
            }
        }
    }

    pub(crate) fn body(
        &self,
        document: &DocumentInfo,
        hit: Option<(i64, Span)>,
        locator: Option<&Locator>,
    ) -> Result<Option<SourceBody>, ProjectError> {
        let format = Format::from_name(&document.kind);
        if format.is_some_and(|f| f.is_paged()) {
            let page = locator.and_then(Locator::page).map_or(1, |p| p.get());
            return Ok(Some(self.page_body(document, page, hit)?));
        }
        let email = matches!(format, Some(Format::Eml | Format::Mbox));
        let Some((segments, first)) = self.shown(document, hit, email)? else {
            return Ok(None);
        };
        let mark = |s: &WindowSegment| match hit {
            Some((id, span)) if id == s.id => Marked::split(&s.text, span.start(), span.end()),
            _ => Marked::plain(s.text.clone()),
        };
        let hit_in = |s: &WindowSegment| hit.filter(|(id, _)| *id == s.id).map(|(_, h)| h);
        Ok(Some(match format {
            Some(Format::Csv) => {
                let named = match locator {
                    Some(Locator::CsvCell { column, .. }) => matches!(column, Column::Name(_)),
                    // Without a hit, a first row that names its columns is shown as the header.
                    _ => false,
                };
                let header = first
                    .as_ref()
                    .filter(|_| named)
                    .map(|f| cells(f, None).into_iter().map(|c| c.before).collect());
                let first_id = first.as_ref().map(|f| f.id);
                let rows = segments
                    .iter()
                    .filter(|s| header.is_none() || Some(s.id) != first_id)
                    .map(|s| ContextRow {
                        row: match s.locator {
                            Locator::CsvRow { row } => row,
                            _ => s.ordinal as u64,
                        },
                        cells: cells(s, hit_in(s)),
                        malformed: s.malformed,
                    })
                    .collect();
                SourceBody::Table { header, rows }
            }
            Some(Format::Docx | Format::Html) => SourceBody::Paragraphs {
                paragraphs: segments.iter().map(mark).collect(),
            },
            Some(Format::Json) => SourceBody::Json {
                rows: json_rows(&segments, &mark),
            },
            Some(Format::Eml | Format::Mbox) => {
                let mut headers = Vec::new();
                let mut parts = Vec::new();
                for s in &segments {
                    match &s.locator {
                        Locator::EmailHeader { name, .. } => headers.push(EmailField {
                            name: name.clone(),
                            value: mark(s),
                        }),
                        Locator::MimePart { part, .. } => parts.push(EmailPart {
                            part: part.clone(),
                            lines: lines(&s.text, hit_in(s)),
                        }),
                        _ => {}
                    }
                }
                let attachments = self
                    .store()
                    .child_documents(document.id)?
                    .into_iter()
                    .map(|(id, _, position)| self.link(id, &position))
                    .collect::<Result<_, _>>()?;
                SourceBody::Email {
                    headers,
                    parts,
                    attachments,
                }
            }
            _ => SourceBody::Text {
                lines: segments
                    .iter()
                    .map(|s| ContextLine {
                        number: match s.locator {
                            Locator::TextLine { line, .. } => line.get(),
                            _ => s.ordinal as u32 + 1,
                        },
                        text: mark(s),
                    })
                    .collect(),
            },
        }))
    }
}

impl Project {
    /// The stored text of one page, the hit marked.
    fn page_body(
        &self,
        document: &DocumentInfo,
        page: u32,
        hit: Option<(i64, Span)>,
    ) -> Result<SourceBody, ProjectError> {
        let segment = self.store().page_segment(document.id, page)?;
        let pages = self.store().last_text_page(document.id)?.max(page);
        let Some(s) = segment else {
            return Ok(SourceBody::Page {
                page,
                pages,
                kind: None,
                lines: Vec::new(),
                ocr_quality: None,
            });
        };
        let span = hit.filter(|(id, _)| *id == s.id).map(|(_, h)| h);
        Ok(SourceBody::Page {
            page,
            pages,
            kind: Some(match s.locator {
                Locator::OcrText { .. } => PageKind::Ocr,
                _ => PageKind::Text,
            }),
            lines: lines(&s.text, span),
            ocr_quality: s.ocr.as_ref().and_then(|o| o.mean_confidence()),
        })
    }
}

/// A text as lines, the part of `hit` in each line marked.
fn lines(text: &str, hit: Option<Span>) -> Vec<Marked> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for line in text.split('\n') {
        let len = line.chars().count();
        let (s, e) = (at, at + len);
        out.push(match hit {
            Some(h) if h.start() < e.max(s + 1) && h.end() > s => {
                Marked::split(line, h.start().max(s) - s, h.end().min(e) - s)
            }
            _ => Marked::plain(line),
        });
        at = e + 1;
    }
    out
}

/// The JSON outline of some values: for each value, the keys on its path not already shown
/// for the value before it, then the value.
fn json_rows(segments: &[WindowSegment], mark: &dyn Fn(&WindowSegment) -> Marked) -> Vec<JsonRow> {
    let mut rows = Vec::new();
    let mut shown: Vec<String> = Vec::new();
    for s in segments {
        let Locator::JsonValue { pointer, .. } = &s.locator else {
            continue;
        };
        let tokens = pointer_tokens(pointer);
        let Some((last, path)) = tokens.split_last() else {
            rows.push(JsonRow {
                depth: 0,
                label: "$".into(),
                value: Some(mark(s)),
            });
            shown.clear();
            continue;
        };
        let same = shown.iter().zip(path).take_while(|(a, b)| a == b).count();
        for (depth, token) in path.iter().enumerate().skip(same) {
            rows.push(JsonRow {
                depth: depth as u32,
                label: label(token),
                value: None,
            });
        }
        rows.push(JsonRow {
            depth: path.len() as u32,
            label: label(last),
            value: Some(mark(s)),
        });
        shown = path.to_vec();
    }
    rows
}

/// The tokens of an RFC 6901 pointer, unescaped.
fn pointer_tokens(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// An array index as `[n]`, a key as itself.
fn label(token: &str) -> String {
    if !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()) {
        format!("[{token}]")
    } else {
        token.to_string()
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

    /// US-08.1 AC4: a body hit is marked in its line, across lines too.
    #[test]
    fn lines_mark_the_hit_in_its_line() {
        let got = lines(
            "Hi,\nmail a@x.example\nbye",
            Some(Span::new(9, 20, 25).unwrap()),
        );
        assert_eq!(got[0], Marked::plain("Hi,"));
        assert_eq!(got[1].hit, "a@x.example");
        assert_eq!(got[2], Marked::plain("bye"));
        let across = lines("ab\ncd", Some(Span::new(1, 4, 5).unwrap()));
        assert_eq!((across[0].hit.as_str(), across[1].hit.as_str()), ("b", "c"));
    }

    /// US-08.1 AC5: the path to each value is shown once, arrays as `[n]`.
    #[test]
    fn json_outline_shows_the_path() {
        let seg = |pointer: &str, text: &str| WindowSegment {
            id: 0,
            ordinal: 0,
            locator: Locator::JsonValue {
                pointer: pointer.into(),
                span: Span::new(0, text.chars().count(), text.chars().count()).unwrap(),
            },
            text: text.into(),
            malformed: false,
            cell_starts: Vec::new(),
            ocr: None,
        };
        let segs = [
            seg("/customers/12/name", "Anna"),
            seg("/customers/12/phone", "+44"),
            seg("/a~1b", "x"),
        ];
        let rows: Vec<(u32, String, Option<String>)> =
            json_rows(&segs, &|s| Marked::plain(s.text.clone()))
                .into_iter()
                .map(|r| (r.depth, r.label, r.value.map(|v| v.before)))
                .collect();
        assert_eq!(
            rows,
            vec![
                (0, "customers".into(), None),
                (1, "[12]".into(), None),
                (2, "name".into(), Some("Anna".into())),
                (2, "phone".into(), Some("+44".into())),
                (0, "a/b".into(), Some("x".into())),
            ]
        );
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
            ocr: None,
        };
        let hit = Span::new(4, 15, 17).unwrap();
        let got = cells(&seg, Some(hit));
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], Marked::plain("Zoë"));
        assert_eq!(got[1].hit, "z@x.example");
        assert_eq!(got[2], Marked::plain("3"));
    }
}
