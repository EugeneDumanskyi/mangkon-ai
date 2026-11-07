//! BC-07 and BC-09 storage (M7a, domain §9.3, §9.5): who belongs to which entity, the decisions that
//! change it, and the one definition of "the mentions of an entity" every read goes through.
//!
//! Groups depend only on merge decisions, so a commit never regroups. A split lists mention keys, so
//! it reaches a mention through its content: each write site of entity mentions calls
//! [`refresh_splits`] for that content, which is one index probe while a project has no split.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mk_corpus::IngestionState;
use mk_graph::entity::{
    alias_anchor, anchor_subjects, group_members, merge_anchor, merge_pair, split_for, split_keys,
};
use mk_graph::proposal::{Candidate, MAX_ENTITIES_PER_IDENTIFIER, Proposal, is_hard_identifier};
use mk_graph::{DecisionFact, DecisionFamily, DecisionKind, LOW_CONFIDENCE};
use mk_kernel::{ContentHash, EntityType, MentionKey, NameKey, is_split_key};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{StoreError, codec, documents, manage, ops};

/// How an entity key reaches its mentions (INV-ER-01).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    /// One name key, without its moved mentions.
    Single,
    /// A group of name keys, without their moved mentions.
    Group,
    /// A split group: the mentions moved to it.
    Split,
}

impl Scope {
    /// `members`: whether the project has groups at all.
    pub(crate) fn of(conn: &Connection, key: &str, members: bool) -> Result<Scope, StoreError> {
        if is_split_key(key) {
            return Ok(Scope::Split);
        }
        if members
            && conn
                .prepare_cached("SELECT 1 FROM entity_member WHERE entity_key = ?1 LIMIT 1")?
                .query_row([key], |_| Ok(()))
                .optional()?
                .is_some()
        {
            return Ok(Scope::Group);
        }
        Ok(Scope::Single)
    }

    /// Scope of `key` when the caller does not know whether the project has groups.
    pub(crate) fn lookup(conn: &Connection, key: &str) -> Result<Scope, StoreError> {
        Self::of(conn, key, true)
    }

    /// The condition on `entity_mention m` that picks an entity's mentions; `?1` is the entity key.
    pub(crate) fn filter(self) -> &'static str {
        match self {
            Scope::Single => {
                "m.name_key = ?1
                 AND NOT EXISTS (SELECT 1 FROM split_mention x WHERE x.mention_id = m.id)"
            }
            Scope::Group => {
                "m.name_key IN (SELECT name_key FROM entity_member WHERE entity_key = ?1)
                 AND NOT EXISTS (SELECT 1 FROM split_mention x WHERE x.mention_id = m.id)"
            }
            Scope::Split => "m.id IN (SELECT mention_id FROM split_mention WHERE split_key = ?1)",
        }
    }
}

/// The group of `key`, if it is in one of two or more name keys.
pub(crate) fn entity_key_of(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT entity_key FROM entity_member WHERE name_key = ?1")?
        .query_row([key], |r| r.get(0))
        .optional()?)
}

/// The entity key holding `key`: itself, unless it is a member of a group.
pub(crate) fn entity_key(conn: &Connection, key: &str) -> Result<String, StoreError> {
    Ok(entity_key_of(conn, key)?.unwrap_or_else(|| key.to_string()))
}

/// The anchors of an entity: its name keys, or its split key.
pub(crate) fn anchors_of(
    conn: &Connection,
    key: &str,
    scope: Scope,
) -> Result<Vec<String>, StoreError> {
    if scope != Scope::Group {
        return Ok(vec![key.to_string()]);
    }
    let mut stmt =
        conn.prepare_cached("SELECT name_key FROM entity_member WHERE entity_key = ?1")?;
    let rows = stmt.query_map([key], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The pairs with a merge decision that hold `name_key`: `(anchor, a, b)`.
pub(crate) fn edges_of(
    conn: &Connection,
    name_key: &str,
) -> Result<Vec<(String, String, String)>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT anchor, a, b FROM merge_edge WHERE a = ?1 OR b = ?1")?;
    let rows = stmt.query_map([name_key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Whether `name_key` has a mention that was not moved out of it.
pub(crate) fn name_present(conn: &Connection, name_key: &str) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT 1 FROM entity_mention m WHERE {} LIMIT 1",
            Scope::Single.filter()
        ))?
        .query_row([name_key], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Whether the entity `key` has a mention written `name`.
pub(crate) fn name_in_scope(
    conn: &Connection,
    key: &str,
    scope: Scope,
    name: &str,
) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT 1 FROM entity_mention m WHERE {} AND m.raw = ?2 LIMIT 1",
            scope.filter()
        ))?
        .query_row(params![key, name], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Whether a stored mention has a key `split` lists: a split's anchor is there (INV-CU-04).
pub(crate) fn split_present(conn: &Connection, split: &str) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached("SELECT 1 FROM split_anchor WHERE split_key = ?1 AND present = 1 LIMIT 1")?
        .query_row([split], |_| Ok(()))
        .optional()?
        .is_some())
}

