//! BC-07, BC-08 and BC-09: derived identifier and connection rows and decision states, rebuilt
//! together per touched key (domain §3, §9, ADR-0005). No I/O (ADR-0002 rule 1).
//!
//! The rules here are used two ways: `mk-store` applies them per touched key inside each commit,
//! and [`model::reference`] applies them to a whole in-memory project, so tests can check that
//! incremental commits and a full rebuild agree (ADR-0005 follow-up).
//!
//! M2b: identifier connections and the ignore / un-ignore decisions. Entities (ADR-0005 step 3)
//! come in M5–M7; there are no entity mentions yet.

pub mod model;

use std::cmp::{Ordering, Reverse};
use std::collections::BTreeSet;
use std::fmt;

use mk_kernel::{IdentifierKey, IdentifierKind};

/// Mentions below this confidence are "low confidence" (US-07.1 AC4, domain §9.1).
pub const LOW_CONFIDENCE: f32 = 0.5;

/// Counted mentions of one identifier (domain §9.1): mentions whose content is not Missing
/// (INV-CO-07). The stop list applies to the whole identifier ([`StopList`]); card-like numbers
/// are never identifiers at all (INV-ID-06), so they need no rule here.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct IdentifierStats {
    /// Distinct documents.
    pub documents: u64,
    /// Distinct file types of those documents.
    pub file_types: u64,
    pub mentions: u64,
    /// Highest confidence; 0 without mentions.
    pub max_confidence: f32,
}

/// What a connection is about. M2b has identifier connections only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SubjectKind {
    Identifier,
}

impl SubjectKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubjectKind::Identifier => "identifier",
        }
    }
}

/// A derived connection row (domain §5.1 `CONNECTION`, §9.1). Never edited directly
/// (INV-LK-05).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConnectionRow {
    /// The identifier key, `type:value`.
    pub subject_key: String,
    pub subject_kind: SubjectKind,
    pub source_count: u64,
    pub file_type_count: u64,
    pub mention_count: u64,
    pub low_confidence: bool,
    pub ignored: bool,
}

/// The project's domain stop list (US-04.3 AC3): registrable domains, lower case.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StopList(BTreeSet<String>);

impl StopList {
    pub fn new(domains: impl IntoIterator<Item = String>) -> Self {
        StopList(domains.into_iter().collect())
    }

    /// Whether `key` is a stop-list domain: stored, never a connection (INV-LK-03).
    pub fn stops(&self, key: &IdentifierKey) -> bool {
        key.kind() == IdentifierKind::Domain && self.0.contains(key.value())
    }

    /// The domain keys (`domain:<d>`) in one list and not the other: what a stop-list edit
    /// must rebuild.
    pub fn changed_keys(&self, other: &StopList) -> BTreeSet<String> {
        self.0
            .symmetric_difference(&other.0)
            .map(|d| IdentifierKey::new(IdentifierKind::Domain, d.as_str()).to_string())
            .collect()
    }
}

/// The connection row for an identifier, if it is one (INV-LK-01): counted mentions in two or
/// more distinct documents, and not a stop-list domain (INV-LK-03). Low confidence when every
/// counted mention is below 0.5 (US-07.1 AC4); ignored when an active ignore decision applies
/// (US-07.4).
pub fn connection_for(
    key: &IdentifierKey,
    stats: &IdentifierStats,
    ignored: bool,
    stop_list: &StopList,
) -> Option<ConnectionRow> {
    (stats.documents >= 2 && !stop_list.stops(key)).then(|| ConnectionRow {
        subject_key: key.to_string(),
        subject_kind: SubjectKind::Identifier,
        source_count: stats.documents,
        file_type_count: stats.file_types,
        mention_count: stats.mentions,
        low_confidence: stats.max_confidence < LOW_CONFIDENCE,
        ignored,
    })
}

/// Ranking order (domain §9.2) as a sortable key: confident first, then more documents, more
/// file types, more mentions, then the key A→Z so the order is stable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RankKey {
    pub low_confidence: bool,
    pub source_count: u64,
    pub file_type_count: u64,
    pub mention_count: u64,
    pub subject_key: String,
}

