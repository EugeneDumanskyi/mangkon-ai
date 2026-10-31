//! M3c reads: the documents list (US-02.8), removal impact (US-01.5 AC2), progress detail
//! (US-03.1), settings (US-01.6) and exclusions.

use std::collections::{BTreeMap, BTreeSet};

use mk_corpus::IngestionState;
use mk_jobs::{ItemState, JobState};
use mk_mapping::MappingState;
use mk_workspace::{DEFAULT_OCR_LANGUAGE, PatternSetting, ProjectSettings};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;

/// What the documents list shows of a path (US-02.8 AC1): the content's state, or Missing when
/// this path is gone.
pub const STATUS_MISSING: &str = "missing";

/// Filters of the documents list (US-02.8 AC2). `None` matches all.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DocumentFilter {
    /// A content state name, or [`STATUS_MISSING`].
    pub status: Option<String>,
    /// A file type, such as `pdf`.
    pub file_type: Option<String>,
}

/// Where the documents list continues: the last row's root path and relative path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocumentCursor {
    pub root: String,
    pub rel_path: String,
}

/// One row of the documents list: one file reference (US-02.8 AC1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DocumentRow {
    pub ref_id: i64,
    pub content_id: i64,
    pub root_id: i64,
    pub root: String,
    pub rel_path: String,
    pub file_type: String,
    pub size: u64,
    /// Pages, rows or messages read, with the unit's name; `None` for other types or before
    /// the file is read.
    pub units: Option<(u64, &'static str)>,
    /// A content state name, or [`STATUS_MISSING`].
    pub status: String,
    pub warnings: Vec<String>,
    pub failure_code: Option<String>,
    pub failure_reason: Option<String>,
    pub failure_detail: Option<String>,
    /// Other paths with the same content (US-01.3 AC4).
    pub other_paths: u64,
    /// Card-like numbers in the content (US-04.6 AC1): "Contains card-like number".
    pub card_like_count: u64,
    /// A CSV or JSON file read as a table: its mapping state (US-05.1, domain §7.3).
    pub mapping_state: Option<MappingState>,
}

impl DocumentRow {
    pub fn cursor(&self) -> DocumentCursor {
        DocumentCursor {
            root: self.root.clone(),
            rel_path: self.rel_path.clone(),
        }
    }
}

const STATUS_SQL: &str = "CASE WHEN f.present = 0 THEN 'missing' ELSE c.state END";

const DOCUMENT_FROM: &str = "
    FROM file_ref f
    JOIN source_root r ON r.id = f.root_id
    JOIN content c ON c.id = f.content_id
    WHERE (?1 IS NULL OR CASE WHEN f.present = 0 THEN 'missing' ELSE c.state END = ?1)
      AND (?2 IS NULL OR c.file_type = ?2)";

/// Pages, rows or messages of a read content, by its type.
fn units(
    conn: &Connection,
    content: i64,
    file_type: &str,
) -> Result<Option<(u64, &'static str)>, StoreError> {
    let (sql, unit) = match file_type {
        "pdf" | "image" => (
            "SELECT MAX(s.loc_pos) FROM document d JOIN segment s ON s.document_id = d.id
             WHERE d.content_id = ?1 AND s.loc_kind IN ('pdf_text', 'ocr_text')",
            "pages",
        ),
        "csv" => (
            "SELECT COUNT(*) FROM document d JOIN segment s ON s.document_id = d.id
             WHERE d.content_id = ?1 AND d.parent_id IS NULL",
            "rows",
        ),
        "mbox" => (
            "SELECT COUNT(*) FROM document WHERE content_id = ?1 AND parent_id IS NULL",
            "messages",
        ),
        _ => return Ok(None),
    };
    let n: Option<i64> = conn
        .prepare_cached(sql)?
        .query_row([content], |r| r.get(0))?;
    Ok(n.filter(|n| *n > 0).map(|n| (n as u64, unit)))
}