/// The alias decisions under `key`, per alias anchor. Reads the `decision_anchor` index: every alias
/// anchor of `key` starts `key#alias:`, and `;` follows `:`.
pub(crate) fn alias_decisions(
    conn: &Connection,
    key: &str,
) -> Result<BTreeMap<String, Vec<DecisionFact>>, StoreError> {
    let from = alias_anchor(key, "");
    let to = format!("{};", &from[..from.len() - 1]);
    let mut stmt = conn.prepare_cached(
        "SELECT seq, kind, anchor FROM decision
         WHERE kind IN (?1, ?2) AND anchor >= ?3 AND anchor < ?4",
    )?;
    let rows = stmt.query_map(
        params![
            DecisionKind::RemoveAlias.as_str(),
            DecisionKind::RestoreAlias.as_str(),
            from,
            to
        ],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut out: BTreeMap<String, Vec<DecisionFact>> = BTreeMap::new();
    for row in rows {
        let (seq, kind, anchor) = row?;
        if let Some(kind) = DecisionKind::from_name(&kind) {
            out.entry(anchor)
                .or_default()
                .push(DecisionFact { seq, kind });
        }
    }
    Ok(out)
}

/// Every subject a decision in the log concerns.
pub(crate) fn decision_subjects(
    conn: &Connection,
    keys: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare("SELECT DISTINCT kind, anchor FROM decision")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (kind, anchor) = row?;
        let kind = DecisionKind::from_name(&kind)
            .ok_or_else(|| StoreError::Invalid(format!("unknown decision kind {kind}")))?;
        keys.extend(anchor_subjects(kind, &anchor));
    }
    Ok(())
}

const ENTITY_GENERATION: &str = "entity_generation";
const PROPOSALS_GENERATION: &str = "proposals_generation";

/// Entities changed: the proposal lane looks at this (ADR-0005: proposals may lag).
pub(crate) fn bump_entity_generation(conn: &Connection) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO project_meta (key, value) VALUES (?1, '1')
         ON CONFLICT (key) DO UPDATE SET value = CAST(value AS INTEGER) + 1",
    )?
    .execute([ENTITY_GENERATION])?;
    Ok(())
}

