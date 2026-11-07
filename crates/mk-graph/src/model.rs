//! An in-memory reference of the whole derived state (ADR-0005 follow-up): the rules of this
//! crate applied to every key at once, with no SQL and no incremental bookkeeping. The property
//! test in `mk-core` checks the store's incremental commits and its full rebuild against it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mk_kernel::{IdentifierKey, NameKey, is_split_key};

use crate::entity::{
    EntityDecision, alias_parts, entity_edits, group_members, merge_pair, newest_merges, split_for,
    split_keys,
};
use crate::{
    ConnectionRow, DecisionFact, DecisionFamily, DecisionKind, DecisionStatus, EntityCounts,
    EntityStats, IdentifierStats, LOW_CONFIDENCE, StopList, SubjectKind, connection_for,
    display_name, entity_connection_for, resolve,
};

/// A stored content as the derived rules see it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContentFact {
    pub id: i64,
    pub file_type: String,
    /// False when the content is Missing (INV-CO-07).
    pub counted: bool,
}

/// A stored identifier mention.
#[derive(Clone, PartialEq, Debug)]
pub struct MentionFact {
    pub content: i64,
    pub document: i64,
    pub key: IdentifierKey,
    pub confidence: f32,
}

/// A stored entity mention (M4c: from a confirmed table column).
#[derive(Clone, PartialEq, Debug)]
pub struct EntityMentionFact {
    pub id: i64,
    /// Its [`mk_kernel::MentionKey`] as text: what a split decision lists.
    pub mention_key: String,
    pub content: i64,
    pub document: i64,
    pub key: NameKey,
    /// The name as written.
    pub raw: String,
    pub confidence: f32,
    /// Made from a table column (US-05.3).
    pub from_table: bool,
}

/// A logged decision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecisionRecord {
    pub seq: i64,
    pub kind: DecisionKind,
    /// The anchor (domain §9.5).
    pub anchor: String,
    /// The value it set: a new name, a type, a note, a split's mention keys.
    pub after: Option<String>,
}

/// Everything derived data is built from: machine output, decisions and the stop list
/// (a project setting, INV-LK-03).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Facts {
    pub contents: Vec<ContentFact>,
    pub mentions: Vec<MentionFact>,
    pub entity_mentions: Vec<EntityMentionFact>,
    pub decisions: Vec<DecisionRecord>,
    pub stop_list: StopList,
}

/// A derived identifier row (domain §5.1 `IDENTIFIER`): present while any mention of the key is
/// stored, Missing content included.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentifierRow {
    pub kind: String,
    pub value: String,
    pub mention_count: u64,
}

/// A derived entity row (domain §5.1 `ENTITY`, M4c): present while any of its mentions is stored,
/// Missing content included (INV-ER-08).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntityRow {
    /// The type a user set, else its display name's type (INV-ER-07).
    pub entity_type: String,
    /// The name a user set, else the most frequent name as written (INV-ER-07).
    pub display_name: String,
    /// The name key of the most frequent name as written: the anchor of decisions on a name-key
    /// entity (domain §9.5).
    pub display_key: String,
    /// A user's note (US-12.2 AC2).
    pub note: Option<String>,
    pub mention_count: u64,
    /// Every mention comes from a table column: "Name (from table)" (US-05.3, domain §9.1).
    pub from_table: bool,
}

/// Every derived row of a project, keyed for comparison.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DerivedState {
    pub identifiers: BTreeMap<String, IdentifierRow>,
    pub entities: BTreeMap<String, EntityRow>,
    pub connections: BTreeMap<String, ConnectionRow>,
    pub decision_status: BTreeMap<i64, DecisionStatus>,
    /// Name key → entity key, for groups of two or more.
    pub entity_members: BTreeMap<String, String>,
    /// Every merge anchor → whether its newest decision accepts it.
    pub merge_edges: BTreeMap<String, bool>,
    /// (split key, mention key) → (the split stands, the mention is stored).
    pub split_anchors: BTreeMap<(String, String), (bool, bool)>,
    /// Mention id → (split key, name key), for mentions moved out.
    pub split_mentions: BTreeMap<i64, (String, String)>,
}