/// One page of the documents list in path order, after `after` (keyset paging).
pub(crate) fn documents_page(
    conn: &Connection,
    filter: &DocumentFilter,
    after: Option<&DocumentCursor>,
    limit: usize,
) -> Result<Vec<DocumentRow>, StoreError> {
    let sql = format!(
        "SELECT f.id, c.id, r.id, r.path, f.rel_path, c.file_type, c.size, {STATUS_SQL},
                c.warnings, c.failure_code, c.failure_reason, c.failure_detail,
                (SELECT COUNT(*) - 1 FROM file_ref o WHERE o.content_id = c.id),
                (SELECT COUNT(*) FROM sensitive_flag k WHERE k.content_id = c.id)
         {DOCUMENT_FROM}
           AND (?3 IS NULL OR (r.path, f.rel_path) > (?3, ?4))
         ORDER BY r.path, f.rel_path
         LIMIT ?5"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(
        params![
            filter.status,
            filter.file_type,
            after.map(|a| a.root.as_str()),
            after.map(|a| a.rel_path.as_str()),
            limit as i64
        ],
        |r| {
            Ok(DocumentRow {
                ref_id: r.get(0)?,
                content_id: r.get(1)?,
                root_id: r.get(2)?,
                root: r.get(3)?,
                rel_path: r.get(4)?,
                file_type: r.get(5)?,
                size: r.get::<_, i64>(6)? as u64,
                units: None,
                status: r.get(7)?,
                warnings: r
                    .get::<_, String>(8)?
                    .split('\n')
                    .filter(|w| !w.is_empty())
                    .map(str::to_string)
                    .collect(),
                failure_code: r.get(9)?,
                failure_reason: r.get(10)?,
                failure_detail: r.get(11)?,
                other_paths: r.get::<_, i64>(12)?.max(0) as u64,
                card_like_count: r.get::<_, i64>(13)? as u64,
                mapping_state: None,
            })
        },
    )?;
    let mut out: Vec<DocumentRow> = rows.collect::<Result<_, _>>()?;
    for row in &mut out {
        if row.status == IngestionState::Done.as_str() || row.status == STATUS_MISSING {
            row.units = units(conn, row.content_id, &row.file_type)?;
        }
        row.mapping_state = crate::mapping::mapping_state(conn, row.content_id)?;
    }
    Ok(out)
}

/// Rows the documents list has with `filter`.
pub(crate) fn documents_count(
    conn: &Connection,
    filter: &DocumentFilter,
) -> Result<u64, StoreError> {
    let n: i64 = conn
        .prepare_cached(&format!("SELECT COUNT(*) {DOCUMENT_FROM}"))?
        .query_row(params![filter.status, filter.file_type], |r| r.get(0))?;
    Ok(n as u64)
}

/// Paths per file type, for the type filter.
pub(crate) fn document_type_counts(conn: &Connection) -> Result<BTreeMap<String, u64>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.file_type, COUNT(*) FROM file_ref f JOIN content c ON c.id = f.content_id
         GROUP BY c.file_type",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// How many connections lose evidence if `refs` are removed (US-01.5 AC2): connections with a
/// mention in a content that would have no present path left.
pub(crate) fn removal_impact(conn: &Connection, refs: &[i64]) -> Result<u64, StoreError> {
    let ids: BTreeSet<i64> = refs.iter().copied().collect();
    let mut contents = BTreeSet::new();
    for &id in &ids {
        let content: Option<i64> = conn
            .prepare_cached("SELECT content_id FROM file_ref WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        let Some(content) = content else { continue };
        let others: Vec<i64> = conn
            .prepare_cached("SELECT id FROM file_ref WHERE content_id = ?1 AND present = 1")?
            .query_map([content], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        if others.iter().all(|o| ids.contains(o)) {
            contents.insert(content);
        }
    }
    let mut keys = BTreeSet::new();
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT m.identifier_key FROM identifier_mention m
         JOIN connection k ON k.subject_key = m.identifier_key
         WHERE m.content_id = ?1",
    )?;
    let mut entities = conn.prepare_cached(
        "SELECT DISTINCT m.name_key FROM entity_mention m
         JOIN connection k ON k.subject_key = m.name_key
         WHERE m.content_id = ?1",
    )?;
    for c in contents {
        for k in stmt.query_map([c], |r| r.get::<_, String>(0))? {
            keys.insert(k?);
        }
        for k in entities.query_map([c], |r| r.get::<_, String>(0))? {
            keys.insert(k?);
        }
    }
    Ok(keys.len() as u64)
}

/// The Stage A queue for the progress panel (US-03.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ProgressDetail {
    /// Items Done or Failed since the run started, and the bytes of their contents.
    pub finished: u64,
    pub finished_bytes: u64,
    /// Items still to do in open jobs (paused ones too), and their bytes.
    pub open: u64,
    pub open_bytes: u64,
    /// Of those, in a worker now.
    pub in_progress: u64,
    /// The queue is paused (INV-PR-06).
    pub paused: bool,
}

