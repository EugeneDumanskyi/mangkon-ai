//! An in-memory reference of the whole derived state (ADR-0005 follow-up): the rules of this
//! crate applied to every key at once, with no SQL and no incremental bookkeeping. The property
//! test in `mk-core` checks the store's incremental commits and its full rebuild against it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mk_kernel::{IdentifierKey, NameKey};

use crate::{
    ConnectionRow, DecisionFact, DecisionKind, DecisionStatus, EntityCounts, EntityStats,
    IdentifierStats, LOW_CONFIDENCE, StopList, SubjectKind, connection_for, display_name,
    entity_connection_for, resolve,
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
    /// The anchor: an identifier key as `type:value`, a name key, or a mapping anchor.
    pub anchor: String,
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

/// A derived entity row (domain §5.1 `ENTITY`, M4c): present while any mention with its name
/// key is stored, Missing content included.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntityRow {
    pub entity_type: String,
    /// The most frequent name as written (INV-ER-07).
    pub display_name: String,
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
    let mut by_name: BTreeMap<String, (&NameKey, Vec<&EntityMentionFact>)> = BTreeMap::new();
    for m in &facts.entity_mentions {
        by_name
            .entry(m.key.to_string())
            .or_insert_with(|| (&m.key, Vec::new()))
            .1
            .push(m);
    }
    let mut decisions: BTreeMap<&str, Vec<DecisionFact>> = BTreeMap::new();
    for d in &facts.decisions {
        // Only connection decisions of the anchor's subject kind have a derived state here.
        if !DecisionKind::ignore_pair(SubjectKind::of_key(&d.anchor)).contains(&d.kind) {
            continue;
        }
        decisions.entry(&d.anchor).or_default().push(DecisionFact {
            seq: d.seq,
            kind: d.kind,
        });
    }

    let mut out = DerivedState::default();
    let anchors: BTreeSet<&str> = by_key
        .keys()
        .chain(by_name.keys())
        .map(String::as_str)
        .chain(decisions.keys().copied())
        .collect();
    for anchor in anchors {
        if let Some((key, mentions)) = by_name.get(anchor) {
            let resolution = resolve(decisions.get(anchor).map_or(&[][..], Vec::as_slice), true);
            out.decision_status
                .extend(resolution.statuses.iter().copied());
            out.entities.insert(
                anchor.to_string(),
                EntityRow {
                    entity_type: key.entity_type().as_str().to_string(),
                    display_name: display_name(mentions.iter().map(|m| m.raw.as_str()))
                        .unwrap_or_default(),
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
            if let Some(row) = entity_connection_for(key, &stats, resolution.ignored) {
                out.connections.insert(anchor.to_string(), row);
            }
            continue;
        }
        let mentions = by_key.get(anchor);
        let resolution = resolve(
            decisions.get(anchor).map_or(&[][..], Vec::as_slice),
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
                },
                DecisionRecord {
                    seq: 2,
                    kind: DecisionKind::IgnoreIdentifier,
                    anchor: "email:gone@x.example".into(),
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
                },
                DecisionRecord {
                    seq: 2,
                    kind: DecisionKind::ConfirmMapping,
                    anchor: "map:1/a.csv|abc".into(),
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
}