/// What the merge and split decisions say, read once.
struct Membership<'a> {
    /// Every pair with a merge decision: anchor → accepted.
    edges: BTreeMap<&'a str, bool>,
    /// Name key → entity key, groups of two or more.
    members: BTreeMap<String, String>,
    /// Split key → its mention keys (of its newest split) and whether that split stands.
    splits: BTreeMap<&'a str, (BTreeSet<&'a str>, Option<i64>)>,
}

fn membership(facts: &Facts) -> Membership<'_> {
    let edges = newest_merges(
        facts
            .decisions
            .iter()
            .map(|d| (d.seq, d.kind, d.anchor.as_str())),
    );
    let members = group_members(
        edges
            .iter()
            .filter(|(_, on)| **on)
            .filter_map(|(anchor, _)| merge_pair(anchor)),
    );
    let mut splits: BTreeMap<&str, (BTreeSet<&str>, Option<i64>)> = BTreeMap::new();
    let mut ordered: Vec<&DecisionRecord> = facts
        .decisions
        .iter()
        .filter(|d| d.kind.family() == DecisionFamily::Split)
        .collect();
    ordered.sort_by_key(|d| d.seq);
    for d in ordered {
        let entry = splits.entry(&d.anchor).or_default();
        if d.kind == DecisionKind::Split {
            entry.0 = split_keys(d.after.as_deref().unwrap_or("")).collect();
            entry.1 = Some(d.seq);
        } else {
            entry.1 = None;
        }
    }
    Membership {
        edges,
        members,
        splits,
    }
}

