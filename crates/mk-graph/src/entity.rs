//! BC-07 and BC-09 rules for entities built from decisions (domain §9.3, §9.5, M7a): which name keys
//! form one group, which split takes a mention, and which edit applies to an entity. Pure: `mk-store`
//! applies them per touched key and [`crate::model::reference`] to a whole project.
//!
//! Membership depends only on decisions, so a commit never changes who belongs to which group, and a
//! commit stays a per-key rebuild (ADR-0005).

use std::collections::BTreeMap;

use mk_kernel::EntityType;

use crate::{DecisionFact, DecisionFamily, DecisionKind, DecisionStatus, resolve};

/// Marks an alias removal's anchor: `<key>#alias:<text>`. A name key or split key holds no `#`.
const ALIAS_MARK: &str = "#alias:";

/// The anchor of a merge decision: the two name keys in order, so `a|b` and `b|a` are one pair
/// (domain §9.5). A name key holds no `|`.
pub fn merge_anchor(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}

/// The two name keys of a merge anchor.
pub fn merge_pair(anchor: &str) -> Option<(&str, &str)> {
    anchor.split_once('|')
}

/// The anchor of an alias removal on `key` (ASSUMPTION, M7a): one per alias, because the newest
/// decision per anchor wins, so a removal must not cancel the removal of another alias.
pub fn alias_anchor(key: &str, alias: &str) -> String {
    format!("{key}{ALIAS_MARK}{alias}")
}

/// The key and the alias of an alias anchor.
pub fn alias_parts(anchor: &str) -> Option<(&str, &str)> {
    anchor.split_once(ALIAS_MARK)
}

/// The subject keys a decision's anchor concerns: what must be rebuilt when it is logged. A mapping
/// decision has none (it acts on a file's table output).
pub fn anchor_subjects(kind: DecisionKind, anchor: &str) -> Vec<String> {
    match kind.family() {
        DecisionFamily::Mapping => Vec::new(),
        DecisionFamily::Merge => merge_pair(anchor)
            .map(|(a, b)| vec![a.to_string(), b.to_string()])
            .unwrap_or_default(),
        DecisionFamily::Alias => alias_parts(anchor)
            .map(|(k, _)| vec![k.to_string()])
            .unwrap_or_default(),
        _ => vec![anchor.to_string()],
    }
}

/// The group of every name key in a group of two or more, over the accepted pairs (INV-ER-03): name
/// key → entity key, the smallest name key of its group. A chain holds through a key with no
/// mentions, because groups read only decisions (ASSUMPTION, M7a).
pub fn group_members<'a>(
    accepted: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, String> {
    let mut parent: BTreeMap<&'a str, &'a str> = BTreeMap::new();
    fn find<'a>(parent: &mut BTreeMap<&'a str, &'a str>, key: &'a str) -> &'a str {
        let mut root = key;
        while let Some(&p) = parent.get(root) {
            if p == root {
                break;
            }
            root = p;
        }
        let mut at = key;
        while at != root {
            let next = parent.insert(at, root).unwrap_or(root);
            at = next;
        }
        root
    }
    for (a, b) in accepted {
        parent.entry(a).or_insert(a);
        parent.entry(b).or_insert(b);
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            // The smaller key stays the root, so it ends up the group's key.
            let (small, large) = if ra < rb { (ra, rb) } else { (rb, ra) };
            parent.insert(large, small);
        }
    }
    let keys: Vec<&str> = parent.keys().copied().collect();
    keys.into_iter()
        .map(|k| {
            let root = find(&mut parent, k);
            (k.to_string(), root.to_string())
        })
        .collect()
}

/// The newest decision of each pair's family, in log order: `(anchor, accepted)`.
pub fn newest_merges<'a>(
    decisions: impl IntoIterator<Item = (i64, DecisionKind, &'a str)>,
) -> BTreeMap<&'a str, bool> {
    let mut newest: BTreeMap<&'a str, (i64, bool)> = BTreeMap::new();
    for (seq, kind, anchor) in decisions {
        if kind.family() != DecisionFamily::Merge {
            continue;
        }
        let entry = newest.entry(anchor).or_insert((seq, false));
        if seq >= entry.0 {
            *entry = (seq, kind == DecisionKind::AcceptMerge);
        }
    }
    newest.into_iter().map(|(a, (_, on))| (a, on)).collect()
}

