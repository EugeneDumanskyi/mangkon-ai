//! Link scoring (NFR-QA-01, NFR-QA-02, M7a) and merge-proposal scoring (spike S7, K9).
//!
//! ASSUMPTION (M7a): NFR-QA-01 and NFR-QA-02 have no written formula, so this is the reading used.
//! A connection row counts over the ground truth's `links`:
//! - an identifier row matches the gold link of its key;
//! - an entity row maps through its names to a gold entity (its canonical name and aliases, by name
//!   key), and matches when it covers two or more of that gold link's documents;
//! - a row only in unannotated public files is counted but not judged, as in M6c.
//!
//! Precision is matching rows over judged rows; recall is gold links with a matching row over gold
//! links. A gold link found by two rows counts once for recall.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::keys::name_key;
use crate::{Annotation, GroundTruth, IdType, IdentifierKey, LinkSubject};

/// One connection row as the app shows it, in ground-truth terms.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FoundLink {
    /// The row's key: `type:value`, or an entity key.
    pub key: String,
    /// For an entity row: the name keys of its mentions. Empty for an identifier row.
    pub names: BTreeSet<String>,
    /// The ground-truth documents its counted mentions are in.
    pub documents: BTreeSet<String>,
}

/// A link score (NFR-QA-01, NFR-QA-02).
#[derive(Serialize, Clone, PartialEq, Debug, Default)]
pub struct LinkScore {
    pub gold: usize,
    pub gold_found: usize,
    /// Rows judged: with a document in an annotated file.
    pub judged: usize,
    pub correct: usize,
    /// Rows only in unannotated files.
    pub not_judged: usize,
    pub gold_identifier: usize,
    pub gold_identifier_found: usize,
    pub gold_entity: usize,
    pub gold_entity_found: usize,
    pub judged_identifier: usize,
    pub correct_identifier: usize,
    pub judged_entity: usize,
    pub correct_entity: usize,
    /// Gold links not found, by key (JSON only).
    pub missed: Vec<String>,
    /// Judged rows that match nothing, by key (JSON only).
    pub wrong: Vec<String>,
}

impl LinkScore {
    /// Percent; 100 with nothing to find.
    pub fn recall(&self) -> f64 {
        percent(self.gold_found, self.gold)
    }

    /// Percent; 100 with nothing judged.
    pub fn precision(&self) -> f64 {
        percent(self.correct, self.judged)
    }
}

fn percent(a: usize, b: usize) -> f64 {
    if b == 0 {
        100.0
    } else {
        a as f64 * 100.0 / b as f64
    }
}

fn id_type_name(t: IdType) -> &'static str {
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

/// The stored form of a gold identifier key.
pub fn stored_key(key: &IdentifierKey) -> String {
    match &key.pattern {
        Some(p) => format!("custom:{p}:{}", key.value),
        None => format!("{}:{}", id_type_name(key.id_type), key.value),
    }
}

/// Name key → the gold entities it can be (a name two entities use maps to both): each entity's
/// canonical name, aliases and merge variants.
pub fn entity_names(gt: &GroundTruth) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in &gt.entities {
        let variants = gt
            .merges
            .iter()
            .filter(|m| m.entity == e.id)
            .flat_map(|m| &m.variants);
        for raw in std::iter::once(&e.canonical)
            .chain(&e.aliases)
            .chain(variants)
        {
            out.entry(name_key(e.kind, raw))
                .or_default()
                .insert(e.id.clone());
        }
    }
    out
}

/// The documents of annotated files, those a row can be judged on.
pub fn annotated_documents(gt: &GroundTruth) -> BTreeSet<&str> {
    let annotated: BTreeSet<&str> = gt
        .files
        .iter()
        .filter(|f| f.annotation == Annotation::Full)
        .map(|f| f.path.as_str())
        .collect();
    gt.documents
        .iter()
        .filter(|d| annotated.contains(d.file.as_str()))
        .map(|d| d.id.as_str())
        .collect()
}