impl RankKey {
    #[allow(clippy::type_complexity)]
    fn tuple(&self) -> (bool, Reverse<u64>, Reverse<u64>, Reverse<u64>, &str) {
        (
            self.low_confidence,
            Reverse(self.source_count),
            Reverse(self.file_type_count),
            Reverse(self.mention_count),
            &self.subject_key,
        )
    }
}

impl Ord for RankKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tuple().cmp(&other.tuple())
    }
}

impl PartialOrd for RankKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ConnectionRow {
    pub fn rank_key(&self) -> RankKey {
        RankKey {
            low_confidence: self.low_confidence,
            source_count: self.source_count,
            file_type_count: self.file_type_count,
            mention_count: self.mention_count,
            subject_key: self.subject_key.clone(),
        }
    }
}

/// Decision kinds. M2b has ignore and un-ignore of an identifier connection, anchored on its
/// [`IdentifierKey`] (domain §9.5). The real kinds come in M4 and M7.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DecisionKind {
    IgnoreIdentifier,
    UnignoreIdentifier,
}

impl DecisionKind {
    pub const ALL: [DecisionKind; 2] = [
        DecisionKind::IgnoreIdentifier,
        DecisionKind::UnignoreIdentifier,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionKind::IgnoreIdentifier => "ignore_identifier",
            DecisionKind::UnignoreIdentifier => "unignore_identifier",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

/// State of a decision (domain §7.5). Derived, never stored in the log itself (ADR-0004).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DecisionStatus {
    Active,
    Superseded,
    Orphaned,
}

impl DecisionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionStatus::Active => "active",
            DecisionStatus::Superseded => "superseded",
            DecisionStatus::Orphaned => "orphaned",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Active, Self::Superseded, Self::Orphaned]
            .into_iter()
            .find(|s| s.as_str() == name)
    }
}

impl fmt::Display for DecisionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One logged decision on an anchor, as the resolver needs it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DecisionFact {
    /// Log order: higher is newer.
    pub seq: i64,
    pub kind: DecisionKind,
}

/// Outcome of resolving the decisions on one anchor.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resolution {
    /// The connection is hidden by an applied ignore.
    pub ignored: bool,
    /// The state of every decision, by `seq`.
    pub statuses: Vec<(i64, DecisionStatus)>,
}