fn generation(conn: &Connection, key: &str) -> Result<u64, StoreError> {
    Ok(documents::meta(conn, key)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// `(entities changed so far, what the stored proposals were made from)`.
pub(crate) fn proposal_generations(conn: &Connection) -> Result<(u64, u64), StoreError> {
    Ok((
        generation(conn, ENTITY_GENERATION)?,
        generation(conn, PROPOSALS_GENERATION)?,
    ))
}

/// Rewrites the merge edges and groups from the log. One decision changes the groups of its own pair
/// only, and only a group's key has rows, so [`apply_decision`] knows what to rebuild from the pair.
fn regroup(conn: &Connection) -> Result<(), StoreError> {
    let decisions: Vec<(i64, String, String)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT seq, kind, anchor FROM decision WHERE kind IN (?1, ?2) ORDER BY seq",
        )?;
        let rows = stmt.query_map(
            [
                DecisionKind::AcceptMerge.as_str(),
                DecisionKind::RejectMerge.as_str(),
            ],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        rows.collect::<Result<_, _>>()?
    };
    let newest =
        mk_graph::entity::newest_merges(decisions.iter().filter_map(|(seq, kind, a)| {
            DecisionKind::from_name(kind).map(|k| (*seq, k, a.as_str()))
        }));
    conn.prepare_cached("DELETE FROM merge_edge")?.execute([])?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO merge_edge (anchor, a, b, accepted) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (anchor, accepted) in &newest {
        let Some((a, b)) = merge_pair(anchor) else {
            continue;
        };
        insert.execute(params![anchor, a, b, accepted])?;
    }
    let new = group_members(
        newest
            .iter()
            .filter(|(_, on)| **on)
            .filter_map(|(anchor, _)| merge_pair(anchor)),
    );
    let old: BTreeMap<String, String> = {
        let mut stmt = conn.prepare_cached("SELECT name_key, entity_key FROM entity_member")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut set = conn.prepare_cached("INSERT OR REPLACE INTO entity_member VALUES (?1, ?2)")?;
    let mut unset = conn.prepare_cached("DELETE FROM entity_member WHERE name_key = ?1")?;
    for (key, root) in &new {
        if old.get(key) != Some(root) {
            set.execute(params![key, root])?;
        }
    }
    for key in old.keys() {
        if !new.contains_key(key) {
            unset.execute([key])?;
        }
    }
    Ok(())
}

/// Rewrites the rows of split anchor `split` from the log: the mention keys of its newest split, and
/// whether a later undo took it back. Their stored presence is set by [`refresh_splits`].
fn sync_split(conn: &Connection, split: &str) -> Result<BTreeSet<ContentHash>, StoreError> {
    let (mut seq, mut active, mut keys) = (0, false, String::new());
    {
        let mut stmt = conn.prepare_cached(
            "SELECT seq, kind, after FROM decision WHERE kind IN (?1, ?2) AND anchor = ?3
             ORDER BY seq",
        )?;
        let mut rows = stmt.query(params![
            DecisionKind::Split.as_str(),
            DecisionKind::UndoSplit.as_str(),
            split
        ])?;
        while let Some(r) = rows.next()? {
            if r.get::<_, String>(1)? == DecisionKind::Split.as_str() {
                seq = r.get(0)?;
                keys = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                active = true;
            } else {
                active = false;
            }
        }
    }
    let mut contents = BTreeSet::new();
    conn.prepare_cached("DELETE FROM split_anchor WHERE split_key = ?1")?
        .execute([split])?;
    let mut insert = conn.prepare_cached(
        "INSERT OR IGNORE INTO split_anchor (split_key, mention_key, content_hash, seq, active,
                                             present)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
    )?;
    for key in split_keys(&keys) {
        let parsed: MentionKey = key
            .parse()
            .map_err(|e: mk_kernel::ParseMentionKeyError| StoreError::Invalid(e.to_string()))?;
        insert.execute(params![split, key, parsed.content.to_hex(), seq, active])?;
        contents.insert(parsed.content);
    }
    Ok(contents)
}

const MENTION_KEY_SQL: &str = "
    SELECT m.id, m.name_key, c.hash, d.position, s.loc_kind, s.loc_pos, s.loc_ref, m.span_start,
           m.span_end, m.raw
    FROM entity_mention m
    JOIN segment s ON s.id = m.segment_id
    JOIN document d ON d.id = s.document_id
    JOIN content c ON c.id = m.content_id";

/// A mention's text-form key from the columns of [`MENTION_KEY_SQL`].
fn mention_key_from(r: &rusqlite::Row<'_>) -> Result<String, StoreError> {
    Ok(MentionKey {
        content: codec::hash_from(&r.get::<_, Vec<u8>>(2)?)?,
        document: r.get(3)?,
        loc_kind: r.get(4)?,
        loc_pos: r.get(5)?,
        loc_ref: r.get(6)?,
        span: (r.get::<_, i64>(7)? as u32, r.get::<_, i64>(8)? as u32),
        evidence: r.get(9)?,
    }
    .to_string())
}

/// `(mention id, text-form key)` of every stored entity mention, for the reference model.
pub(crate) fn all_mention_keys(conn: &Connection) -> Result<BTreeMap<i64, String>, StoreError> {
    let mut stmt = conn.prepare(MENTION_KEY_SQL)?;
    let mut rows = stmt.query([])?;
    let mut out = BTreeMap::new();
    while let Some(r) = rows.next()? {
        out.insert(r.get(0)?, mention_key_from(r)?);
    }
    Ok(out)
}

/// Re-reads which mentions of a content sit in which split, after its mentions changed. The name keys
/// and split keys this moves go into `touched`. One probe when no split lists this content.
pub(crate) fn refresh_splits(
    conn: &Connection,
    content: i64,
    touched: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let any: bool = conn
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM split_anchor)")?
        .query_row([], |r| r.get(0))?;
    if !any {
        return Ok(());
    }
    let hash: Option<Vec<u8>> = conn
        .prepare_cached("SELECT hash FROM content WHERE id = ?1")?
        .query_row([content], |r| r.get(0))
        .optional()?;
    let Some(hash) = hash else { return Ok(()) };
    let hex = codec::hash_from(&hash)?.to_hex();
    {
        let mut old = conn.prepare_cached(
            "SELECT DISTINCT split_key, name_key FROM split_mention WHERE content_id = ?1",
        )?;
        for row in old.query_map([content], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (split, name) = row?;
            touched.insert(split);
            touched.insert(name);
        }
    }
    conn.prepare_cached("DELETE FROM split_mention WHERE content_id = ?1")?
        .execute([content])?;
    let listed: bool = {
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT split_key FROM split_anchor WHERE content_hash = ?1",
        )?;
        let mut rows = stmt.query([&hex])?;
        let mut any = false;
        while let Some(r) = rows.next()? {
            touched.insert(r.get(0)?);
            any = true;
        }
        any
    };
    if !listed {
        return Ok(());
    }
    conn.prepare_cached("UPDATE split_anchor SET present = 0 WHERE content_hash = ?1")?
        .execute([&hex])?;
    let mentions: Vec<(i64, String, String)> = {
        let mut stmt =
            conn.prepare_cached(&format!("{MENTION_KEY_SQL} WHERE m.content_id = ?1"))?;
        let mut rows = stmt.query([content])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, mention_key_from(r)?));
        }
        out
    };
    let mut mark =
        conn.prepare_cached("UPDATE split_anchor SET present = 1 WHERE mention_key = ?1")?;
    let mut find = conn.prepare_cached(
        "SELECT seq, split_key FROM split_anchor WHERE mention_key = ?1 AND active = 1",
    )?;
    let mut place = conn.prepare_cached(
        "INSERT INTO split_mention (mention_id, split_key, name_key, content_id)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (id, name, key) in mentions {
        if mark.execute([&key])? == 0 {
            continue;
        }
        let active: Vec<(i64, String)> = find
            .query_map([&key], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        if let Some(split) = split_for(active.iter().map(|(seq, k)| (*seq, k.as_str()))) {
            place.execute(params![id, split, name, content])?;
            touched.insert(split.to_string());
            touched.insert(name);
        }
    }
    Ok(())
}

