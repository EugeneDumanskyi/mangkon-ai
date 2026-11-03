//! Read queries, run on the reader pool.

use std::collections::BTreeMap;

use mk_corpus::{FileMeta, IngestionState, StoredRef};
use mk_graph::model::{
    ContentFact, DecisionRecord, DerivedState, EntityMentionFact, EntityRow, Facts, IdentifierRow,
    MentionFact,
};
use mk_graph::{ConnectionRow, DecisionKind, DecisionStatus, RankKey, SubjectKind};
use mk_jobs::{ItemState, JobState, PendingItem};
use mk_kernel::{ContentHash, IdentifierKey, Locator, OcrGeometry};
use rusqlite::{Connection, OptionalExtension, params};

use crate::codec::MentionCols;
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
    pub document_id: i64,
    pub locator: Locator,
    pub text: String,
    pub malformed: bool,
    pub cell_starts: Vec<u32>,
}

/// A stored identifier mention with its locator rebuilt.
#[derive(Clone, PartialEq, Debug)]
pub struct MentionRow {
    pub document_id: i64,
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

/// The project's OCR languages, `+`-joined (US-02.2 AC4); `eng` until set (INV-WS-06).
pub(crate) fn ocr_languages(conn: &Connection) -> Result<String, StoreError> {
    Ok(conn
        .query_row(
            "SELECT value FROM project_meta WHERE key = 'ocr_languages'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_else(|| DEFAULT_OCR_LANGUAGES.to_string()))
}

/// The OCR languages of a project that never set them.
pub const DEFAULT_OCR_LANGUAGES: &str = "eng";

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

/// Pending items of Queued and Running jobs: a Paused or Cancelled job starts nothing
/// (INV-PR-06).
pub(crate) fn pending_items(conn: &Connection) -> Result<Vec<PendingWork>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT w.id, w.priority, COALESCE(c.size, 0), COALESCE(c.file_type, '')
         FROM work_item w
         JOIN job j ON j.id = w.job_id
         LEFT JOIN content c ON c.id = w.content_id
         WHERE w.state = ?1 AND j.state IN (?2, ?3) ORDER BY w.priority, w.id",
    )?;
    let args = params![
        ItemState::Pending.as_str(),
        JobState::Queued.as_str(),
        JobState::Running.as_str()
    ];
    let rows = stmt.query_map(args, |r| {
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

/// The column the connections list is ordered by (US-07.2). [`SortColumn::Rank`] is the
/// default ranking of domain §9.2, ascending only.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortColumn {
    #[default]
    Rank,
    Value,
    Kind,
    Sources,
    Mentions,
    FileTypes,
}

/// What the Connections screen asks for (US-07.1 AC3, US-07.2, US-07.4).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ConnectionQuery {
    /// Type names (`email`, `person`, ...); empty = all.
    pub kinds: Vec<String>,
    /// At least this many sources.
    pub min_sources: u64,
    /// Connections found in any of these file types; empty = all.
    pub file_types: Vec<String>,
    /// Case-insensitive "contains" on the shown value; blank = no filter.
    pub find: String,
    pub sort: SortColumn,
    pub descending: bool,
    pub include_ignored: bool,
}

/// A connection row with what the list shows of it (M4d).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ListedConnection {
    pub row: ConnectionRow,
    pub kind: String,
    pub value: String,
    /// Counted file types, sorted.
    pub file_types: Vec<String>,
}

/// Where a connections page continues: the last row loaded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ListCursor {
    pub rank: RankKey,
    pub kind: String,
    pub value: String,
}

impl std::ops::Deref for ListedConnection {
    type Target = ConnectionRow;
    fn deref(&self) -> &ConnectionRow {
        &self.row
    }
}

impl ListedConnection {
    pub fn cursor(&self) -> ListCursor {
        ListCursor {
            rank: self.row.rank_key(),
            kind: self.kind.clone(),
            value: self.value.clone(),
        }
    }
}

