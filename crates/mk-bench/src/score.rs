//! Layer 1 scoring (US-14.3 AC2, M4b): the identifiers an ingest found against the ground
//! truth, per type and per class (text-native, OCR'd), the negative checks, and the CI gate
//! (NFR-MNT-05). Pure: the caller ingests and maps what it found to ground-truth documents
//! and [`Place`]s (`mangkon-cli bench`).
//!
//! Matching, per document, each found mention used at most once except as noted
//! (`docs/07-test-corpus.md` §8):
//! 1. Text places (line, cell, paragraph, JSON value, header, MIME part) match on the same key
//!    at the same place and offsets.
//! 2. Pages (`pdf_text`, `scan`, `pdf_page`) match on the same key on the same page, as a
//!    multiset: the boxes are the page view's business (ASSUMPTION M4b).
//! 3. HTML sources and the MIME-part mentions left over match on the same key anywhere in the
//!    document, a found mention may stand for more than one of them: the ground truth counts
//!    offsets in the HTML source, the app in the extracted text, and an HTML alternative is
//!    stored as its plain part (US-02.6 AC3).
//!
//! Recall counts every annotated mention; precision only the found mentions in fully
//! annotated files (public files are partly annotated, §8). Custom patterns (M4c) and entities
//! (M6) are not scored here.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Annotation, GroundTruth, IdType, Location, Negative, ScanTier, Subject};

/// Where a mention is, in ground-truth terms.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Place {
    Line {
        line: u32,
        start: u32,
        end: u32,
    },
    /// `column` is the cell's index in its row; offsets are in the cell.
    Cell {
        row: u32,
        column: u32,
        start: u32,
        end: u32,
    },
    Paragraph {
        paragraph: u32,
        start: u32,
        end: u32,
    },
    Json {
        pointer: String,
        start: u32,
        end: u32,
    },
    /// `name` in lower case.
    Header {
        name: String,
        start: u32,
        end: u32,
    },
    Part {
        part: String,
        start: u32,
        end: u32,
    },
    Page {
        page: u32,
    },
    /// Somewhere in an HTML document's text.
    Html,
}

impl Place {
    /// The place of a ground-truth location.
    pub fn of(location: &Location) -> Place {
        match location.clone() {
            Location::TextLine { line, start, end } => Place::Line { line, start, end },
            Location::CsvCell {
                row,
                column,
                start,
                end,
            } => Place::Cell {
                row,
                column,
                start,
                end,
            },
            Location::DocxParagraph {
                paragraph,
                start,
                end,
            } => Place::Paragraph {
                paragraph,
                start,
                end,
            },
            Location::JsonValue {
                pointer,
                start,
                end,
            } => Place::Json {
                pointer,
                start,
                end,
            },
            Location::EmailHeader { header, start, end } => Place::Header {
                name: header.to_lowercase(),
                start,
                end,
            },
            Location::MimePart { part, start, end } => Place::Part { part, start, end },
            Location::PdfText { page, .. }
            | Location::Scan { page, .. }
            | Location::PdfPage { page } => Place::Page { page },
            Location::HtmlSource { .. } => Place::Html,
        }
    }

    fn page(&self) -> Option<u32> {
        match self {
            Place::Page { page } => Some(*page),
            _ => None,
        }
    }
}

/// Text-native or OCR'd: the two recall targets (NFR-QA-05/06).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Text,
    Ocr,
}

/// One identifier mention the app stored.
#[derive(Clone, PartialEq, Debug)]
pub struct FoundMention {
    /// The ground-truth document id it was mapped to.
    pub document: String,
    /// `type:value` as the app keys it (`phone_unnormalized:…` scores as a phone).
    pub key: String,
    pub raw: String,
    pub place: Place,
    pub class: Class,
}

/// What one bench run found.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Found {
    pub mentions: Vec<FoundMention>,
    /// Card-like flags as `(ground-truth document, last four digits)`.
    pub card_flags: Vec<(String, String)>,
    /// Every identifier key the app stored, in any document.
    pub keys: BTreeSet<String>,
    /// Every connection's key.
    pub connections: BTreeSet<String>,
    /// Stored documents that map to no ground-truth document, by path and position.
    pub unmapped: Vec<String>,
}

