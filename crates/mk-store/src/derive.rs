//! Per-key rebuild of the derived tables, inside the caller's transaction (ADR-0005): for each
//! touched key, the identifier row (or, for a name key, the entity row), the decision states on
//! that anchor and the connection row are recomputed from the stored mentions and the decision
//! log with the rules of `mk-graph`.

use std::collections::BTreeSet;

use mk_corpus::IngestionState;
use mk_graph::{
    ConnectionRow, DecisionFact, DecisionKind, EntityStats, IdentifierStats, LOW_CONFIDENCE,
    StopList, SubjectKind, connection_for, entity_connection_for, resolve,
};
use mk_kernel::{IdentifierKey, NameKey};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;

pub(crate) const DERIVED_SQL: &str = include_str!("../migrations/derived.sql");
/// Bump when `derived.sql` or the derived rules change: the derived tables are rebuilt on open.
/// 2 (M4b): stop-list domains are not connections (INV-LK-03). 3 (M4c): entities.
pub(crate) const DERIVED_VERSION: u32 = 3;

/// The project's domain stop list (US-04.3 AC3): the stored setting, or the default list.
pub(crate) fn stop_list(conn: &Connection) -> Result<StopList, StoreError> {
    Ok(StopList::new(crate::documents::domain_stop_list(conn)?))
}

/// Drops and recreates the derived tables, then rebuilds every key that has a mention or a
/// decision (ADR-0005 "full rebuild", ADR-0004 after a migration).
pub(crate) fn rebuild_all(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS identifier;
         DROP TABLE IF EXISTS entity;
         DROP TABLE IF EXISTS connection;
         DROP TABLE IF EXISTS decision_status;",
    )?;
    conn.execute_batch(DERIVED_SQL)?;
    // A test's shorter migration list may stop before entity mentions (schema v7).
    let entities: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'entity_mention')",
        [],
        |r| r.get(0),
    )?;
    let keys: BTreeSet<String> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT identifier_key FROM identifier_mention
             {}
             UNION SELECT DISTINCT anchor FROM decision WHERE kind != ?1",
            if entities {
                "UNION SELECT DISTINCT name_key FROM entity_mention"
            } else {
                ""
            }
        ))?;
        stmt.query_map([DecisionKind::ConfirmMapping.as_str()], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    rebuild_keys(conn, &keys)?;
    conn.execute(
        "INSERT INTO project_meta (key, value) VALUES ('derived_version', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [DERIVED_VERSION.to_string()],
    )?;
    Ok(())
}

/// Rebuilds the derived rows of `keys`. Returns true if any connection row was added, removed or
/// changed (the `ConnectionsChanged` event, ADR-0005 step 5).
pub(crate) fn rebuild_keys(conn: &Connection, keys: &BTreeSet<String>) -> Result<bool, StoreError> {
    if keys.is_empty() {
        return Ok(false);
    }
    let stop = stop_list(conn)?;
    let mut changed = false;
    for key in keys {
        changed |= rebuild_key(conn, key, &stop)?;
    }
    Ok(changed)
}

fn rebuild_key(conn: &Connection, key: &str, stop: &StopList) -> Result<bool, StoreError> {
    let before = connection_row(conn, key)?;
    let kind = SubjectKind::of_key(key);
    let decisions = decisions_on(conn, key, kind)?;
    let after = match kind {
        SubjectKind::Identifier => rebuild_identifier(conn, key, &decisions, stop)?,
        SubjectKind::Entity => rebuild_entity(conn, key, &decisions)?,
    };
    match &after {
        Some(row) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO connection (subject_key, subject_kind, source_count,
                    file_type_count, mention_count, low_confidence, ignored)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                row.subject_key,
                row.subject_kind.as_str(),
                row.source_count as i64,
                row.file_type_count as i64,
                row.mention_count as i64,
                row.low_confidence,
                row.ignored
            ])?;
        }
        None if before.is_some() => {
            conn.prepare_cached("DELETE FROM connection WHERE subject_key = ?1")?
                .execute([key])?;
        }
        None => {}
    }
    Ok(before != after)
}

/// The ignore and un-ignore decisions on anchor `key` of a subject of `kind` (domain §9.5).
fn decisions_on(
    conn: &Connection,
    key: &str,
    kind: SubjectKind,
) -> Result<Vec<DecisionFact>, StoreError> {
    let [ignore, unignore] = DecisionKind::ignore_pair(kind);
    let mut stmt = conn
        .prepare_cached("SELECT seq, kind FROM decision WHERE kind IN (?2, ?3) AND anchor = ?1")?;
    let rows = stmt.query_map(params![key, ignore.as_str(), unignore.as_str()], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, kind) = row?;
        let kind = DecisionKind::from_name(&kind)
            .ok_or_else(|| StoreError::Invalid(format!("unknown decision kind {kind}")))?;
        out.push(DecisionFact { seq, kind });
    }
    Ok(out)
}

/// Writes the decision states on `key` (newest wins, INV-CU-05) and returns whether it is
/// ignored.
fn write_statuses(
    conn: &Connection,
    key: &str,
    decisions: &[DecisionFact],
    anchor_present: bool,
) -> Result<bool, StoreError> {
    let resolution = resolve(decisions, anchor_present);
    conn.prepare_cached("DELETE FROM decision_status WHERE anchor = ?1")?
        .execute([key])?;
    let mut insert = conn
        .prepare_cached("INSERT INTO decision_status (seq, anchor, status) VALUES (?1, ?2, ?3)")?;
    for (seq, status) in &resolution.statuses {
        insert.execute(params![seq, key, status.as_str()])?;
    }
    Ok(resolution.ignored)
}