/// Escapes `%`, `_` and the escape character for `LIKE ... ESCAPE '\\'`.
fn like_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The WHERE conditions of a query and their parameters (the parameters are `?1..`).
fn connection_filter(query: &ConnectionQuery) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    let mut sql = String::from("(?1 OR ignored = 0) AND source_count >= ?2");
    let mut args: Vec<Value> = vec![
        Value::Integer(i64::from(query.include_ignored)),
        Value::Integer(i64::try_from(query.min_sources).unwrap_or(i64::MAX)),
    ];
    if !query.kinds.is_empty() {
        let marks: Vec<String> = query
            .kinds
            .iter()
            .map(|k| {
                args.push(Value::Text(k.clone()));
                format!("?{}", args.len())
            })
            .collect();
        sql.push_str(&format!(" AND kind IN ({})", marks.join(",")));
    }
    if !query.file_types.is_empty() {
        let likes: Vec<String> = query
            .file_types
            .iter()
            .map(|t| {
                args.push(Value::Text(format!("%,{},%", like_escape(t))));
                format!("file_types LIKE ?{} ESCAPE '\\'", args.len())
            })
            .collect();
        sql.push_str(&format!(" AND ({})", likes.join(" OR ")));
    }
    let find = query.find.trim();
    if !find.is_empty() {
        args.push(Value::Text(format!("%{}%", like_escape(find))));
        sql.push_str(&format!(" AND value LIKE ?{} ESCAPE '\\'", args.len()));
    }
    (sql, args)
}