/// Rewrites groups and splits from the log (a full rebuild).
pub(crate) fn rebuild_membership(conn: &Connection, entities: bool) -> Result<(), StoreError> {
    regroup(conn)?;
    let splits: Vec<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT anchor FROM decision WHERE kind = ?1")?;
        let rows = stmt.query_map([DecisionKind::Split.as_str()], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut contents = BTreeSet::new();
    for split in &splits {
        contents.extend(sync_split(conn, split)?);
    }
    if entities {
        let mut touched = BTreeSet::new();
        refresh_contents(conn, &contents, &mut touched)?;
    }
    Ok(())
}

/// [`refresh_splits`] for each of `hashes` still stored.
fn refresh_contents(
    conn: &Connection,
    hashes: &BTreeSet<ContentHash>,
    touched: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    let mut find = conn.prepare_cached("SELECT id FROM content WHERE hash = ?1")?;
    for hash in hashes {
        let id: Option<i64> = find
            .query_row([&hash.as_bytes()[..]], |r| r.get(0))
            .optional()?;
        if let Some(id) = id {
            refresh_splits(conn, id, touched)?;
        }
    }
    Ok(())
}

/// A logged decision takes effect: what it regroups or moves, and the subjects to rebuild.
pub(crate) fn apply_decision(
    conn: &Connection,
    kind: DecisionKind,
    anchor: &str,
) -> Result<BTreeSet<String>, StoreError> {
    let mut touched: BTreeSet<String> = anchor_subjects(kind, anchor).into_iter().collect();
    match kind.family() {
        DecisionFamily::Merge => {
            // A rebuild finds a name's holder as it is after this; only the one before needs adding.
            let mut holders = BTreeSet::new();
            for key in &touched {
                holders.insert(entity_key(conn, key)?);
            }
            regroup(conn)?;
            touched.extend(holders);
            drop_proposals(conn, &touched)?;
        }
        DecisionFamily::Split => {
            let hashes = sync_split(conn, anchor)?;
            refresh_contents(conn, &hashes, &mut touched)?;
            drop_proposals(conn, &touched)?;
        }
        DecisionFamily::ChangeType => {
            let mut all = touched.clone();
            for key in &touched {
                all.insert(entity_key(conn, key)?);
            }
            drop_proposals(conn, &all)?;
        }
        _ => {}
    }
    Ok(touched)
}

/// Removes the proposals a decision ended (domain §7.4 "Stale"). The proposal lane makes the others
/// again (ADR-0005).
fn drop_proposals(conn: &Connection, keys: &BTreeSet<String>) -> Result<(), StoreError> {
    let mut stmt = conn.prepare_cached(
        "DELETE FROM merge_proposal
         WHERE a = ?1 OR b = ?1 OR left_key = ?1 OR right_key = ?1",
    )?;
    for key in keys {
        stmt.execute([key])?;
    }
    Ok(())
}

fn no_entity() -> StoreError {
    StoreError::Invalid("This person or organization is no longer in the project".into())
}

fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::Invalid(msg.into())
}

/// An entity as a decision sees it.
struct Current {
    key: String,
    /// The anchor of decisions on it (domain §9.5).
    anchor: String,
    display: String,
    entity_type: String,
    note: Option<String>,
}

fn current(conn: &Connection, key: &str) -> Result<Current, StoreError> {
    let key = if is_split_key(key) {
        key.to_string()
    } else {
        entity_key(conn, key)?
    };
    let (display, display_key, entity_type, note): (String, String, String, Option<String>) = conn
        .prepare_cached(
            "SELECT display_name, display_key, entity_type, note FROM entity WHERE entity_key = ?1",
        )?
        .query_row([&key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .optional()?
        .ok_or_else(no_entity)?;
    Ok(Current {
        anchor: if is_split_key(&key) {
            key.clone()
        } else {
            display_key
        },
        key,
        display,
        entity_type,
        note,
    })
}

/// A decision's outcome.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecisionOutcome {
    pub seq: i64,
    pub connections_changed: bool,
}

/// The name and type chosen when accepting a proposal (US-11.3 AC4).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MergeEdit {
    pub name: Option<String>,
    pub entity_type: Option<EntityType>,
}

