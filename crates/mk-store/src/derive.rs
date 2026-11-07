//! Per-key rebuild of the derived tables, inside the caller's transaction (ADR-0005): for each
//! touched key, the identifier row (or, for a name key or split key, the row of the entity holding
//! it), the decision states on its anchors and the connection row are recomputed from the stored
//! mentions and the decision log with the rules of `mk-graph`. Who belongs to which entity is kept
//! apart, in [`crate::curation`]: only a decision changes it.

use std::collections::BTreeSet;

use mk_corpus::IngestionState;
use mk_graph::entity::{EntityDecision, alias_parts, anchor_statuses, by_family, entity_edits};
use mk_graph::{
    ConnectionRow, DecisionFact, DecisionKind, EntityCounts, EntityStats, IdentifierStats,
    LOW_CONFIDENCE, StopList, SubjectKind, connection_for, entity_connection_for, resolve,
};
use mk_kernel::{IdentifierKey, NameKey};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::curation::{self, Scope};

pub(crate) const DERIVED_SQL: &str = include_str!("../migrations/derived.sql");
/// Bump when `derived.sql` or the derived rules change: the derived tables are rebuilt on open.
/// 2 (M4b): stop-list domains are not connections (INV-LK-03). 3 (M4c): entities. 4 (M4d):
/// `connection.kind`, `value` and `file_types`. 5 (M6a): `connection.low_only`. 6 (M7a): entities
/// by entity key, groups, splits and proposals.
pub(crate) const DERIVED_VERSION: u32 = 6;

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
         DROP TABLE IF EXISTS decision_status;
         DROP TABLE IF EXISTS entity_member;
         DROP TABLE IF EXISTS merge_edge;
         DROP TABLE IF EXISTS split_anchor;
         DROP TABLE IF EXISTS split_mention;
         DROP TABLE IF EXISTS merge_proposal;",
    )?;
    conn.execute_batch(DERIVED_SQL)?;
    // A test's shorter migration list may stop before entity mentions (schema v7).
    let entities: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'entity_mention')",
        [],
        |r| r.get(0),
    )?;
    curation::rebuild_membership(conn, entities)?;
    let mut keys: BTreeSet<String> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT DISTINCT identifier_key FROM identifier_mention {}",
            if entities {
                "UNION SELECT DISTINCT name_key FROM entity_mention"
            } else {
                ""
            }
        ))?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    curation::decision_subjects(conn, &mut keys)?;
    rebuild_keys(conn, &keys)?;
    conn.execute(
        "INSERT INTO project_meta (key, value) VALUES ('derived_version', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [DERIVED_VERSION.to_string()],
    )?;
    Ok(())
}

/// What the decision log holds, read once per rebuild: a project without a kind of decision skips
/// its lookups, so a commit's cost stays what it was (S2c, NFR-PERF-34).
#[derive(Clone, Copy, Default)]
pub(crate) struct Decided {
    /// Groups of two or more name keys.
    pub members: bool,
    /// Ignores or edits on entities.
    pub entity: bool,
    pub merges: bool,
    pub aliases: bool,
    pub splits: bool,
}

impl Decided {
    fn load(conn: &Connection) -> Result<Decided, StoreError> {
        let any = |sql: &str| -> Result<bool, StoreError> {
            Ok(conn
                .prepare_cached(sql)?
                .query_row([], |r| r.get::<_, bool>(0))?)
        };
        let of_kinds = |kinds: &[DecisionKind]| -> Result<bool, StoreError> {
            let list: Vec<String> = kinds.iter().map(|k| format!("'{}'", k.as_str())).collect();
            any(&format!(
                "SELECT EXISTS (SELECT 1 FROM decision WHERE kind IN ({}))",
                list.join(",")
            ))
        };
        Ok(Decided {
            members: any("SELECT EXISTS (SELECT 1 FROM entity_member)")?,
            merges: any("SELECT EXISTS (SELECT 1 FROM merge_edge)")?,
            splits: any("SELECT EXISTS (SELECT 1 FROM split_anchor)")?,
            entity: of_kinds(&ENTITY_KINDS)?,
            aliases: of_kinds(&[DecisionKind::RemoveAlias, DecisionKind::RestoreAlias])?,
        })
    }
}

