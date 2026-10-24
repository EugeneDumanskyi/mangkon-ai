//! Extraction scoring (`docs/09-model-check.md` §3.1): micro precision and recall over the
//! per-segment name-key sets, after the strict parse (INV-AI-03) and the span check (INV-AI-01).
//! A wrong type counts as one false positive and one false negative.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mk_bench::EntityKind;
use mk_bench::keys::name_key;
use serde::{Deserialize, Serialize};

use crate::extract::{self, Record, RunMeta, normalize_ws, span_ok};
use crate::segments::{Segment, Source};

#[derive(Serialize, Deserialize, Default, Debug, Clone, Copy, PartialEq)]
pub struct Prf {
    pub tp: u64,
    pub fp: u64,
    #[serde(rename = "fn")]
    pub fn_: u64,
}

impl Prf {
    pub fn add_sets(&mut self, gold: &BTreeSet<String>, pred: &BTreeSet<String>) {
        self.tp += gold.intersection(pred).count() as u64;
        self.fp += pred.difference(gold).count() as u64;
        self.fn_ += gold.difference(pred).count() as u64;
    }
    pub fn precision(&self) -> f64 {
        ratio(self.tp, self.tp + self.fp)
    }
    pub fn recall(&self) -> f64 {
        ratio(self.tp, self.tp + self.fn_)
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 { 1.0 } else { a as f64 / b as f64 }
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Score {
    pub label: String,
    pub segments: usize,
    pub missing_records: usize,
    pub overall: Prf,
    /// Only mentions with confidence ≥ 0.5, the ones shown by default (INV-AI-05).
    pub shown: Prf,
    pub person: Prf,
    pub org: Prf,
    /// Diagnostic for M6: predictions whose name looks like an identifier (no whitespace, holds
    /// `@` or `.`, e.g. `velmora-port.example`) dropped, as a Stage A overlap rule would.
    pub id_like_dropped: Prf,
    pub id_like: usize,
    /// Type ignored: both sides keyed as `name_key(org, …)` without the prefix.
    pub type_agnostic: Prf,
    pub by_source: BTreeMap<String, Prf>,
    /// First attempt parsed / parsed after the retry / skipped after the retry.
    pub schema_ok_first: usize,
    pub schema_ok_retry: usize,
    pub schema_failed: usize,
    pub predictions: usize,
    /// INV-AI-01 discards: evidence not in the segment text.
    pub span_discarded: usize,
    /// Kept predictions whose `name` itself is not in the segment text (whitespace-normalized).
    pub name_not_in_segment: usize,
    pub prompt_tokens_mean: f64,
    pub completion_tokens_mean: f64,
    pub truncated: usize,
    pub links: LinkEstimate,
    pub top_fp: Vec<(String, usize)>,
    pub top_fn: Vec<(String, usize)>,
}

/// NFR-QA-01/02 estimate at name-key level: a link is a name key found in ≥ 2 documents.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct LinkEstimate {
    pub gold: usize,
    pub predicted: usize,
    pub correct: usize,
    pub precision: f64,
    pub recall: f64,
}

fn agnostic(name: &str) -> String {
    let k = name_key(EntityKind::Org, name);
    k.strip_prefix("org:").unwrap_or(&k).to_string()
}

pub fn id_like(name: &str) -> bool {
    let n = name.trim();
    !n.contains(char::is_whitespace) && (n.contains('@') || n.trim_end_matches('.').contains('.'))
}

fn kind_of(key: &str) -> EntityKind {
    if key.starts_with("org:") {
        EntityKind::Org
    } else {
        EntityKind::Person
    }
}

fn split(set: &BTreeSet<String>, kind: EntityKind) -> BTreeSet<String> {
    set.iter().filter(|k| kind_of(k) == kind).cloned().collect()
}

fn top(counts: HashMap<String, usize>, n: usize) -> Vec<(String, usize)> {
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

pub fn score(label: &str, segments: &[Segment], records: &[Record]) -> Score {
    let by_id: HashMap<&str, &Record> = records.iter().map(|r| (r.segment.as_str(), r)).collect();
    let mut s = Score {
        label: label.to_string(),
        segments: segments.len(),
        ..Score::default()
    };
    let (mut fp_count, mut fn_count) = (HashMap::new(), HashMap::new());
    let mut gold_docs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pred_docs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let (mut prompt, mut completion, mut firsts) = (0u64, 0u64, 0u64);

    for seg in segments {
        let gold = seg.gold_keys();
        let mut pred = BTreeSet::new();
        let mut pred_shown = BTreeSet::new();
        let mut pred_agn = BTreeSet::new();
        let mut pred_no_id = BTreeSet::new();
        match by_id.get(seg.id.as_str()) {
            None => s.missing_records += 1,
            Some(rec) => {
                if let Some(a) = rec.attempts.first() {
                    prompt += a.usage.prompt_tokens;
                    completion += a.usage.completion_tokens;
                    firsts += 1;
                }
                s.truncated += rec
                    .attempts
                    .iter()
                    .filter(|a| a.finish_reason.as_deref() == Some("length"))
                    .count();
                match (rec.attempts.first().map(|a| a.error.is_none()), rec.preds()) {
                    (Some(true), _) => s.schema_ok_first += 1,
                    (_, Some(_)) => s.schema_ok_retry += 1,
                    _ => s.schema_failed += 1,
                }
                let text = normalize_ws(&seg.text);
                for p in rec.preds().unwrap_or_default() {
                    s.predictions += 1;
                    if !span_ok(&seg.text, &p.evidence) {
                        s.span_discarded += 1;
                        continue;
                    }
                    if !text.contains(&normalize_ws(&p.name)) {
                        s.name_not_in_segment += 1;
                    }
                    let key = name_key(p.kind, &p.name);
                    if id_like(&p.name) {
                        s.id_like += 1;
                    } else {
                        pred_no_id.insert(key.clone());
                    }
                    if p.confidence >= 0.5 {
                        pred_shown.insert(key.clone());
                    }
                    pred.insert(key);
                    pred_agn.insert(agnostic(&p.name));
                }
            }
        }
        s.overall.add_sets(&gold, &pred);
        s.shown.add_sets(&gold, &pred_shown);
        s.id_like_dropped.add_sets(&gold, &pred_no_id);
        s.person.add_sets(
            &split(&gold, EntityKind::Person),
            &split(&pred, EntityKind::Person),
        );
        s.org.add_sets(
            &split(&gold, EntityKind::Org),
            &split(&pred, EntityKind::Org),
        );
        let gold_agn: BTreeSet<String> = seg.gold.iter().map(|g| agnostic(&g.raw)).collect();
        s.type_agnostic.add_sets(&gold_agn, &pred_agn);
        s.by_source
            .entry(seg.source.name().to_string())
            .or_default()
            .add_sets(&gold, &pred);
        for k in pred.difference(&gold) {
            *fp_count.entry(k.clone()).or_insert(0) += 1;
        }
        for k in gold.difference(&pred) {
            *fn_count.entry(k.clone()).or_insert(0) += 1;
        }
        for k in &gold {
            gold_docs
                .entry(k.clone())
                .or_default()
                .insert(seg.document.clone());
        }
        for k in &pred {
            pred_docs
                .entry(k.clone())
                .or_default()
                .insert(seg.document.clone());
        }
    }
    let linked = |m: &BTreeMap<String, BTreeSet<String>>| -> BTreeSet<String> {
        m.iter()
            .filter(|(_, d)| d.len() >= 2)
            .map(|(k, _)| k.clone())
            .collect()
    };
    let (gl, pl) = (linked(&gold_docs), linked(&pred_docs));
    let correct = gl.intersection(&pl).count();
    s.links = LinkEstimate {
        gold: gl.len(),
        predicted: pl.len(),
        correct,
        precision: ratio(correct as u64, pl.len() as u64),
        recall: ratio(correct as u64, gl.len() as u64),
    };
    s.prompt_tokens_mean = prompt as f64 / firsts.max(1) as f64;
    s.completion_tokens_mean = completion as f64 / firsts.max(1) as f64;
    s.top_fp = top(fp_count, 15);
    s.top_fn = top(fn_count, 15);
    // Keep the source order stable in reports even when a source has no segments.
    for src in Source::ALL {
        s.by_source.entry(src.name().to_string()).or_default();
    }
    s
}

pub fn print(s: &Score, meta: Option<&RunMeta>) {
    let pct = |x: f64| format!("{:.1}%", x * 100.0);
    println!("== {} ({} segments)", s.label, s.segments);
    println!(
        "overall P {} R {}  | shown (conf>=0.5) P {} R {}  | type-agnostic P {} R {}",
        pct(s.overall.precision()),
        pct(s.overall.recall()),
        pct(s.shown.precision()),
        pct(s.shown.recall()),
        pct(s.type_agnostic.precision()),
        pct(s.type_agnostic.recall())
    );
    println!(
        "identifier-like names dropped ({}): P {} R {}",
        s.id_like,
        pct(s.id_like_dropped.precision()),
        pct(s.id_like_dropped.recall())
    );
    println!(
        "person P {} R {}  | org P {} R {}",
        pct(s.person.precision()),
        pct(s.person.recall()),
        pct(s.org.precision()),
        pct(s.org.recall())
    );
    for (src, p) in &s.by_source {
        println!(
            "  {src:<10} P {} R {}  ({:?})",
            pct(p.precision()),
            pct(p.recall()),
            p
        );
    }
    println!(
        "schema ok first {} / after retry {} / failed {}; predictions {}, span discarded {}, name not in segment {}, truncated {}",
        s.schema_ok_first,
        s.schema_ok_retry,
        s.schema_failed,
        s.predictions,
        s.span_discarded,
        s.name_not_in_segment,
        s.truncated
    );
    println!(
        "links (name-key estimate): gold {}, predicted {}, correct {} → P {} R {}",
        s.links.gold,
        s.links.predicted,
        s.links.correct,
        pct(s.links.precision),
        pct(s.links.recall)
    );
    if let Some(m) = meta {
        println!(
            "speed {:.2} segments/s (np {}), {:.0} prompt + {:.0} completion tokens/segment; load {:.1} s; RSS peak {:.1} GB (VmHWM {:.1} GB); VRAM proc {} MiB, device {} MiB (baseline {})",
            m.items as f64 / m.wall_s,
            m.np,
            s.prompt_tokens_mean,
            s.completion_tokens_mean,
            m.load_s,
            m.peaks.rss_max_bytes as f64 / 1e9,
            m.peaks.vm_hwm_bytes as f64 / 1e9,
            m.peaks.vram_proc_max_mib,
            m.peaks.vram_device_max_mib,
            m.peaks.vram_device_baseline_mib
        );
    }
    println!("top FP {:?}", s.top_fp);
    println!("top FN {:?}", s.top_fn);
}

pub fn run(label: &str, segments: &[Segment], out: &std::path::Path) -> Result<Score, String> {
    let records = extract::load_records(&out.join(format!("extract-{label}.jsonl")))?;
    let meta: Option<RunMeta> = std::fs::read_to_string(out.join(format!("extract-{label}.json")))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let s = score(label, segments, &records);
    print(&s, meta.as_ref());
    std::fs::write(
        out.join(format!("score-{label}.json")),
        serde_json::to_string_pretty(&s).expect("serializes") + "\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Usage;
    use crate::extract::Attempt;
    use crate::segments::Gold;

    fn seg(id: &str, doc: &str, text: &str, gold: &[(EntityKind, &str)]) -> Segment {
        Segment {
            id: id.into(),
            document: doc.into(),
            source: Source::TextPdf,
            text: text.into(),
            gold: gold
                .iter()
                .map(|(k, raw)| Gold {
                    mention: "m".into(),
                    kind: *k,
                    name_key: name_key(*k, raw),
                    raw: raw.to_string(),
                })
                .collect(),
        }
    }

    fn rec(id: &str, contents: &[&str]) -> Record {
        Record {
            segment: id.into(),
            attempts: contents
                .iter()
                .map(|c| Attempt {
                    content: c.to_string(),
                    finish_reason: Some("stop".into()),
                    usage: Usage::default(),
                    ms: 1,
                    error: extract::parse(c).err(),
                })
                .collect(),
        }
    }

    fn m(name: &str, kind: &str, evidence: &str, conf: f64) -> String {
        format!(
            r#"{{"name":"{name}","type":"{kind}","evidence":"{evidence}","confidence":{conf}}}"#
        )
    }

    #[test]
    fn nfr_qa_08_09_scoring_hand_built() {
        use EntityKind::*;
        let segs = vec![
            seg(
                "a",
                "d1",
                "Invoice from BRIGHTWATER LOGISTICS LTD to\nVelmora Port Authority. Mr Hale",
                &[
                    (Org, "BRIGHTWATER LOGISTICS LTD"),
                    (Org, "Velmora Port Authority"),
                    (Person, "Mr Hale"),
                ],
            ),
            seg(
                "b",
                "d2",
                "Paid by Velmora Port Authority.",
                &[(Org, "Velmora Port Authority")],
            ),
            seg("c", "d3", "Nothing here.", &[]),
        ];
        let recs = vec![
            // Suffix-normalized hit, a wrong type (FP + FN), an invented evidence (discarded).
            rec(
                "a",
                &[&format!(
                    r#"{{"mentions":[{},{},{}]}}"#,
                    m(
                        "Brightwater Logistics Limited",
                        "org",
                        "BRIGHTWATER LOGISTICS LTD",
                        0.9
                    ),
                    m(
                        "Velmora Port Authority",
                        "person",
                        "to Velmora Port Authority",
                        0.4
                    ),
                    m("Corvin Hale", "person", "Mr Corvin Hale", 0.9)
                )],
            ),
            // Fails first, parses on retry.
            rec(
                "b",
                &[
                    "{oops",
                    &format!(
                        r#"{{"mentions":[{}]}}"#,
                        m(
                            "Velmora Port Authority",
                            "org",
                            "Velmora Port Authority",
                            0.8
                        )
                    ),
                ],
            ),
            // An FP on a negative segment.
            rec(
                "c",
                &[&format!(
                    r#"{{"mentions":[{}]}}"#,
                    m("Nothing", "org", "Nothing", 0.2)
                )],
            ),
        ];
        let s = score("t", &segs, &recs);
        assert_eq!(
            s.overall,
            Prf {
                tp: 2,
                fp: 2,
                fn_: 2
            }
        );
        assert_eq!(
            s.org,
            Prf {
                tp: 2,
                fp: 1,
                fn_: 1
            }
        );
        assert_eq!(
            s.person,
            Prf {
                tp: 0,
                fp: 1,
                fn_: 1
            }
        );
        // Type ignored: "velmora port authority" now matches; Mr Hale is still missed.
        assert_eq!(
            s.type_agnostic,
            Prf {
                tp: 3,
                fp: 1,
                fn_: 1
            }
        );
        // Confidence < 0.5 is not shown: the wrong-type and the negative-segment FPs go away.
        assert_eq!(
            s.shown,
            Prf {
                tp: 2,
                fp: 0,
                fn_: 2
            }
        );
        assert_eq!(
            (s.schema_ok_first, s.schema_ok_retry, s.schema_failed),
            (2, 1, 0)
        );
        assert_eq!((s.predictions, s.span_discarded), (5, 1));
        assert_eq!(s.name_not_in_segment, 1); // "Brightwater Logistics Limited"
        // Gold: VPA in d1 and d2 → 1 link. Predicted: org VPA only in d2 (d1 had it as a person).
        assert_eq!(
            (s.links.gold, s.links.predicted, s.links.correct),
            (1, 0, 0)
        );
    }

    #[test]
    fn id_like_names() {
        assert!(id_like("velmora-port.example"));
        assert!(id_like("d.oh@harbourmail.example"));
        assert!(!id_like("Saltmarsh Holdings S.A."));
        assert!(!id_like("C. Hale"));
        assert!(!id_like("VPA"));
        assert!(!id_like("Ltd."));
    }

    #[test]
    fn inv_ai_03_skipped_after_retry() {
        let segs = vec![seg(
            "a",
            "d1",
            "Corvin Hale",
            &[(EntityKind::Person, "Corvin Hale")],
        )];
        let s = score("t", &segs, &[rec("a", &["{", "still bad"])]);
        assert_eq!(s.schema_failed, 1);
        assert_eq!(
            s.overall,
            Prf {
                tp: 0,
                fp: 0,
                fn_: 1
            }
        );
    }
}