/// Accepts a pending proposal (US-11.2 AC2, INV-ER-03): the merge decision, the chosen edits and
/// everything they change, in one transaction. A pair that is no longer pending is refused.
pub(crate) fn accept_proposal(
    conn: &Connection,
    a: &str,
    b: &str,
    edit: &MergeEdit,
) -> Result<DecisionOutcome, StoreError> {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let pending: Option<(String, String)> = conn
        .prepare_cached("SELECT left_key, right_key FROM merge_proposal WHERE a = ?1 AND b = ?2")?
        .query_row([first, second], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    if pending.is_none() {
        return Err(invalid("This merge proposal is no longer pending"));
    }
    let (left, right) = (current(conn, first)?, current(conn, second)?);
    if left.key == right.key {
        return Err(invalid("These two are already one entity"));
    }
    let counts = |key: &str| -> Result<i64, StoreError> {
        Ok(conn
            .prepare_cached("SELECT mention_count FROM entity WHERE entity_key = ?1")?
            .query_row([key], |r| r.get(0))?)
    };
    let chosen = if counts(&left.key)? >= counts(&right.key)? {
        &left.display
    } else {
        &right.display
    };
    let before = format!("{} / {}", left.display, right.display);
    let (seq, mut changed) = ops::append_decision(
        conn,
        DecisionKind::AcceptMerge,
        &merge_anchor(first, second),
        Some(&before),
        Some(edit.name.as_deref().unwrap_or(chosen)),
    )?;
    if let Some(name) = &edit.name {
        changed |= edit_entity(conn, first, &EntityEdit::Rename(name.clone()))?.connections_changed;
    }
    if let Some(t) = edit.entity_type {
        changed |= edit_entity(conn, first, &EntityEdit::ChangeType(t))?.connections_changed;
    }
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// Rejects a pair (US-11.3 AC3, INV-ER-04): never proposed again, and its two sides stay apart.
pub(crate) fn reject_merge(
    conn: &Connection,
    a: &str,
    b: &str,
) -> Result<DecisionOutcome, StoreError> {
    check_pair(a, b)?;
    let (seq, changed) = ops::append_decision(
        conn,
        DecisionKind::RejectMerge,
        &merge_anchor(a, b),
        Some("proposed"),
        Some("rejected"),
    )?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// Undoes an accepted merge (US-11.4 AC1): a new decision storing the pair as rejected.
pub(crate) fn undo_merge(
    conn: &Connection,
    a: &str,
    b: &str,
) -> Result<DecisionOutcome, StoreError> {
    check_pair(a, b)?;
    let anchor = merge_anchor(a, b);
    let accepted: Option<bool> = conn
        .prepare_cached("SELECT accepted FROM merge_edge WHERE anchor = ?1")?
        .query_row([&anchor], |r| r.get(0))
        .optional()?;
    if accepted != Some(true) {
        return Err(invalid("These two were not merged"));
    }
    let (seq, changed) = ops::append_decision(
        conn,
        DecisionKind::RejectMerge,
        &anchor,
        Some("merged"),
        Some("rejected"),
    )?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

fn check_pair(a: &str, b: &str) -> Result<(), StoreError> {
    let parse = |k: &str| {
        k.parse::<NameKey>()
            .map_err(|_| invalid(format!("{k} is not a name key")))
    };
    let (ka, kb) = (parse(a)?, parse(b)?);
    if ka == kb {
        return Err(invalid("A name cannot be merged with itself"));
    }
    Ok(())
}

/// Moves mentions of an entity to a new entity (US-11.4 AC2): one decision listing their keys.
/// `mentions` are ids as an occurrence row has them.
pub(crate) fn split_mentions(
    conn: &Connection,
    key: &str,
    mentions: &[i64],
) -> Result<DecisionOutcome, StoreError> {
    let entity = current(conn, key)?;
    let scope = Scope::lookup(conn, &entity.key)?;
    let ids: BTreeSet<i64> = mentions.iter().map(|m| m.abs()).collect();
    if ids.is_empty() {
        return Err(invalid("Pick the mentions to split out"));
    }
    let mut keys = BTreeSet::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "{MENTION_KEY_SQL} WHERE m.id = ?2 AND ({})",
            scope.filter()
        ))?;
        for id in &ids {
            let mut rows = stmt.query(params![entity.key, id])?;
            let row = rows.next()?.ok_or_else(|| {
                invalid("A picked mention is not one of this person's or organization's")
            })?;
            keys.insert(mention_key_from(row)?);
        }
    }
    let total: i64 = conn
        .prepare_cached("SELECT mention_count FROM entity WHERE entity_key = ?1")?
        .query_row([&entity.key], |r| r.get(0))?;
    if ids.len() as i64 >= total {
        return Err(invalid("Keep at least one mention in the entity"));
    }
    let list: Vec<&str> = keys.iter().map(String::as_str).collect();
    let anchor = MentionKey::split_key(list.iter().copied())
        .ok_or_else(|| invalid("Pick the mentions to split out"))?;
    let (seq, changed) = ops::append_decision(
        conn,
        DecisionKind::Split,
        &anchor,
        Some(&entity.display),
        Some(&list.join("\n")),
    )?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// Puts a split group's mentions back (US-11.4): a new decision on its anchor.
pub(crate) fn undo_split(conn: &Connection, split: &str) -> Result<DecisionOutcome, StoreError> {
    if !is_split_key(split) {
        return Err(invalid("This was not split out"));
    }
    let active: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM split_anchor WHERE split_key = ?1 AND active = 1)",
        )?
        .query_row([split], |r| r.get(0))?;
    if !active {
        return Err(invalid("This was not split out"));
    }
    let (seq, changed) = ops::append_decision(
        conn,
        DecisionKind::UndoSplit,
        split,
        Some("split"),
        Some("put back"),
    )?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// A user's change to an entity (US-12.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntityEdit {
    Rename(String),
    ChangeType(EntityType),
    /// An empty note clears it.
    Note(String),
}

/// Applies an edit to the entity holding `key` (US-12.2), anchored per domain §9.5.
pub(crate) fn edit_entity(
    conn: &Connection,
    key: &str,
    edit: &EntityEdit,
) -> Result<DecisionOutcome, StoreError> {
    let entity = current(conn, key)?;
    let (kind, before, after) = match edit {
        EntityEdit::Rename(name) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(invalid("A name cannot be empty"));
            }
            (
                DecisionKind::Rename,
                Some(entity.display.clone()),
                name.to_string(),
            )
        }
        EntityEdit::ChangeType(t) => (
            DecisionKind::ChangeType,
            Some(entity.entity_type.clone()),
            t.as_str().to_string(),
        ),
        EntityEdit::Note(note) => (
            DecisionKind::SetNote,
            entity.note.clone(),
            note.trim().to_string(),
        ),
    };
    let (seq, changed) =
        ops::append_decision(conn, kind, &entity.anchor, before.as_deref(), Some(&after))?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// Removes an alias from an entity's names, or brings it back (US-12.2 AC1).