/// The derived state for `facts`, computed from scratch.
pub fn reference(facts: &Facts) -> DerivedState {
    let contents: HashMap<i64, &ContentFact> = facts.contents.iter().map(|c| (c.id, c)).collect();
    let mut by_key: BTreeMap<String, (&IdentifierKey, Vec<&MentionFact>)> = BTreeMap::new();
    for m in &facts.mentions {
        by_key
            .entry(m.key.to_string())
            .or_insert_with(|| (&m.key, Vec::new()))
            .1
            .push(m);
    }
    let Membership {
        edges,
        members,
        splits,
    } = membership(facts);

    let mut out = DerivedState {
        entity_members: members.clone(),
        ..DerivedState::default()
    };
    out.merge_edges = edges
        .iter()
        .map(|(anchor, on)| (anchor.to_string(), *on))
        .collect();

    // Which stored mention keys exist, and each mention's entity (INV-ER-01).
    let stored: BTreeSet<&str> = facts
        .entity_mentions
        .iter()
        .map(|m| m.mention_key.as_str())
        .collect();
    for (key, (keys, active)) in &splits {
        for k in keys {
            out.split_anchors.insert(
                (key.to_string(), k.to_string()),
                (active.is_some(), stored.contains(k)),
            );
        }
    }
    let mut moved: HashMap<&str, Vec<(i64, &str)>> = HashMap::new();
    for (key, (keys, active)) in &splits {
        if let Some(seq) = active {
            for k in keys {
                moved.entry(k).or_default().push((*seq, key));
            }
        }
    }
    let root = |k: &str| members.get(k).cloned().unwrap_or_else(|| k.to_string());
    let mut by_entity: BTreeMap<String, Vec<&EntityMentionFact>> = BTreeMap::new();
    let mut unmoved_names: BTreeSet<String> = BTreeSet::new();
    for m in &facts.entity_mentions {
        let split = moved
            .get(m.mention_key.as_str())
            .and_then(|s| split_for(s.iter().copied()));
        match split {
            Some(s) => {
                out.split_mentions
                    .insert(m.id, (s.to_string(), m.key.to_string()));
                by_entity.entry(s.to_string()).or_default().push(m);
            }
            None => {
                unmoved_names.insert(m.key.to_string());
                by_entity
                    .entry(root(&m.key.to_string()))
                    .or_default()
                    .push(m);
            }
        }
    }

    // The decisions on each anchor, by the rule that reads them.
    let mut identifier_decisions: BTreeMap<&str, Vec<DecisionFact>> = BTreeMap::new();
    let mut entity_decisions: BTreeMap<String, Vec<&DecisionRecord>> = BTreeMap::new();
    for d in &facts.decisions {
        let fact = DecisionFact {
            seq: d.seq,
            kind: d.kind,
        };
        match d.kind.family() {
            DecisionFamily::Mapping => {}
            DecisionFamily::Merge => {
                let present = merge_pair(&d.anchor)
                    .is_some_and(|(a, b)| unmoved_names.contains(a) && unmoved_names.contains(b));
                push_status(&mut out, facts, d, present);
            }
            DecisionFamily::Split => {
                let present = splits
                    .get(d.anchor.as_str())
                    .is_some_and(|(keys, _)| keys.iter().any(|k| stored.contains(k)));
                push_status(&mut out, facts, d, present);
            }
            DecisionFamily::Alias => {
                let present = alias_parts(&d.anchor).is_some_and(|(key, alias)| {
                    let entity = if is_split_key(key) {
                        key.to_string()
                    } else {
                        root(key)
                    };
                    by_entity.get(&entity).is_some_and(|ms| {
                        ms.iter().any(|m| {
                            m.raw == alias
                                && (is_split_key(key)
                                    || !moved.contains_key(m.mention_key.as_str()))
                        })
                    })
                });
                push_status(&mut out, facts, d, present);
            }
            DecisionFamily::Ignore => {
                if !DecisionKind::ignore_pair(SubjectKind::of_key(&d.anchor)).contains(&d.kind) {
                    continue;
                }
                match SubjectKind::of_key(&d.anchor) {
                    SubjectKind::Identifier => identifier_decisions
                        .entry(&d.anchor)
                        .or_default()
                        .push(fact),
                    SubjectKind::Entity => {
                        let e = entity_of_anchor(&d.anchor, &root);
                        entity_decisions.entry(e).or_default().push(d);
                    }
                }
            }
            DecisionFamily::Rename | DecisionFamily::ChangeType | DecisionFamily::Note => {
                let e = entity_of_anchor(&d.anchor, &root);
                entity_decisions.entry(e).or_default().push(d);
            }
        }
    }

    for (key, mentions) in &by_entity {
        let decisions = entity_decisions.remove(key).unwrap_or_default();
        apply_entity_statuses(&mut out, &decisions, true);
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
        let auto = display_name(mentions.iter().map(|m| m.raw.as_str())).unwrap_or_default();
        let display_key = mentions
            .iter()
            .filter(|m| m.raw == auto)
            .map(|m| m.key.to_string())
            .min()
            .unwrap_or_default();
        let entity_type = edits.entity_type.map_or_else(
            || {
                display_key
                    .parse::<NameKey>()
                    .map(|k| k.entity_type().as_str().to_string())
                    .unwrap_or_default()
            },
            |t| t.as_str().to_string(),
        );
        out.entities.insert(
            key.clone(),
            EntityRow {
                entity_type,
                display_name: edits.name.clone().unwrap_or(auto),
                display_key,
                note: edits.note.clone(),
                mention_count: mentions.len() as u64,
                from_table: mentions.iter().all(|m| m.from_table),
            },
        );
        let all: Vec<&&EntityMentionFact> = mentions
            .iter()
            .filter(|m| contents.get(&m.content).is_some_and(|c| c.counted))
            .collect();
        let counts = |ms: &[&&EntityMentionFact]| EntityCounts {
            documents: ms.iter().map(|m| m.document).collect::<BTreeSet<_>>().len() as u64,
            file_types: ms
                .iter()
                .map(|m| contents[&m.content].file_type.as_str())
                .collect::<BTreeSet<_>>()
                .len() as u64,
            mentions: ms.len() as u64,
        };
        let high: Vec<&&EntityMentionFact> = all
            .iter()
            .copied()
            .filter(|m| m.confidence >= LOW_CONFIDENCE)
            .collect();
        let shown = counts(&high);
        let stats = EntityStats {
            documents: shown.documents,
            file_types: shown.file_types,
            mentions: shown.mentions,
            low: counts(&all),
        };
        if let Some(row) = entity_connection_for(key, &stats, edits.ignored) {
            out.connections.insert(key.clone(), row);
        }
    }
    // Anchors with no entity: their decisions stand orphaned (INV-ER-08, INV-CU-04).
    for decisions in entity_decisions.values() {
        apply_entity_statuses(&mut out, decisions, false);
    }

    let anchors: BTreeSet<&str> = by_key
        .keys()
        .map(String::as_str)
        .chain(identifier_decisions.keys().copied())
        .collect();
    for anchor in anchors {
        let mentions = by_key.get(anchor);
        let resolution = resolve(
            identifier_decisions
                .get(anchor)
                .map_or(&[][..], Vec::as_slice),
            mentions.is_some(),
        );
        out.decision_status
            .extend(resolution.statuses.iter().copied());
        let Some((key, mentions)) = mentions else {
            continue;
        };
        out.identifiers.insert(
            anchor.to_string(),
            IdentifierRow {
                kind: key.kind().as_str().to_string(),
                value: key.value().to_string(),
                mention_count: mentions.len() as u64,
            },
        );
        let counted: Vec<&&MentionFact> = mentions
            .iter()
            .filter(|m| contents.get(&m.content).is_some_and(|c| c.counted))
            .collect();
        let stats = IdentifierStats {
            documents: counted
                .iter()
                .map(|m| m.document)
                .collect::<BTreeSet<_>>()
                .len() as u64,
            file_types: counted
                .iter()
                .map(|m| contents[&m.content].file_type.as_str())
                .collect::<BTreeSet<_>>()
                .len() as u64,
            mentions: counted.len() as u64,
            max_confidence: counted.iter().map(|m| m.confidence).fold(0.0, f32::max),
        };
        if let Some(row) = connection_for(key, &stats, resolution.ignored, &facts.stop_list) {
            out.connections.insert(anchor.to_string(), row);
        }
    }
    out
}