/// The states on one anchor: per family the newest wins (INV-CU-05) and is Active when its anchor is
/// there for that family, else Orphaned (INV-CU-04). `families` holds each family's decisions with
/// whether its anchor is present.
pub fn anchor_statuses(families: &[(&[DecisionFact], bool)]) -> Vec<(i64, DecisionStatus)> {
    families
        .iter()
        .flat_map(|(decisions, present)| resolve(decisions, *present).statuses)
        .collect()
}

/// Splits `decisions` on one anchor by family.
pub fn by_family(decisions: &[DecisionFact]) -> BTreeMap<DecisionFamily, Vec<DecisionFact>> {
    let mut out: BTreeMap<DecisionFamily, Vec<DecisionFact>> = BTreeMap::new();
    for d in decisions {
        out.entry(d.kind.family()).or_default().push(*d);
    }
    out
}

/// One decision on one anchor of an entity, with the value it set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntityDecision<'a> {
    pub seq: i64,
    pub kind: DecisionKind,
    pub anchor: &'a str,
    pub after: Option<&'a str>,
}

/// What a user's decisions set on an entity (INV-ER-07).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct EntityEdits {
    pub name: Option<String>,
    pub entity_type: Option<EntityType>,
    pub note: Option<String>,
    pub ignored: bool,
}

/// The decisions that apply to an entity whose anchors hold `decisions`: per anchor the newest of a
/// family stands, so an undone merge gives each part its own decisions back; across anchors the
/// newest of those applies (ASSUMPTION, M7a).
pub fn entity_edits(decisions: &[EntityDecision<'_>]) -> EntityEdits {
    let mut standing: BTreeMap<(DecisionFamily, &str), EntityDecision<'_>> = BTreeMap::new();
    for d in decisions {
        let at = standing.entry((d.kind.family(), d.anchor)).or_insert(*d);
        if d.seq >= at.seq {
            *at = *d;
        }
    }
    let mut newest: BTreeMap<DecisionFamily, EntityDecision<'_>> = BTreeMap::new();
    for ((family, _), d) in standing {
        let at = newest.entry(family).or_insert(d);
        if d.seq >= at.seq {
            *at = d;
        }
    }
    let text = |family| {
        newest
            .get(&family)
            .and_then(|d: &EntityDecision<'_>| d.after)
            .filter(|t: &&str| !t.trim().is_empty())
            .map(str::to_string)
    };
    EntityEdits {
        name: text(DecisionFamily::Rename),
        entity_type: text(DecisionFamily::ChangeType).and_then(|t| EntityType::from_name(&t)),
        note: text(DecisionFamily::Note),
        ignored: newest
            .get(&DecisionFamily::Ignore)
            .is_some_and(|d| d.kind.is_ignore()),
    }
}

/// The split a mention moves to: the newest active split holding its key (ASSUMPTION, M7a). `active`
/// holds `(seq, split key)` of the splits that list it.
pub fn split_for<'a>(active: impl IntoIterator<Item = (i64, &'a str)>) -> Option<&'a str> {
    active
        .into_iter()
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, k)| k)
}

