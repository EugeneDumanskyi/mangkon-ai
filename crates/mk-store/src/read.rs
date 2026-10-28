//! Read queries, run on the reader pool.

use std::collections::BTreeMap;

use mk_corpus::{FileMeta, IngestionState, StoredRef};
use mk_graph::model::{
    ContentFact, DecisionRecord, DerivedState, Facts, IdentifierRow, MentionFact,
};
use mk_graph::{ConnectionRow, DecisionKind, DecisionStatus, RankKey};
use mk_jobs::{ItemState, PendingItem};
use mk_kernel::{ContentHash, IdentifierKey, Locator};
use rusqlite::{Connection, OptionalExtension, params};

use crate::derive::row_to_connection;
use crate::{StoreError, codec};

/// A stored source root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RootRow {
    pub id: i64,
    pub path: String,
    pub kind: String,
}

/// Counts for the progress panel (US-03.1): contents and work items per state.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    pub contents: BTreeMap<String, u64>,
    pub items: BTreeMap<String, u64>,
    pub jobs: BTreeMap<String, u64>,
}

impl Progress {
    pub fn contents_in(&self, s: IngestionState) -> u64 {
        self.contents.get(s.as_str()).copied().unwrap_or(0)
    }

    pub fn items_in(&self, s: ItemState) -> u64 {
        self.items.get(s.as_str()).copied().unwrap_or(0)
    }
}

/// A stored content and how its ingestion went.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContentRow {
    pub id: i64,
    pub hash: ContentHash,
    pub file_type: String,
    pub state: IngestionState,
    pub failure_code: Option<String>,
    pub failure_reason: Option<String>,
    pub warnings: Vec<String>,
}

/// A stored segment with its locator rebuilt (INV-CO-03).
#[derive(Clone, PartialEq, Debug)]
pub struct SegmentRow {
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
}

/// A stored identifier mention with its locator rebuilt.
#[derive(Clone, PartialEq, Debug)]
pub struct MentionRow {
    pub key: IdentifierKey,
    pub raw: String,
    pub locator: Locator,
    pub confidence: f32,
    /// The text of its segment, so a caller can check the span (INV-CO-04).
    pub segment_text: String,
}

pub(crate) fn source_roots(conn: &Connection) -> Result<Vec<RootRow>, StoreError> {
    let mut stmt = conn.prepare("SELECT id, path, kind FROM source_root ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(RootRow {
            id: r.get(0)?,
            path: r.get(1)?,
            kind: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn stored_refs(conn: &Connection) -> Result<Vec<StoredRef>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT f.root_id, f.rel_path, f.size, f.mtime_ns, c.hash, f.present
         FROM file_ref f JOIN content c ON c.id = f.content_id",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(StoredRef {
            root: r.get(0)?,
            rel_path: r.get(1)?,
            meta: FileMeta {
                size: r.get::<_, i64>(2)? as u64,
                mtime_ns: r.get(3)?,
            },
            hash: codec::hash_from(&r.get::<_, Vec<u8>>(4)?)?,
            present: r.get(5)?,
        });
    }
    Ok(out)
}

/// A pending Stage A item with its content's file type, so the lane can estimate its memory
/// need (gap A2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PendingWork {
    pub item: PendingItem,
    pub file_type: String,
}

