//! The table step (BC-05, M4c): what a content's CSV or JSON document holds per column, and what
//! its confirmed mapping makes of it. It runs inside the transaction that wrote the content's
//! machine output — a Stage A commit, an identifier or custom-pattern re-run — and inside a
//! confirm decision, so a confirmed mapping applies before any result shows (INV-CU-03,
//! INV-CM-04) and record links follow the current identifiers.
//!
//! Per top-level CSV or JSON document it writes:
//! 1. the header signature and column count (`table_document`);
//! 2. a suggestion per column from its header and up to 200 non-empty values (US-05.1);
//! 3. when a confirm decision applies (domain §7.3), an entity mention per value of a confirmed
//!    person or organization column (US-05.3 AC1, INV-CM-03) and a record link from each to
//!    every identifier in the same row or JSON record (INV-CM-05).
//!
//! Without a confirmed name column only the header and the first rows are read; JSON documents
//! are read once for their keys.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mk_graph::DecisionKind;
use mk_identifiers::{Region, Rules};
use mk_mapping::{
    Assignments, ColumnType, ConfirmFact, MAX_COLUMNS, MappingState, SAMPLE_SIZE, csv_column_key,
    entity_in_cell, header_signature, json_column_key, mapping_anchor_prefix, record_of,
    resolve_state, row_cells, suggest,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::codec;

/// Sample values kept per column for the mapping screen (US-05.1 AC1).
pub const SHOWN_SAMPLES: usize = 5;
/// Separates the stored sample values.
const SAMPLE_SEP: char = '\u{1f}';
/// Document kinds read as tables.
pub(crate) const TABLE_KINDS: [&str; 2] = ["csv", "json"];

/// One column of a table document.
struct ColumnAcc {
    key: String,
    /// The header text (CSV) or the key itself (JSON).
    header: String,
    /// For a JSON key, the key above it (`officers` for `/officers/*/name`).
    parent: Option<String>,
    /// Up to [`SAMPLE_SIZE`] non-empty values, trimmed.
    sample: Vec<String>,
}

impl ColumnAcc {
    fn push(&mut self, value: &str) {
        let v = value.trim();
        if !v.is_empty() && self.sample.len() < SAMPLE_SIZE {
            self.sample.push(v.to_string());
        }
    }
}

/// A table document's columns as read.
struct Table {
    document: i64,
    kind: TableKind,
    columns: Vec<ColumnAcc>,
    signature: String,
    column_count: usize,
}

enum TableKind {
    /// `names`: the column names the parser gave (unique, non-empty header names); `header`:
    /// whether row 0 is a header.
    Csv {
        header: bool,
        names: Vec<Option<String>>,
    },
    Json,
}

/// The project's identifier rules for suggestions (its phone region).
fn rules(conn: &Connection) -> Result<Rules, StoreError> {
    let settings = crate::documents::settings(conn)?;
    Ok(Rules {
        phone_region: settings.phone_region.as_deref().and_then(Region::parse),
        ..Rules::default()
    })
}

/// The name keys of a content's entity mentions, and the split groups some of them moved to.
pub(crate) fn content_entity_keys(
    conn: &Connection,
    content: i64,
    keys: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT DISTINCT name_key FROM entity_mention WHERE content_id = ?1")?;
    for k in stmt.query_map([content], |r| r.get::<_, String>(0))? {
        keys.insert(k?);
    }
    let mut stmt =
        conn.prepare_cached("SELECT DISTINCT split_key FROM split_mention WHERE content_id = ?1")?;
    for k in stmt.query_map([content], |r| r.get::<_, String>(0))? {
        keys.insert(k?);
    }
    Ok(())
}

/// The top-level table documents of a content: `(id, kind)`.
fn table_documents(conn: &Connection, content: i64) -> Result<Vec<(i64, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind FROM document
         WHERE content_id = ?1 AND parent_id IS NULL AND kind IN (?2, ?3) ORDER BY id",
    )?;
    let rows = stmt.query_map(params![content, TABLE_KINDS[0], TABLE_KINDS[1]], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Runs the table step for `content` (see the module doc). The name keys whose mentions changed
/// go into `touched` for the caller's rebuild.
pub(crate) fn apply_table_step(
    conn: &Connection,
    content: i64,
    touched: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    content_entity_keys(conn, content, touched)?;
    for sql in [
        // Table mentions only: AI mentions are not this step's, and a mapping never changes them
        // (INV-AI-04).
        "DELETE FROM entity_mention WHERE content_id = ?1 AND producer = 'table'",
        "DELETE FROM record_link WHERE content_id = ?1",
        "DELETE FROM column_suggestion WHERE content_id = ?1",
        "DELETE FROM table_document WHERE content_id = ?1",
    ] {
        conn.prepare_cached(sql)?.execute([content])?;
    }
    let documents = table_documents(conn, content)?;
    if documents.is_empty() {
        return crate::curation::refresh_splits(conn, content, touched);
    }
    let rules = rules(conn)?;
    let confirms = confirm_decisions(conn, content)?;
    for (document, kind) in documents {
        let table = if kind == "csv" {
            read_csv(conn, document)?
        } else {
            read_json(conn, document)?
        };
        let Some(table) = table else { continue };
        write_columns(conn, content, &table, &rules)?;
        let assignments = applied_assignments(&table, &confirms);
        let names: BTreeMap<&str, ColumnType> = assignments
            .0
            .iter()
            .filter(|(_, t)| t.entity_type().is_some())
            .map(|(k, t)| (k.as_str(), *t))
            .collect();
        if !names.is_empty() {
            write_entities(conn, content, &table, &names, touched)?;
        }
    }
    crate::curation::refresh_splits(conn, content, touched)
}

/// A confirm decision on one of the content's paths: `(seq, signature, assignments)`.
pub(crate) type Confirm = (i64, String, Assignments);

/// The confirm decisions on any path of `content` (present or missing).
pub(crate) fn confirm_decisions(
    conn: &Connection,
    content: i64,
) -> Result<Vec<Confirm>, StoreError> {
    let paths: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare_cached("SELECT root_id, rel_path FROM file_ref WHERE content_id = ?1")?;
        stmt.query_map([content], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut out = Vec::new();
    for (root, rel) in paths {
        out.extend(confirms_at(conn, root, &rel)?);
    }
    Ok(out)
}

/// The confirm decisions on one path, oldest first. Reads the `decision_anchor` index: every
/// anchor of the path starts with its prefix, which ends in `|`, and `}` is the next character.
fn confirms_at(conn: &Connection, root: i64, rel_path: &str) -> Result<Vec<Confirm>, StoreError> {
    let prefix = mapping_anchor_prefix(root, rel_path);
    let end = format!("{}}}", &prefix[..prefix.len() - 1]);
    let mut stmt = conn.prepare_cached(
        "SELECT seq, anchor, after FROM decision
         WHERE kind = ?1 AND anchor >= ?2 AND anchor < ?3 ORDER BY seq",
    )?;
    let rows = stmt.query_map(
        params![DecisionKind::ConfirmMapping.as_str(), prefix, end],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, anchor, after) = row?;
        let Some(signature) = anchor.strip_prefix(&prefix) else {
            continue;
        };
        let assignments = after
            .as_deref()
            .and_then(Assignments::from_json)
            .unwrap_or_default();
        out.push((seq, signature.to_string(), assignments));
    }
    Ok(out)
}

/// The mapping state of a table with `signature` and the assignments that apply: all of the
/// confirmed ones (Confirmed), those of the older mapping for the columns still there (Needs
/// review, D5), or none (Suggested).
pub(crate) fn resolve_assignments(
    signature: &str,
    keys: &BTreeSet<&str>,
    confirms: &[Confirm],
) -> (MappingState, Assignments) {
    let facts: Vec<ConfirmFact<'_>> = confirms
        .iter()
        .map(|(seq, sig, _)| ConfirmFact {
            seq: *seq,
            signature: sig,
        })
        .collect();
    let (state, seq) = resolve_state(signature, &facts);
    let decided = seq
        .and_then(|s| confirms.iter().find(|c| c.0 == s))
        .map(|c| c.2.restricted_to(keys))
        .unwrap_or_default();
    (state, decided)
}

fn applied_assignments(table: &Table, confirms: &[Confirm]) -> Assignments {
    let keys: BTreeSet<&str> = table.columns.iter().map(|c| c.key.as_str()).collect();
    resolve_assignments(&table.signature, &keys, confirms).1
}

/// The segments of a document after ordinal `from`, in order: `(id, loc_pos, loc_ref, text,
/// cell_starts)`.
type SegRow = (i64, i64, Option<String>, String, Vec<u32>);

fn for_each_segment(
    conn: &Connection,
    document: i64,
    from: i64,
    limit: Option<usize>,
    mut f: impl FnMut(SegRow) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, loc_pos, loc_ref, text, cell_starts FROM segment
         WHERE document_id = ?1 AND ordinal >= ?2 ORDER BY ordinal LIMIT ?3",
    )?;
    let limit = limit.map_or(-1, |n| n as i64);
    let mut rows = stmt.query(params![document, from, limit])?;
    while let Some(r) = rows.next()? {
        f((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            codec::cells_from_blob(r.get(4)?),
        ))?;
    }
    Ok(())
}

/// A CSV document's columns: row 0 decides the header as the parser did
/// (`mk_kernel::csv_header_row`); the sample is the first 200 data rows.
fn read_csv(conn: &Connection, document: i64) -> Result<Option<Table>, StoreError> {
    let first: Option<(String, Vec<u8>)> = conn
        .prepare_cached(
            "SELECT text, cell_starts FROM segment WHERE document_id = ?1
             ORDER BY ordinal LIMIT 1",
        )?
        .query_row([document], |r| {
            Ok((
                r.get(0)?,
                r.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(),
            ))
        })
        .optional()?;
    let Some((text, cells)) = first else {
        return Ok(None);
    };
    let first: Vec<String> = row_cells(&text, &codec::cells_from_blob(Some(cells)))
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let header = mk_kernel::csv_header_row(&first);
    let names = if header {
        mk_kernel::csv_column_names(&first)
    } else {
        Vec::new()
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    for_each_segment(
        conn,
        document,
        i64::from(header),
        Some(SAMPLE_SIZE),
        |(_, _, _, text, cells)| {
            rows.push(
                row_cells(&text, &cells)
                    .into_iter()
                    .map(|(_, c)| c)
                    .collect(),
            );
            Ok(())
        },
    )?;
    if !header {
        rows.insert(0, first.clone());
        rows.truncate(SAMPLE_SIZE);
    }
    let column_count = if header {
        first.len()
    } else {
        rows.iter().map(Vec::len).max().unwrap_or(0)
    };
    let keys: Vec<String> = (0..column_count)
        .map(|i| csv_column_key(names.get(i).and_then(|n| n.as_deref()), i))
        .collect();
    let signature = header_signature(keys.iter().map(String::as_str), false);
    let mut columns: Vec<ColumnAcc> = keys
        .into_iter()
        .take(MAX_COLUMNS)
        .enumerate()
        .map(|(i, key)| ColumnAcc {
            key,
            header: if header {
                first[i].trim().to_string()
            } else {
                String::new()
            },
            parent: None,
            sample: Vec::new(),
        })
        .collect();
    for row in &rows {
        for (col, cell) in columns.iter_mut().zip(row) {
            col.push(cell);
        }
    }
    Ok(Some(Table {
        document,
        kind: TableKind::Csv { header, names },
        columns,
        signature,
        column_count,
    }))
}

/// A JSON pointer token as written (RFC 6901 unescaping).
fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// The header words a JSON key gives: its last named token, and the one above it.
fn json_header(key: &str) -> (String, Option<String>) {
    let named: Vec<&str> = key
        .split('/')
        .filter(|t| !t.is_empty() && *t != "*")
        .collect();
    let header = named.last().map(|t| unescape(t)).unwrap_or_default();
    let parent = named.len().checked_sub(2).map(|i| unescape(named[i]));
    (header, parent)
}

/// A JSON document's columns: every key in it (in order of first appearance), each with a
/// sample of its first 200 non-empty values.
fn read_json(conn: &Connection, document: i64) -> Result<Option<Table>, StoreError> {
    let mut columns: Vec<ColumnAcc> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for_each_segment(conn, document, 0, None, |(_, _, pointer, text, _)| {
        let key = json_column_key(pointer.as_deref().unwrap_or(""));
        let i = match at.get(&key) {
            Some(&i) => i,
            None => {
                at.insert(key.clone(), columns.len());
                columns.push(ColumnAcc {
                    key: key.clone(),
                    header: key,
                    parent: None,
                    sample: Vec::new(),
                });
                columns.len() - 1
            }
        };
        columns[i].push(&text);
        Ok(())
    })?;
    if columns.is_empty() {
        return Ok(None);
    }
    let signature = header_signature(columns.iter().map(|c| c.key.as_str()), true);
    let column_count = columns.len();
    columns.truncate(MAX_COLUMNS);
    Ok(Some(Table {
        document,
        kind: TableKind::Json,
        columns,
        signature,
        column_count,
    }))
}

/// Up to [`SHOWN_SAMPLES`] values as stored: card-like numbers masked (NFR-SEC-14).
fn stored_samples(sample: &[String]) -> String {
    sample
        .iter()
        .take(SHOWN_SAMPLES)
        .map(|v| mk_search::mask_card_like(&v.replace(SAMPLE_SEP, " ")).into_owned())
        .collect::<Vec<_>>()
        .join(&SAMPLE_SEP.to_string())
}

/// The stored samples back as a list.
pub(crate) fn samples_from(stored: &str) -> Vec<String> {
    if stored.is_empty() {
        return Vec::new();
    }
    stored.split(SAMPLE_SEP).map(str::to_string).collect()
}

fn write_columns(
    conn: &Connection,
    content: i64,
    table: &Table,
    rules: &Rules,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO table_document (document_id, content_id, signature, column_count)
         VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        table.document,
        content,
        table.signature,
        table.column_count as i64
    ])?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO column_suggestion (document_id, content_id, ordinal, column_key, header,
                                        suggested_type, share, samples)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for (i, col) in table.columns.iter().enumerate() {
        let sample: Vec<&str> = col.sample.iter().map(String::as_str).collect();
        let (header, parent) = match table.kind {
            TableKind::Csv { .. } => (col.header.clone(), col.parent.clone()),
            TableKind::Json => json_header(&col.key),
        };
        let s = suggest(&header, parent.as_deref(), &sample, rules);
        insert.execute(params![
            table.document,
            content,
            i as i64,
            col.key,
            col.header,
            s.column_type.as_str(),
            f64::from(s.share.clamp(0.0, 1.0)),
            stored_samples(&col.sample)
        ])?;
    }
    Ok(())
}

