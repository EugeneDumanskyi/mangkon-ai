//! BC-05 Structured mapping (domain §3, E05): what each CSV column or JSON key holds, and what a
//! confirmed mapping makes of its values. Pure functions, no I/O (ADR-0002 rule 1); `mk-store`
//! runs them inside the transactions that write a table's machine output.
//!
//! - [`ColumnType`]: the one assignment a column has (INV-CM-01).
//! - Columns: a CSV column is keyed by its header text (or `Column n` without a header), a JSON
//!   key by its pointer with array indices as `*` ([`json_column_key`]); the
//!   [`header_signature`] of a file is a hash of those keys.
//! - [`suggest`] (US-05.1 AC2): an identifier type when ≥ 80% of a sample of up to 200
//!   non-empty values are valid for it, a date likewise, a person or organization from the
//!   header words, else ignore.
//! - [`Assignments`], [`resolve_state`] (US-05.2, domain §7.3) and [`entity_in_cell`]
//!   (US-05.3): only confirmed person and organization columns make entity mentions
//!   (INV-CM-03), each with its record ([`record_of`], INV-CM-05).

mod date;
mod suggest;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mk_kernel::{ContentHash, EntityType, NameKey, Span};

pub use date::is_date;
pub use suggest::{Suggestion, header_words, suggest};

/// Non-empty values read per column for a suggestion (US-05.1 AC2). ASSUMPTION: 200.
pub const SAMPLE_SIZE: usize = 200;
/// Share of the sample that must be valid for a type to be suggested. ASSUMPTION: 80%.
pub const SUGGEST_SHARE: f32 = 0.8;
/// Columns past this many get no suggestion (they stay Ignore), so a very wide file costs
/// little (TD-3). ASSUMPTION (M4c): 1,000.
pub const MAX_COLUMNS: usize = 1_000;

/// What a column holds (US-05.1 AC1, INV-CM-01).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum ColumnType {
    Email,
    Phone,
    UrlDomain,
    Iban,
    Crypto,
    PersonName,
    OrgName,
    Date,
    Ignore,
}

impl ColumnType {
    pub const ALL: [ColumnType; 9] = [
        ColumnType::Email,
        ColumnType::Phone,
        ColumnType::UrlDomain,
        ColumnType::Iban,
        ColumnType::Crypto,
        ColumnType::PersonName,
        ColumnType::OrgName,
        ColumnType::Date,
        ColumnType::Ignore,
    ];

    /// Stable name, as stored, in decisions and in the ground truth.
    pub fn as_str(&self) -> &'static str {
        match self {
            ColumnType::Email => "email",
            ColumnType::Phone => "phone",
            ColumnType::UrlDomain => "url_domain",
            ColumnType::Iban => "iban",
            ColumnType::Crypto => "crypto",
            ColumnType::PersonName => "person",
            ColumnType::OrgName => "org",
            ColumnType::Date => "date",
            ColumnType::Ignore => "ignore",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == name)
    }

    /// The entity type a confirmed column of this type makes (US-05.3 AC1).
    pub fn entity_type(&self) -> Option<EntityType> {
        match self {
            ColumnType::PersonName => Some(EntityType::Person),
            ColumnType::OrgName => Some(EntityType::Org),
            _ => None,
        }
    }
}

impl fmt::Display for ColumnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The key of CSV column `index` (0-based): its header text, trimmed, or `Column <n>` (1-based)
/// when the file has no header or the header cell is empty.
pub fn csv_column_key(header: Option<&str>, index: usize) -> String {
    match header.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => h.to_string(),
        None => format!("Column {}", index + 1),
    }
}

/// The key of a JSON value's column: its pointer with every array index as `*`
/// (`/officers/0/name` → `/officers/*/name`). ASSUMPTION (M4c): an object key made only of
/// digits reads as an index too.
pub fn json_column_key(pointer: &str) -> String {
    pointer
        .split('/')
        .map(|t| {
            if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
                "*"
            } else {
                t
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The record a JSON value belongs to (INV-CM-05): the pointer of its nearest enclosing array
/// item (`/officers/0/name` → `/officers/0`), else the whole top-level object (`""`).
pub fn record_of(pointer: &str) -> String {
    let tokens: Vec<&str> = pointer.split('/').collect();
    match tokens
        .iter()
        .rposition(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
    {
        Some(i) => tokens[..=i].join("/"),
        None => String::new(),
    }
}

/// A file's header signature: BLAKE3 (hex) of its column keys, `\n`-joined, in header order for
/// a CSV, sorted for a JSON file (a JSON file has no column order). Two files with the same
/// headers have the same signature (US-05.2 AC2).
pub fn header_signature<'a>(keys: impl IntoIterator<Item = &'a str>, sorted: bool) -> String {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    if sorted {
        keys.sort_unstable();
        keys.dedup();
    }
    ContentHash::of(keys.join("\n").as_bytes()).to_hex()
}

/// The cells of a stored CSV row: its text is the cells joined by `\t`, with each cell's start
/// (in characters) in `cell_starts`.
pub fn row_cells(text: &str, cell_starts: &[u32]) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let starts: &[u32] = if cell_starts.is_empty() {
        &[0]
    } else {
        cell_starts
    };
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let s = (s as usize).min(chars.len());
            let end = starts
                .get(i + 1)
                .map_or(chars.len(), |&n| (n as usize).saturating_sub(1))
                .max(s);
            (s, chars[s..end].iter().collect())
        })
        .collect()
}

