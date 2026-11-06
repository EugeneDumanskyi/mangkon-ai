//! People and organization scoring (NFR-QA-08, NFR-QA-09, `docs/09-model-check.md` §3.1, M6c): micro
//! precision and recall over the sets of name keys per cell, on what the app stored from real Stage
//! A text. A wrong type counts as one false positive and one false negative.
//!
//! A cell is (document, page) for PDF and scan documents and (document) for everything else, the
//! closest match to M1's segments. Public files have no name annotation and CSV and JSON gold is
//! Layer 1, so neither is scored. Names found in documents outside the score are ignored.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::keys::name_key;
use crate::{Document, EntityKind, FileKind, GroundTruth, Location, Origin, Subject, Unit};

/// The 500-page ledger: its row is reported on its own, to compare with M1's 29-page slice.
pub const LEDGER_DOC: &str = "d-ledger";

/// Mentions at or above this are the ones shown by default (INV-AI-05).
pub const SHOWN_CONFIDENCE: f32 = 0.5;

/// The sources a report has a row for, in print order.
pub const SOURCES: [&str; 8] = [
    "text PDF",
    "ledger",
    "scan",
    "OCR layer",
    "notes",
    "email",
    "DOCX",
    "HTML",
];

/// One name the app stored.
#[derive(Clone, PartialEq, Debug)]
pub struct FoundName {
    /// The ground-truth document id.
    pub document: String,
    pub page: Option<u32>,
    pub key: String,
    /// The name as written.
    pub raw: String,
    pub confidence: f32,
}

#[derive(Serialize, Default, Debug, Clone, Copy, PartialEq)]
pub struct Prf {
    pub tp: u64,
    pub fp: u64,
    #[serde(rename = "fn")]
    pub fn_: u64,
}

impl Prf {
    fn add_sets(&mut self, gold: &BTreeSet<String>, pred: &BTreeSet<String>) {
        self.tp += gold.intersection(pred).count() as u64;
        self.fp += pred.difference(gold).count() as u64;
        self.fn_ += gold.difference(pred).count() as u64;
    }

    /// Percent; 100 with nothing found.
    pub fn precision(&self) -> f64 {
        ratio(self.tp, self.tp + self.fp)
    }

    /// Percent; 100 with nothing to find.
    pub fn recall(&self) -> f64 {
        ratio(self.tp, self.tp + self.fn_)
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 {
        100.0
    } else {
        a as f64 * 100.0 / b as f64
    }
}

/// One row of a report: over all stored mentions and over those shown by default.
#[derive(Serialize, Default, Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub all: Prf,
    pub shown: Prf,
}

/// NFR-QA-01/02 estimate at name-key level: a link is a name key found in two or more documents.
#[derive(Serialize, Default, Debug, Clone, PartialEq)]
pub struct LinkEstimate {
    pub gold: usize,
    pub predicted: usize,
    pub correct: usize,
}

impl LinkEstimate {
    pub fn precision(&self) -> f64 {
        ratio(self.correct as u64, self.predicted as u64)
    }

    pub fn recall(&self) -> f64 {
        ratio(self.correct as u64, self.gold as u64)
    }
}