const INSERT_ENTITY: &str = "INSERT INTO entity_mention (segment_id, document_id, content_id,
        name_key, entity_type, raw, span_start, span_end, column_index, column_name, column_key,
        record, confidence, producer)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1.0, 'table')";
const INSERT_LINK: &str = "INSERT INTO record_link (content_id, document_id, segment_id, record,
        name_key, identifier_key)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

/// The identifier keys of one segment's mentions.
fn segment_identifiers(conn: &Connection, segment: i64) -> Result<BTreeSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT identifier_key FROM identifier_mention WHERE segment_id = ?1",
    )?;
    let rows = stmt.query_map([segment], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Writes the entity mentions of the confirmed name columns `names` and their record links.
fn write_entities(
    conn: &Connection,
    content: i64,
    table: &Table,
    names: &BTreeMap<&str, ColumnType>,
    touched: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let mut insert = conn.prepare_cached(INSERT_ENTITY)?;
    let mut link = conn.prepare_cached(INSERT_LINK)?;
    let document = table.document;
    match &table.kind {
        TableKind::Csv {
            header,
            names: col_names,
        } => {
            // Every column of the row, not only the first 1,000 (a name column is one of them).
            let keys: Vec<String> = (0..table.column_count)
                .map(|i| csv_column_key(col_names.get(i).and_then(|n| n.as_deref()), i))
                .collect();
            let assigned: Vec<(usize, ColumnType)> = keys
                .iter()
                .enumerate()
                .filter_map(|(i, k)| names.get(k.as_str()).map(|t| (i, *t)))
                .collect();
            for_each_segment(
                conn,
                document,
                i64::from(*header),
                None,
                |(segment, row, _, text, cells)| {
                    let row_cells = row_cells(&text, &cells);
                    let len = text.chars().count();
                    let mut found: BTreeSet<String> = BTreeSet::new();
                    for &(i, t) in &assigned {
                        let Some((start, cell)) = row_cells.get(i) else {
                            continue;
                        };
                        let Some(hit) = entity_in_cell(Some(t), cell, *start, len) else {
                            continue;
                        };
                        let key = hit.name_key.to_string();
                        insert.execute(params![
                            segment,
                            document,
                            content,
                            key,
                            hit.name_key.entity_type().as_str(),
                            hit.raw,
                            hit.span.start() as i64,
                            hit.span.end() as i64,
                            i as i64,
                            col_names.get(i).cloned().flatten(),
                            keys[i],
                            row.to_string()
                        ])?;
                        touched.insert(key.clone());
                        found.insert(key);
                    }
                    if !found.is_empty() {
                        let ids = segment_identifiers(conn, segment)?;
                        for name in &found {
                            for id in &ids {
                                link.execute(params![
                                    content,
                                    document,
                                    segment,
                                    row.to_string(),
                                    name,
                                    id
                                ])?;
                            }
                        }
                    }
                    Ok(())
                },
            )?;
        }
        TableKind::Json => {
            // Per record: its names (with the segment of the first) and its identifiers.
            let mut records: BTreeMap<String, (i64, BTreeSet<String>, BTreeSet<String>)> =
                BTreeMap::new();
            for_each_segment(conn, document, 0, None, |(segment, _, pointer, text, _)| {
                let pointer = pointer.unwrap_or_default();
                let column = json_column_key(&pointer);
                let record = record_of(&pointer);
                let hit = entity_in_cell(
                    names.get(column.as_str()).copied(),
                    &text,
                    0,
                    text.chars().count(),
                );
                let ids = segment_identifiers(conn, segment)?;
                if hit.is_none() && ids.is_empty() {
                    return Ok(());
                }
                let entry = records
                    .entry(record.clone())
                    .or_insert_with(|| (0, BTreeSet::new(), BTreeSet::new()));
                entry.2.extend(ids);
                if let Some(hit) = hit {
                    let key = hit.name_key.to_string();
                    insert.execute(params![
                        segment,
                        document,
                        content,
                        key,
                        hit.name_key.entity_type().as_str(),
                        hit.raw,
                        hit.span.start() as i64,
                        hit.span.end() as i64,
                        Option::<i64>::None,
                        Option::<String>::None,
                        column,
                        record
                    ])?;
                    if entry.1.is_empty() {
                        entry.0 = segment;
                    }
                    entry.1.insert(key.clone());
                    touched.insert(key);
                }
                Ok(())
            })?;
            for (record, (segment, found, ids)) in &records {
                for name in found {
                    for id in ids {
                        link.execute(params![content, document, segment, record, name, id])?;
                    }
                }
            }
        }
    }
    Ok(())
}