/// The decisions a name key or split key anchors directly.
const ENTITY_KINDS: [DecisionKind; 5] = [
    DecisionKind::IgnoreEntity,
    DecisionKind::UnignoreEntity,
    DecisionKind::Rename,
    DecisionKind::ChangeType,
    DecisionKind::SetNote,
];

/// Rebuilds the derived rows of `keys`, each through the entity holding it. Returns true if any
/// connection row was added, removed or changed (the `ConnectionsChanged` event, ADR-0005 step 5).
pub(crate) fn rebuild_keys(conn: &Connection, keys: &BTreeSet<String>) -> Result<bool, StoreError> {
    if keys.is_empty() {
        return Ok(false);
    }
    let stop = stop_list(conn)?;
    let decided = Decided::load(conn)?;
    let mut changed = false;
    let mut subjects: BTreeSet<&str> = BTreeSet::new();
    let mut held: BTreeSet<String> = BTreeSet::new();
    let mut any_entity = false;
    for key in keys {
        if SubjectKind::of_key(key) == SubjectKind::Identifier {
            subjects.insert(key);
            continue;
        }
        any_entity = true;
        match decided
            .members
            .then(|| curation::entity_key_of(conn, key))
            .transpose()?
            .flatten()
        {
            Some(root) if root != *key => {
                changed |= retire_key(conn, key)?;
                held.insert(root);
            }
            _ => {
                subjects.insert(key);
            }
        }
    }
    for key in subjects
        .iter()
        .copied()
        .chain(held.iter().map(String::as_str))
    {
        changed |= rebuild_key(conn, key, &stop, decided)?;
    }
    if any_entity {
        curation::bump_entity_generation(conn)?;
    }
    Ok(changed)
}

/// `key` is a member under another key now: its own rows go.
fn retire_key(conn: &Connection, key: &str) -> Result<bool, StoreError> {
    conn.prepare_cached("DELETE FROM entity WHERE entity_key = ?1")?
        .execute([key])?;
    Ok(conn
        .prepare_cached("DELETE FROM connection WHERE subject_key = ?1")?
        .execute([key])?
        > 0)
}

fn rebuild_key(
    conn: &Connection,
    key: &str,
    stop: &StopList,
    decided: Decided,
) -> Result<bool, StoreError> {
    let before = connection_shown(conn, key)?;
    let after = match SubjectKind::of_key(key) {
        SubjectKind::Identifier => {
            let decisions = identifier_decisions(conn, key)?;
            rebuild_identifier(conn, key, &decisions, stop)?
        }
        SubjectKind::Entity => rebuild_entity(conn, key, decided)?,
    };
    match &after {
        Some((row, shown)) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO connection (subject_key, subject_kind, source_count,
                    file_type_count, mention_count, low_confidence, ignored, low_only, kind,
                    value, file_types)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![
                row.subject_key,
                row.subject_kind.as_str(),
                row.source_count as i64,
                row.file_type_count as i64,
                row.mention_count as i64,
                row.low_confidence,
                row.ignored,
                row.low_only,
                shown.kind,
                shown.value,
                shown.file_types
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

/// The ignore and un-ignore decisions on an identifier (domain §9.5).
fn identifier_decisions(conn: &Connection, key: &str) -> Result<Vec<DecisionFact>, StoreError> {
    let [ignore, unignore] = DecisionKind::ignore_pair(SubjectKind::Identifier);
    let mut stmt = conn
        .prepare_cached("SELECT seq, kind FROM decision WHERE kind IN (?2, ?3) AND anchor = ?1")?;
    let rows = stmt.query_map(params![key, ignore.as_str(), unignore.as_str()], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, kind) = row?;
        out.push(DecisionFact {
            seq,
            kind: decision_kind(&kind)?,
        });
    }
    Ok(out)
}

fn decision_kind(name: &str) -> Result<DecisionKind, StoreError> {
    DecisionKind::from_name(name)
        .ok_or_else(|| StoreError::Invalid(format!("unknown decision kind {name}")))
}

/// Writes the states of the decisions on `anchor`: a family stands or falls with its own presence.
fn write_statuses(
    conn: &Connection,
    anchor: &str,
    families: &[(&[DecisionFact], bool)],
) -> Result<(), StoreError> {
    conn.prepare_cached("DELETE FROM decision_status WHERE anchor = ?1")?
        .execute([anchor])?;
    let mut insert = conn
        .prepare_cached("INSERT INTO decision_status (seq, anchor, status) VALUES (?1, ?2, ?3)")?;
    for (seq, status) in anchor_statuses(families) {
        insert.execute(params![seq, anchor, status.as_str()])?;
    }
    Ok(())
}

/// One decision on an entity's anchor.
struct OnAnchor {
    seq: i64,
    kind: DecisionKind,
    anchor: String,
    after: Option<String>,
}

/// The decisions of `kinds` at `anchor`: `(seq, kind, after)`.
fn decisions_at(
    conn: &Connection,
    kinds: &[DecisionKind],
    anchor: &str,
) -> Result<Vec<(i64, DecisionKind, Option<String>)>, StoreError> {
    let list: Vec<String> = kinds.iter().map(|k| format!("'{}'", k.as_str())).collect();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT seq, kind, after FROM decision WHERE kind IN ({}) AND anchor = ?1",
        list.join(",")
    ))?;
    let rows = stmt.query_map([anchor], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, kind, after) = row?;
        out.push((seq, decision_kind(&kind)?, after));
    }
    Ok(out)
}