/// Column key → type: a confirmed mapping (US-05.2), or the suggestions.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Assignments(pub BTreeMap<String, ColumnType>);

impl Assignments {
    pub fn get(&self, column_key: &str) -> Option<ColumnType> {
        self.0.get(column_key).copied()
    }

    /// The decision's `after` text: a JSON object `{column_key: type}`.
    pub fn to_json(&self) -> String {
        let map: serde_json::Map<String, serde_json::Value> = self
            .0
            .iter()
            .map(|(k, t)| (k.clone(), serde_json::Value::from(t.as_str())))
            .collect();
        serde_json::Value::Object(map).to_string()
    }

    /// Reads [`Assignments::to_json`]'s form; unknown types are left out.
    pub fn from_json(text: &str) -> Option<Self> {
        let serde_json::Value::Object(map) = serde_json::from_str(text).ok()? else {
            return None;
        };
        Some(Assignments(
            map.into_iter()
                .filter_map(|(k, v)| Some((k, ColumnType::from_name(v.as_str()?)?)))
                .collect(),
        ))
    }

    /// Only the columns in `keys`: what a changed file keeps of an older mapping (D5).
    pub fn restricted_to(&self, keys: &BTreeSet<&str>) -> Assignments {
        Assignments(
            self.0
                .iter()
                .filter(|(k, _)| keys.contains(k.as_str()))
                .map(|(k, t)| (k.clone(), *t))
                .collect(),
        )
    }
}

/// A table's mapping state (domain §7.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MappingState {
    /// Nothing confirmed: suggestions only, no table entities (INV-CM-03).
    Suggested,
    /// A confirmed mapping for these headers applies (INV-CM-04).
    Confirmed,
    /// The file's headers changed since its mapping was confirmed: the old assignments apply to
    /// the columns still there; the rest stay suggested until the user confirms (D5).
    NeedsReview,
}

impl MappingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            MappingState::Suggested => "suggested",
            MappingState::Confirmed => "confirmed",
            MappingState::NeedsReview => "needs_review",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Suggested, Self::Confirmed, Self::NeedsReview]
            .into_iter()
            .find(|s| s.as_str() == name)
    }
}

/// A logged confirm decision on one of the file's paths: its `seq` and the signature its anchor
/// names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConfirmFact<'a> {
    pub seq: i64,
    pub signature: &'a str,
}

/// The state of a table with `signature` given the confirm decisions on its paths, and the
/// decision that applies (its `seq`): the newest one for these headers (Confirmed), else the
/// newest one for the path (Needs review), else none (Suggested). Newest wins (INV-CU-05).
pub fn resolve_state(signature: &str, confirms: &[ConfirmFact<'_>]) -> (MappingState, Option<i64>) {
    if let Some(c) = confirms
        .iter()
        .filter(|c| c.signature == signature)
        .max_by_key(|c| c.seq)
    {
        return (MappingState::Confirmed, Some(c.seq));
    }
    match confirms.iter().max_by_key(|c| c.seq) {
        Some(c) => (MappingState::NeedsReview, Some(c.seq)),
        None => (MappingState::Suggested, None),
    }
}

/// The anchor of a confirm decision (domain §9.5 "file path + header signature"): the source
/// root id and the path in it, so it survives a relocate (D12). ASSUMPTION (M4c).
pub fn mapping_anchor(root: i64, rel_path: &str, signature: &str) -> String {
    format!("{}{signature}", mapping_anchor_prefix(root, rel_path))
}

/// What every confirm anchor of one path starts with.
pub fn mapping_anchor_prefix(root: i64, rel_path: &str) -> String {
    format!("map:{root}/{rel_path}|")
}

/// The signature a confirm anchor names.
pub fn anchor_signature(anchor: &str) -> Option<&str> {
    anchor
        .strip_prefix("map:")?
        .rsplit_once('|')
        .map(|(_, s)| s)
}

/// An entity mention a confirmed name column gives (US-05.3 AC1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntityHit {
    pub name_key: NameKey,
    /// The value as written, trimmed.
    pub raw: String,
    /// Characters of the segment text: inside the cell (CSV) or the value (JSON).
    pub span: Span,
}

