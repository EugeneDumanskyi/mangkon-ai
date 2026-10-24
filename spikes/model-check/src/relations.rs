//! Optional relations probe (spike S1-P, informs the M8 go/no-go): the INV-AI-06 relation types
//! on the segments that hold the 20 gold relations. Informative only, no pass mark.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use mk_bench::keys::name_key;
use mk_bench::{EntityKind, GroundTruth};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::extract::{self, Attempt, Job, span_ok};
use crate::segments::Segment;

pub const TYPES: [&str; 6] = [
    "works_for",
    "owns",
    "director_of",
    "paid",
    "sent_to",
    "related_to",
];

pub const SYSTEM_PROMPT: &str = "You find relations between people and organizations in a text.

Relation types:
- works_for: a person works for an organization (employee, officer, partner, clerk).
- owns: a person or organization owns an organization (shareholder, parent company).
- director_of: a person is a director of an organization.
- paid: a person or organization paid money to another.
- sent_to: a person or organization sent a message or document to another.
- related_to: two people are family (for example in-laws).

For each relation the text states, give:
- \"type\": one of the types above.
- \"subject\" and \"object\": the two names exactly as written in the text (the payer, sender, owner, director or employee is the subject).
- \"evidence\": a short exact quote from the text (at most 20 words) that states the relation, copied character for character.

Only list relations the text states. Do not guess. If there are none, return an empty list.

Reply with compact JSON on one line, without indentation.";

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "relations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "type": {"type": "string", "enum": TYPES},
                        "subject": {"type": "string"},
                        "object": {"type": "string"},
                        "evidence": {"type": "string"}
                    },
                    "required": ["type", "subject", "object", "evidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["relations"],
        "additionalProperties": false
    })
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Rel {
    #[serde(rename = "type")]
    pub rel_type: String,
    pub subject: String,
    pub object: String,
    pub evidence: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    relations: Vec<Rel>,
}

pub fn parse(content: &str) -> Result<Vec<Rel>, String> {
    let r: Response = serde_json::from_str(content.trim()).map_err(|e| e.to_string())?;
    if let Some(bad) = r
        .relations
        .iter()
        .find(|x| !TYPES.contains(&x.rel_type.as_str()))
    {
        return Err(format!("unknown type {:?}", bad.rel_type));
    }
    Ok(r.relations)
}

fn parse_unit(c: &str) -> Result<(), String> {
    parse(c).map(|_| ())
}

#[derive(Serialize, Deserialize)]
pub struct Record {
    pub segment: String,
    pub attempts: Vec<Attempt>,
}

#[derive(Serialize, Default)]
pub struct RelScore {
    pub label: String,
    pub segments: usize,
    pub gold: usize,
    pub predicted: usize,
    pub span_discarded: usize,
    /// Predictions whose names do not resolve to a cast entity.
    pub unresolved: usize,
    pub correct: usize,
    /// Correct type and entities, either direction.
    pub correct_any_direction: usize,
    pub precision: f64,
    pub recall: f64,
    pub schema_failed: usize,
}

pub fn run(job: &Job, gt: &GroundTruth, segments: &[Segment]) -> Result<(), String> {
    // Segment holding both evidence mentions of each gold relation.
    let mut gold: HashMap<String, BTreeSet<(String, String, String)>> = HashMap::new();
    for r in &gt.relations {
        let t = serde_json::to_value(r.rel_type)
            .expect("serializes")
            .as_str()
            .unwrap_or("")
            .to_string();
        if let Some(s) = segments.iter().find(|s| {
            r.evidence
                .iter()
                .all(|m| s.gold.iter().any(|g| &g.mention == m))
        }) {
            gold.entry(s.id.clone())
                .or_default()
                .insert((t, r.subject.clone(), r.object.clone()));
        }
    }
    let segs: Vec<&Segment> = segments
        .iter()
        .filter(|s| gold.contains_key(&s.id))
        .collect();
    let items = segs
        .iter()
        .map(|s| (s.id.clone(), s.text.clone()))
        .collect();
    let schema = Arc::new(schema());
    extract::run_job(job, items, move |client, id, text| Record {
        segment: id.to_string(),
        attempts: extract::one(
            client,
            SYSTEM_PROMPT,
            &schema,
            "relations",
            text,
            parse_unit,
        ),
    })?;

    // Name → entity, through the name keys of every canonical name and alias.
    let mut entity_of: HashMap<String, String> = HashMap::new();
    for e in &gt.entities {
        for n in std::iter::once(&e.canonical).chain(&e.aliases) {
            entity_of.insert(name_key(e.kind, n), e.id.clone());
        }
    }
    let resolve = |name: &str| {
        [EntityKind::Person, EntityKind::Org]
            .iter()
            .find_map(|k| entity_of.get(&name_key(*k, name)).cloned())
    };
    let path = job.out.join(format!("relations-{}.jsonl", job.label));
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut s = RelScore {
        label: job.label.clone(),
        segments: segs.len(),
        gold: gold.values().map(BTreeSet::len).sum(),
        ..RelScore::default()
    };
    let seg_text: HashMap<&str, &str> = segs
        .iter()
        .map(|s| (s.id.as_str(), s.text.as_str()))
        .collect();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let rec: Record = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let Some(rels) = rec.attempts.last().and_then(|a| parse(&a.content).ok()) else {
            s.schema_failed += 1;
            continue;
        };
        let g = &gold[&rec.segment];
        let mut found = BTreeSet::new();
        for r in rels {
            s.predicted += 1;
            if !span_ok(seg_text[rec.segment.as_str()], &r.evidence) {
                s.span_discarded += 1;
                continue;
            }
            let (Some(a), Some(b)) = (resolve(&r.subject), resolve(&r.object)) else {
                s.unresolved += 1;
                continue;
            };
            let fwd = (r.rel_type.clone(), a.clone(), b.clone());
            if g.contains(&fwd) {
                found.insert(fwd);
            } else if g.contains(&(r.rel_type.clone(), b, a)) {
                s.correct_any_direction += 1;
            }
        }
        s.correct += found.len();
    }
    s.correct_any_direction += s.correct;
    s.precision = s.correct as f64 / s.predicted.max(1) as f64;
    s.recall = s.correct as f64 / s.gold.max(1) as f64;
    println!(
        "== relations {}: {} segments, gold {}, predicted {}, correct {} (any direction {}), span discarded {}, unresolved {}, schema failed {} → P {:.1}% R {:.1}%",
        s.label,
        s.segments,
        s.gold,
        s.predicted,
        s.correct,
        s.correct_any_direction,
        s.span_discarded,
        s.unresolved,
        s.schema_failed,
        s.precision * 100.0,
        s.recall * 100.0
    );
    std::fs::write(
        job.out.join(format!("relations-score-{}.json", job.label)),
        serde_json::to_string_pretty(&s).expect("serializes") + "\n",
    )
    .map_err(|e| e.to_string())
}