/// The states of the decisions an entity's anchors hold, and those decisions, for its edits.
/// Name-key anchors find the entity containing them (domain §9.5); a pair stands while both its keys
/// have mentions; an alias while the name does.
fn entity_decisions(
    conn: &Connection,
    key: &str,
    scope: Scope,
    present: bool,
    decided: Decided,
) -> Result<Vec<OnAnchor>, StoreError> {
    let anchors = curation::anchors_of(conn, key, scope)?;
    let mut applied = Vec::new();
    for anchor in &anchors {
        let on_anchor = if decided.entity {
            decisions_at(conn, &ENTITY_KINDS, anchor)?
        } else {
            Vec::new()
        };
        let facts: Vec<DecisionFact> = on_anchor
            .iter()
            .map(|(seq, kind, _)| DecisionFact {
                seq: *seq,
                kind: *kind,
            })
            .collect();
        let split_family = if scope == Scope::Split {
            decisions_at(
                conn,
                &[DecisionKind::Split, DecisionKind::UndoSplit],
                anchor,
            )?
        } else {
            Vec::new()
        };
        let split_facts: Vec<DecisionFact> = split_family
            .iter()
            .map(|(seq, kind, _)| DecisionFact {
                seq: *seq,
                kind: *kind,
            })
            .collect();
        if !facts.is_empty() || !split_facts.is_empty() {
            let families = by_family(&facts);
            let mut parts: Vec<(&[DecisionFact], bool)> =
                families.values().map(|f| (f.as_slice(), present)).collect();
            if !split_facts.is_empty() {
                parts.push((&split_facts, curation::split_present(conn, anchor)?));
            }
            write_statuses(conn, anchor, &parts)?;
        }
        applied.extend(on_anchor.into_iter().map(|(seq, kind, after)| OnAnchor {
            seq,
            kind,
            anchor: anchor.clone(),
            after,
        }));
        if decided.aliases {
            for (alias_anchor, decisions) in curation::alias_decisions(conn, anchor)? {
                let alias = alias_parts(&alias_anchor).map_or("", |(_, a)| a);
                let there = curation::name_in_scope(conn, key, scope, alias)?;
                write_statuses(conn, &alias_anchor, &[(&decisions, there)])?;
            }
        }
        if decided.merges {
            for (pair, a, b) in curation::edges_of(conn, anchor)? {
                let facts: Vec<DecisionFact> = decisions_at(
                    conn,
                    &[DecisionKind::AcceptMerge, DecisionKind::RejectMerge],
                    &pair,
                )?
                .into_iter()
                .map(|(seq, kind, _)| DecisionFact { seq, kind })
                .collect();
                let there = curation::name_present(conn, &a)? && curation::name_present(conn, &b)?;
                write_statuses(conn, &pair, &[(&facts, there)])?;
            }
        }
    }
    Ok(applied)
}