#[derive(Serialize, Default, Debug, Clone, PartialEq)]
pub struct NameReport {
    /// Cells with a gold name or a found name.
    pub cells: usize,
    pub gold_names: usize,
    pub overall: Row,
    pub person: Row,
    pub org: Row,
    /// Type ignored: both sides normalized the same way, without the prefix.
    pub type_agnostic: Row,
    pub by_source: BTreeMap<String, Row>,
    pub links_all: LinkEstimate,
    pub links_shown: LinkEstimate,
    /// Shown names no gold has, most cells first (name key, cells). JSON only: the names are
    /// synthetic, but they are long lists.
    pub false_positives: Vec<(String, usize)>,
    /// Gold names not shown, most cells first (name key, cells).
    pub missed: Vec<(String, usize)>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Cell {
    document: String,
    page: Option<u32>,
}

/// Name keys found in one cell.
#[derive(Default)]
struct Keys {
    /// Name keys and, type ignored, the normalized names.
    keys: BTreeSet<String>,
    agnostic: BTreeSet<String>,
}

impl Keys {
    fn add(&mut self, key: &str, raw: &str) {
        self.keys.insert(key.to_string());
        self.agnostic.insert(agnostic(raw));
    }
}

fn agnostic(raw: &str) -> String {
    let key = name_key(EntityKind::Org, raw);
    key.strip_prefix("org:").unwrap_or(&key).to_string()
}

fn of_type(keys: &BTreeSet<String>, prefix: &str) -> BTreeSet<String> {
    keys.iter()
        .filter(|k| k.starts_with(prefix))
        .cloned()
        .collect()
}

fn paged(kind: FileKind) -> bool {
    matches!(
        kind,
        FileKind::PdfText
            | FileKind::PdfScan
            | FileKind::PdfMixed
            | FileKind::PdfOcrLayer
            | FileKind::Image
    )
}

/// The documents that are scored, by id.
fn scored(gt: &GroundTruth) -> BTreeMap<&str, &Document> {
    let synthetic: BTreeSet<&str> = gt
        .files
        .iter()
        .filter(|f| f.origin == Origin::Synthetic)
        .map(|f| f.path.as_str())
        .collect();
    gt.documents
        .iter()
        .filter(|d| {
            synthetic.contains(d.file.as_str()) && !matches!(d.kind, FileKind::Csv | FileKind::Json)
        })
        .map(|d| (d.id.as_str(), d))
        .collect()
}

fn source_of(doc: &Document, page_is_scan: bool) -> &'static str {
    if doc.id == LEDGER_DOC {
        return "ledger";
    }
    match doc.kind {
        FileKind::PdfOcrLayer => "OCR layer",
        FileKind::Image | FileKind::PdfScan => "scan",
        FileKind::PdfMixed if page_is_scan => "scan",
        FileKind::PdfText | FileKind::PdfMixed => "text PDF",
        FileKind::Txt | FileKind::Md => "notes",
        FileKind::Eml | FileKind::Mbox => "email",
        FileKind::Docx => "DOCX",
        FileKind::Html => "HTML",
        FileKind::Csv | FileKind::Json => unreachable!("not scored"),
    }
}