/// Counts of one cell of the report.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counts {
    /// Annotated mentions, and those found.
    pub expected: u32,
    pub recalled: u32,
    /// Found mentions in fully annotated files, and those that match an annotated one.
    pub found: u32,
    pub correct: u32,
}

impl Counts {
    /// Recall in percent; `None` with nothing expected.
    pub fn recall(&self) -> Option<f64> {
        (self.expected > 0).then(|| 100.0 * f64::from(self.recalled) / f64::from(self.expected))
    }

    /// Precision in percent; `None` with nothing found.
    pub fn precision(&self) -> Option<f64> {
        (self.found > 0).then(|| 100.0 * f64::from(self.correct) / f64::from(self.found))
    }

    fn add(&mut self, other: &Counts) {
        self.expected += other.expected;
        self.recalled += other.recalled;
        self.found += other.found;
        self.correct += other.correct;
    }
}

/// A target of `docs/03-nfr.md` and the value measured for it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Target {
    pub id: String,
    pub what: String,
    pub goal: f64,
    pub value: Option<f64>,
    pub met: bool,
}

/// The result of one bench run. Rows are keyed `type/class`, `type/all`, `all/class`,
/// `checksum/all`, `other/all`, `all/all`, and `all/ocr-<tier>` for the OCR'd mentions of each
/// scan tier (`clean`, `office`, `poor`; NFR-QA-06 is on `clean`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Report {
    pub rows: BTreeMap<String, Counts>,
    pub targets: Vec<Target>,
    /// Failed negative checks, by ground-truth id; never the value itself (NFR-SEC-14).
    pub negative_failures: Vec<String>,
    pub negatives_checked: u32,
    pub unmapped: Vec<String>,
    /// Annotated mentions not found, by ground-truth id.
    #[serde(default)]
    pub missed: Vec<String>,
}

/// Types whose values carry a checksum (NFR-QA-03).
const CHECKSUM: [IdType; 4] = [IdType::Iban, IdType::Btc, IdType::Evm, IdType::Tron];

fn type_name(t: IdType) -> &'static str {
    match t {
        IdType::Email => "email",
        IdType::Phone => "phone",
        IdType::Url => "url",
        IdType::Domain => "domain",
        IdType::Iban => "iban",
        IdType::Btc => "btc",
        IdType::Evm => "evm",
        IdType::Sol => "sol",
        IdType::Tron => "tron",
        IdType::Custom => "custom",
    }
}

fn tier_name(t: ScanTier) -> &'static str {
    match t {
        ScanTier::Clean => "clean",
        ScanTier::Office => "office",
        ScanTier::Poor => "poor",
    }
}

fn class_name(c: Class) -> &'static str {
    match c {
        Class::Text => "text",
        Class::Ocr => "ocr",
    }
}

/// The ground-truth type of an app key: `phone_unnormalized` is a phone that missed its
/// region. `None` for a type the ground truth does not have.
fn found_type(key: &str) -> Option<IdType> {
    Some(match key.split_once(':')?.0 {
        "email" => IdType::Email,
        "phone" | "phone_unnormalized" => IdType::Phone,
        "url" => IdType::Url,
        "domain" => IdType::Domain,
        "iban" => IdType::Iban,
        "btc" => IdType::Btc,
        "evm" => IdType::Evm,
        "sol" => IdType::Sol,
        "tron" => IdType::Tron,
        _ => return None,
    })
}

struct Expected<'a> {
    id: &'a str,
    id_type: IdType,
    key: String,
    place: Place,
    class: Class,
    exact: bool,
}