/// Resolves the ignore / un-ignore decisions on one identifier anchor (domain §7.5, §9.4).
///
/// The newest decision wins (INV-CU-05); the older ones are Superseded. The newest is Active
/// when the anchor exists (the identifier has a stored mention), otherwise Orphaned (INV-CU-04),
/// and it becomes Active again when the anchor comes back. Only an Active ignore hides the
/// connection.
pub fn resolve(decisions: &[DecisionFact], anchor_present: bool) -> Resolution {
    let Some(newest) = decisions.iter().max_by_key(|d| d.seq) else {
        return Resolution::default();
    };
    let statuses = decisions
        .iter()
        .map(|d| {
            let status = if d.seq != newest.seq {
                DecisionStatus::Superseded
            } else if anchor_present {
                DecisionStatus::Active
            } else {
                DecisionStatus::Orphaned
            };
            (d.seq, status)
        })
        .collect();
    Resolution {
        ignored: anchor_present && newest.kind == DecisionKind::IgnoreIdentifier,
        statuses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mk_kernel::IdentifierKind;

    fn key() -> IdentifierKey {
        IdentifierKey::new(IdentifierKind::Email, "a@x.example")
    }

    fn stats(documents: u64, max_confidence: f32) -> IdentifierStats {
        IdentifierStats {
            documents,
            file_types: 1,
            mentions: documents * 2,
            max_confidence,
        }
    }

    /// INV-LK-01: a connection needs counted mentions in two or more documents.
    /// INV-LK-03, US-04.3 AC3: a stop-list domain is never a connection; other kinds with the
    /// same text are.
    #[test]
    fn inv_lk_03_stop_list_domain_is_not_a_connection() {
        let stop = StopList::new(["gmail.com".to_string()]);
        let domain = IdentifierKey::new(IdentifierKind::Domain, "gmail.com");
        assert_eq!(connection_for(&domain, &stats(5, 1.0), false, &stop), None);
        let email = IdentifierKey::new(IdentifierKind::Email, "a@gmail.com");
        assert!(connection_for(&email, &stats(2, 1.0), false, &stop).is_some());
        let other = IdentifierKey::new(IdentifierKind::Domain, "shell-co.example");
        assert!(connection_for(&other, &stats(2, 1.0), false, &stop).is_some());
        assert_eq!(
            stop.changed_keys(&StopList::new(["shell-co.example".to_string()])),
            BTreeSet::from([
                "domain:gmail.com".to_string(),
                "domain:shell-co.example".to_string()
            ])
        );
    }

    #[test]
    fn inv_lk_01_two_documents_make_a_connection() {
        assert_eq!(
            connection_for(&key(), &stats(1, 1.0), false, &StopList::default()),
            None
        );
        let row = connection_for(&key(), &stats(2, 1.0), false, &StopList::default()).unwrap();
        assert_eq!(row.subject_key, "email:a@x.example");
        assert_eq!(
            (row.source_count, row.mention_count, row.low_confidence),
            (2, 4, false)
        );
    }

    /// US-07.1 AC4: low confidence only when every mention is below 0.5.
    #[test]
    fn us_07_1_ac4_low_confidence_when_all_below_half() {
        assert!(
            connection_for(&key(), &stats(2, 0.49), false, &StopList::default())
                .unwrap()
                .low_confidence
        );
        assert!(
            !connection_for(&key(), &stats(2, 0.5), false, &StopList::default())
                .unwrap()
                .low_confidence
        );
    }

    /// Domain §9.2: ranking order.
    #[test]
    fn ranking_order() {
        let row = |k: &str, low, s, f, m| ConnectionRow {
            subject_key: k.into(),
            subject_kind: SubjectKind::Identifier,
            source_count: s,
            file_type_count: f,
            mention_count: m,
            low_confidence: low,
            ignored: false,
        };
        let mut rows = [
            row("e", true, 9, 9, 9),
            row("d", false, 2, 1, 2),
            row("c", false, 2, 1, 5),
            row("b", false, 2, 2, 2),
            row("a", false, 3, 1, 2),
            row("f", false, 2, 1, 2),
        ];
        rows.sort_by_key(ConnectionRow::rank_key);
        let order: Vec<&str> = rows.iter().map(|r| r.subject_key.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c", "d", "f", "e"]);
    }

    fn d(seq: i64, kind: DecisionKind) -> DecisionFact {
        DecisionFact { seq, kind }
    }

    /// INV-CU-05: the newest decision wins; older ones are superseded.
    #[test]
    fn inv_cu_05_newest_wins() {
        use DecisionKind::*;
        use DecisionStatus::*;
        let r = resolve(&[d(3, IgnoreIdentifier), d(7, UnignoreIdentifier)], true);
        assert!(!r.ignored);
        assert_eq!(r.statuses, vec![(3, Superseded), (7, Active)]);
        let r = resolve(
            &[
                d(3, IgnoreIdentifier),
                d(7, UnignoreIdentifier),
                d(9, IgnoreIdentifier),
            ],
            true,
        );
        assert!(r.ignored);
        assert_eq!(r.statuses[2], (9, Active));
    }

    /// INV-CU-04: without its anchor the newest decision is orphaned and applies nothing; it
    /// is active again once the anchor is back.
    #[test]
    fn inv_cu_04_orphaned_without_anchor() {
        use DecisionKind::*;
        use DecisionStatus::*;
        let ds = [d(1, IgnoreIdentifier), d(2, IgnoreIdentifier)];
        let gone = resolve(&ds, false);
        assert!(!gone.ignored);
        assert_eq!(gone.statuses, vec![(1, Superseded), (2, Orphaned)]);
        assert_eq!(
            resolve(&ds, true).statuses,
            vec![(1, Superseded), (2, Active)]
        );
        assert_eq!(resolve(&[], true), Resolution::default());
    }
}