/// What the list shows of a connection beyond its ranking row (M4d).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Shown {
    pub kind: String,
    pub value: String,
    pub file_types: String,
}

/// `kind` and `value` of a subject key as the list shows them: the text before the first `:`
/// and the rest, a custom pattern's label dropped (US-04.7).
pub(crate) fn kind_and_value(key: &str) -> (String, String) {
    let (kind, rest) = key.split_once(':').unwrap_or(("", key));
    let value = if kind == "custom" {
        rest.split_once(':').map_or(rest, |(_, v)| v)
    } else {
        rest
    };
    (kind.to_string(), value.to_string())
}

/// `types` (distinct, any order) as the sorted comma list stored in `connection.file_types`.
fn file_type_list(types: Option<String>) -> String {
    let mut all: Vec<&str> = types
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|t| !t.is_empty())
        .collect();
    all.sort_unstable();
    all.dedup();
    if all.is_empty() {
        String::new()
    } else {
        format!(",{},", all.join(","))
    }
}

fn rebuild_identifier(
    conn: &Connection,
    key: &str,
    decisions: &[DecisionFact],
    stop: &StopList,
) -> Result<Option<(ConnectionRow, Shown)>, StoreError> {
    // Every mention of the key, and the counted ones: content not Missing (INV-CO-07).
    let missing = IngestionState::Missing.as_str();
    let (total, documents, file_types, mentions, max_confidence, types): (
        i64,
        i64,
        i64,
        i64,
        Option<f64>,
        Option<String>,
    ) = conn
        .prepare_cached(
            "SELECT COUNT(*),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 THEN m.document_id END),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 THEN c.file_type END),
                    COALESCE(SUM(c.state != ?2), 0),
                    MAX(CASE WHEN c.state != ?2 THEN m.confidence END),
                    GROUP_CONCAT(DISTINCT CASE WHEN c.state != ?2 THEN c.file_type END)
             FROM identifier_mention m JOIN content c ON c.id = m.content_id
             WHERE m.identifier_key = ?1",
        )?
        .query_row(params![key, missing], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
    let anchor_present = total > 0;
    write_statuses(conn, key, &[(decisions, anchor_present)])?;
    let ignored = resolve(decisions, anchor_present).ignored;
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
    let (kind, value) = kind_and_value(key);
    Ok(connection_for(&parsed, &stats, ignored, stop).map(|row| {
        (
            row,
            Shown {
                kind,
                value,
                file_types: file_type_list(types),
            },
        )
    }))
}

/// An entity's row and connection (domain §9.3; INV-ER-01, INV-ER-07): its mentions are those of
/// [`Scope`]. Counted mentions are those of a content not Missing with confidence ≥ 0.5; the same
/// counts with the lower ones decide a `low_only` row (INV-AI-05).
fn rebuild_entity(
    conn: &Connection,
    key: &str,
    decided: Decided,
) -> Result<Option<(ConnectionRow, Shown)>, StoreError> {
    let scope = Scope::of(conn, key, decided.members)?;
    let missing = IngestionState::Missing.as_str();
    let (total, from_table, documents, file_types, mentions, types, low, low_types): (
        i64,
        bool,
        i64,
        i64,
        i64,
        Option<String>,
        EntityCounts,
        Option<String>,
    ) = conn
        .prepare_cached(&format!(
            "SELECT COUNT(*), COALESCE(MIN(m.producer = 'table'), 0),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 AND m.confidence >= ?3
                                        THEN m.document_id END),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 AND m.confidence >= ?3
                                        THEN c.file_type END),
                    COALESCE(SUM(c.state != ?2 AND m.confidence >= ?3), 0),
                    GROUP_CONCAT(DISTINCT CASE WHEN c.state != ?2 AND m.confidence >= ?3
                                               THEN c.file_type END),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 THEN m.document_id END),
                    COUNT(DISTINCT CASE WHEN c.state != ?2 THEN c.file_type END),
                    COALESCE(SUM(c.state != ?2), 0),
                    GROUP_CONCAT(DISTINCT CASE WHEN c.state != ?2 THEN c.file_type END)
             FROM entity_mention m JOIN content c ON c.id = m.content_id
             WHERE {}",
            scope.filter()
        ))?
        .query_row(params![key, missing, f64::from(LOW_CONFIDENCE)], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                EntityCounts {
                    documents: r.get::<_, i64>(6)? as u64,
                    file_types: r.get::<_, i64>(7)? as u64,
                    mentions: r.get::<_, i64>(8)? as u64,
                },
                r.get(9)?,
            ))
        })?;
    let present = total > 0;
    let decisions = if decided.entity || decided.merges || decided.aliases || decided.splits {
        entity_decisions(conn, key, scope, present, decided)?
    } else {
        Vec::new()
    };
    if !present {
        conn.prepare_cached("DELETE FROM entity WHERE entity_key = ?1")?
            .execute([key])?;
        return Ok(None);
    }
    let used: Vec<EntityDecision<'_>> = decisions
        .iter()
        .map(|d| EntityDecision {
            seq: d.seq,
            kind: d.kind,
            anchor: &d.anchor,
            after: d.after.as_deref(),
        })
        .collect();
    let edits = entity_edits(&used);
    // Most frequent name as written, then A→Z: BINARY collation is byte order, as in
    // `mk_graph::display_name`.
    let (auto, display_key): (String, String) = conn
        .prepare_cached(&format!(
            "SELECT m.raw, MIN(m.name_key) FROM entity_mention m WHERE {}
             GROUP BY m.raw ORDER BY COUNT(*) DESC, m.raw LIMIT 1",
            scope.filter()
        ))?
        .query_row([key], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let parsed: NameKey = display_key
        .parse()
        .map_err(|e: mk_kernel::ParseNameKeyError| StoreError::Invalid(e.to_string()))?;
    let kind = edits
        .entity_type
        .unwrap_or_else(|| parsed.entity_type())
        .as_str()
        .to_string();
    let display = edits.name.clone().unwrap_or(auto);
    conn.prepare_cached(
        "INSERT OR REPLACE INTO entity (entity_key, entity_type, display_name, display_key, note,
                                        mention_count, from_table)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?
    .execute(params![
        key,
        kind,
        display,
        display_key,
        edits.note,
        total,
        from_table
    ])?;
    let stats = EntityStats {
        documents: documents as u64,
        file_types: file_types as u64,
        mentions: mentions as u64,
        low,
    };
    Ok(
        entity_connection_for(key, &stats, edits.ignored).map(|row| {
            let types = if row.low_only { low_types } else { types };
            (
                row,
                Shown {
                    kind,
                    value: display,
                    file_types: file_type_list(types),
                },
            )
        }),
    )
}

/// A connection row with what the list shows of it; `None` if it has none.
fn connection_shown(
    conn: &Connection,
    key: &str,
) -> Result<Option<(ConnectionRow, Shown)>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT subject_key, source_count, file_type_count, mention_count, low_confidence,
                    ignored, subject_kind, low_only, kind, value, file_types
             FROM connection WHERE subject_key = ?1",
        )?
        .query_row([key], |r| {
            Ok((
                row_to_connection(r)?,
                Shown {
                    kind: r.get(8)?,
                    value: r.get(9)?,
                    file_types: r.get(10)?,
                },
            ))
        })
        .optional()?)
}

/// A connection row from `subject_key, source_count, file_type_count, mention_count,
/// low_confidence, ignored, subject_kind, low_only`.
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
        low_only: r.get(7)?,
    })
}
