//! Column mapping reads (E05): a table's state, its columns for the mapping screen, and the
//! files a mapping can be reused from.

use std::collections::{BTreeMap, BTreeSet};

use mk_graph::DecisionKind;
use mk_mapping::{Assignments, ColumnType, MappingState};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::table::{confirm_decisions, resolve_assignments, samples_from};

/// One column on the mapping screen (US-05.1 AC1).
#[derive(Clone, PartialEq, Debug)]
pub struct ColumnRow {
    pub key: String,
    /// The header text (CSV, empty without a header) or the key (JSON).
    pub header: String,
    /// Up to 5 values, card-like numbers masked.
    pub samples: Vec<String>,
    pub suggested: ColumnType,
    /// Share of the sample that supports the suggestion (0–1).
    pub share: f32,
    /// The type a confirm decision gives it, if one applies to this column.
    pub assigned: Option<ColumnType>,
}

/// A file with the same headers and a confirmed mapping (US-05.2 AC2).
#[derive(Clone, PartialEq, Debug)]
pub struct ReuseCandidate {
    pub ref_id: i64,
    pub path: String,
    pub assignments: Assignments,
}

/// What the mapping screen shows of one file.
#[derive(Clone, PartialEq, Debug)]
pub struct MappingView {
    pub ref_id: i64,
    pub content_id: i64,
    pub document_id: i64,
    pub path: String,
    pub file_type: String,
    pub state: MappingState,
    pub signature: String,
    /// Columns in the file; only the first 1,000 are in `columns`.
    pub column_count: u64,
    pub columns: Vec<ColumnRow>,
    pub reuse: Vec<ReuseCandidate>,
}

/// The first table document of a content: `(document, signature, column count)`.
fn table_of(conn: &Connection, content: i64) -> Result<Option<(i64, String, i64)>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT document_id, signature, column_count FROM table_document
             WHERE content_id = ?1 ORDER BY document_id LIMIT 1",
        )?
        .query_row([content], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?)
}

/// The column keys of a table document, from its suggestions (the first 1,000).
fn column_keys(conn: &Connection, document: i64) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT column_key FROM column_suggestion WHERE document_id = ?1 ORDER BY ordinal",
    )?;
    let rows = stmt.query_map([document], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A content's mapping state, if it was read as a table (domain §7.3).
pub(crate) fn mapping_state(
    conn: &Connection,
    content: i64,
) -> Result<Option<MappingState>, StoreError> {
    let Some((_, signature, _)) = table_of(conn, content)? else {
        return Ok(None);
    };
    let confirms = confirm_decisions(conn, content)?;
    let state = mk_mapping::resolve_state(
        &signature,
        &confirms
            .iter()
            .map(|(seq, sig, _)| mk_mapping::ConfirmFact {
                seq: *seq,
                signature: sig,
            })
            .collect::<Vec<_>>(),
    )
    .0;
    Ok(Some(state))
}

/// The mapping screen of file reference `ref_id`; `None` if the path is gone or its file was
/// not read as a table.
pub(crate) fn mapping_view(
    conn: &Connection,
    ref_id: i64,
) -> Result<Option<MappingView>, StoreError> {
    let row: Option<(i64, String, String, i64, String)> = conn
        .prepare_cached(
            "SELECT f.content_id, r.path, f.rel_path, f.root_id, c.file_type
             FROM file_ref f JOIN source_root r ON r.id = f.root_id
             JOIN content c ON c.id = f.content_id WHERE f.id = ?1",
        )?
        .query_row([ref_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .optional()?;
    let Some((content, root_path, rel, _, file_type)) = row else {
        return Ok(None);
    };
    let Some((document, signature, column_count)) = table_of(conn, content)? else {
        return Ok(None);
    };
    let keys = column_keys(conn, document)?;
    let key_set: BTreeSet<&str> = keys.iter().map(String::as_str).collect();
    let confirms = confirm_decisions(conn, content)?;
    let (state, assigned) = resolve_assignments(&signature, &key_set, &confirms);
    let columns = {
        let mut stmt = conn.prepare_cached(
            "SELECT column_key, header, samples, suggested_type, share FROM column_suggestion
             WHERE document_id = ?1 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map([document], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (key, header, samples, suggested, share) = row?;
            out.push(ColumnRow {
                assigned: assigned.get(&key),
                suggested: ColumnType::from_name(&suggested).ok_or_else(|| {
                    StoreError::Invalid(format!("unknown column type {suggested}"))
                })?,
                share: share as f32,
                samples: samples_from(&samples),
                header,
                key,
            });
        }
        out
    };
    Ok(Some(MappingView {
        ref_id,
        content_id: content,
        document_id: document,
        path: crate::read::display_path(Some(root_path), Some(rel)).unwrap_or_default(),
        file_type,
        state,
        signature: signature.clone(),
        column_count: column_count as u64,
        columns,
        reuse: reuse_candidates(conn, content, &signature)?,
    }))
}

/// Other files whose content has these headers and a confirmed mapping for them (US-05.2 AC2):
/// one per path, with the newest confirmed assignments, in path order.
fn reuse_candidates(
    conn: &Connection,
    content: i64,
    signature: &str,
) -> Result<Vec<ReuseCandidate>, StoreError> {
    let suffix = format!("|{signature}");
    let mut stmt = conn.prepare_cached(
        "SELECT anchor, after FROM decision
         WHERE kind = ?1 AND substr(anchor, -length(?2)) = ?2 ORDER BY seq",
    )?;
    let rows = stmt.query_map(
        params![DecisionKind::ConfirmMapping.as_str(), suffix],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    )?;
    // Path → newest assignments; later rows overwrite earlier ones.
    let mut newest: BTreeMap<(i64, String), Assignments> = BTreeMap::new();
    for row in rows {
        let (anchor, after) = row?;
        let Some(path) = anchor
            .strip_prefix("map:")
            .and_then(|a| a.strip_suffix(&suffix))
        else {
            continue;
        };
        let Some((root, rel)) = path.split_once('/') else {
            continue;
        };
        let Ok(root) = root.parse::<i64>() else {
            continue;
        };
        let assignments = after
            .as_deref()
            .and_then(Assignments::from_json)
            .unwrap_or_default();
        newest.insert((root, rel.to_string()), assignments);
    }
    let mut out = Vec::new();
    for ((root, rel), assignments) in newest {
        let found: Option<(i64, i64, String)> = conn
            .prepare_cached(
                "SELECT f.id, f.content_id, r.path FROM file_ref f
                 JOIN source_root r ON r.id = f.root_id
                 WHERE f.root_id = ?1 AND f.rel_path = ?2",
            )?
            .query_row(params![root, rel], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let Some((ref_id, other, root_path)) = found else {
            continue;
        };
        if other == content || table_of(conn, other)?.is_none_or(|(_, sig, _)| sig != signature) {
            continue;
        }
        out.push(ReuseCandidate {
            ref_id,
            path: crate::read::display_path(Some(root_path), Some(rel)).unwrap_or_default(),
            assignments,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