/// Scores connection rows against the ground truth's links.
pub fn score_links(gt: &GroundTruth, found: &[FoundLink]) -> LinkScore {
    let names = entity_names(gt);
    let annotated = annotated_documents(gt);
    let mut identifiers: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    let mut entities: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for l in &gt.links {
        let docs = l.documents.iter().map(String::as_str).collect();
        match &l.subject {
            LinkSubject::Identifier { key } => {
                identifiers.insert(stored_key(key), docs);
            }
            LinkSubject::Entity { entity } => {
                entities.insert(entity.as_str(), docs);
            }
        }
    }
    let mut report = LinkScore {
        gold_identifier: identifiers.len(),
        gold_entity: entities.len(),
        gold: identifiers.len() + entities.len(),
        ..LinkScore::default()
    };
    let mut hit_identifiers: BTreeSet<&str> = BTreeSet::new();
    let mut hit_entities: BTreeSet<&str> = BTreeSet::new();
    for row in found {
        let judged = row.documents.iter().any(|d| annotated.contains(d.as_str()));
        let is_entity = !row.names.is_empty();
        if !judged {
            report.not_judged += 1;
            continue;
        }
        let correct = if is_entity {
            let mut any = false;
            for name in &row.names {
                for id in names.get(name).into_iter().flatten() {
                    let Some((gold, docs)) = entities.get_key_value(id.as_str()) else {
                        continue;
                    };
                    let covered = docs.iter().filter(|d| row.documents.contains(**d)).count();
                    if covered >= 2 {
                        hit_entities.insert(gold);
                        any = true;
                    }
                }
            }
            any
        } else if let Some((gold, _)) = identifiers.get_key_value(&row.key) {
            hit_identifiers.insert(gold);
            true
        } else {
            false
        };
        report.judged += 1;
        if is_entity {
            report.judged_entity += 1;
        } else {
            report.judged_identifier += 1;
        }
        if correct {
            report.correct += 1;
            if is_entity {
                report.correct_entity += 1;
            } else {
                report.correct_identifier += 1;
            }
        } else {
            report.wrong.push(row.key.clone());
        }
    }
    report.gold_identifier_found = hit_identifiers.len();
    report.gold_entity_found = hit_entities.len();
    report.gold_found = hit_identifiers.len() + hit_entities.len();
    report.missed = identifiers
        .keys()
        .map(String::as_str)
        .filter(|k| !hit_identifiers.contains(k))
        .chain(
            entities
                .keys()
                .copied()
                .filter(|k| !hit_entities.contains(k)),
        )
        .map(str::to_string)
        .collect();
    report
}

/// One scored pair of name keys (spike S7).
#[derive(Clone, PartialEq, Debug)]
pub struct ScoredPair {
    pub a: String,
    pub b: String,
    pub score: f32,
    pub reasons: usize,
}

/// One threshold's result (spike S7).
#[derive(Serialize, Clone, PartialEq, Debug, Default)]
pub struct ProposalPoint {
    pub threshold: f32,
    pub proposals: usize,
    /// Pairs with both sides in annotated documents.
    pub judged: usize,
    pub correct: usize,
    /// Gold variants the model found that reach their entity's main key through proposals.
    pub variants_found: usize,
    pub variants_reached: usize,
    pub without_reason: usize,
}

impl ProposalPoint {
    pub fn precision(&self) -> f64 {
        percent(self.correct, self.judged)
    }

    pub fn coverage(&self) -> f64 {
        percent(self.variants_reached, self.variants_found)
    }

    /// S7's pass marks, fixed before measuring (ASSUMPTION, M7a).
    pub fn passes(&self) -> bool {
        self.coverage() >= 80.0
            && self.precision() >= 60.0
            && self.proposals <= 100
            && self.without_reason == 0
    }
}

