//! Per-key rebuild of the derived tables, inside the caller's transaction (ADR-0005): for each
//! touched identifier key, the identifier row, the decision states on that anchor and the
//! connection row are recomputed from the stored mentions and the decision log with the rules
//! of `mk-graph`.

use std::collections::BTreeSet;

use mk_corpus::IngestionState;
use mk_graph::{
    ConnectionRow, DecisionFact, DecisionKind, IdentifierStats, SubjectKind, connection_for,
    resolve,
};
use mk_kernel::IdentifierKey;
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;

pub(crate) const DERIVED_SQL: &str = include_str!("../migrations/derived.sql");
/// Bump when `derived.sql` or the derived rules change: the derived tables are rebuilt on open.
pub(crate) const DERIVED_VERSION: u32 = 1;

/// Drops and recreates the derived tables, then rebuilds every key that has a mention or a
/// decision (ADR-0005 "full rebuild", ADR-0004 after a migration).
pub(crate) fn rebuild_all(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS identifier;
         DROP TABLE IF EXISTS connection;
         DROP TABLE IF EXISTS decision_status;",
    )?;
    conn.execute_batch(DERIVED_SQL)?;
    let keys: BTreeSet<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT identifier_key FROM identifier_mention
             UNION SELECT DISTINCT anchor FROM decision",
        )?;
        stmt.query_map([], |r| r.get(0))?
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
    let mut changed = false;
    for key in keys {
        changed |= rebuild_key(conn, key)?;
    }
    Ok(changed)
}

fn rebuild_key(conn: &Connection, key: &str) -> Result<bool, StoreError> {
    let before = connection_row(conn, key)?;

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

    // Decisions on this anchor (domain §9.5), newest wins (INV-CU-05).
    let decisions: Vec<DecisionFact> = {
        let mut stmt = conn.prepare_cached(
            "SELECT seq, kind FROM decision WHERE kind IN (?2, ?3) AND anchor = ?1",
        )?;
        let rows = stmt.query_map(
            params![
                key,
                DecisionKind::IgnoreIdentifier.as_str(),
                DecisionKind::UnignoreIdentifier.as_str()
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, kind) = row?;
            let kind = DecisionKind::from_name(&kind)
                .ok_or_else(|| StoreError::Invalid(format!("unknown decision kind {kind}")))?;
            out.push(DecisionFact { seq, kind });
        }
        out
    };
    let resolution = resolve(&decisions, anchor_present);
    conn.prepare_cached("DELETE FROM decision_status WHERE anchor = ?1")?
        .execute([key])?;
    {
        let mut insert = conn.prepare_cached(
            "INSERT INTO decision_status (seq, anchor, status) VALUES (?1, ?2, ?3)",
        )?;
        for (seq, status) in &resolution.statuses {
            insert.execute(params![seq, key, status.as_str()])?;
        }
    }

    let after = if anchor_present {
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
        connection_for(&parsed, &stats, resolution.ignored)
    } else {
        conn.prepare_cached("DELETE FROM identifier WHERE key = ?1")?
            .execute([key])?;
        None
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

pub(crate) fn connection_row(
    conn: &Connection,
    key: &str,
) -> Result<Option<ConnectionRow>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                    ignored
             FROM connection WHERE subject_key = ?1",
        )?
        .query_row([key], row_to_connection)
        .optional()?)
}

pub(crate) fn row_to_connection(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectionRow> {
    Ok(ConnectionRow {
        subject_key: r.get(0)?,
        subject_kind: SubjectKind::Identifier,
        source_count: r.get::<_, i64>(1)? as u64,
        file_type_count: r.get::<_, i64>(2)? as u64,
        mention_count: r.get::<_, i64>(3)? as u64,
        low_confidence: r.get(4)?,
        ignored: r.get(5)?,
    })
}