/// The entity mention of one cell or JSON value, if its column is assigned a name type:
/// `cell_start` is where the cell starts in the segment text of `text_len` characters
/// (0 for a JSON value). Spaces around the value are not part of the span.
pub fn entity_in_cell(
    column_type: Option<ColumnType>,
    cell: &str,
    cell_start: usize,
    text_len: usize,
) -> Option<EntityHit> {
    let entity_type = column_type?.entity_type()?;
    let raw = cell.trim();
    let name_key = NameKey::new(entity_type, raw)?;
    let lead = cell.chars().take_while(|c| c.is_whitespace()).count();
    let start = cell_start + lead;
    let span = Span::new(start, start + raw.chars().count(), text_len).ok()?;
    Some(EntityHit {
        name_key,
        raw: raw.to_string(),
        span,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_keys_records_and_signatures() {
        assert_eq!(csv_column_key(Some(" supplier "), 1), "supplier");
        assert_eq!(csv_column_key(None, 0), "Column 1");
        assert_eq!(csv_column_key(Some(""), 4), "Column 5");
        assert_eq!(json_column_key("/officers/0/name"), "/officers/*/name");
        assert_eq!(json_column_key("/name"), "/name");
        assert_eq!(json_column_key("/0/1"), "/*/*");
        // INV-CM-05 (M4c note): the nearest array item, else the whole object.
        assert_eq!(record_of("/officers/0/name"), "/officers/0");
        assert_eq!(record_of("/a/2/b/3/c"), "/a/2/b/3");
        assert_eq!(record_of("/contact/email"), "");
        assert_eq!(record_of("/transactions/12"), "/transactions/12");
        let a = header_signature(["date", "supplier"], false);
        assert_eq!(a, header_signature(["date", "supplier"], false));
        assert_ne!(a, header_signature(["supplier", "date"], false));
        assert_eq!(
            header_signature(["/b", "/a"], true),
            header_signature(["/a", "/b", "/a"], true)
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn cells_of_a_stored_row() {
        let cells = row_cells("a\tbé\t\tdd", &[0, 2, 5, 6]);
        assert_eq!(
            cells,
            vec![
                (0, "a".into()),
                (2, "bé".into()),
                (5, "".into()),
                (6, "dd".into())
            ]
        );
        assert_eq!(row_cells("x", &[]), vec![(0, "x".into())]);
    }

    #[test]
    fn assignments_round_trip_and_restrict() {
        let a = Assignments(BTreeMap::from([
            ("supplier".to_string(), ColumnType::OrgName),
            ("date".to_string(), ColumnType::Date),
        ]));
        assert_eq!(Assignments::from_json(&a.to_json()), Some(a.clone()));
        let kept = a.restricted_to(&BTreeSet::from(["supplier"]));
        assert_eq!(kept.0.len(), 1);
        assert_eq!(kept.get("supplier"), Some(ColumnType::OrgName));
        assert!(Assignments::from_json("[1]").is_none());
    }

    /// Domain §7.3, D5: same headers → Confirmed with the newest; changed → Needs review.
    #[test]
    fn mapping_states() {
        let c = |seq, signature| ConfirmFact { seq, signature };
        assert_eq!(resolve_state("s1", &[]), (MappingState::Suggested, None));
        assert_eq!(
            resolve_state("s1", &[c(1, "s1"), c(4, "s1")]),
            (MappingState::Confirmed, Some(4))
        );
        assert_eq!(
            resolve_state("s2", &[c(1, "s1"), c(3, "s0")]),
            (MappingState::NeedsReview, Some(3))
        );
        assert_eq!(
            resolve_state("s1", &[c(1, "s1"), c(3, "s0")]),
            (MappingState::Confirmed, Some(1))
        );
        let anchor = mapping_anchor(3, "csv/a|b.csv", "abc");
        assert_eq!(anchor, "map:3/csv/a|b.csv|abc");
        assert!(anchor.starts_with(&mapping_anchor_prefix(3, "csv/a|b.csv")));
        assert_eq!(anchor_signature(&anchor), Some("abc"));
    }

    /// US-05.3 AC1: the span lies inside the cell, without its spaces.
    #[test]
    fn us_05_3_ac1_entity_span_inside_cell() {
        let text = "12.03.2024\t Northgate Marine Supplies \tx";
        let hit = entity_in_cell(
            Some(ColumnType::OrgName),
            " Northgate Marine Supplies ",
            11,
            40,
        )
        .unwrap();
        assert_eq!(hit.name_key.to_string(), "org:northgate marine supplies");
        let got: String = text
            .chars()
            .skip(hit.span.start())
            .take(hit.span.len())
            .collect();
        assert_eq!(got, "Northgate Marine Supplies");
        assert!(entity_in_cell(Some(ColumnType::Date), "x", 0, 1).is_none());
        assert!(entity_in_cell(None, "x", 0, 1).is_none());
        assert!(entity_in_cell(Some(ColumnType::PersonName), "  ", 0, 2).is_none());
    }
}