pub(crate) fn pending_items(conn: &Connection) -> Result<Vec<PendingWork>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT w.id, w.priority, COALESCE(c.size, 0), COALESCE(c.file_type, '')
         FROM work_item w LEFT JOIN content c ON c.id = w.content_id
         WHERE w.state = ?1 ORDER BY w.priority, w.id",
    )?;
    let rows = stmt.query_map([ItemState::Pending.as_str()], |r| {
        Ok(PendingWork {
            item: PendingItem {
                id: r.get(0)?,
                priority: r.get(1)?,
                text_native: true,
                size: r.get::<_, i64>(2)? as u64,
            },
            file_type: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One page of connections in ranking order (domain §9.2), after `after` (keyset paging).
/// Ignored rows only when `include_ignored` (US-07.4).
pub(crate) fn connections_page(
    conn: &Connection,
    after: Option<&RankKey>,
    limit: usize,
    include_ignored: bool,
) -> Result<Vec<ConnectionRow>, StoreError> {
    let sql = "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                      ignored
               FROM connection
               WHERE (?1 OR ignored = 0)
                 AND (?2 IS NULL OR
                      (low_confidence, -source_count, -file_type_count, -mention_count, subject_key)
                      > (?2, -?3, -?4, -?5, ?6))
               ORDER BY low_confidence, source_count DESC, file_type_count DESC,
                        mention_count DESC, subject_key
               LIMIT ?7";
    let mut stmt = conn.prepare_cached(sql)?;
    let a = after;
    let rows = stmt.query_map(
        params![
            include_ignored,
            a.map(|k| k.low_confidence),
            a.map(|k| k.source_count as i64),
            a.map(|k| k.file_type_count as i64),
            a.map(|k| k.mention_count as i64),
            a.map(|k| k.subject_key.as_str()),
            limit as i64
        ],
        row_to_connection,
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn counts(conn: &Connection, sql: &str) -> Result<BTreeMap<String, u64>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn progress(conn: &Connection) -> Result<Progress, StoreError> {
    Ok(Progress {
        contents: counts(conn, "SELECT state, COUNT(*) FROM content GROUP BY state")?,
        items: counts(conn, "SELECT state, COUNT(*) FROM work_item GROUP BY state")?,
        jobs: counts(conn, "SELECT state, COUNT(*) FROM job GROUP BY state")?,
    })
}

pub(crate) fn content(
    conn: &Connection,
    hash: &ContentHash,
) -> Result<Option<ContentRow>, StoreError> {
    let row = conn
        .query_row(
            "SELECT id, file_type, state, failure_code, failure_reason, warnings
             FROM content WHERE hash = ?1",
            [&hash.as_bytes()[..]],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(id, file_type, state, failure_code, failure_reason, warnings)| {
            Ok(ContentRow {
                id,
                hash: *hash,
                file_type,
                state: IngestionState::from_name(&state)
                    .ok_or_else(|| StoreError::Invalid(format!("unknown content state {state}")))?,
                failure_code,
                failure_reason,
                warnings: warnings
                    .split('\n')
                    .filter(|w| !w.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        },
    )
    .transpose()
}

pub(crate) fn segments(conn: &Connection, content: i64) -> Result<Vec<SegmentRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT s.loc_kind, s.loc_pos, s.text, s.malformed, s.cell_starts
         FROM segment s JOIN document d ON d.id = s.document_id
         WHERE d.content_id = ?1 ORDER BY d.id, s.ordinal",
    )?;
    let mut rows = stmt.query([content])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let kind: String = r.get(0)?;
        let text: String = r.get(2)?;
        out.push(SegmentRow {
            locator: codec::segment_from(&kind, r.get(1)?, &text)?,
            malformed: r.get(3)?,
            cell_starts: codec::cells_from_blob(r.get(4)?),
            text,
        });
    }
    Ok(out)
}

pub(crate) fn mentions(conn: &Connection, content: i64) -> Result<Vec<MentionRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT m.identifier_key, m.raw, s.loc_kind, s.loc_pos, s.text, m.span_start, m.span_end,
                m.column_index, m.column_name, m.confidence
         FROM identifier_mention m JOIN segment s ON s.id = m.segment_id
         WHERE m.content_id = ?1 ORDER BY m.id",
    )?;
    let mut rows = stmt.query([content])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let key: String = r.get(0)?;
        let text: String = r.get(4)?;
        out.push(MentionRow {
            key: key
                .parse()
                .map_err(|e: mk_kernel::ParseKeyError| StoreError::Invalid(e.to_string()))?,
            raw: r.get(1)?,
            locator: codec::mention_from(
                &r.get::<_, String>(2)?,
                r.get(3)?,
                text.chars().count(),
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            )?,
            confidence: r.get::<_, f64>(9)? as f32,
            segment_text: text,
        });
    }
    Ok(out)
}

/// Every derived row (for tests and the property check, ADR-0005 follow-up).
pub(crate) fn derived_state(conn: &Connection) -> Result<DerivedState, StoreError> {
    let mut out = DerivedState::default();
    {
        let mut stmt = conn.prepare("SELECT key, kind, value, mention_count FROM identifier")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                IdentifierRow {
                    kind: r.get(1)?,
                    value: r.get(2)?,
                    mention_count: r.get::<_, i64>(3)? as u64,
                },
            ))
        })?;
        out.identifiers = rows.collect::<Result<_, _>>()?;
    }
    {
        let mut stmt = conn.prepare(
            "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                    ignored
             FROM connection",
        )?;
        let rows = stmt.query_map([], row_to_connection)?;
        for row in rows {
            let row = row?;
            out.connections.insert(row.subject_key.clone(), row);
        }
    }
    {
        let mut stmt = conn.prepare("SELECT seq, status FROM decision_status")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let status: String = r.get(1)?;
            out.decision_status.insert(
                r.get(0)?,
                DecisionStatus::from_name(&status)
                    .ok_or_else(|| StoreError::Invalid(format!("unknown status {status}")))?,
            );
        }
    }
    Ok(out)
}

