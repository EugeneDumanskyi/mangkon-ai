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