pub(crate) fn set_alias_removed(
    conn: &Connection,
    key: &str,
    alias: &str,
    removed: bool,
) -> Result<DecisionOutcome, StoreError> {
    let entity = current(conn, key)?;
    let scope = Scope::lookup(conn, &entity.key)?;
    if !name_in_scope(conn, &entity.key, scope, alias)? {
        return Err(invalid("This name is not one of its aliases"));
    }
    if removed && alias == entity.display {
        return Err(invalid("This is its main name: rename it first"));
    }
    let kind = if removed {
        DecisionKind::RemoveAlias
    } else {
        DecisionKind::RestoreAlias
    };
    let (seq, changed) = ops::append_decision(
        conn,
        kind,
        &alias_anchor(&entity.anchor, alias),
        Some(if removed { "alias" } else { "removed" }),
        Some(if removed { "removed" } else { "alias" }),
    )?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// Ignores or un-ignores an entity connection (US-07.4), anchored per domain §9.5. An entity that is
/// gone keeps the decision on the given key, orphaned (INV-CU-04).
pub(crate) fn set_entity_ignored(
    conn: &Connection,
    key: &str,
    ignored: bool,
) -> Result<DecisionOutcome, StoreError> {
    let anchor = current(conn, key).map_or_else(|_| key.to_string(), |e| e.anchor);
    let kind = if ignored {
        DecisionKind::IgnoreEntity
    } else {
        DecisionKind::UnignoreEntity
    };
    let (seq, changed) = ops::append_decision(
        conn,
        kind,
        &anchor,
        Some(if ignored { "shown" } else { "ignored" }),
        Some(if ignored { "ignored" } else { "shown" }),
    )?;
    Ok(DecisionOutcome {
        seq,
        connections_changed: changed,
    })
}

/// What a proposal run reads, in one read transaction.
pub struct ProposalInput {
    /// The entity generation these were read at.
    pub generation: u64,
    pub candidates: Vec<Candidate>,
    /// Entity-key pairs a rejected name-key pair stands between, in key order (INV-ER-04).
    pub blocked: BTreeSet<(String, String)>,
}

fn blocked_pairs(
    conn: &Connection,
    root: &impl Fn(&str) -> String,
) -> Result<BTreeSet<(String, String)>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT a, b FROM merge_edge WHERE accepted = 0")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = BTreeSet::new();
    for row in rows {
        let (a, b) = row?;
        let (ra, rb) = (root(&a), root(&b));
        out.insert(if ra <= rb { (ra, rb) } else { (rb, ra) });
    }
    Ok(out)
}