/// Machine output and decisions as `mk-graph` facts (for the reference model).
pub(crate) fn facts(conn: &Connection) -> Result<Facts, StoreError> {
    let mut out = Facts::default();
    {
        let mut stmt = conn.prepare("SELECT id, file_type, state FROM content")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let state: String = r.get(2)?;
            let state = IngestionState::from_name(&state)
                .ok_or_else(|| StoreError::Invalid(format!("unknown content state {state}")))?;
            out.contents.push(ContentFact {
                id: r.get(0)?,
                file_type: r.get(1)?,
                counted: state.counts_for_connections(),
            });
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT content_id, document_id, identifier_key, confidence FROM identifier_mention",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(2)?;
            out.mentions.push(MentionFact {
                content: r.get(0)?,
                document: r.get(1)?,
                key: key
                    .parse()
                    .map_err(|e: mk_kernel::ParseKeyError| StoreError::Invalid(e.to_string()))?,
                confidence: r.get::<_, f64>(3)? as f32,
            });
        }
    }
    {
        let mut stmt = conn.prepare("SELECT seq, kind, anchor FROM decision ORDER BY seq")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let kind: String = r.get(1)?;
            out.decisions.push(DecisionRecord {
                seq: r.get(0)?,
                kind: DecisionKind::from_name(&kind)
                    .ok_or_else(|| StoreError::Invalid(format!("unknown decision kind {kind}")))?,
                anchor: r.get(2)?,
            });
        }
    }
    Ok(out)
}

/// Number of connection rows, for the list length (US-07.1). Ignored rows only when
/// `include_ignored` (US-07.4).
pub(crate) fn connection_count(
    conn: &Connection,
    include_ignored: bool,
) -> Result<u64, StoreError> {
    let n: i64 = conn
        .prepare_cached("SELECT COUNT(*) FROM connection WHERE (?1 OR ignored = 0)")?
        .query_row([include_ignored], |r| r.get(0))?;
    Ok(n as u64)
}

/// One mention of an identifier, with what the occurrence list and the source view need
/// (US-07.3 AC2): where it is (INV-CO-03) and the file it came from.
#[derive(Clone, PartialEq, Debug)]
pub struct OccurrenceRow {
    pub mention_id: i64,
    pub content_id: i64,
    pub document_id: i64,
    pub segment_id: i64,
    pub file_type: String,
    /// The first present path of the content (source root joined with its relative path);
    /// `None` when no path is present.
    pub path: Option<String>,
    pub locator: Locator,
    pub raw: String,
    pub confidence: f32,
}

const OCCURRENCE_SELECT: &str = "
    SELECT m.id, m.content_id, m.document_id, m.segment_id, c.file_type, c.state,
           s.loc_kind, s.loc_pos, s.text, m.span_start, m.span_end, m.column_index,
           m.column_name, m.raw, m.confidence, r.path, f.rel_path
    FROM identifier_mention m
    JOIN segment s ON s.id = m.segment_id
    JOIN content c ON c.id = m.content_id
    LEFT JOIN file_ref f ON f.id = (
        SELECT id FROM file_ref WHERE content_id = m.content_id AND present = 1
        ORDER BY id LIMIT 1)
    LEFT JOIN source_root r ON r.id = f.root_id";

/// A display path from a source root and a relative path, when the content has a present path.
fn display_path(root: Option<String>, rel: Option<String>) -> Option<String> {
    let (root, rel) = (root?, rel?);
    Some(
        mk_corpus::fs::resolve(root.as_ref(), &rel)
            .to_string_lossy()
            .into_owned(),
    )
}

/// An occurrence row and whether its content counts for connections (INV-CO-07).
fn occurrence_from(r: &rusqlite::Row<'_>) -> Result<(OccurrenceRow, bool), StoreError> {
    let state: String = r.get(5)?;
    let counted = IngestionState::from_name(&state)
        .ok_or_else(|| StoreError::Invalid(format!("unknown content state {state}")))?
        .counts_for_connections();
    let text: String = r.get(8)?;
    let path = display_path(r.get(15)?, r.get(16)?);
    Ok((
        OccurrenceRow {
            mention_id: r.get(0)?,
            content_id: r.get(1)?,
            document_id: r.get(2)?,
            segment_id: r.get(3)?,
            file_type: r.get(4)?,
            path,
            locator: codec::mention_from(
                &r.get::<_, String>(6)?,
                r.get(7)?,
                text.chars().count(),
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
                r.get(12)?,
            )?,
            raw: r.get(13)?,
            confidence: r.get::<_, f64>(14)? as f32,
        },
        counted,
    ))
}