/// One threshold of the sweep. `present` holds the name keys with shown mentions; `scored` those with
/// a mention in an annotated document. A pair is judged when both sides are in `scored`, and correct
/// when both are names of one gold entity, so a name the ground truth does not have counts as wrong,
/// and so do the lookalikes, which are entities of their own.
pub fn proposal_point(
    gt: &GroundTruth,
    pairs: &[ScoredPair],
    present: &BTreeSet<String>,
    scored: &BTreeSet<String>,
    threshold: f32,
) -> ProposalPoint {
    let names = entity_names(gt);
    let mut point = ProposalPoint {
        threshold,
        ..ProposalPoint::default()
    };
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in pairs.iter().filter(|p| p.score >= threshold) {
        point.proposals += 1;
        if p.reasons == 0 {
            point.without_reason += 1;
        }
        edges.entry(&p.a).or_default().push(&p.b);
        edges.entry(&p.b).or_default().push(&p.a);
        if !scored.contains(&p.a) || !scored.contains(&p.b) {
            continue;
        }
        point.judged += 1;
        if let (Some(ea), Some(eb)) = (names.get(&p.a), names.get(&p.b))
            && ea.intersection(eb).next().is_some()
        {
            point.correct += 1;
        }
    }
    for e in &gt.entities {
        let main = name_key(e.kind, &e.canonical);
        let found: BTreeSet<&str> = names
            .iter()
            .filter(|(key, ids)| ids.contains(&e.id) && present.contains(*key))
            .map(|(key, _)| key.as_str())
            .collect();
        // Without the main name in the project, the variants must reach one variant of theirs.
        let Some(target) = found
            .get(main.as_str())
            .copied()
            .or_else(|| found.iter().next().copied())
        else {
            continue;
        };
        let mut seen = BTreeSet::from([target]);
        let mut queue = vec![target];
        while let Some(at) = queue.pop() {
            for next in edges.get(at).into_iter().flatten() {
                if seen.insert(next) {
                    queue.push(next);
                }
            }
        }
        for v in found.iter().filter(|v| **v != target) {
            point.variants_found += 1;
            if seen.contains(v) {
                point.variants_reached += 1;
            }
        }
    }
    point
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn truth() -> GroundTruth {
        let file = |path: &str, annotation: &str| {
            json!({ "path": path, "sha256": "", "bytes": 0, "origin": "synthetic", "kind": "txt",
                    "annotation": annotation, "purpose": "" })
        };
        let doc = |id: &str, f: &str| json!({ "id": id, "file": f, "part": { "kind": "whole" }, "kind": "txt" });
        serde_json::from_value(json!({
            "schema_version": 1,
            "generator": { "name": "t", "version": 1, "seed": 1 },
            "page_text_file": "p.jsonl",
            "files": [file("a.txt", "full"), file("b.txt", "full"), file("c.txt", "full"), file("z.txt", "partial")],
            "documents": [doc("d1", "a.txt"), doc("d2", "b.txt"), doc("d3", "c.txt"), doc("d9", "z.txt")],
            "entities": [
                { "id": "corvin", "kind": "person", "canonical": "Corvin Hale", "aliases": ["C. Hale", "Mr Hale"], "role": "" },
                { "id": "gregor", "kind": "person", "canonical": "Gregor Almqvist", "aliases": [], "role": "" },
                { "id": "gregory", "kind": "person", "canonical": "Gregory Almquist", "aliases": [], "role": "" },
            ],
            "mentions": [], "negatives": [],
            "links": [
                { "subject": { "kind": "identifier", "key": { "type": "email", "value": "a@x.example" } },
                  "documents": ["d1", "d2"], "mentions": [] },
                { "subject": { "kind": "identifier", "key": { "type": "email", "value": "b@x.example" } },
                  "documents": ["d1", "d2"], "mentions": [] },
                { "subject": { "kind": "entity", "entity": "corvin" },
                  "documents": ["d1", "d2", "d3"], "mentions": [] },
            ],
            "merges": [{ "entity": "corvin", "variants": ["C. Hale", "Corvin Hale", "Mr Hale"] }],
            "relations": [], "events": [],
        }))
        .unwrap()
    }

    fn row(key: &str, names: &[&str], docs: &[&str]) -> FoundLink {
        FoundLink {
            key: key.into(),
            names: names.iter().map(|n| n.to_string()).collect(),
            documents: docs.iter().map(|d| d.to_string()).collect(),
        }
    }

    /// NFR-QA-01, NFR-QA-02: identifiers by key; an entity row by its names, over two or more of the
    /// gold documents; a row only in an unannotated file is counted, not judged.
    #[test]
    fn nfr_qa_01_02_link_score() {
        let s = score_links(
            &truth(),
            &[
                row("email:a@x.example", &[], &["d1", "d2"]),
                row("email:wrong@x.example", &[], &["d1", "d2"]),
                row("person:c hale", &["person:c hale"], &["d1", "d3"]),
                row("person:mr hale", &["person:mr hale"], &["d1", "d9"]),
                row("person:nobody", &["person:nobody"], &["d1", "d2"]),
                row("email:public@x.example", &[], &["d9"]),
            ],
        );
        assert_eq!((s.gold, s.gold_found), (3, 2));
        assert_eq!((s.judged, s.correct, s.not_judged), (5, 2, 1));
        assert_eq!(s.gold_entity_found, 1);
        assert_eq!(s.missed, ["email:b@x.example"]);
        assert_eq!(s.wrong.len(), 3);
        assert!((s.recall() - 66.66).abs() < 0.1);
        assert_eq!(s.precision(), 40.0);
        assert_eq!(score_links(&truth(), &[]).precision(), 100.0);
    }

    fn pair(a: &str, b: &str, score: f32) -> ScoredPair {
        ScoredPair {
            a: a.into(),
            b: b.into(),
            score,
            reasons: 1,
        }
    }

    /// S7: what a threshold keeps, how many variants reach the main key, and a lookalike or a name the
    /// ground truth does not have counts as wrong; a side only in unscored files is not judged.
    #[test]
    fn s7_sweep_point() {
        let set =
            |keys: &[&str]| -> BTreeSet<String> { keys.iter().map(|s| s.to_string()).collect() };
        let scored = set(&[
            "person:corvin hale",
            "person:c hale",
            "person:mr hale",
            "person:gregor almqvist",
            "person:gregory almquist",
        ]);
        let mut present = scored.clone();
        present.insert("person:other".to_string());
        let pairs = [
            pair("person:c hale", "person:corvin hale", 0.85),
            pair("person:c hale", "person:mr hale", 0.7),
            pair("person:gregor almqvist", "person:gregory almquist", 0.9),
            pair("person:c hale", "person:other", 0.65),
        ];
        let p = proposal_point(&truth(), &pairs, &present, &scored, 0.8);
        assert_eq!((p.proposals, p.judged, p.correct), (2, 2, 1));
        assert_eq!((p.variants_found, p.variants_reached), (2, 1));
        assert!(!p.passes(), "coverage 50");
        let p = proposal_point(&truth(), &pairs, &present, &scored, 0.6);
        assert_eq!((p.proposals, p.judged, p.correct), (4, 3, 2));
        assert_eq!((p.variants_found, p.variants_reached), (2, 2));
        assert_eq!(p.without_reason, 0);
        assert!(p.passes());

        // A name the ground truth does not have, in a scored document, is judged and wrong.
        let mut scored = scored;
        scored.insert("person:other".to_string());
        let p = proposal_point(&truth(), &pairs, &present, &scored, 0.6);
        assert_eq!((p.judged, p.correct), (4, 2));
        assert!(!p.passes(), "precision 50");

        let mut none = pair("person:c hale", "person:corvin hale", 0.9);
        none.reasons = 0;
        let p = proposal_point(&truth(), &[none], &present, &scored, 0.6);
        assert_eq!(p.without_reason, 1);
        assert_eq!(p.coverage(), 50.0);
        assert!(!p.passes());
    }
}