pub(crate) fn queue_paused(conn: &Connection) -> Result<bool, StoreError> {
    Ok(meta(conn, "queue_paused")?.as_deref() == Some("1"))
}

/// See [`ProgressDetail`]; `since` is the run's start in UTC milliseconds.
pub(crate) fn progress_detail(
    conn: &Connection,
    since: Option<i64>,
) -> Result<ProgressDetail, StoreError> {
    let mut out = ProgressDetail {
        paused: queue_paused(conn)?,
        ..ProgressDetail::default()
    };
    if let Some(since) = since {
        let (n, bytes): (i64, i64) = conn
            .prepare_cached(
                "SELECT COUNT(*), COALESCE(SUM(c.size), 0)
                 FROM work_item w LEFT JOIN content c ON c.id = w.content_id
                 WHERE w.finished_at >= ?1 AND w.state IN (?2, ?3)",
            )?
            .query_row(
                params![since, ItemState::Done.as_str(), ItemState::Failed.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
        out.finished = n as u64;
        out.finished_bytes = bytes as u64;
    }
    let (open, bytes, running): (i64, i64, i64) = conn
        .prepare_cached(
            "SELECT COUNT(*), COALESCE(SUM(c.size), 0), COALESCE(SUM(w.state = ?2), 0)
             FROM work_item w
             JOIN job j ON j.id = w.job_id
             LEFT JOIN content c ON c.id = w.content_id
             WHERE w.state IN (?1, ?2) AND j.state IN (?3, ?4, ?5, ?6)",
        )?
        .query_row(
            params![
                ItemState::Pending.as_str(),
                ItemState::InProgress.as_str(),
                JobState::Queued.as_str(),
                JobState::Running.as_str(),
                JobState::Paused.as_str(),
                JobState::Blocked.as_str()
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    out.open = open as u64;
    out.open_bytes = bytes as u64;
    out.in_progress = running as u64;
    Ok(out)
}

/// A project-meta value.
pub(crate) fn meta(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT value FROM project_meta WHERE key = ?1")?
        .query_row([key], |r| r.get(0))
        .optional()?)
}

/// The project's settings; the defaults for what was never set (INV-WS-05).
pub(crate) fn settings(conn: &Connection) -> Result<ProjectSettings, StoreError> {
    let ocr = meta(conn, "ocr_languages")?.unwrap_or_else(|| DEFAULT_OCR_LANGUAGE.to_string());
    Ok(ProjectSettings {
        phone_region: meta(conn, "phone_region")?.filter(|r| !r.is_empty()),
        ocr_languages: ocr
            .split('+')
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .collect(),
        domain_stop_list: domain_stop_list(conn)?,
        custom_patterns: custom_patterns(conn)?,
    })
}

/// The project's domain stop list (US-04.3 AC3): the stored one, or the default list.
pub(crate) fn domain_stop_list(conn: &Connection) -> Result<Vec<String>, StoreError> {
    Ok(match meta(conn, "domain_stop_list")? {
        Some(list) => list
            .split('\n')
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .collect(),
        None => ProjectSettings::default().domain_stop_list,
    })
}

/// The project's custom patterns in the order they were added (US-04.7).
pub(crate) fn custom_patterns(conn: &Connection) -> Result<Vec<PatternSetting>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT name, expression FROM custom_pattern ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(PatternSetting::new(
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Paths removed from a root (US-01.5).
pub(crate) fn exclusions(conn: &Connection, root: i64) -> Result<BTreeSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT rel_path FROM root_exclusion WHERE root_id = ?1")?;
    let rows = stmt.query_map([root], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Whether a Pending item is in a Queued or Running job: a run would start something.
pub(crate) fn has_runnable_work(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM work_item w JOIN job j ON j.id = w.job_id
                            WHERE w.state = ?1 AND j.state IN (?2, ?3))",
        )?
        .query_row(
            params![
                ItemState::Pending.as_str(),
                JobState::Queued.as_str(),
                JobState::Running.as_str()
            ],
            |r| r.get(0),
        )?)
}