/// The entity a name-key or split-key anchor finds (domain §9.5).
fn entity_of_anchor(anchor: &str, root: &impl Fn(&str) -> String) -> String {
    if is_split_key(anchor) {
        anchor.to_string()
    } else {
        root(anchor)
    }
}

/// The state of `d` in its family on its anchor, given whether the anchor is there.
fn push_status(out: &mut DerivedState, facts: &Facts, d: &DecisionRecord, present: bool) {
    let family: Vec<DecisionFact> = facts
        .decisions
        .iter()
        .filter(|o| o.anchor == d.anchor && o.kind.family() == d.kind.family())
        .map(|o| DecisionFact {
            seq: o.seq,
            kind: o.kind,
        })
        .collect();
    out.decision_status.extend(
        resolve(&family, present)
            .statuses
            .into_iter()
            .filter(|(seq, _)| *seq == d.seq),
    );
}

/// The states of an entity's ignore and edit decisions: per anchor and family (INV-CU-05).
fn apply_entity_statuses(out: &mut DerivedState, decisions: &[&DecisionRecord], present: bool) {
    let mut by_anchor: BTreeMap<(&str, DecisionFamily), Vec<DecisionFact>> = BTreeMap::new();
    for d in decisions {
        by_anchor
            .entry((d.anchor.as_str(), d.kind.family()))
            .or_default()
            .push(DecisionFact {
                seq: d.seq,
                kind: d.kind,
            });
    }
    for family in by_anchor.values() {
        out.decision_status
            .extend(resolve(family, present).statuses.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mk_kernel::IdentifierKind;

    fn key(v: &str) -> IdentifierKey {
        IdentifierKey::new(IdentifierKind::Email, v)
    }

    fn content(id: i64, file_type: &str, counted: bool) -> ContentFact {
        ContentFact {
            id,
            file_type: file_type.into(),
            counted,
        }
    }

    fn mention(content: i64, v: &str) -> MentionFact {
        MentionFact {
            content,
            document: content * 10,
            key: key(v),
            confidence: 1.0,
        }
    }

    /// INV-LK-01, INV-CO-07, INV-CU-04: connections need two counted documents; a Missing
    /// content's mentions keep the identifier but do not count; decisions without an anchor
    /// are orphaned.
    #[test]
    fn reference_applies_every_rule() {
        let facts = Facts {
            contents: vec![
                content(1, "txt", true),
                content(2, "csv", true),
                content(3, "md", false),
            ],
            mentions: vec![
                mention(1, "a@x.example"),
                mention(2, "a@x.example"),
                mention(1, "b@x.example"),
                mention(3, "b@x.example"),
            ],
            decisions: vec![
                DecisionRecord {
                    seq: 1,
                    kind: DecisionKind::IgnoreIdentifier,
                    anchor: "email:a@x.example".into(),
                    after: None,
                },
                DecisionRecord {
                    seq: 2,
                    kind: DecisionKind::IgnoreIdentifier,
                    anchor: "email:gone@x.example".into(),
                    after: None,
                },
            ],
            stop_list: StopList::default(),
            ..Facts::default()
        };
        let d = reference(&facts);
        assert_eq!(d.identifiers.len(), 2);
        assert_eq!(d.identifiers["email:b@x.example"].mention_count, 2);
        let a = &d.connections["email:a@x.example"];
        assert_eq!((a.source_count, a.file_type_count, a.ignored), (2, 2, true));
        assert!(!d.connections.contains_key("email:b@x.example"));
        assert_eq!(
            d.decision_status,
            BTreeMap::from([(1, DecisionStatus::Active), (2, DecisionStatus::Orphaned)])
        );
    }

    /// Domain §9.3 steps 1, 2, 4 for table entities: grouped by name key, display name by
    /// frequency, from-table mark; entity ignores hide the row; mapping decisions have no
    /// connection state.
    #[test]
    fn reference_builds_table_entities() {
        let em = |content: i64, raw: &str| EntityMentionFact {
            id: 0,
            mention_key: format!("{content} {raw}"),
            content,
            document: content * 10,
            key: NameKey::new(mk_kernel::EntityType::Org, raw).unwrap(),
            raw: raw.into(),
            confidence: 1.0,
            from_table: true,
        };
        let facts = Facts {
            contents: vec![
                content(1, "csv", true),
                content(2, "json", true),
                content(3, "csv", false),
            ],
            entity_mentions: vec![
                em(1, "Brightwater Logistics Ltd"),
                em(1, "Brightwater Logistics Ltd"),
                em(2, "BRIGHTWATER LOGISTICS LTD"),
                em(3, "Northgate Marine"),
                em(1, "Northgate Marine"),
            ],
            decisions: vec![
                DecisionRecord {
                    seq: 1,
                    kind: DecisionKind::IgnoreEntity,
                    anchor: "org:brightwater logistics".into(),
                    after: None,
                },
                DecisionRecord {
                    seq: 2,
                    kind: DecisionKind::ConfirmMapping,
                    anchor: "map:1/a.csv|abc".into(),
                    after: None,
                },
            ],
            ..Facts::default()
        };
        let d = reference(&facts);
        let bw = &d.entities["org:brightwater logistics"];
        assert_eq!(bw.display_name, "Brightwater Logistics Ltd");
        assert_eq!((bw.mention_count, bw.from_table), (3, true));
        let row = &d.connections["org:brightwater logistics"];
        assert_eq!(
            (row.source_count, row.file_type_count, row.ignored),
            (2, 2, true)
        );
        assert!(!row.low_only);
        // Northgate's second document is Missing: no connection, but the entity stays.
        assert!(d.entities.contains_key("org:northgate marine"));
        assert!(!d.connections.contains_key("org:northgate marine"));
        assert_eq!(
            d.decision_status,
            BTreeMap::from([(1, DecisionStatus::Active)])
        );
    }

    fn name_mention(id: i64, document: i64, raw: &str) -> EntityMentionFact {
        EntityMentionFact {
            id,
            mention_key: format!("mk{id}"),
            content: document,
            document,
            key: NameKey::new(mk_kernel::EntityType::Person, raw).unwrap(),
            raw: raw.into(),
            confidence: 1.0,
            from_table: false,
        }
    }

    fn decision(seq: i64, kind: DecisionKind, anchor: &str, after: Option<&str>) -> DecisionRecord {
        DecisionRecord {
            seq,
            kind,
            anchor: anchor.into(),
            after: after.map(str::to_string),
        }
    }

    fn people() -> Facts {
        Facts {
            contents: (1..=3).map(|i| content(i, "txt", true)).collect(),
            entity_mentions: vec![
                name_mention(1, 1, "Corvin Hale"),
                name_mention(2, 2, "Corvin Hale"),
                name_mention(3, 2, "C. Hale"),
                name_mention(4, 3, "C. Hale"),
                name_mention(5, 3, "Mr Hale"),
            ],
            ..Facts::default()
        }
    }

    /// INV-ER-02/03, US-11.4 AC1: an accepted pair is one entity under its smallest key, with its
    /// counted documents together; a newer reject on the pair splits it back and the pieces keep
    /// their own decisions.
    #[test]
    fn inv_er_03_merge_and_undo() {
        use DecisionKind::*;
        let mut facts = people();
        facts.decisions = vec![
            decision(1, AcceptMerge, "person:c hale|person:corvin hale", None),
            decision(2, Rename, "person:corvin hale", Some("Corvin H.")),
        ];
        let d = reference(&facts);
        assert!(!d.entities.contains_key("person:corvin hale"));
        let e = &d.entities["person:c hale"];
        assert_eq!((e.display_name.as_str(), e.mention_count), ("Corvin H.", 4));
        assert_eq!(d.connections["person:c hale"].source_count, 3);
        assert_eq!(d.decision_status[&1], DecisionStatus::Active);
        assert_eq!(d.entity_members["person:corvin hale"], "person:c hale");

        facts.decisions.push(decision(
            3,
            RejectMerge,
            "person:c hale|person:corvin hale",
            None,
        ));
        let d = reference(&facts);
        assert_eq!(d.entities["person:c hale"].display_name, "C. Hale");
        assert_eq!(d.entities["person:corvin hale"].display_name, "Corvin H.");
        assert_eq!(d.decision_status[&1], DecisionStatus::Superseded);
        assert_eq!(d.decision_status[&2], DecisionStatus::Active);
        assert!(d.entity_members.is_empty());
    }

    /// INV-ER-08, INV-CU-04: a merge whose side has no mention reads Orphaned, and a chain through
    /// it still holds, so its parts stay together (ASSUMPTION, M7a).
    #[test]
    fn inv_cu_04_chain_through_an_absent_key() {
        use DecisionKind::*;
        let mut facts = people();
        facts.decisions = vec![
            decision(1, AcceptMerge, "person:c hale|person:gone name", None),
            decision(2, AcceptMerge, "person:corvin hale|person:gone name", None),
        ];
        let d = reference(&facts);
        assert_eq!(d.decision_status[&1], DecisionStatus::Orphaned);
        assert_eq!(d.decision_status[&2], DecisionStatus::Orphaned);
        assert_eq!(d.entities["person:c hale"].mention_count, 4);
    }

    /// US-11.4 AC2, INV-ER-01/02, INV-CU-04: split mentions are a new entity, and leave their
    /// old one. The newest split takes a mention in two. Splits whose mentions are gone are Orphaned.
    #[test]
    fn us_11_4_ac2_split_moves_mentions() {
        use DecisionKind::*;
        let (s1, s2) = (
            format!("split:{}", "a".repeat(64)),
            format!("split:{}", "b".repeat(64)),
        );
        let mut facts = people();
        facts.decisions = vec![
            decision(1, Split, &s1, Some("mk1\nmk2")),
            decision(2, Split, &s2, Some("mk2")),
            decision(3, Rename, &s1, Some("Another Corvin")),
            decision(4, Split, &format!("split:{}", "c".repeat(64)), Some("gone")),
        ];
        let d = reference(&facts);
        assert_eq!(d.entities[&s1].mention_count, 1);
        assert_eq!(d.entities[&s1].display_name, "Another Corvin");
        assert_eq!(d.entities[&s1].entity_type, "person");
        assert_eq!(d.entities[&s2].mention_count, 1);
        assert!(!d.entities.contains_key("person:corvin hale"));
        assert_eq!(d.split_mentions[&2].0, s2);
        assert_eq!(d.decision_status[&1], DecisionStatus::Active);
        assert_eq!(d.decision_status[&4], DecisionStatus::Orphaned);

        facts.decisions.push(decision(5, UndoSplit, &s2, None));
        let d = reference(&facts);
        assert_eq!(d.entities[&s1].mention_count, 2);
        assert!(!d.entities.contains_key(&s2));
        assert_eq!(d.decision_status[&2], DecisionStatus::Superseded);
        assert_eq!(d.decision_status[&5], DecisionStatus::Active);
    }

    /// Domain §9.5: an alias decision is anchored per alias and is Orphaned once that name is gone.
    #[test]
    fn alias_decisions_have_their_own_anchor() {
        use DecisionKind::*;
        let mut facts = people();
        facts.decisions = vec![
            decision(1, RemoveAlias, "person:c hale#alias:C. Hale", None),
            decision(2, RemoveAlias, "person:c hale#alias:Nothing", None),
            decision(3, RestoreAlias, "person:c hale#alias:C. Hale", None),
        ];
        let d = reference(&facts);
        assert_eq!(d.decision_status[&1], DecisionStatus::Superseded);
        assert_eq!(d.decision_status[&2], DecisionStatus::Orphaned);
        assert_eq!(d.decision_status[&3], DecisionStatus::Active);
    }
}