fn all_members(conn: &Connection) -> Result<BTreeMap<String, String>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT name_key, entity_key FROM entity_member")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Reads what a proposal run needs: the name-key entities with a mention shown by default (split
/// groups are not proposed, ASSUMPTION M7a), their documents and their hard identifiers (D7 links).
pub(crate) fn proposal_input(conn: &Connection) -> Result<ProposalInput, StoreError> {
    let missing = IngestionState::Missing.as_str();
    let members = all_members(conn)?;
    let root = |k: &str| members.get(k).cloned().unwrap_or_else(|| k.to_string());
    let mut entities: BTreeMap<String, Candidate> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.name_key, MIN(m.document_id), MAX(m.document_id)
             FROM entity_mention m JOIN content c ON c.id = m.content_id
             WHERE c.state != ?1 AND m.confidence >= ?2
               AND NOT EXISTS (SELECT 1 FROM split_mention x WHERE x.mention_id = m.id)
             GROUP BY m.name_key",
        )?;
        let mut rows = stmt.query(params![missing, f64::from(LOW_CONFIDENCE)])?;
        while let Some(r) = rows.next()? {
            let name: String = r.get(0)?;
            let (low, high): (i64, i64) = (r.get(1)?, r.get(2)?);
            let key = root(&name);
            let c = entities.entry(key.clone()).or_insert_with(|| Candidate {
                key,
                entity_type: EntityType::Person,
                name_keys: Vec::new(),
                documents: Vec::new(),
                identifiers: BTreeMap::new(),
            });
            c.name_keys.push(name);
            for d in [low, high] {
                c.documents.push(d);
            }
            c.documents.sort_unstable();
            c.documents.dedup();
            if c.documents.len() > 2 {
                let last = c.documents[c.documents.len() - 1];
                c.documents.truncate(1);
                c.documents.push(last);
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT entity_key, entity_type FROM entity WHERE entity_key NOT LIKE 'split:%'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let key: String = r.get(0)?;
            if let Some(c) = entities.get_mut(&key) {
                let t: String = r.get(1)?;
                c.entity_type = EntityType::from_name(&t).unwrap_or(EntityType::Person);
            }
        }
    }
    let mut candidates: Vec<Candidate> = entities.into_values().collect();
    let index: HashMap<String, u32> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (c.key.clone(), i as u32))
        .collect();
    // Held by identifier, not by entity: most identifiers belong to one entity and can pair nothing,
    // and on TD-2 there are millions of links.
    let mut links: HashMap<String, Vec<(u32, i64)>> = HashMap::new();
    let mut link = |name: &str, identifier: String, document: i64| {
        if !is_hard_identifier(&identifier) {
            return;
        }
        let entity = members.get(name).map_or(name, String::as_str);
        if let Some(&i) = index.get(entity) {
            links.entry(identifier).or_default().push((i, document));
        }
    };
    {
        let mut stmt = conn.prepare(
            "SELECT l.name_key, l.identifier_key, l.document_id
             FROM record_link l JOIN content c ON c.id = l.content_id
             WHERE c.state != ?1
             GROUP BY l.name_key, l.identifier_key, l.document_id",
        )?;
        let mut rows = stmt.query([missing])?;
        while let Some(r) = rows.next()? {
            link(&r.get::<_, String>(0)?, r.get(1)?, r.get(2)?);
        }
    }
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT m.name_key, i.identifier_key, i.document_id,
                    substr(s.text, m.span_end + 1, i.span_start - m.span_end)
             FROM entity_mention m
             JOIN segment s ON s.id = m.segment_id
             JOIN content c ON c.id = m.content_id
             JOIN identifier_mention i ON i.segment_id = m.segment_id AND i.span_start >= m.span_end
             WHERE m.producer = 'model' AND s.loc_kind = '{}' AND c.state != ?1 AND m.confidence >= ?2",
            codec::EMAIL_HEADER,
        ))?;
        let mut rows = stmt.query(params![missing, f64::from(LOW_CONFIDENCE)])?;
        while let Some(r) = rows.next()? {
            let between: String = r.get(3)?;
            if mk_graph::is_header_pair(&between) {
                link(&r.get::<_, String>(0)?, r.get(1)?, r.get(2)?);
            }
        }
    }
    for (identifier, mut held) in links {
        held.sort_unstable();
        let holders = held.chunk_by(|x, y| x.0 == y.0).count();
        if !(2..=MAX_ENTITIES_PER_IDENTIFIER).contains(&holders) {
            continue;
        }
        for (i, document) in held {
            candidates[i as usize]
                .identifiers
                .entry(identifier.clone())
                .or_default()
                .insert(document);
        }
    }
    let generation = generation(conn, ENTITY_GENERATION)?;
    let blocked = blocked_pairs(conn, &root)?;
    Ok(ProposalInput {
        generation,
        candidates,
        blocked,
    })
}

/// Replaces the stored proposals in one short transaction, filtering them again against the decisions
/// as they are now, so a rejected or merged pair never shows even for a moment (INV-ER-04). Skipped
/// when newer ones are stored already. Returns how many are stored.
pub(crate) fn replace_proposals(
    conn: &Connection,
    from_generation: u64,
    proposals: &[Proposal],
) -> Result<Option<usize>, StoreError> {
    if generation(conn, PROPOSALS_GENERATION)? > from_generation {
        return Ok(None);
    }
    let members = all_members(conn)?;
    let root = |k: &str| members.get(k).cloned().unwrap_or_else(|| k.to_string());
    let blocked = blocked_pairs(conn, &root)?;
    conn.prepare_cached("DELETE FROM merge_proposal")?
        .execute([])?;
    let mut insert = conn.prepare_cached(
        "INSERT OR REPLACE INTO merge_proposal (a, b, left_key, right_key, score, reasons)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut stored = 0;
    for p in proposals {
        let (left, right) = (root(&p.a), root(&p.b));
        if left == right {
            continue;
        }
        let pair = if left <= right {
            (left.clone(), right.clone())
        } else {
            (right.clone(), left.clone())
        };
        if blocked.contains(&pair) {
            continue;
        }
        insert.execute(params![
            p.a,
            p.b,
            left,
            right,
            f64::from(p.score),
            p.reasons.join("\n")
        ])?;
        stored += 1;
    }
    manage::set_meta(
        conn,
        PROPOSALS_GENERATION,
        Some(&from_generation.to_string()),
    )?;
    Ok(Some(stored))
}

/// A pending proposal, as the queue will list it (US-11.3).
#[derive(Clone, PartialEq, Debug)]
pub struct ProposalRow {
    pub a: String,
    pub b: String,
    pub left_key: String,
    pub right_key: String,
    pub left_name: String,
    pub right_name: String,
    pub score: f32,
    pub reasons: Vec<String>,
}

/// Where a proposals page continues: the last row loaded.
#[derive(Clone, PartialEq, Debug)]
pub struct ProposalCursor {
    pub score: f32,
    pub a: String,
    pub b: String,
}

/// One page of pending proposals, highest score first (US-11.3 AC1). A proposal's side that is gone
/// is not shown (domain §7.4 "Stale").
pub(crate) fn proposals_page(
    conn: &Connection,
    after: Option<&ProposalCursor>,
    limit: usize,
) -> Result<Vec<ProposalRow>, StoreError> {
    let map = |r: &rusqlite::Row<'_>| {
        Ok(ProposalRow {
            a: r.get(0)?,
            b: r.get(1)?,
            left_key: r.get(2)?,
            right_key: r.get(3)?,
            left_name: r.get(6)?,
            right_name: r.get(7)?,
            score: r.get::<_, f64>(4)? as f32,
            reasons: r
                .get::<_, String>(5)?
                .split('\n')
                .map(str::to_string)
                .collect(),
        })
    };
    const SELECT: &str = "SELECT p.a, p.b, p.left_key, p.right_key, p.score, p.reasons,
                                 l.display_name, r.display_name
                          FROM merge_proposal p
                          JOIN entity l ON l.entity_key = p.left_key
                          JOIN entity r ON r.entity_key = p.right_key";
    const ORDER: &str = "ORDER BY p.score DESC, p.a, p.b LIMIT";
    let rows = match after {
        None => {
            let mut stmt = conn.prepare_cached(&format!("{SELECT} {ORDER} ?1"))?;
            stmt.query_map([limit as i64], map)?
                .collect::<Result<_, _>>()?
        }
        Some(c) => {
            let mut stmt = conn.prepare_cached(&format!(
                "{SELECT} WHERE (-p.score, p.a, p.b) > (-?1, ?2, ?3) {ORDER} ?4"
            ))?;
            stmt.query_map(params![f64::from(c.score), c.a, c.b, limit as i64], map)?
                .collect::<Result<_, _>>()?
        }
    };
    Ok(rows)
}