/// The mention keys in a split decision's `after`, one per line.
pub fn split_keys(after: &str) -> impl Iterator<Item = &str> {
    after.lines().filter(|l| !l.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(seq: i64, kind: DecisionKind) -> DecisionFact {
        DecisionFact { seq, kind }
    }

    /// Domain §9.5: an unordered pair; INV-ER-06: `a|b` and `b|a` are one anchor.
    #[test]
    fn merge_anchor_is_unordered() {
        assert_eq!(merge_anchor("org:b", "org:a"), "org:a|org:b");
        assert_eq!(merge_anchor("org:a", "org:b"), "org:a|org:b");
        assert_eq!(merge_pair("org:a|org:b"), Some(("org:a", "org:b")));
        assert_eq!(merge_pair("org:a"), None);
    }

    /// Alias removals have one anchor per alias, and every anchor names its subjects.
    #[test]
    fn anchors_name_their_subjects() {
        use DecisionKind::*;
        let a = alias_anchor("person:a b", "A. B#1");
        assert_eq!(alias_parts(&a), Some(("person:a b", "A. B#1")));
        assert_eq!(anchor_subjects(RemoveAlias, &a), vec!["person:a b"]);
        assert_eq!(
            anchor_subjects(RejectMerge, "org:a|org:b"),
            vec!["org:a", "org:b"]
        );
        assert_eq!(anchor_subjects(Rename, "org:a"), vec!["org:a"]);
        assert!(anchor_subjects(ConfirmMapping, "map:1/a.csv|x").is_empty());
    }

    /// INV-ER-03: groups come only from accepted pairs; the key is the smallest name key; a chain
    /// holds through a key with no mentions (groups never read mentions).
    #[test]
    fn inv_er_03_groups_from_accepted_pairs() {
        let groups = group_members([("org:b", "org:c"), ("org:a", "org:b"), ("org:x", "org:y")]);
        assert_eq!(groups["org:c"], "org:a");
        assert_eq!(groups["org:b"], "org:a");
        assert_eq!(groups["org:a"], "org:a");
        assert_eq!(groups["org:y"], "org:x");
        assert_eq!(groups.len(), 5);
        assert!(group_members([]).is_empty());
    }

    /// INV-CU-05: a merge and its undo are one family; the newest wins.
    #[test]
    fn inv_cu_05_merge_family_newest_wins() {
        use DecisionKind::*;
        let n = newest_merges([
            (1, AcceptMerge, "a|b"),
            (2, RejectMerge, "a|b"),
            (3, RejectMerge, "c|d"),
            (4, AcceptMerge, "c|d"),
            (5, Rename, "a|b"),
        ]);
        assert_eq!(n, BTreeMap::from([("a|b", false), ("c|d", true)]));
    }

    /// INV-CU-04/05 per family: a rename and an ignore on one anchor do not supersede each other.
    #[test]
    fn inv_cu_05_families_are_separate() {
        use DecisionKind::*;
        use DecisionStatus::*;
        let ds = [fact(1, IgnoreEntity), fact(2, Rename), fact(3, Rename)];
        let families = by_family(&ds);
        let statuses: BTreeMap<i64, DecisionStatus> = families
            .values()
            .flat_map(|d| resolve(d, true).statuses)
            .collect();
        assert_eq!(
            statuses,
            BTreeMap::from([(1, Active), (2, Superseded), (3, Active)])
        );
        let gone: BTreeMap<i64, DecisionStatus> = families
            .values()
            .flat_map(|d| resolve(d, false).statuses)
            .collect();
        assert_eq!(gone[&3], Orphaned);
        assert_eq!(gone[&1], Orphaned);
    }

    fn edit<'a>(
        seq: i64,
        kind: DecisionKind,
        anchor: &'a str,
        after: &'a str,
    ) -> EntityDecision<'a> {
        EntityDecision {
            seq,
            kind,
            anchor,
            after: Some(after),
        }
    }

    /// INV-ER-07: a user-set name and type win; within a group the newest across its members applies,
    /// but an older decision of an anchor never comes back while that anchor has a newer one.
    #[test]
    fn inv_er_07_edits_across_members() {
        use DecisionKind::*;
        let e = entity_edits(&[
            edit(1, Rename, "org:a", "First"),
            edit(2, Rename, "org:b", "Second"),
            edit(3, ChangeType, "org:a", "person"),
            edit(4, SetNote, "org:b", "a note"),
            edit(5, SetNote, "org:b", ""),
            edit(6, IgnoreEntity, "org:a", "ignored"),
            edit(7, UnignoreEntity, "org:a", "shown"),
            edit(8, IgnoreEntity, "org:b", "ignored"),
        ]);
        assert_eq!(e.name.as_deref(), Some("Second"));
        assert_eq!(e.entity_type, Some(EntityType::Person));
        assert_eq!(e.note, None);
        assert!(e.ignored);
        // The part on its own after an undo: only its own decisions.
        let part = entity_edits(&[edit(1, Rename, "org:a", "First")]);
        assert_eq!(part.name.as_deref(), Some("First"));
        assert!(!part.ignored);
        assert_eq!(entity_edits(&[]), EntityEdits::default());
    }

    /// INV-ER-01: a mention in two active splits belongs to one entity: the newest split's.
    #[test]
    fn inv_er_01_newest_split_takes_the_mention() {
        assert_eq!(split_for([(2, "split:a"), (9, "split:b")]), Some("split:b"));
        assert_eq!(split_for([]), None);
        assert_eq!(split_keys("k1\nk2\n").collect::<Vec<_>>(), ["k1", "k2"]);
    }
}