/// The counted mentions of `key` (INV-CO-07), by content, then document, then position in it.
/// At most `limit`. Reads the `identifier_mention_key` index.
pub(crate) fn mentions_of(
    conn: &Connection,
    key: &str,
    limit: usize,
) -> Result<Vec<OccurrenceRow>, StoreError> {
    let sql = format!(
        "{OCCURRENCE_SELECT}
         WHERE m.identifier_key = ?1
         ORDER BY m.content_id, m.document_id, s.ordinal, m.span_start"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query([key])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        if out.len() >= limit {
            break;
        }
        let (row, counted) = occurrence_from(r)?;
        if counted {
            out.push(row);
        }
    }
    Ok(out)
}

/// One mention by id, counted or not.
pub(crate) fn mention(conn: &Connection, id: i64) -> Result<Option<OccurrenceRow>, StoreError> {
    let sql = format!("{OCCURRENCE_SELECT} WHERE m.id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query([id])?;
    rows.next()?
        .map(|r| occurrence_from(r).map(|(row, _)| row))
        .transpose()
}

/// A stored segment with its id and position in its document.
#[derive(Clone, PartialEq, Debug)]
pub struct WindowSegment {
    pub id: i64,
    pub ordinal: i64,
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
}

/// A segment with up to `before` and `after` neighbours by ordinal in its document, and the
/// document's first segment (a CSV header row, if the file has one).
#[derive(Clone, PartialEq, Debug)]
pub struct SegmentWindow {
    pub document_id: i64,
    /// In ordinal order; holds the asked-for segment.
    pub segments: Vec<WindowSegment>,
    pub first: WindowSegment,
}

const SEGMENT_SELECT: &str =
    "SELECT id, ordinal, loc_kind, loc_pos, text, malformed, cell_starts FROM segment";

fn window_segment(r: &rusqlite::Row<'_>) -> Result<WindowSegment, StoreError> {
    let kind: String = r.get(2)?;
    let text: String = r.get(4)?;
    Ok(WindowSegment {
        id: r.get(0)?,
        ordinal: r.get(1)?,
        locator: codec::segment_from(&kind, r.get(3)?, &text)?,
        malformed: r.get(5)?,
        cell_starts: codec::cells_from_blob(r.get(6)?),
        text,
    })
}

fn segments_where(
    conn: &Connection,
    clause: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<WindowSegment>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!("{SEGMENT_SELECT} {clause}"))?;
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(window_segment(r)?);
    }
    Ok(out)
}

/// See [`SegmentWindow`]. Keyset reads on `segment_document`, so a 1M-row file costs the same as
/// a small one. `None` if the segment is gone.
pub(crate) fn segment_window(
    conn: &Connection,
    segment: i64,
    before: usize,
    after: usize,
) -> Result<Option<SegmentWindow>, StoreError> {
    let at: Option<(i64, i64)> = conn
        .prepare_cached("SELECT document_id, ordinal FROM segment WHERE id = ?1")?
        .query_row([segment], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((doc, ordinal)) = at else {
        return Ok(None);
    };
    let mut segments = segments_where(
        conn,
        "WHERE document_id = ?1 AND ordinal < ?2 ORDER BY ordinal DESC LIMIT ?3",
        params![doc, ordinal, before as i64],
    )?;
    segments.reverse();
    segments.extend(segments_where(
        conn,
        "WHERE document_id = ?1 AND ordinal >= ?2 ORDER BY ordinal LIMIT ?3",
        params![doc, ordinal, after as i64 + 1],
    )?);
    let first = segments_where(
        conn,
        "WHERE document_id = ?1 ORDER BY ordinal LIMIT 1",
        params![doc],
    )?
    .pop()
    .ok_or_else(|| StoreError::Invalid(format!("document {doc} has no segments")))?;
    Ok(Some(SegmentWindow {
        document_id: doc,
        segments,
        first,
    }))
}

/// A content whose ingestion failed, with its first path and the plain reason (NFR-USE-05).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FailedFile {
    pub path: Option<String>,
    pub file_type: String,
    pub code: Option<String>,
    pub reason: Option<String>,
}

/// Failed contents in id order, at most `limit`.
pub(crate) fn failed_files(conn: &Connection, limit: usize) -> Result<Vec<FailedFile>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.file_type, c.failure_code, c.failure_reason, r.path, f.rel_path
         FROM content c
         LEFT JOIN file_ref f ON f.id = (
             SELECT id FROM file_ref WHERE content_id = c.id AND present = 1 ORDER BY id LIMIT 1)
         LEFT JOIN source_root r ON r.id = f.root_id
         WHERE c.state = ?1 ORDER BY c.id LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        params![IngestionState::Failed.as_str(), limit as i64],
        |r| {
            Ok(FailedFile {
                path: display_path(r.get(3)?, r.get(4)?),
                file_type: r.get(0)?,
                code: r.get(1)?,
                reason: r.get(2)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}