/// Those of `keys` with a pending proposal: the "Unreviewed merge" mark (D6, domain §9.1). Read per
/// page, because proposals may lag behind connections (ADR-0005).
pub(crate) fn unreviewed_keys(
    conn: &Connection,
    keys: &[String],
) -> Result<BTreeSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM merge_proposal WHERE left_key = ?1 OR right_key = ?1)",
    )?;
    let mut out = BTreeSet::new();
    for key in keys {
        if stmt.query_row([key], |r| r.get::<_, bool>(0))? {
            out.insert(key.clone());
        }
    }
    Ok(out)
}

/// An entity's names (INV-ER-07): how often each is written; those a decision removed from the
/// aliases apart.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct EntityNames {
    pub display: String,
    /// `(name as written, mentions)`, most frequent first, the display name left out.
    pub aliases: Vec<(String, u64)>,
    pub removed: Vec<(String, u64)>,
}

/// The names of the entity holding `key`, read at query time. `None` if it is gone.
pub(crate) fn entity_names(
    conn: &Connection,
    key: &str,
) -> Result<Option<EntityNames>, StoreError> {
    let Ok(entity) = current(conn, key) else {
        return Ok(None);
    };
    let scope = Scope::lookup(conn, &entity.key)?;
    let mut newest: BTreeMap<String, (i64, DecisionKind)> = BTreeMap::new();
    for anchor in anchors_of(conn, &entity.key, scope)? {
        for (alias_at, decisions) in alias_decisions(conn, &anchor)? {
            let Some((_, alias)) = mk_graph::entity::alias_parts(&alias_at) else {
                continue;
            };
            for d in decisions {
                let at = newest.entry(alias.to_string()).or_insert((d.seq, d.kind));
                if d.seq >= at.0 {
                    *at = (d.seq, d.kind);
                }
            }
        }
    }
    let mut out = EntityNames {
        display: entity.display.clone(),
        ..EntityNames::default()
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT m.raw, COUNT(*) FROM entity_mention m WHERE {}
         GROUP BY m.raw ORDER BY COUNT(*) DESC, m.raw",
        scope.filter()
    ))?;
    let rows = stmt.query_map([&entity.key], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
    })?;
    for row in rows {
        let (name, count) = row?;
        if name == entity.display {
            continue;
        }
        let gone = newest
            .get(&name)
            .is_some_and(|(_, k)| *k == DecisionKind::RemoveAlias);
        if gone {
            out.removed.push((name, count));
        } else {
            out.aliases.push((name, count));
        }
    }
    Ok(Some(out))
}

/// A decision as the log shows it (US-11.5 AC2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecisionRow {
    pub seq: i64,
    pub kind: DecisionKind,
    pub anchor: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub decided_at: String,
    /// Active, Superseded or Orphaned; `None` for a mapping decision, whose state is its file's
    /// mapping state.
    pub status: Option<mk_graph::DecisionStatus>,
}

/// A page of the decision log, newest first, older than `before` (`None`: from the newest).
pub(crate) fn decisions_page(
    conn: &Connection,
    before: Option<i64>,
    limit: usize,
) -> Result<Vec<DecisionRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.seq, d.kind, d.anchor, d.before, d.after, d.decided_at, s.status
         FROM decision d LEFT JOIN decision_status s ON s.seq = d.seq
         WHERE d.seq < ?1 ORDER BY d.seq DESC LIMIT ?2",
    )?;
    let mut rows = stmt.query(params![before.unwrap_or(i64::MAX), limit as i64])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let kind: String = r.get(1)?;
        let status: Option<String> = r.get(6)?;
        out.push(DecisionRow {
            seq: r.get(0)?,
            kind: DecisionKind::from_name(&kind)
                .ok_or_else(|| StoreError::Invalid(format!("unknown decision kind {kind}")))?,
            anchor: r.get(2)?,
            before: r.get(3)?,
            after: r.get(4)?,
            decided_at: r.get(5)?,
            status: status
                .as_deref()
                .and_then(mk_graph::DecisionStatus::from_name),
        });
    }
    Ok(out)
}