/// Scores `found` against `gt` (see the module comment for the rules).
pub fn score(gt: &GroundTruth, found: &Found) -> Report {
    let full: BTreeSet<&str> = gt
        .files
        .iter()
        .filter(|f| f.annotation == Annotation::Full)
        .map(|f| f.path.as_str())
        .collect();
    let full_docs: BTreeSet<&str> = gt
        .documents
        .iter()
        .filter(|d| full.contains(d.file.as_str()))
        .map(|d| d.id.as_str())
        .collect();
    let file_tier: BTreeMap<&str, ScanTier> = gt
        .files
        .iter()
        .filter_map(|f| Some((f.path.as_str(), f.scan_tier?)))
        .collect();
    let tier: BTreeMap<&str, ScanTier> = gt
        .documents
        .iter()
        .filter_map(|d| Some((d.id.as_str(), *file_tier.get(d.file.as_str())?)))
        .collect();

    let mut expected: BTreeMap<&str, Vec<Expected>> = BTreeMap::new();
    for m in &gt.mentions {
        let Subject::Identifier { key } = &m.subject else {
            continue;
        };
        if key.id_type == IdType::Custom {
            continue;
        }
        let place = Place::of(&m.location);
        expected
            .entry(m.document.as_str())
            .or_default()
            .push(Expected {
                id: &m.id,
                id_type: key.id_type,
                key: format!("{}:{}", type_name(key.id_type), key.value),
                exact: !matches!(place, Place::Page { .. } | Place::Html),
                class: if matches!(m.location, Location::Scan { .. }) {
                    Class::Ocr
                } else {
                    Class::Text
                },
                place,
            });
    }
    let mut by_doc: BTreeMap<&str, Vec<&FoundMention>> = BTreeMap::new();
    for f in &found.mentions {
        by_doc.entry(f.document.as_str()).or_default().push(f);
    }

    let mut cells: BTreeMap<(IdType, Class), Counts> = BTreeMap::new();
    let mut tiers: BTreeMap<ScanTier, Counts> = BTreeMap::new();
    let mut missed = Vec::new();
    let docs: BTreeSet<&str> = expected.keys().chain(by_doc.keys()).copied().collect();
    for doc in docs {
        let want = expected.get(doc).map_or(&[][..], Vec::as_slice);
        let got = by_doc.get(doc).map_or(&[][..], Vec::as_slice);
        let mut used = vec![false; got.len()];
        let mut hit = vec![false; want.len()];
        let mut take = |hit: &mut bool, pick: &dyn Fn(&FoundMention) -> bool, reuse: bool| {
            if *hit {
                return;
            }
            let free = (0..got.len()).find(|&i| !used[i] && pick(got[i]));
            let any = || (0..got.len()).find(|&i| pick(got[i]));
            if let Some(i) = free.or_else(|| if reuse { any() } else { None }) {
                used[i] = true;
                *hit = true;
            }
        };
        // 1. Exact text places.
        for (w, h) in want.iter().zip(hit.iter_mut()) {
            if w.exact {
                take(h, &|f| f.key == w.key && f.place == w.place, false);
            }
        }
        // 2. Pages.
        for (w, h) in want.iter().zip(hit.iter_mut()) {
            if let Some(page) = w.place.page() {
                take(
                    h,
                    &|f| f.key == w.key && f.place.page() == Some(page),
                    false,
                );
            }
        }
        // 3. HTML and MIME parts left over: the key anywhere in the document.
        for (w, h) in want.iter().zip(hit.iter_mut()) {
            if matches!(w.place, Place::Html | Place::Part { .. }) {
                take(h, &|f| f.key == w.key, true);
            }
        }
        let doc_tier = tier.get(doc).copied();
        for (w, h) in want.iter().zip(&hit) {
            let c = cells.entry((w.id_type, w.class)).or_default();
            c.expected += 1;
            c.recalled += u32::from(*h);
            if let (Class::Ocr, Some(t)) = (w.class, doc_tier) {
                let c = tiers.entry(t).or_default();
                c.expected += 1;
                c.recalled += u32::from(*h);
            }
            if !*h {
                missed.push(w.id.to_string());
            }
        }
        if full_docs.contains(doc) {
            for (f, u) in got.iter().zip(&used) {
                let Some(t) = found_type(&f.key) else {
                    continue;
                };
                let c = cells.entry((t, f.class)).or_default();
                c.found += 1;
                c.correct += u32::from(*u);
                if let (Class::Ocr, Some(t)) = (f.class, doc_tier) {
                    let c = tiers.entry(t).or_default();
                    c.found += 1;
                    c.correct += u32::from(*u);
                }
            }
        }
    }

    let mut rows = BTreeMap::new();
    for ((t, class), c) in &cells {
        let mut add = |key: String| rows.entry(key).or_insert_with(Counts::default).add(c);
        add(format!("{}/{}", type_name(*t), class_name(*class)));
        add(format!("{}/all", type_name(*t)));
        add(format!("all/{}", class_name(*class)));
        add(if CHECKSUM.contains(t) {
            "checksum/all".to_string()
        } else {
            "other/all".to_string()
        });
        add("all/all".to_string());
    }
    for (t, c) in &tiers {
        rows.insert(format!("all/ocr-{}", tier_name(*t)), *c);
    }
    let target = |id: &str, what: &str, goal: f64, value: Option<f64>| Target {
        id: id.into(),
        what: what.into(),
        goal,
        value,
        met: value.is_some_and(|v| v >= goal),
    };
    let row = |k: &str| rows.get(k).copied().unwrap_or_default();
    let targets = vec![
        target(
            "NFR-QA-03",
            "precision, checksum types",
            99.0,
            row("checksum/all").precision(),
        ),
        target(
            "NFR-QA-04",
            "precision, other types",
            95.0,
            row("other/all").precision(),
        ),
        target(
            "NFR-QA-05",
            "recall, text-native",
            95.0,
            row("all/text").recall(),
        ),
        target(
            "NFR-QA-06",
            "recall, OCR'd clean scans",
            85.0,
            row("all/ocr-clean").recall(),
        ),
    ];
    let (negative_failures, negatives_checked) = negatives(gt, found);
    Report {
        rows,
        targets,
        negative_failures,
        negatives_checked,
        unmapped: found.unmapped.clone(),
        missed,
    }
}