fn top(counts: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

fn links(
    gold: &BTreeMap<String, BTreeSet<&str>>,
    pred: &BTreeMap<String, BTreeSet<&str>>,
) -> LinkEstimate {
    let linked = |m: &BTreeMap<String, BTreeSet<&str>>| -> BTreeSet<String> {
        m.iter()
            .filter(|(_, d)| d.len() >= 2)
            .map(|(k, _)| k.clone())
            .collect()
    };
    let (g, p) = (linked(gold), linked(pred));
    LinkEstimate {
        gold: g.len(),
        predicted: p.len(),
        correct: g.intersection(&p).count(),
    }
}

/// Scores the names found against the ground truth.
pub fn score(gt: &GroundTruth, found: &[FoundName]) -> NameReport {
    let docs = scored(gt);
    let scan_pages: BTreeSet<(&str, u32)> = gt
        .page_text
        .iter()
        .filter(|p| p.unit == Unit::Px)
        .map(|p| (p.document.as_str(), p.page))
        .collect();

    let mut report = NameReport::default();
    let mut gold: BTreeMap<Cell, Keys> = BTreeMap::new();
    for m in &gt.mentions {
        let Subject::Entity { name_key, .. } = &m.subject else {
            continue;
        };
        if !docs.contains_key(m.document.as_str()) {
            continue;
        }
        let page = match m.location {
            Location::PdfText { page, .. }
            | Location::Scan { page, .. }
            | Location::PdfPage { page } => Some(page),
            _ => None,
        };
        let cell = Cell {
            document: m.document.clone(),
            page,
        };
        gold.entry(cell).or_default().add(name_key, &m.raw);
    }
    let (mut all, mut shown): (BTreeMap<Cell, Keys>, BTreeMap<Cell, Keys>) = Default::default();
    for f in found {
        let Some(doc) = docs.get(f.document.as_str()) else {
            continue;
        };
        let cell = Cell {
            document: f.document.clone(),
            page: if paged(doc.kind) { f.page } else { None },
        };
        all.entry(cell.clone()).or_default().add(&f.key, &f.raw);
        if f.confidence >= SHOWN_CONFIDENCE {
            shown.entry(cell).or_default().add(&f.key, &f.raw);
        }
    }

    let cells: BTreeSet<&Cell> = gold.keys().chain(all.keys()).collect();
    report.cells = cells.len();
    for source in SOURCES {
        report.by_source.insert(source.to_string(), Row::default());
    }
    let empty = Keys::default();
    let (mut fp, mut missed): (BTreeMap<String, usize>, BTreeMap<String, usize>) =
        Default::default();
    let mut gold_docs: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    let mut all_docs: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    let mut shown_docs: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for cell in cells {
        let g = gold.get(cell).unwrap_or(&empty);
        let a = all.get(cell).unwrap_or(&empty);
        let s = shown.get(cell).unwrap_or(&empty);
        report.gold_names += g.keys.len();
        let scan = cell
            .page
            .is_some_and(|p| scan_pages.contains(&(cell.document.as_str(), p)));
        let source = source_of(docs[cell.document.as_str()], scan);
        let by_source = report.by_source.entry(source.to_string()).or_default();
        for row in [&mut report.overall, by_source] {
            row.all.add_sets(&g.keys, &a.keys);
            row.shown.add_sets(&g.keys, &s.keys);
        }
        for (row, prefix) in [(&mut report.person, "person:"), (&mut report.org, "org:")] {
            let gold_part = of_type(&g.keys, prefix);
            row.all.add_sets(&gold_part, &of_type(&a.keys, prefix));
            row.shown.add_sets(&gold_part, &of_type(&s.keys, prefix));
        }
        report.type_agnostic.all.add_sets(&g.agnostic, &a.agnostic);
        report
            .type_agnostic
            .shown
            .add_sets(&g.agnostic, &s.agnostic);
        for k in s.keys.difference(&g.keys) {
            *fp.entry(k.clone()).or_default() += 1;
        }
        for k in g.keys.difference(&s.keys) {
            *missed.entry(k.clone()).or_default() += 1;
        }
        for (map, keys) in [
            (&mut gold_docs, &g.keys),
            (&mut all_docs, &a.keys),
            (&mut shown_docs, &s.keys),
        ] {
            for k in keys {
                map.entry(k.clone())
                    .or_default()
                    .insert(cell.document.as_str());
            }
        }
    }
    report.links_all = links(&gold_docs, &all_docs);
    report.links_shown = links(&gold_docs, &shown_docs);
    report.false_positives = top(fp);
    report.missed = top(missed);
    report
}

/// The middle value; the lower of the middle two with an even count. `None` with no values.
pub fn median(values: &[f64]) -> Option<f64> {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(v.len().saturating_sub(1) / 2).copied()
}

/// M1's 50 hand-written meaning queries (synthetic and public topics only, NFR-LEG-04).
pub fn hand_written_queries() -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Query {
        text: String,
    }
    let all: Vec<Query> =
        serde_json::from_str(include_str!("../data/queries.json")).expect("queries parse");
    all.into_iter().map(|q| q.text).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entity(
        id: &str,
        doc: &str,
        kind: &str,
        raw: &str,
        location: serde_json::Value,
    ) -> serde_json::Value {
        let kind = if kind == "org" {
            EntityKind::Org
        } else {
            EntityKind::Person
        };
        json!({
            "id": id, "document": doc, "raw": raw, "location": location, "difficulty": "normal",
            "subject": { "kind": "entity", "entity": raw, "name_key": name_key(kind, raw) },
        })
    }

    /// Four documents: a text PDF d1 and an email d2 with gold names, a note d3 without, a CSV d4
    /// whose gold is not scored; and a public file d5.
    fn truth() -> GroundTruth {
        let page = |p: u32| json!({ "kind": "pdf_text", "page": p, "boxes": [] });
        let part = json!({ "kind": "mime_part", "part": "1", "start": 0, "end": 1 });
        let csv = json!({ "kind": "csv_cell", "row": 1, "column": 0, "start": 0, "end": 1 });
        let file = |path: &str, origin: &str, kind: &str| {
            json!({ "path": path, "sha256": "", "bytes": 0, "origin": origin, "kind": kind,
                    "annotation": "full", "purpose": "" })
        };
        let doc = |id: &str, f: &str, kind: &str| json!({ "id": id, "file": f, "part": { "kind": "whole" }, "kind": kind });
        serde_json::from_value(json!({
            "schema_version": 1,
            "generator": { "name": "t", "version": 1, "seed": 1 },
            "page_text_file": "p.jsonl",
            "files": [
                file("a.pdf", "synthetic", "pdf_text"),
                file("b.eml", "synthetic", "eml"),
                file("c.txt", "synthetic", "txt"),
                file("d.csv", "synthetic", "csv"),
                file("e.pdf", "public", "pdf_text"),
            ],
            "documents": [
                doc("d1", "a.pdf", "pdf_text"),
                doc("d2", "b.eml", "eml"),
                doc("d3", "c.txt", "txt"),
                doc("d4", "d.csv", "csv"),
                doc("d5", "e.pdf", "pdf_text"),
            ],
            "entities": [],
            "mentions": [
                entity("m1", "d1", "org", "BRIGHTWATER LOGISTICS LTD", page(1)),
                entity("m2", "d1", "org", "Velmora Port Authority", page(1)),
                entity("m3", "d1", "person", "Mr Hale", page(2)),
                entity("m4", "d2", "org", "Velmora Port Authority", part),
                entity("m5", "d4", "person", "Only In Csv", csv),
            ],
            "negatives": [], "links": [], "merges": [], "relations": [], "events": [],
        }))
        .unwrap()
    }

    fn found(
        document: &str,
        page: Option<u32>,
        kind: EntityKind,
        raw: &str,
        conf: f32,
    ) -> FoundName {
        FoundName {
            document: document.into(),
            page,
            key: name_key(kind, raw),
            raw: raw.into(),
            confidence: conf,
        }
    }

    fn prf(tp: u64, fp: u64, fn_: u64) -> Prf {
        Prf { tp, fp, fn_ }
    }

    /// M1's hand-built case, on cells.
    #[test]
    fn nfr_qa_08_09_scoring_hand_built() {
        use EntityKind::{Org, Person};
        let r = score(
            &truth(),
            &[
                // Suffix normalized: a hit. A wrong type: one false positive and one miss.
                found("d1", Some(1), Org, "Brightwater Logistics Limited", 0.9),
                found("d1", Some(1), Person, "Velmora Port Authority", 0.4),
                // Right document, right name, wrong page: not a hit.
                found("d1", Some(3), Person, "Mr Hale", 0.9),
                found("d2", None, Org, "Velmora Port Authority", 0.8),
                // In a document with no gold names, under 0.5.
                found("d3", None, Org, "Nothing", 0.2),
                // Outside the score: CSV gold and a public file.
                found("d4", None, Person, "Only In Csv", 0.9),
                found("d5", Some(1), Org, "Public Name", 0.9),
            ],
        );
        assert_eq!(r.gold_names, 4);
        assert_eq!(r.overall.all, prf(2, 3, 2));
        // Shown: the wrong type and the note go away.
        assert_eq!(r.overall.shown, prf(2, 1, 2));
        assert_eq!(r.org.all, prf(2, 1, 1));
        assert_eq!(r.person.all, prf(0, 2, 1));
        assert_eq!(r.by_source["email"].shown, prf(1, 0, 0));
        assert_eq!(r.by_source["text PDF"].shown, prf(1, 1, 2));
        assert_eq!(r.by_source["notes"].all, prf(0, 1, 0));
        assert_eq!(r.by_source["notes"].shown, prf(0, 0, 0));
        // Type ignored: Velmora on page 1 is a hit.
        assert_eq!(r.type_agnostic.all, prf(3, 2, 1));
        // Gold: Velmora in d1 and d2. All: org Velmora only in d2, Mr Hale only in d1.
        assert_eq!(
            r.links_all,
            LinkEstimate {
                gold: 1,
                predicted: 0,
                correct: 0
            }
        );
        assert_eq!(r.false_positives[0].1, 1);
        assert!(r.missed.iter().any(|(k, _)| k == "person:mr hale"));
        assert!(!r.missed.iter().any(|(k, _)| k.contains("only in csv")));
    }

    #[test]
    fn nothing_found_is_all_missed_and_full_precision() {
        let r = score(&truth(), &[]);
        assert_eq!(r.overall.shown, prf(0, 0, 4));
        assert_eq!(r.overall.shown.recall(), 0.0);
        assert_eq!(r.overall.shown.precision(), 100.0);
    }

    #[test]
    fn invented_names_lower_precision_only() {
        use EntityKind::Org;
        let right = [
            found("d1", Some(1), Org, "Brightwater Logistics", 0.9),
            found("d1", Some(1), Org, "Velmora Port Authority", 0.9),
            found("d1", Some(2), EntityKind::Person, "Mr Hale", 0.9),
            found("d2", None, Org, "Velmora Port Authority", 0.9),
        ];
        let clean = score(&truth(), &right);
        assert_eq!(
            (
                clean.overall.shown.precision(),
                clean.overall.shown.recall()
            ),
            (100.0, 100.0)
        );
        let mut more = right.to_vec();
        more.push(found("d3", None, Org, "Invented Holdings", 0.9));
        let noisy = score(&truth(), &more);
        assert_eq!(noisy.overall.shown.precision(), 80.0);
        assert_eq!(noisy.overall.shown.recall(), 100.0);
    }

    #[test]
    fn median_of_runs() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 2.0]), Some(2.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn the_queries_are_all_there() {
        assert_eq!(hand_written_queries().len(), 50);
    }
}