fn rebuild_identifier(
    conn: &Connection,
    key: &str,
    decisions: &[DecisionFact],
    stop: &StopList,
) -> Result<Option<ConnectionRow>, StoreError> {
    // Every mention of the key, and the counted ones: content not Missing (INV-CO-07).
    let missing = IngestionState::Missing.as_str();
    let (total, documents, file_types, mentions, max_confidence): (
        i64,
        i64,
        i64,
        i64,
        Option<f64>,
    ) = conn
        .prepare_cached(
            "SELECT COUNT(*),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 THEN m.document_id END),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 THEN c.file_type END),
                    COALESCE(SUM(c.state != ?2), 0),
                    MAX(CASE WHEN c.state != ?2 THEN m.confidence END)
             FROM identifier_mention m JOIN content c ON c.id = m.content_id
             WHERE m.identifier_key = ?1",
        )?
        .query_row(params![key, missing], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
    let anchor_present = total > 0;
    let ignored = write_statuses(conn, key, decisions, anchor_present)?;
    if !anchor_present {
        conn.prepare_cached("DELETE FROM identifier WHERE key = ?1")?
            .execute([key])?;
        return Ok(None);
    }
    let parsed: IdentifierKey = key
        .parse()
        .map_err(|e: mk_kernel::ParseKeyError| StoreError::Invalid(e.to_string()))?;
    conn.prepare_cached(
        "INSERT INTO identifier (key, kind, value, mention_count) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (key) DO UPDATE SET mention_count = excluded.mention_count",
    )?
    .execute(params![key, parsed.kind().as_str(), parsed.value(), total])?;
    let stats = IdentifierStats {
        documents: documents as u64,
        file_types: file_types as u64,
        mentions: mentions as u64,
        max_confidence: max_confidence.unwrap_or(0.0) as f32,
    };
    Ok(connection_for(&parsed, &stats, ignored, stop))
}

/// An entity's row and connection (domain §9.3 steps 1, 2, 4; INV-ER-07): counted mentions are
/// those of a content not Missing with confidence ≥ 0.5.
fn rebuild_entity(
    conn: &Connection,
    key: &str,
    decisions: &[DecisionFact],
) -> Result<Option<ConnectionRow>, StoreError> {
    let missing = IngestionState::Missing.as_str();
    let (total, from_table, documents, file_types, mentions): (i64, bool, i64, i64, i64) = conn
        .prepare_cached(
            "SELECT COUNT(*), COALESCE(MIN(m.producer = 'table'), 0),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 AND m.confidence >= ?3
                                        THEN m.document_id END),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 AND m.confidence >= ?3
                                        THEN c.file_type END),
                    COALESCE(SUM(c.state != ?2 AND m.confidence >= ?3), 0)
             FROM entity_mention m JOIN content c ON c.id = m.content_id
             WHERE m.name_key = ?1",
        )?
        .query_row(params![key, missing, f64::from(LOW_CONFIDENCE)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
    let anchor_present = total > 0;
    let ignored = write_statuses(conn, key, decisions, anchor_present)?;
    if !anchor_present {
        conn.prepare_cached("DELETE FROM entity WHERE name_key = ?1")?
            .execute([key])?;
        return Ok(None);
    }
    let parsed: NameKey = key
        .parse()
        .map_err(|e: mk_kernel::ParseNameKeyError| StoreError::Invalid(e.to_string()))?;
    // Most frequent name as written, then A→Z: BINARY collation is byte order, as in
    // `mk_graph::display_name`.
    let display: String = conn
        .prepare_cached(
            "SELECT raw FROM entity_mention WHERE name_key = ?1
             GROUP BY raw ORDER BY COUNT(*) DESC, raw LIMIT 1",
        )?
        .query_row([key], |r| r.get(0))?;
    conn.prepare_cached(
        "INSERT OR REPLACE INTO entity (name_key, entity_type, display_name, mention_count,
                                        from_table)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        key,
        parsed.entity_type().as_str(),
        display,
        total,
        from_table
    ])?;
    let stats = EntityStats {
        documents: documents as u64,
        file_types: file_types as u64,
        mentions: mentions as u64,
    };
    Ok(entity_connection_for(&parsed, &stats, ignored))
}

pub(crate) fn connection_row(
    conn: &Connection,
    key: &str,
) -> Result<Option<ConnectionRow>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                    ignored, subject_kind
             FROM connection WHERE subject_key = ?1",
        )?
        .query_row([key], row_to_connection)
        .optional()?)
}

/// A connection row from `subject_key, source_count, file_type_count, mention_count,
/// low_confidence, ignored, subject_kind`.
pub(crate) fn row_to_connection(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectionRow> {
    Ok(ConnectionRow {
        subject_key: r.get(0)?,
        subject_kind: SubjectKind::from_name(&r.get::<_, String>(6)?)
            .unwrap_or(SubjectKind::Identifier),
        source_count: r.get::<_, i64>(1)? as u64,
        file_type_count: r.get::<_, i64>(2)? as u64,
        mention_count: r.get::<_, i64>(3)? as u64,
        low_confidence: r.get(4)?,
        ignored: r.get(5)?,
    })
}
