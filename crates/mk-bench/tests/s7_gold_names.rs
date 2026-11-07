//! Spike S7 on TD-1's gold names, the repeatable input (K9): every name variant the ground truth
//! lists goes through the proposal scorer as if a perfect model had found it. The real model's run is
//! `mangkon-cli proposal-bench`. The corpus is not committed: see `corpus.rs` for when this skips.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mk_bench::links::{ScoredPair, annotated_documents, proposal_point};
use mk_bench::{GroundTruth, Subject};
use mk_graph::proposal::{Candidate, THRESHOLD, proposals};
use mk_kernel::EntityType;

fn ground_truth() -> Option<GroundTruth> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1/ground-truth.json");
    if !path.exists() {
        assert!(
            std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
            "testdata/td1 is missing"
        );
        eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
        return None;
    }
    Some(mk_bench::load(&path).expect("ground truth loads"))
}

fn scored(gt: &GroundTruth) -> (Vec<ScoredPair>, BTreeSet<String>, BTreeSet<String>) {
    let annotated = annotated_documents(gt);
    let doc_ids: BTreeMap<&str, i64> = gt
        .documents
        .iter()
        .enumerate()
        .map(|(i, d)| (d.id.as_str(), i as i64))
        .collect();
    let mut by_key: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut in_scored = BTreeSet::new();
    for m in &gt.mentions {
        if let Subject::Entity { name_key, .. } = &m.subject {
            by_key
                .entry(name_key.clone())
                .or_default()
                .insert(doc_ids[m.document.as_str()]);
            if annotated.contains(m.document.as_str()) {
                in_scored.insert(name_key.clone());
            }
        }
    }
    let present: BTreeSet<String> = by_key.keys().cloned().collect();
    let candidates: Vec<Candidate> = by_key
        .into_iter()
        .map(|(key, docs)| {
            let low = docs.iter().next().copied().into_iter();
            let high = docs.iter().next_back().copied().into_iter();
            Candidate {
                entity_type: if key.starts_with("org:") {
                    EntityType::Org
                } else {
                    EntityType::Person
                },
                name_keys: vec![key.clone()],
                key,
                documents: low
                    .chain(high)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                identifiers: BTreeMap::new(),
            }
        })
        .collect();
    let pairs = proposals(candidates, &BTreeSet::new(), 0.0, &AtomicBool::new(false))
        .into_iter()
        .map(|p| ScoredPair {
            a: p.a,
            b: p.b,
            score: p.score,
            reasons: p.reasons.len(),
        })
        .collect();
    (pairs, present, in_scored)
}

/// K9: at the threshold in use, the scorer's marks hold on what a perfect model would find.
#[test]
fn s7_gold_names_meet_the_marks() {
    let Some(gt) = ground_truth() else { return };
    let (pairs, present, in_scored) = scored(&gt);
    for t in [0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9] {
        let p = proposal_point(&gt, &pairs, &present, &in_scored, t);
        eprintln!(
            "{t:.2}: {} proposals, {} of {} correct ({:.1}%), {} of {} variants reached ({:.1}%)",
            p.proposals,
            p.correct,
            p.judged,
            p.precision(),
            p.variants_reached,
            p.variants_found,
            p.coverage()
        );
    }
    let p = proposal_point(&gt, &pairs, &present, &in_scored, THRESHOLD);
    assert!(p.passes(), "{p:?}");
}

/// The lookalikes the corpus plants (TD-1: Gregor and Gregory, two Brightwaters) may be proposed, but
/// count as wrong: a person decides those, and precision shows it.
#[test]
fn s7_lookalikes_count_as_wrong() {
    let Some(gt) = ground_truth() else { return };
    let (pairs, present, in_scored) = scored(&gt);
    let names = mk_bench::links::entity_names(&gt);
    let at = proposal_point(&gt, &pairs, &present, &in_scored, THRESHOLD);
    let wrong: Vec<&ScoredPair> = pairs
        .iter()
        .filter(|p| p.score >= THRESHOLD)
        .filter(|p| match (names.get(&p.a), names.get(&p.b)) {
            (Some(a), Some(b)) => a.intersection(b).next().is_none(),
            _ => true,
        })
        .collect();
    assert_eq!(at.judged - at.correct, wrong.len());
    for w in &wrong {
        eprintln!("{:.2}  {} | {}", w.score, w.a, w.b);
    }
}