fn compact(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The negative checks: no bad-checksum value is stored (US-04.4 AC2, US-04.5 AC2); every
/// card-like number is a flag in its document and its digits are in no key (US-04.6,
/// INV-ID-06, NFR-SEC-14); a stop-list domain has no connection (US-04.3 AC3, INV-LK-03).
fn negatives(gt: &GroundTruth, found: &Found) -> (Vec<String>, u32) {
    let mut failures = Vec::new();
    let mut checked = 0;
    for n in &gt.negatives {
        let ok = match n {
            Negative::BadChecksum { raw, document, .. } => {
                let raw = compact(raw);
                !found
                    .mentions
                    .iter()
                    .any(|m| &m.document == document && compact(&m.raw) == raw)
            }
            Negative::CardLike {
                raw,
                last4,
                document,
                ..
            } => {
                let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
                let flagged = found
                    .card_flags
                    .iter()
                    .any(|(d, l)| d == document && l == last4);
                let in_key = found.keys.iter().any(|k| {
                    let k: String = k.chars().filter(char::is_ascii_digit).collect();
                    k.contains(&digits)
                });
                flagged && !in_key
            }
            Negative::StopListDomain { domain, .. } => {
                !found.connections.contains(&format!("domain:{domain}"))
            }
            Negative::DistinctEntities { .. } => continue,
        };
        checked += 1;
        if !ok {
            failures.push(n.id().to_string());
        }
    }
    (failures, checked)
}

/// A metric that fell by more than the allowed points against the baseline.
#[derive(Clone, PartialEq, Debug)]
pub struct Drop {
    pub row: String,
    pub metric: &'static str,
    pub baseline: f64,
    pub now: f64,
}

/// The CI gate (NFR-MNT-05): every recall and precision of `baseline` that `report` has
/// fallen below by more than `max_drop` points, or that `report` no longer has.
pub fn compare(report: &Report, baseline: &Report, max_drop: f64) -> Vec<Drop> {
    let mut drops = Vec::new();
    for (row, base) in &baseline.rows {
        let now = report.rows.get(row).copied().unwrap_or_default();
        for (metric, b, n) in [
            ("recall", base.recall(), now.recall()),
            ("precision", base.precision(), now.precision()),
        ] {
            let Some(b) = b else { continue };
            let n = n.unwrap_or(0.0);
            if b - n > max_drop {
                drops.push(Drop {
                    row: row.clone(),
                    metric,
                    baseline: b,
                    now: n,
                });
            }
        }
    }
    drops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn gt() -> GroundTruth {
        let file = |path: &str, annotation| FileEntry {
            path: path.into(),
            sha256: String::new(),
            bytes: 0,
            origin: Origin::Synthetic,
            kind: FileKind::Txt,
            pages: None,
            encoding: None,
            scan_tier: None,
            dpi: None,
            annotation,
            purpose: String::new(),
        };
        let doc = |id: &str, file: &str| Document {
            id: id.into(),
            file: file.into(),
            part: DocPart::Whole,
            kind: FileKind::Txt,
            parent: None,
            sha256: None,
        };
        let mention = |id: &str, doc: &str, t, v: &str, location| Mention {
            id: id.into(),
            document: doc.into(),
            subject: Subject::Identifier {
                key: IdentifierKey {
                    id_type: t,
                    value: v.into(),
                    pattern: None,
                },
            },
            raw: v.into(),
            location,
            difficulty: Difficulty::Normal,
            derived_from: None,
            note: None,
        };
        let line = |line, start, end| Location::TextLine { line, start, end };
        GroundTruth {
            schema_version: SCHEMA_VERSION,
            generator: Generator {
                name: "test".into(),
                version: 1,
                seed: 0,
            },
            page_text_file: String::new(),
            files: vec![
                file("a.txt", Annotation::Full),
                FileEntry {
                    scan_tier: Some(ScanTier::Clean),
                    ..file("p.pdf", Annotation::Partial)
                },
            ],
            documents: vec![doc("a", "a.txt"), doc("p", "p.pdf")],
            entities: Vec::new(),
            mentions: vec![
                mention("m1", "a", IdType::Email, "x@y.example", line(1, 0, 11)),
                mention(
                    "m2",
                    "a",
                    IdType::Iban,
                    "GB86QMTB60991248213377",
                    line(2, 5, 27),
                ),
                mention(
                    "m3",
                    "p",
                    IdType::Phone,
                    "+12025550147",
                    Location::Scan {
                        page: 2,
                        dpi: 300,
                        boxes: Vec::new(),
                    },
                ),
                mention(
                    "m4",
                    "a",
                    IdType::Url,
                    "https://y.example/",
                    Location::HtmlSource { start: 0, end: 9 },
                ),
            ],
            negatives: vec![
                Negative::CardLike {
                    id: "card-1".into(),
                    raw: "4111 1111 1111 1111".into(),
                    last4: "1111".into(),
                    document: "a".into(),
                    location: line(3, 0, 19),
                },
                Negative::StopListDomain {
                    id: "stop-1".into(),
                    domain: "gmail.com".into(),
                    document: "a".into(),
                    location: line(4, 0, 9),
                },
                Negative::BadChecksum {
                    id: "bad-1".into(),
                    claimed_type: IdType::Iban,
                    raw: "GB86 QMTB 6099 1248 2133 70".into(),
                    document: "a".into(),
                    location: line(5, 0, 27),
                },
            ],
            links: Vec::new(),
            merges: Vec::new(),
            relations: Vec::new(),
            events: Vec::new(),
            page_text: Vec::new(),
        }
    }

    fn found(document: &str, key: &str, place: Place, class: Class) -> FoundMention {
        FoundMention {
            document: document.into(),
            key: key.into(),
            raw: key.split_once(':').unwrap().1.into(),
            place,
            class,
        }
    }

    fn all_found() -> Found {
        Found {
            mentions: vec![
                found(
                    "a",
                    "email:x@y.example",
                    Place::Line {
                        line: 1,
                        start: 0,
                        end: 11,
                    },
                    Class::Text,
                ),
                found(
                    "a",
                    "iban:GB86QMTB60991248213377",
                    Place::Line {
                        line: 2,
                        start: 5,
                        end: 27,
                    },
                    Class::Text,
                ),
                found(
                    "p",
                    "phone:+12025550147",
                    Place::Page { page: 2 },
                    Class::Ocr,
                ),
                found("a", "url:https://y.example/", Place::Html, Class::Text),
            ],
            card_flags: vec![("a".into(), "1111".into())],
            ..Found::default()
        }
    }

    /// US-14.3 AC2: everything found at its place scores 100% in every row, and the negative
    /// checks pass.
    #[test]
    fn us_14_3_ac2_perfect_run() {
        let r = score(&gt(), &all_found());
        assert_eq!(r.rows["all/all"].recall(), Some(100.0));
        assert_eq!(r.rows["all/all"].precision(), Some(100.0));
        assert_eq!(r.rows["phone/ocr"].expected, 1);
        assert_eq!(r.rows["all/ocr-clean"].recall(), Some(100.0));
        assert!(!r.rows.contains_key("all/ocr-poor"));
        assert_eq!(
            r.rows["phone/ocr"].found, 0,
            "partly annotated: no precision"
        );
        assert!(r.negative_failures.is_empty());
        assert_eq!(r.negatives_checked, 3);
        assert!(
            r.targets
                .iter()
                .filter(|t| t.value.is_some())
                .all(|t| t.met)
        );
    }

    /// A text mention at other offsets is a miss and a false positive; a page mention on
    /// another page is a miss.
    #[test]
    fn wrong_place_misses() {
        let mut f = all_found();
        f.mentions[0].place = Place::Line {
            line: 1,
            start: 1,
            end: 11,
        };
        f.mentions[2].place = Place::Page { page: 3 };
        let r = score(&gt(), &f);
        assert_eq!(r.rows["email/text"].recalled, 0);
        assert_eq!(r.rows["email/text"].correct, 0);
        assert_eq!(r.rows["email/text"].found, 1);
        assert_eq!(r.rows["phone/ocr"].recalled, 0);
        assert_eq!(r.missed, ["m1", "m3"]);
        let qa06 = r.targets.iter().find(|t| t.id == "NFR-QA-06").unwrap();
        assert_eq!((qa06.value, qa06.met), (Some(0.0), false));
    }

    /// NFR-SEC-14, INV-LK-03, US-04.4 AC2: each negative fails on its own, and the failure
    /// names the negative, not the value.
    #[test]
    fn negatives_fail_by_id() {
        let mut f = all_found();
        f.card_flags.clear();
        f.keys.insert("phone:+4111111111111111".into());
        f.connections.insert("domain:gmail.com".into());
        f.mentions.push(found(
            "a",
            "iban:GB86QMTB60991248213370",
            Place::Line {
                line: 5,
                start: 0,
                end: 27,
            },
            Class::Text,
        ));
        f.mentions.last_mut().unwrap().raw = "GB86 QMTB 6099 1248 2133 70".into();
        let r = score(&gt(), &f);
        assert_eq!(r.negative_failures, ["card-1", "stop-1", "bad-1"]);
        assert!(!format!("{r:?}").contains("4111 1111"));
    }

    /// NFR-MNT-05: a drop of more than 2 points against the baseline trips the gate; 2 points
    /// or less, or a rise, does not.
    #[test]
    fn nfr_mnt_05_gate_trips_on_a_drop_over_two_points() {
        let base = score(&gt(), &all_found());
        assert!(compare(&base, &base, 2.0).is_empty());
        let mut f = all_found();
        f.mentions.remove(1);
        let worse = score(&gt(), &f);
        let drops = compare(&worse, &base, 2.0);
        assert!(
            drops
                .iter()
                .any(|d| d.row == "iban/all" && d.metric == "recall")
        );
        assert!(
            drops
                .iter()
                .any(|d| d.row == "all/text" && d.metric == "recall")
        );
        assert!(compare(&base, &worse, 2.0).is_empty(), "a rise passes");
        let mut nudged = base.clone();
        nudged.rows.get_mut("all/all").unwrap().expected = 100;
        nudged.rows.get_mut("all/all").unwrap().recalled = 98;
        assert!(compare(&nudged, &base, 2.0).is_empty(), "2 points pass");
        // The report round-trips through its JSON, as the baseline file is read back.
        let json = serde_json::to_string(&base).unwrap();
        assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), base);
    }

    /// An HTML alternative stored as its plain part: one found mention stands for both.
    #[test]
    fn html_and_part_mentions_match_by_key() {
        let mut g = gt();
        let mut alt = g.mentions[0].clone();
        alt.id = "m5".into();
        alt.location = Location::MimePart {
            part: "2".into(),
            start: 40,
            end: 51,
        };
        g.mentions.push(alt);
        let r = score(&g, &all_found());
        assert_eq!(r.rows["email/text"].expected, 2);
        assert_eq!(r.rows["email/text"].recalled, 2);
        assert_eq!(r.rows["email/text"].correct, 1);
    }
}