/// One page of connections in the query's order, after `after` (keyset paging, ADR-0003). The
/// default order is domain §9.2; any other column ties on the subject key.
pub(crate) fn connections_page(
    conn: &Connection,
    query: &ConnectionQuery,
    after: Option<&ListCursor>,
    limit: usize,
) -> Result<Vec<ListedConnection>, StoreError> {
    use rusqlite::types::Value;
    let (mut sql, mut args) = connection_filter(query);
    let mut next = |v: Value| {
        args.push(v);
        format!("?{}", args.len())
    };
    let order = if query.sort == SortColumn::Rank {
        if let Some(c) = after {
            let k = &c.rank;
            let ps = [
                next(Value::Integer(i64::from(k.low_confidence))),
                next(Value::Integer(k.source_count as i64)),
                next(Value::Integer(k.file_type_count as i64)),
                next(Value::Integer(k.mention_count as i64)),
                next(Value::Text(k.subject_key.clone())),
            ];
            sql.push_str(&format!(
                " AND (low_confidence, -source_count, -file_type_count, -mention_count, subject_key)
                   > ({}, -{}, -{}, -{}, {})",
                ps[0], ps[1], ps[2], ps[3], ps[4]
            ));
        }
        "low_confidence, source_count DESC, file_type_count DESC, mention_count DESC, subject_key"
            .to_string()
    } else {
        let col = match query.sort {
            SortColumn::Value => "value COLLATE NOCASE",
            SortColumn::Kind => "kind",
            SortColumn::Sources => "source_count",
            SortColumn::Mentions => "mention_count",
            _ => "file_type_count",
        };
        if let Some(c) = after {
            let v = match query.sort {
                SortColumn::Value => Value::Text(c.value.clone()),
                SortColumn::Kind => Value::Text(c.kind.clone()),
                SortColumn::Sources => Value::Integer(c.rank.source_count as i64),
                SortColumn::Mentions => Value::Integer(c.rank.mention_count as i64),
                _ => Value::Integer(c.rank.file_type_count as i64),
            };
            let (v1, v2) = (next(v.clone()), next(v));
            let k = next(Value::Text(c.rank.subject_key.clone()));
            let cmp = if query.descending { "<" } else { ">" };
            sql.push_str(&format!(
                " AND ({col} {cmp} {v1} OR ({col} = {v2} AND subject_key > {k}))"
            ));
        }
        let dir = if query.descending { "DESC" } else { "ASC" };
        format!("{col} {dir}, subject_key")
    };
    let limit_param = next(Value::Integer(limit as i64));
    let sql = format!(
        "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                ignored, subject_kind, kind, value, file_types
         FROM connection WHERE {sql} ORDER BY {order} LIMIT {limit_param}"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
        Ok(ListedConnection {
            row: row_to_connection(r)?,
            kind: r.get(7)?,
            value: r.get(8)?,
            file_types: r
                .get::<_, String>(9)?
                .split(',')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect(),
        })
    })?;
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
        "SELECT s.loc_kind, s.loc_pos, s.text, s.malformed, s.cell_starts, s.loc_ref,
                s.document_id, s.ocr
         FROM segment s JOIN document d ON d.id = s.document_id
         WHERE d.content_id = ?1 ORDER BY d.id, s.ordinal",
    )?;
    let mut rows = stmt.query([content])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let kind: String = r.get(0)?;
        let text: String = r.get(2)?;
        let ocr: Option<Vec<u8>> = r.get(7)?;
        out.push(SegmentRow {
            document_id: r.get(6)?,
            locator: codec::segment_from(&kind, r.get(1)?, r.get(5)?, &text, ocr.as_deref())?,
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
                m.column_index, m.column_name, m.confidence, s.loc_ref, m.document_id, s.ocr
         FROM identifier_mention m JOIN segment s ON s.id = m.segment_id
         WHERE m.content_id = ?1 ORDER BY m.id",
    )?;
    let mut rows = stmt.query([content])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let key: String = r.get(0)?;
        let text: String = r.get(4)?;
        let ocr: Option<Vec<u8>> = r.get(12)?;
        out.push(MentionRow {
            document_id: r.get(11)?,
            key: key
                .parse()
                .map_err(|e: mk_kernel::ParseKeyError| StoreError::Invalid(e.to_string()))?,
            raw: r.get(1)?,
            locator: codec::mention_from(
                &r.get::<_, String>(2)?,
                r.get(3)?,
                r.get(10)?,
                &text,
                ocr.as_deref(),
                MentionCols {
                    start: r.get(5)?,
                    end: r.get(6)?,
                    column_index: r.get(7)?,
                    column_name: r.get(8)?,
                },
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
            "SELECT name_key, entity_type, display_name, mention_count, from_table FROM entity",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                EntityRow {
                    entity_type: r.get(1)?,
                    display_name: r.get(2)?,
                    mention_count: r.get::<_, i64>(3)? as u64,
                    from_table: r.get(4)?,
                },
            ))
        })?;
        out.entities = rows.collect::<Result<_, _>>()?;
    }
    {
        let mut stmt = conn.prepare(
            "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                    ignored, subject_kind
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
        let mut stmt = conn.prepare(
            "SELECT content_id, document_id, name_key, raw, confidence, producer
             FROM entity_mention",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(2)?;
            out.entity_mentions.push(EntityMentionFact {
                content: r.get(0)?,
                document: r.get(1)?,
                key: key.parse().map_err(|e: mk_kernel::ParseNameKeyError| {
                    StoreError::Invalid(e.to_string())
                })?,
                raw: r.get(3)?,
                confidence: r.get::<_, f64>(4)? as f32,
                from_table: r.get::<_, String>(5)? == "table",
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
    out.stop_list = crate::derive::stop_list(conn)?;
    Ok(out)
}

/// Number of connection rows that match `query`, for the list length (US-07.1).
pub(crate) fn connection_count(
    conn: &Connection,
    query: &ConnectionQuery,
) -> Result<u64, StoreError> {
    let (filter, args) = connection_filter(query);
    let n: i64 = conn
        .prepare_cached(&format!("SELECT COUNT(*) FROM connection WHERE {filter}"))?
        .query_row(rusqlite::params_from_iter(args), |r| r.get(0))?;
    Ok(n as u64)
}

/// Connections that break NFR-QA-10: fewer than two counted documents behind them (their
/// mentions all have a segment, hence a locator). Keys only, at most `limit`.
pub(crate) fn weak_connections(conn: &Connection, limit: usize) -> Result<Vec<String>, StoreError> {
    let missing = IngestionState::Missing.as_str();
    let mut stmt = conn.prepare(
        "SELECT subject_key FROM connection k WHERE
           CASE k.subject_kind WHEN 'entity' THEN
             (SELECT COUNT(DISTINCT m.document_id) FROM entity_mention m
                JOIN content c ON c.id = m.content_id
                WHERE m.name_key = k.subject_key AND c.state != ?1 AND m.confidence >= 0.5)
           ELSE
             (SELECT COUNT(DISTINCT m.document_id) FROM identifier_mention m
                JOIN content c ON c.id = m.content_id
                WHERE m.identifier_key = k.subject_key AND c.state != ?1)
           END < 2
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![missing, limit as i64], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Number of ignored connection rows (the "Show ignored" label, US-07.4).
pub(crate) fn ignored_count(conn: &Connection) -> Result<u64, StoreError> {
    let n: i64 = conn
        .prepare_cached("SELECT COUNT(*) FROM connection WHERE ignored = 1")?
        .query_row([], |r| r.get(0))?;
    Ok(n as u64)
}

/// One mention of an identifier or an entity, with what the occurrence list and the source view
/// need (US-07.3 AC2): where it is (INV-CO-03) and the file it came from. An entity mention's
/// id is the negative of its row id, so both kinds share one id space (M4c).
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
    /// The text around the value with the value marked, card-like numbers masked (US-07.3 AC1,
    /// NFR-SEC-14).
    pub snippet: Vec<mk_search::Run>,
    /// The identifier key, such as `email:a@x.example`, or the entity's name key.
    pub key: String,
}

const OCCURRENCE_SELECT: &str = "
    SELECT m.id, m.content_id, m.document_id, m.segment_id, c.file_type, c.state,
           s.loc_kind, s.loc_pos, s.text, m.span_start, m.span_end, m.column_index,
           m.column_name, m.raw, m.confidence, r.path, f.rel_path, s.loc_ref, s.ocr,
           m.identifier_key
    FROM identifier_mention m
    JOIN segment s ON s.id = m.segment_id
    JOIN content c ON c.id = m.content_id
    LEFT JOIN file_ref f ON f.id = (
        SELECT id FROM file_ref WHERE content_id = m.content_id AND present = 1
        ORDER BY id LIMIT 1)
    LEFT JOIN source_root r ON r.id = f.root_id";

/// [`OCCURRENCE_SELECT`] for entity mentions: the same columns, the id negated.
const ENTITY_OCCURRENCE_SELECT: &str = "
    SELECT -m.id, m.content_id, m.document_id, m.segment_id, c.file_type, c.state,
           s.loc_kind, s.loc_pos, s.text, m.span_start, m.span_end, m.column_index,
           m.column_name, m.raw, m.confidence, r.path, f.rel_path, s.loc_ref, s.ocr,
           m.name_key
    FROM entity_mention m
    JOIN segment s ON s.id = m.segment_id
    JOIN content c ON c.id = m.content_id
    LEFT JOIN file_ref f ON f.id = (
        SELECT id FROM file_ref WHERE content_id = m.content_id AND present = 1
        ORDER BY id LIMIT 1)
    LEFT JOIN source_root r ON r.id = f.root_id";

/// A display path from a source root and a relative path, when the content has a present path.
pub(crate) fn display_path(root: Option<String>, rel: Option<String>) -> Option<String> {
    let (root, rel) = (root?, rel?);
    Some(
        mk_corpus::fs::resolve(root.as_ref(), &rel)
            .to_string_lossy()
            .into_owned(),
    )
}

/// The snippet of a mention: its segment's text, card-like numbers masked, the mention marked.
/// Masking keeps character positions, so the span still fits.
fn occurrence_snippet(text: &str, locator: &Locator) -> Vec<mk_search::Run> {
    let shown = mk_search::mask_card_like(text);
    let hits: Vec<(usize, usize)> = locator
        .span()
        .map(|s| (s.start(), s.end()))
        .into_iter()
        .collect();
    mk_search::snippet(&shown, &hits, 120)
}

/// An occurrence row and whether its content counts for connections (INV-CO-07).
fn occurrence_from(r: &rusqlite::Row<'_>) -> Result<(OccurrenceRow, bool), StoreError> {
    let state: String = r.get(5)?;
    let counted = IngestionState::from_name(&state)
        .ok_or_else(|| StoreError::Invalid(format!("unknown content state {state}")))?
        .counts_for_connections();
    let text: String = r.get(8)?;
    let ocr: Option<Vec<u8>> = r.get(18)?;
    let path = display_path(r.get(15)?, r.get(16)?);
    let locator = codec::mention_from(
        &r.get::<_, String>(6)?,
        r.get(7)?,
        r.get(17)?,
        &text,
        ocr.as_deref(),
        MentionCols {
            start: r.get(9)?,
            end: r.get(10)?,
            column_index: r.get(11)?,
            column_name: r.get(12)?,
        },
    )?;
    let snippet = occurrence_snippet(&text, &locator);
    Ok((
        OccurrenceRow {
            mention_id: r.get(0)?,
            content_id: r.get(1)?,
            document_id: r.get(2)?,
            segment_id: r.get(3)?,
            file_type: r.get(4)?,
            path,
            locator,
            raw: r.get(13)?,
            confidence: r.get::<_, f64>(14)? as f32,
            snippet,
            key: r.get(19)?,
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
    let sql = match SubjectKind::of_key(key) {
        SubjectKind::Identifier => format!(
            "{OCCURRENCE_SELECT}
             WHERE m.identifier_key = ?1
             ORDER BY m.content_id, m.document_id, s.ordinal, m.span_start"
        ),
        SubjectKind::Entity => format!(
            "{ENTITY_OCCURRENCE_SELECT}
             WHERE m.name_key = ?1
             ORDER BY m.content_id, m.document_id, s.ordinal, m.span_start"
        ),
    };
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

/// The counted mentions of `key` in one document (INV-CO-07), in reading order: by segment,
/// then position in it (US-08.2 AC1). No cap: a document holds few mentions of one value.
pub(crate) fn mentions_in_document(
    conn: &Connection,
    key: &str,
    document: i64,
) -> Result<Vec<i64>, StoreError> {
    let sql = match SubjectKind::of_key(key) {
        SubjectKind::Identifier => {
            "SELECT m.id, c.state FROM identifier_mention m
             JOIN segment s ON s.id = m.segment_id
             JOIN content c ON c.id = m.content_id
             WHERE m.identifier_key = ?1 AND m.document_id = ?2
             ORDER BY s.ordinal, m.span_start, m.id"
        }
        SubjectKind::Entity => {
            "SELECT -m.id, c.state FROM entity_mention m
             JOIN segment s ON s.id = m.segment_id
             JOIN content c ON c.id = m.content_id
             WHERE m.name_key = ?1 AND m.document_id = ?2
             ORDER BY s.ordinal, m.span_start, m.id"
        }
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let mut rows = stmt.query(params![key, document])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let state: String = r.get(1)?;
        let counted = IngestionState::from_name(&state)
            .ok_or_else(|| StoreError::Invalid(format!("unknown content state {state}")))?
            .counts_for_connections();
        if counted {
            out.push(r.get(0)?);
        }
    }
    Ok(out)
}

/// One mention by id, counted or not: an entity mention for a negative id.
pub(crate) fn mention(conn: &Connection, id: i64) -> Result<Option<OccurrenceRow>, StoreError> {
    let sql = if id < 0 {
        format!("{ENTITY_OCCURRENCE_SELECT} WHERE m.id = ?1")
    } else {
        format!("{OCCURRENCE_SELECT} WHERE m.id = ?1")
    };
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query([id.checked_abs().unwrap_or(0)])?;
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
    /// OCR'd pages: the word geometry (US-02.2 AC3, US-08.3).
    pub ocr: Option<OcrGeometry>,
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

const SEGMENT_SELECT: &str = "SELECT id, ordinal, loc_kind, loc_pos, text, malformed, cell_starts,
    loc_ref, ocr FROM segment";

fn window_segment(r: &rusqlite::Row<'_>) -> Result<WindowSegment, StoreError> {
    let kind: String = r.get(2)?;
    let text: String = r.get(4)?;
    let blob: Option<Vec<u8>> = r.get(8)?;
    let ocr = blob
        .as_deref()
        .map(|b| codec::ocr_from_blob(b, &text))
        .transpose()?;
    Ok(WindowSegment {
        id: r.get(0)?,
        ordinal: r.get(1)?,
        locator: codec::segment_from(&kind, r.get(3)?, r.get(7)?, &text, blob.as_deref())?,
        malformed: r.get(5)?,
        cell_starts: codec::cells_from_blob(r.get(6)?),
        ocr,
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

/// A document: what it is, where it sits in its file, and its parent (an attachment's message,
/// INV-CO-05).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocumentInfo {
    pub id: i64,
    pub content_id: i64,
    pub hash: ContentHash,
    pub file_type: String,
    pub kind: String,
    pub position: String,
    pub parent: Option<i64>,
    pub segments: u64,
    /// The first present path of the content, if any.
    pub path: Option<String>,
    /// That path, or when none is present the path last seen (US-08.5): with the size and
    /// modified time it had when it was read. An attachment's is its email file's.
    pub known: Option<KnownFile>,
}

/// A path of a content as the store last saw it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KnownFile {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    /// Whether the last re-scan found it.
    pub present: bool,
}

pub(crate) fn document_info(
    conn: &Connection,
    document: i64,
) -> Result<Option<DocumentInfo>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.id, d.content_id, c.file_type, d.kind, d.position, d.parent_id,
                (SELECT COUNT(*) FROM segment WHERE document_id = d.id), r.path, f.rel_path,
                c.hash, f.size, f.mtime_ns, f.present
         FROM document d JOIN content c ON c.id = d.content_id
         LEFT JOIN file_ref f ON f.id = (
             SELECT id FROM file_ref WHERE content_id = d.content_id
             ORDER BY present DESC, CASE WHEN present = 1 THEN id END, last_seen_at DESC, id DESC
             LIMIT 1)
         LEFT JOIN source_root r ON r.id = f.root_id
         WHERE d.id = ?1",
    )?;
    let mut rows = stmt.query([document])?;
    let Some(r) = rows.next()? else {
        return Ok(None);
    };
    let path = display_path(r.get(7)?, r.get(8)?);
    let present = r.get::<_, Option<bool>>(12)?.unwrap_or(false);
    let known = match (&path, r.get::<_, Option<i64>>(10)?, r.get(11)?) {
        (Some(p), Some(size), Some(mtime_ns)) => Some(KnownFile {
            path: p.clone(),
            size: size as u64,
            mtime_ns,
            present,
        }),
        _ => None,
    };
    Ok(Some(DocumentInfo {
        id: r.get(0)?,
        content_id: r.get(1)?,
        hash: codec::hash_from(&r.get::<_, Vec<u8>>(9)?)?,
        file_type: r.get(2)?,
        kind: r.get(3)?,
        position: r.get(4)?,
        parent: r.get(5)?,
        segments: r.get::<_, i64>(6)? as u64,
        path: path.filter(|_| present),
        known,
    }))
}

/// The first `limit` segments of a document, in order (a whole email, US-08.1 AC5).
pub(crate) fn document_segments(
    conn: &Connection,
    document: i64,
    limit: usize,
) -> Result<Vec<WindowSegment>, StoreError> {
    segments_where(
        conn,
        "WHERE document_id = ?1 ORDER BY ordinal LIMIT ?2",
        params![document, limit as i64],
    )
}

/// The segment of page `page` (1-based) of a PDF or image document: its text layer or its OCR
/// text. `None` for a page with no text.
pub(crate) fn page_segment(
    conn: &Connection,
    document: i64,
    page: u32,
) -> Result<Option<WindowSegment>, StoreError> {
    let mut segs = segments_where(
        conn,
        "WHERE document_id = ?1 AND loc_kind IN ('pdf_text', 'ocr_text') AND loc_pos = ?2
         ORDER BY ordinal LIMIT 1",
        params![document, page],
    )?;
    Ok(segs.pop())
}

/// The last page of a PDF or image document that has text, or 0.
pub(crate) fn last_text_page(conn: &Connection, document: i64) -> Result<u32, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT COALESCE(MAX(loc_pos), 0) FROM segment
             WHERE document_id = ?1 AND loc_kind IN ('pdf_text', 'ocr_text')",
        )?
        .query_row([document], |r| r.get(0))?)
}

/// The child documents of `document` (its attachments), in order.
pub(crate) fn child_documents(
    conn: &Connection,
    document: i64,
) -> Result<Vec<(i64, String, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind, position FROM document WHERE parent_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([document], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A content whose ingestion failed, with its first path and the plain reason (NFR-USE-05).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FailedFile {
    pub content_id: i64,
    pub path: Option<String>,
    pub file_type: String,
    pub code: Option<String>,
    pub reason: Option<String>,
}

/// Failed contents in id order, at most `limit`.
pub(crate) fn failed_files(conn: &Connection, limit: usize) -> Result<Vec<FailedFile>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.file_type, c.failure_code, c.failure_reason, r.path, f.rel_path, c.id
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
                content_id: r.get(5)?,
                path: display_path(r.get(3)?, r.get(4)?),
                file_type: r.get(0)?,
                code: r.get(1)?,
                reason: r.get(2)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A card-like flag on a document (US-04.6 AC1): where the number is and its last four digits.
#[derive(Clone, PartialEq, Debug)]
pub struct CardFlagRow {
    pub segment_id: i64,
    pub locator: Locator,
    pub last4: String,
}

impl CardFlagRow {
    /// `**** **** **** 1234` (US-04.6 AC1, NFR-SEC-14).
    pub fn masked(&self) -> String {
        format!("**** **** **** {}", self.last4)
    }
}

/// The card-like flags of a document, in reading order.
pub(crate) fn card_flags(conn: &Connection, document: i64) -> Result<Vec<CardFlagRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT k.segment_id, s.loc_kind, s.loc_pos, s.loc_ref, s.text, s.ocr, k.span_start,
                k.span_end, k.column_index, k.column_name, k.last4
         FROM sensitive_flag k JOIN segment s ON s.id = k.segment_id
         WHERE k.document_id = ?1 ORDER BY s.ordinal, k.span_start",
    )?;
    let mut rows = stmt.query([document])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let text: String = r.get(4)?;
        let ocr: Option<Vec<u8>> = r.get(5)?;
        out.push(CardFlagRow {
            segment_id: r.get(0)?,
            locator: codec::mention_from(
                &r.get::<_, String>(1)?,
                r.get(2)?,
                r.get(3)?,
                &text,
                ocr.as_deref(),
                codec::MentionCols {
                    start: r.get(6)?,
                    end: r.get(7)?,
                    column_index: r.get(8)?,
                    column_name: r.get(9)?,
                },
            )?,
            last4: r.get(10)?,
        });
    }
    Ok(out)
}

/// Every card-like flag's last four digits with its content (tests and the bench).
pub(crate) fn all_card_flags(conn: &Connection) -> Result<Vec<(i64, i64, String)>, StoreError> {
    let mut stmt = conn.prepare("SELECT content_id, document_id, last4 FROM sensitive_flag")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Contents with stored documents and an id after `after`, at most `limit`, in id order: what
/// an identifier re-run walks (M4b).
pub(crate) fn rerun_contents(
    conn: &Connection,
    after: i64,
    limit: usize,
) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.id FROM content c
         WHERE c.id > ?1 AND EXISTS (SELECT 1 FROM document d WHERE d.content_id = c.id)
         ORDER BY c.id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![after, limit as i64], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Where the stored identifiers stand: the rules version they were made with (`None` before
/// M4b) and, while a re-run is unfinished, the last content it committed.
pub(crate) fn identifier_state(
    conn: &Connection,
) -> Result<(Option<u32>, Option<i64>), StoreError> {
    let version =
        crate::documents::meta(conn, crate::ops::IDENTIFIER_VERSION)?.and_then(|v| v.parse().ok());
    let after = crate::documents::meta(conn, crate::ops::RERUN_AFTER)?.and_then(|v| v.parse().ok());
    Ok((version, after))
}

/// Each custom pattern with its stored match count (US-04.7 AC4), in the order they were added.
/// Counts a key range of the `identifier_mention_key` index: `custom:<name>:` up to
/// `custom:<name>;` (`;` follows `:`).
pub(crate) fn pattern_counts(conn: &Connection) -> Result<Vec<(String, String, u64)>, StoreError> {
    let patterns = crate::documents::custom_patterns(conn)?;
    let mut stmt = conn.prepare_cached(
        "SELECT COUNT(*) FROM identifier_mention WHERE identifier_key >= ?1 AND identifier_key < ?2",
    )?;
    let mut out = Vec::with_capacity(patterns.len());
    for p in patterns {
        let n: i64 = stmt.query_row(
            params![format!("custom:{}:", p.name), format!("custom:{};", p.name)],
            |r| r.get(0),
        )?;
        out.push((p.name, p.expression, n as u64));
    }
    Ok(out)
}

/// The entity rows of `keys` that exist (connection labels: display name, type, table mark).
pub(crate) fn entities(
    conn: &Connection,
    keys: &[String],
) -> Result<BTreeMap<String, EntityRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT entity_type, display_name, mention_count, from_table FROM entity
         WHERE name_key = ?1",
    )?;
    let mut out = BTreeMap::new();
    for key in keys {
        let row = stmt
            .query_row([key], |r| {
                Ok(EntityRow {
                    entity_type: r.get(0)?,
                    display_name: r.get(1)?,
                    mention_count: r.get::<_, i64>(2)? as u64,
                    from_table: r.get(3)?,
                })
            })
            .optional()?;
        if let Some(row) = row {
            out.insert(key.clone(), row);
        }
    }
    Ok(out)
}

/// A record link (INV-CM-05): an entity and an identifier in one CSV row or JSON record.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct RecordLinkRow {
    pub document_id: i64,
    pub record: String,
    pub name_key: String,
    pub identifier_key: String,
}

/// A content's record links, in document, record, entity and identifier order.
pub(crate) fn record_links(
    conn: &Connection,
    content: i64,
) -> Result<Vec<RecordLinkRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT document_id, record, name_key, identifier_key FROM record_link
         WHERE content_id = ?1 ORDER BY document_id, record, name_key, identifier_key",
    )?;
    let rows = stmt.query_map([content], |r| {
        Ok(RecordLinkRow {
            document_id: r.get(0)?,
            record: r.get(1)?,
            name_key: r.get(2)?,
            identifier_key: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Where a re-run step stands: what the stored mentions were made with (the rules version or
/// the pattern set's hash; `None` if never stamped) and, while a re-run is unfinished, the last
/// content it committed.
pub(crate) fn rerun_state(
    conn: &Connection,
    step: crate::ops::RerunStep,
) -> Result<(Option<String>, Option<i64>), StoreError> {
    let stamp = crate::documents::meta(conn, step.stamp_key())?;
    let after = crate::documents::meta(conn, step.cursor_key())?.and_then(|v| v.parse().ok());
    Ok((stamp, after))
}
