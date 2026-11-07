//! Merge proposals (US-11.2, domain §7.4, spike S7): a score and plain reasons for two entities that
//! may be one. Never applied on their own (INV-ER-03). Pure: `mk-core` feeds it what the store holds.
//!
//! Names are compared on their normalized form: titles and joining words dropped, in any word order,
//! with initials, word subsets, organization acronyms and Jaro-Winkler. A hard identifier from a D7
//! link (email, phone, IBAN, crypto, custom; never a domain or URL) adds to the score. Entities are
//! paired only inside blocks of a shared word, word start, acronym or identifier, and a block too
//! large to say anything is skipped, so the work stays close to linear in the number of names.
//!
//! Spike S7 on the real model's names decided what may propose on its own: exact aliases and shared
//! identifiers. Similar spellings, acronyms and looser word subsets only add to a shared identifier,
//! and a one-word name proposes only if it fits a single longer entity
//! (`docs/17-entity-resolution.md` §1).

use std::collections::{BTreeMap, BTreeSet, HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

use mk_kernel::{EntityType, IdentifierKind};

use crate::entity::merge_anchor;

/// Only proposals at or above this are shown (US-11.2 AC3, INV-ER-05).
pub const THRESHOLD: f32 = 0.6;

/// A word start this long joins spellings that differ later ("almqvist", "almquist").
const WORD_START: usize = 4;
/// Entities sharing one block before it says too little to compare them all.
const MAX_BLOCK: usize = 128;
/// An identifier linked to more entities than this says nothing about any two of them.
pub const MAX_ENTITIES_PER_IDENTIFIER: usize = 4;
/// Below this Jaro-Winkler two different names are not "similar".
const MIN_SIMILARITY: f32 = 0.84;
/// Jaro-Winkler never scores above an equal name in another order, or an initial.
const MAX_SIMILARITY: f32 = 0.9;
/// What a shared hard identifier alone scores.
const IDENTIFIER_SCORE: f32 = 0.7;

const TITLES: &[&str] = &[
    "mr", "mrs", "ms", "miss", "mx", "dr", "prof", "sir", "madam", "herr", "frau",
];
const JOINING_WORDS: &[&str] = &["the", "of", "and", "for", "de"];

/// A proposal between two entities, as the store keeps it.
#[derive(Clone, PartialEq, Debug)]
pub struct Proposal {
    /// The name keys of the pair: the decision's anchor (domain §9.5).
    pub a: String,
    pub b: String,
    /// The entity keys of the two sides when it was made.
    pub left: String,
    pub right: String,
    pub score: f32,
    /// Plain reasons, in order; at least one (INV-ER-05).
    pub reasons: Vec<String>,
}

impl Proposal {
    /// The reasons as the list shows them (US-11.2 AC1).
    pub fn reason_text(&self) -> String {
        self.reasons.join(" · ")
    }
}

/// One entity as proposals see it (a name-key entity: split groups are not proposed).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    pub key: String,
    pub entity_type: EntityType,
    /// Its name keys with mentions.
    pub name_keys: Vec<String>,
    /// Up to two of its documents: all that "are the two sides in one document?" needs.
    pub documents: Vec<i64>,
    /// Its hard identifiers: key → the documents the link is in.
    pub identifiers: BTreeMap<String, BTreeSet<i64>>,
}

/// Whether an identifier's kind is hard evidence (M7a): a domain or URL is shared by accident.
pub fn is_hard_identifier(key: &str) -> bool {
    let (kind, _) = key.split_once(':').unwrap_or((key, ""));
    !matches!(
        IdentifierKind::from_name(kind),
        None | Some(IdentifierKind::Domain) | Some(IdentifierKind::Url)
    )
}

/// The words a name is compared on: titles and joining words dropped, unless nothing would be left.
fn words(name: &str, entity_type: EntityType) -> Vec<&str> {
    let all: Vec<&str> = name.split_whitespace().collect();
    let dropped = match entity_type {
        EntityType::Person => TITLES,
        EntityType::Org => JOINING_WORDS,
    };
    let kept: Vec<&str> = all
        .iter()
        .copied()
        .filter(|w| !dropped.contains(w))
        .collect();
    if kept.is_empty() { all } else { kept }
}

/// Jaro similarity over `char`s.
fn jaro(a: &[char], b: &[char]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let (mut a_hit, mut b_hit) = (vec![false; a.len()], vec![false; b.len()]);
    let mut matches = 0usize;
    for (i, ca) in a.iter().enumerate() {
        let from = i.saturating_sub(window);
        let to = (i + window + 1).min(b.len());
        for j in from..to {
            if !b_hit[j] && b[j] == *ca {
                a_hit[i] = true;
                b_hit[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let (mut swaps, mut at) = (0usize, 0usize);
    for (i, hit) in a_hit.iter().enumerate() {
        if *hit {
            while !b_hit[at] {
                at += 1;
            }
            if a[i] != b[at] {
                swaps += 1;
            }
            at += 1;
        }
    }
    let m = matches as f32;
    (m / a.len() as f32 + m / b.len() as f32 + (m - (swaps / 2) as f32) / m) / 3.0
}

/// Jaro-Winkler similarity, 0 to 1 (prefix scale 0.1, up to 4 characters).
pub fn jaro_winkler(a: &str, b: &str) -> f32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let j = jaro(&a, &b);
    let prefix = a.iter().zip(&b).take(4).take_while(|(x, y)| x == y).count();
    j + prefix as f32 * 0.1 * (1.0 - j)
}

fn sorted(words: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    v.sort_unstable();
    v
}

fn is_initial(word: &str) -> bool {
    word.chars().count() == 1
}

fn starts_with_letter(word: &str, initial: &str) -> bool {
    word.chars().next() == initial.chars().next()
}

/// Every word of `short` paired with a distinct word of `long`, the same or by its first letter.
/// Returns (words the same, initials used), or `None` if one has no partner.
fn pair_words(short: &[&str], long: &[&str]) -> Option<(usize, usize)> {
    let mut used = vec![false; long.len()];
    let mut left = Vec::new();
    let mut same = 0;
    // Whole words first, so an initial cannot take the partner of a whole word.
    for w in short {
        match (0..long.len()).find(|&i| !used[i] && long[i] == *w) {
            Some(i) => {
                used[i] = true;
                same += 1;
            }
            None => left.push(*w),
        }
    }
    let initials = left.len();
    for w in left {
        let at = (0..long.len()).find(|&i| {
            !used[i]
                && ((is_initial(w) && starts_with_letter(long[i], w))
                    || (is_initial(long[i]) && starts_with_letter(w, long[i])))
        })?;
        used[at] = true;
    }
    Some((same, initials))
}

fn acronym_of(words: &[&str]) -> String {
    words.iter().filter_map(|w| w.chars().next()).collect()
}

/// What a name match can do without a shared identifier.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Standing {
    /// It proposes.
    Alone,
    /// It proposes only if this one word fits a single longer entity.
    OneWord(String),
    /// It only adds to a shared identifier.
    Support,
}

#[derive(Clone, PartialEq, Debug)]
struct NameMatch {
    score: f32,
    reason: String,
    standing: Standing,
}

impl NameMatch {
    fn new(score: f32, reason: impl Into<String>, standing: Standing) -> Self {
        Self {
            score,
            reason: reason.into(),
            standing,
        }
    }
}

/// The score of two names and its reason; `None` if they do not look alike. A score alone does not
/// make a proposal: see [`score_pair`].
pub fn name_score(a: &str, b: &str, entity_type: EntityType) -> Option<(f32, String)> {
    name_match(a, b, entity_type).map(|m| (m.score, m.reason))
}

fn name_match(a: &str, b: &str, entity_type: EntityType) -> Option<NameMatch> {
    let (wa, wb) = (words(a, entity_type), words(b, entity_type));
    if wa == wb {
        return Some(NameMatch::new(
            0.95,
            "Same name without a title or joining word",
            Standing::Alone,
        ));
    }
    if sorted(&wa) == sorted(&wb) {
        return Some(NameMatch::new(
            0.95,
            "Same words in another order",
            Standing::Alone,
        ));
    }
    let (short, long) = if wa.len() <= wb.len() {
        (&wa, &wb)
    } else {
        (&wb, &wa)
    };
    if entity_type == EntityType::Person
        && let Some((same, initials)) = pair_words(short, long)
        && initials > 0
        && long
            .iter()
            .any(|w| w.chars().count() >= 3 && short.contains(w))
        && same > 0
    {
        return Some(NameMatch::new(
            0.85,
            "An initial matches a first name",
            Standing::Alone,
        ));
    }
    if short.iter().all(|w| long.contains(w)) {
        let chars = |ws: &[&str]| ws.iter().map(|w| w.chars().count()).sum::<usize>() as f32;
        let part = chars(short) / chars(long);
        // "commission" and "nuclear regulatory commission": an organization's name grows by a word
        // at a time, and most longer ones are something else.
        let standing = if entity_type == EntityType::Org && long.len() - short.len() != 1 {
            Standing::Support
        } else if let [word] = short.as_slice() {
            Standing::OneWord((*word).to_string())
        } else {
            Standing::Alone
        };
        return Some(NameMatch::new(
            0.6 + 0.25 * part,
            "One name is part of the other",
            standing,
        ));
    }
    if entity_type == EntityType::Org && short.len() == 1 && long.len() >= 2 {
        let word = short[0];
        if word.chars().count() >= 2 && word == acronym_of(long) {
            return Some(NameMatch::new(
                0.8,
                format!("{} could be short for the full name", word.to_uppercase()),
                Standing::Support,
            ));
        }
    }
    // "Name 1234" and "Name 1235" are two records, however alike they look.
    let digits = |ws: &[&str]| -> String {
        ws.iter()
            .flat_map(|w| w.chars())
            .filter(char::is_ascii_digit)
            .collect()
    };
    if digits(&wa) != digits(&wb) {
        return None;
    }
    let similarity = jaro_winkler(&sorted(&wa).join(" "), &sorted(&wb).join(" "));
    (similarity >= MIN_SIMILARITY).then(|| {
        NameMatch::new(
            similarity.min(MAX_SIMILARITY),
            format!("Similar names ({similarity:.2})"),
            Standing::Support,
        )
    })
}

/// The name after its `type:` prefix.
fn name_of(key: &str) -> &str {
    key.split_once(':').map_or(key, |(_, n)| n)
}

/// The start of an identifier's value and its type, for a reason: `phone`, `+4420…`.
fn identifier_reason(key: &str, documents: usize) -> String {
    let (kind, value) = key.split_once(':').unwrap_or(("identifier", key));
    let value = value
        .rsplit_once(':')
        .filter(|_| kind == "custom")
        .map_or(value, |(_, v)| v);
    let shown: String = value.chars().take(6).collect();
    let more = if value.chars().count() > 6 { "…" } else { "" };
    let label = match kind {
        "email" => "email",
        "phone" => "phone",
        "iban" => "IBAN",
        "btc" | "evm" | "sol" | "tron" => "crypto address",
        _ => "value",
    };
    let docs = if documents == 1 {
        "document"
    } else {
        "documents"
    };
    format!("Same {label} {shown}{more} in {documents} {docs}")
}

/// A one-word name and the longer entity it was matched with, for [`proposals`] to count.
struct OneWordFit {
    word: String,
    longer: String,
}

/// The score of two entities with its reasons and the name keys behind it, or `None` if nothing ties
/// them. Merging two sides in one document links nothing, so such a pair is skipped.
pub fn score_pair(a: &Candidate, b: &Candidate) -> Option<Proposal> {
    scored_pair(a, b).map(|(p, _)| p)
}

fn scored_pair(a: &Candidate, b: &Candidate) -> Option<(Proposal, Option<OneWordFit>)> {
    if a.key == b.key || a.entity_type != b.entity_type {
        return None;
    }
    let docs: BTreeSet<i64> = a.documents.iter().chain(&b.documents).copied().collect();
    if docs.len() < 2 {
        return None;
    }
    let mut shared: Vec<(&String, usize)> = a
        .identifiers
        .iter()
        .filter_map(|(k, da)| {
            let db = b.identifiers.get(k)?;
            Some((k, da.union(db).count()))
        })
        .collect();
    shared.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(y.0)));
    // With a shared identifier any match adds to it. Without one, a match that proposes on its own
    // goes before one that depends on what else its word fits.
    let rank = |m: &NameMatch| match m.standing {
        _ if !shared.is_empty() => 1,
        Standing::Alone => 2,
        Standing::OneWord(_) => 1,
        Standing::Support => 0,
    };
    let mut best: Option<(NameMatch, &str, &str)> = None;
    for ka in &a.name_keys {
        for kb in &b.name_keys {
            let Some(m) = name_match(name_of(ka), name_of(kb), a.entity_type) else {
                continue;
            };
            if rank(&m) == 0 {
                continue;
            }
            let better = best.as_ref().is_none_or(|(s, ..)| {
                (rank(&m), m.score).partial_cmp(&(rank(s), s.score))
                    == Some(std::cmp::Ordering::Greater)
            });
            if better {
                best = Some((m, ka, kb));
            }
        }
    }
    if best.is_none() && shared.is_empty() {
        return None;
    }
    let names_score = best.as_ref().map_or(0.0, |(m, ..)| m.score);
    let score = if shared.is_empty() {
        names_score
    } else {
        1.0 - (1.0 - names_score) * (1.0 - IDENTIFIER_SCORE)
    };
    let mut reasons: Vec<String> = best.iter().map(|(m, ..)| m.reason.clone()).collect();
    reasons.extend(shared.iter().map(|(k, d)| identifier_reason(k, *d)));
    let fit = best.as_ref().and_then(|(m, ka, kb)| match &m.standing {
        Standing::OneWord(word) if shared.is_empty() => {
            let a_is_longer =
                words(name_of(ka), a.entity_type).len() > words(name_of(kb), a.entity_type).len();
            Some(OneWordFit {
                word: word.clone(),
                longer: if a_is_longer { &a.key } else { &b.key }.clone(),
            })
        }
        _ => None,
    });
    let (ka, kb) = best
        .as_ref()
        .map(|(_, x, y)| (x.to_string(), y.to_string()))
        .or_else(|| Some((a.name_keys.first()?.clone(), b.name_keys.first()?.clone())))?;
    let (first, second) = if ka <= kb { (ka, kb) } else { (kb, ka) };
    Some((
        Proposal {
            a: first,
            b: second,
            left: a.key.clone(),
            right: b.key.clone(),
            score,
            reasons,
        },
        fit,
    ))
}

fn block(entity_type: EntityType, tag: u8, text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    (entity_type, tag, text).hash(&mut h);
    h.finish()
}

/// Every block `c` is in.
fn blocks_of(c: &Candidate, out: &mut Vec<u64>) {
    let t = c.entity_type;
    for key in &c.name_keys {
        let ws = words(name_of(key), t);
        for w in &ws {
            let chars = w.chars().count();
            // A number says nothing about who this is, and it can pair up whole tables of names.
            if chars >= 3 && !w.bytes().all(|b| b.is_ascii_digit()) {
                out.push(block(t, b'w', w));
            }
            if chars > WORD_START && !w.bytes().any(|b| b.is_ascii_digit()) {
                let start: String = w.chars().take(WORD_START).collect();
                out.push(block(t, b's', &start));
            }
            // An initial or a short name meets the long form through the other word; an acronym
            // meets its full name here.
            if t == EntityType::Org && ws.len() == 1 {
                out.push(block(t, b'a', w));
            }
        }
        if t == EntityType::Org && ws.len() >= 2 {
            out.push(block(t, b'a', &acronym_of(&ws)));
        }
    }
    for k in c.identifiers.keys() {
        out.push(block(t, b'i', k));
    }
}

/// The identifiers linked to too many entities: left out of scoring and blocks.
fn drop_common_identifiers(candidates: &mut [Candidate]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in candidates.iter() {
        for k in c.identifiers.keys() {
            *counts.entry(k.clone()).or_default() += 1;
        }
    }
    for c in candidates.iter_mut() {
        c.identifiers
            .retain(|k, _| counts.get(k).copied().unwrap_or(0) <= MAX_ENTITIES_PER_IDENTIFIER);
    }
}

/// Every proposal at or above `threshold` (one per pair of entities, INV-ER-06). `blocked` holds
/// entity-key pairs a rejected name-key pair stands between, in key order (INV-ER-04). Stops early,
/// returning what it has, once `cancel` is set.
pub fn proposals(
    mut candidates: Vec<Candidate>,
    blocked: &BTreeSet<(String, String)>,
    threshold: f32,
    cancel: &AtomicBool,
) -> Vec<Proposal> {
    candidates.retain(|c| !c.name_keys.is_empty());
    drop_common_identifiers(&mut candidates);
    let mut entries: Vec<(u64, u32)> = Vec::new();
    let mut scratch = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        scratch.clear();
        blocks_of(c, &mut scratch);
        scratch.sort_unstable();
        scratch.dedup();
        entries.extend(scratch.iter().map(|b| (*b, i as u32)));
    }
    entries.sort_unstable();
    entries.shrink_to_fit();
    let mut found: Vec<(Proposal, Option<OneWordFit>)> = Vec::new();
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut start = 0;
    while start < entries.len() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let id = entries[start].0;
        let end = start + entries[start..].partition_point(|e| e.0 == id);
        let run = &entries[start..end];
        start = end;
        if run.len() < 2 || run.len() > MAX_BLOCK {
            continue;
        }
        for (x, &(_, i)) in run.iter().enumerate() {
            for &(_, j) in &run[x + 1..] {
                if !seen.insert((i, j)) {
                    continue;
                }
                let (ca, cb) = (&candidates[i as usize], &candidates[j as usize]);
                found.extend(scored_pair(ca, cb));
            }
        }
    }
    // A one-word name that fits several longer entities says nothing about any of them. A rejected
    // pair still counts, so rejecting one does not bring the others up; accepting a merge does.
    let mut fits: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for fit in found.iter().filter_map(|(_, fit)| fit.as_ref()) {
        fits.entry(&fit.word).or_default().insert(&fit.longer);
    }
    let ambiguous: BTreeSet<String> = fits
        .iter()
        .filter(|(_, longer)| longer.len() > 1)
        .map(|(word, _)| word.to_string())
        .collect();
    let mut out: Vec<Proposal> = found
        .into_iter()
        .filter(|(p, fit)| {
            let unclear = fit.as_ref().is_some_and(|f| ambiguous.contains(&f.word));
            let pair = if p.left <= p.right {
                (p.left.clone(), p.right.clone())
            } else {
                (p.right.clone(), p.left.clone())
            };
            !unclear && p.score >= threshold && !blocked.contains(&pair)
        })
        .map(|(p, _)| p)
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| merge_anchor(&a.a, &a.b).cmp(&merge_anchor(&b.a, &b.b)))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(a: &str, b: &str) -> Option<(f32, String)> {
        name_score(a, b, EntityType::Person)
    }

    fn org(a: &str, b: &str) -> Option<(f32, String)> {
        name_score(a, b, EntityType::Org)
    }

    fn candidate(t: EntityType, name: &str, docs: &[i64]) -> Candidate {
        let key = format!("{t}:{name}");
        Candidate {
            key: key.clone(),
            entity_type: t,
            name_keys: vec![key],
            documents: docs.to_vec(),
            identifiers: BTreeMap::new(),
        }
    }

    fn run(cs: Vec<Candidate>) -> Vec<Proposal> {
        proposals(cs, &BTreeSet::new(), THRESHOLD, &AtomicBool::new(false))
    }

    #[test]
    fn jaro_winkler_known_values() {
        assert!((jaro_winkler("martha", "marhta") - 0.9611).abs() < 0.001);
        assert!((jaro_winkler("dixon", "dicksonx") - 0.8133).abs() < 0.001);
        assert_eq!(jaro_winkler("same", "same"), 1.0);
        assert_eq!(jaro_winkler("", ""), 1.0);
        assert_eq!(jaro_winkler("abc", ""), 0.0);
    }

    /// US-11.2 AC1 on the kinds of name variant TD-1 uses.
    #[test]
    fn us_11_2_ac1_name_variants_score_with_a_reason() {
        let cases = [
            ("mr okonkwo hart", "okonkwo hart"),
            ("okonkwo hart daniel", "daniel okonkwo hart"),
            ("d okonkwo hart", "daniel okonkwo hart"),
            ("t reinholt", "tomas reinholt"),
            ("mr hale", "corvin hale"),
            ("irene", "irene vasquez lindqvist"),
            ("irene vasquez", "irene vasquez lindqvist"),
            ("gregor almqvist", "gregory almquist"),
        ];
        for (a, b) in cases {
            let (score, reason) = person(a, b).unwrap_or_else(|| panic!("{a} / {b}"));
            assert!(score >= THRESHOLD, "{a} / {b}: {score}");
            assert!(!reason.is_empty());
        }
        let orgs = [
            ("harbour post", "the harbour post"),
            ("port of velmora", "velmora port authority"),
            ("vpa", "velmora port authority"),
            ("brightwater", "brightwater logistics"),
            ("companies registry", "velmora companies registry"),
        ];
        for (a, b) in orgs {
            let (score, reason) = org(a, b).unwrap_or_else(|| panic!("{a} / {b}"));
            assert!(score >= THRESHOLD, "{a} / {b}: {score}");
            assert!(!reason.is_empty());
        }
        assert!(person("anna berg", "tomas reinholt").is_none());
        assert!(person("a b", "a c").is_none_or(|(s, _)| s < THRESHOLD));
        // An initial alone says nothing: no word in common.
        assert!(person("d h", "daniel hale").is_none());
        // Acronyms are for organizations.
        assert!(person("vpa", "velmora port authority").is_none());
    }

    /// US-11.2 AC1: a shared hard identifier scores on its own and adds to a name, with its reason.
    /// A domain or URL is not hard evidence.
    #[test]
    fn us_11_2_ac1_shared_identifier() {
        let mut a = candidate(EntityType::Person, "anna berg", &[1]);
        let mut b = candidate(EntityType::Person, "tomas reinholt", &[2]);
        let phone = "phone:+442079460000".to_string();
        a.identifiers.insert(phone.clone(), BTreeSet::from([1]));
        b.identifiers.insert(phone, BTreeSet::from([2]));
        let p = score_pair(&a, &b).unwrap();
        assert!((p.score - IDENTIFIER_SCORE).abs() < 0.001);
        assert_eq!(p.reasons, ["Same phone +44207… in 2 documents"]);
        assert_eq!(
            (p.a.as_str(), p.b.as_str()),
            ("person:anna berg", "person:tomas reinholt")
        );

        let mut c = candidate(EntityType::Person, "a berg", &[3]);
        c.identifiers
            .insert("phone:+442079460000".into(), BTreeSet::from([3]));
        let p = score_pair(&a, &c).unwrap();
        assert!(p.score > 0.85);
        assert_eq!(p.reasons.len(), 2, "{:?}", p.reasons);
        assert!(!p.reason_text().is_empty());

        for hard in [
            "email:a@x.example",
            "phone:+1",
            "iban:GB1",
            "btc:1a",
            "evm:0x",
            "sol:a",
            "tron:t",
            "custom:ref:1",
        ] {
            assert!(is_hard_identifier(hard), "{hard}");
        }
        for soft in ["domain:x.example", "url:https://x.example", "person:a"] {
            assert!(!is_hard_identifier(soft), "{soft}");
        }
    }

    /// An identifier many entities share says nothing about two of them.
    #[test]
    fn a_common_identifier_is_skipped() {
        let mut cs: Vec<Candidate> = (0..6)
            .map(|i| {
                let mut c = candidate(EntityType::Org, &format!("firm{i}x"), &[i]);
                c.identifiers
                    .insert("email:info@shared.example".into(), BTreeSet::from([i]));
                c
            })
            .collect();
        assert!(run(cs.clone()).is_empty());
        cs.truncate(2);
        assert_eq!(run(cs).len(), 1);
    }

    /// INV-ER-06: one proposal per pair, never with itself. INV-ER-05: at or above the threshold,
    /// each with a reason.
    #[test]
    fn inv_er_05_06_one_proposal_per_pair_above_the_threshold() {
        let cs = vec![
            candidate(EntityType::Person, "corvin hale", &[1]),
            candidate(EntityType::Person, "c hale", &[2]),
            candidate(EntityType::Person, "mr hale", &[3]),
            candidate(EntityType::Person, "unrelated person", &[4]),
            candidate(EntityType::Org, "kestrel", &[5]),
            candidate(EntityType::Org, "kestrel advisory", &[6]),
        ];
        let out = run(cs);
        let pairs: BTreeSet<String> = out.iter().map(|p| merge_anchor(&p.a, &p.b)).collect();
        assert_eq!(pairs.len(), out.len());
        assert!(pairs.contains("person:c hale|person:corvin hale"));
        assert!(pairs.contains("org:kestrel|org:kestrel advisory"));
        for p in &out {
            assert!(p.score >= THRESHOLD);
            assert!(!p.reasons.is_empty());
            assert_ne!(p.left, p.right);
        }
        assert!(out.windows(2).all(|w| w[0].score >= w[1].score));
        assert!(run(vec![candidate(EntityType::Org, "acme", &[1])]).is_empty());
    }

    /// INV-ER-04: a rejected pair stands between the two entities, in key order.
    #[test]
    fn inv_er_04_rejected_groups_are_not_proposed() {
        let cs = vec![
            candidate(EntityType::Org, "kestrel", &[1]),
            candidate(EntityType::Org, "kestrel advisory", &[2]),
        ];
        assert_eq!(run(cs.clone()).len(), 1);
        let blocked = BTreeSet::from([(
            "org:kestrel".to_string(),
            "org:kestrel advisory".to_string(),
        )]);
        assert!(proposals(cs, &blocked, THRESHOLD, &AtomicBool::new(false)).is_empty());
    }

    /// Merging two sides that only sit in one document links nothing; other types never pair.
    #[test]
    fn one_document_and_other_types_are_skipped() {
        let a = candidate(EntityType::Person, "corvin hale", &[1]);
        let same_doc = candidate(EntityType::Person, "c hale", &[1]);
        assert!(score_pair(&a, &same_doc).is_none());
        let other = candidate(EntityType::Org, "corvin hale", &[2]);
        assert!(score_pair(&a, &other).is_none());
        let elsewhere = candidate(EntityType::Person, "c hale", &[1, 2]);
        assert!(score_pair(&a, &elsewhere).is_some());
    }

    /// A block too large is skipped, so a common word cannot make the work quadratic.
    #[test]
    fn a_huge_block_is_skipped() {
        let word = |i: usize| -> String {
            (0..6)
                .map(|p| (b'a' + ((i * 7 + p * 11) % 26) as u8) as char)
                .collect()
        };
        let mut cs: Vec<Candidate> = (0..MAX_BLOCK)
            .map(|i| {
                let name = format!("acme {}", word(i));
                candidate(EntityType::Person, &name, &[i as i64])
            })
            .collect();
        let pair = [
            candidate(EntityType::Person, "acme", &[1000]),
            candidate(EntityType::Person, "mr acme", &[1001]),
        ];
        cs.extend(pair.iter().cloned());
        let touches = |out: &[Proposal]| out.iter().any(|p| p.left == "person:acme");
        assert!(!touches(&run(cs.clone())));
        cs.truncate(4);
        cs.extend(pair);
        assert!(touches(&run(cs)));
    }

    /// S7: a spelling alike is not a proposal on its own (Gregor and Gregory), but adds to a shared
    /// identifier, as in US-11.2 AC1's "Similar names (0.86) · Same phone +4420… in 2 documents".
    #[test]
    fn s7_a_similar_spelling_only_adds_to_an_identifier() {
        let mut a = candidate(EntityType::Person, "gregor almqvist", &[1]);
        let mut b = candidate(EntityType::Person, "gregory almquist", &[2]);
        assert!(score_pair(&a, &b).is_none());
        let phone = "phone:+442079460000".to_string();
        a.identifiers.insert(phone.clone(), BTreeSet::from([1]));
        b.identifiers.insert(phone, BTreeSet::from([2]));
        let p = score_pair(&a, &b).unwrap();
        assert!(p.score > IDENTIFIER_SCORE, "{p:?}");
        assert!(p.reasons[0].starts_with("Similar names ("), "{p:?}");
        assert_eq!(p.reasons[1], "Same phone +44207… in 2 documents");
    }

    /// S7: an acronym only adds to a shared identifier: "st" is not the Secretary of the Treasury.
    #[test]
    fn s7_an_acronym_only_adds_to_an_identifier() {
        let mut a = candidate(EntityType::Org, "vpa", &[1]);
        let mut b = candidate(EntityType::Org, "velmora port authority", &[2]);
        assert!(score_pair(&a, &b).is_none());
        let email = "email:a@x.example".to_string();
        a.identifiers.insert(email.clone(), BTreeSet::from([1]));
        b.identifiers.insert(email, BTreeSet::from([2]));
        let p = score_pair(&a, &b).unwrap();
        assert_eq!(p.reasons[0], "VPA could be short for the full name");
    }

    /// S7: an organization's name proposes as part of another one word apart, not further.
    #[test]
    fn s7_an_organization_is_part_of_a_name_one_word_longer() {
        let pair = |x: &str, y: &str| {
            score_pair(
                &candidate(EntityType::Org, x, &[1]),
                &candidate(EntityType::Org, y, &[2]),
            )
        };
        assert!(pair("meridian trust", "meridian trust bank").is_some());
        assert!(pair("port of velmora", "velmora port authority").is_some());
        assert!(pair("meridian trust", "meridian trust bank group").is_none());
        // People's names are not held to this: "irene" and "irene vasquez lindqvist".
        let (irene, full) = (
            candidate(EntityType::Person, "irene", &[1]),
            candidate(EntityType::Person, "irene vasquez lindqvist", &[2]),
        );
        assert!(score_pair(&irene, &full).is_some());
    }

    /// S7: a one-word name proposes only while it fits a single longer entity. One entity with several
    /// names is one fit, so accepting a merge brings the short name up.
    #[test]
    fn s7_a_one_word_name_must_fit_one_entity() {
        let thomas = candidate(EntityType::Person, "thomas", &[1]);
        let jefferson = candidate(EntityType::Person, "thomas jefferson", &[2]);
        let horan = candidate(EntityType::Person, "thomas horan", &[3]);
        assert_eq!(run(vec![thomas.clone(), jefferson.clone()]).len(), 1);
        assert!(run(vec![thomas.clone(), jefferson.clone(), horan.clone()]).is_empty());

        let (mr, c, corvin) = (
            candidate(EntityType::Person, "mr hale", &[1]),
            candidate(EntityType::Person, "c hale", &[2]),
            candidate(EntityType::Person, "corvin hale", &[3]),
        );
        let before = run(vec![mr.clone(), c.clone(), corvin.clone()]);
        assert!(!before.iter().any(|p| p.left == mr.key), "{before:?}");
        assert_eq!(before.len(), 1, "the two longer names are still proposed");
        let merged = Candidate {
            key: c.key.clone(),
            entity_type: EntityType::Person,
            name_keys: vec![c.key.clone(), corvin.key.clone()],
            documents: vec![2, 3],
            identifiers: BTreeMap::new(),
        };
        let after = run(vec![mr, merged]);
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(
            (after[0].a.as_str(), after[0].b.as_str()),
            ("person:c hale", "person:mr hale")
        );
    }

    /// A rejected pair still counts as a fit: rejecting noise must not bring the rest of it up.
    #[test]
    fn s7_a_rejected_fit_still_counts() {
        let cs = vec![
            candidate(EntityType::Person, "thomas", &[1]),
            candidate(EntityType::Person, "thomas jefferson", &[2]),
            candidate(EntityType::Person, "thomas horan", &[3]),
        ];
        let blocked = BTreeSet::from([(
            "person:thomas".to_string(),
            "person:thomas horan".to_string(),
        )]);
        assert!(proposals(cs, &blocked, THRESHOLD, &AtomicBool::new(false)).is_empty());
    }

    /// A shared identifier stands whatever the name does: it is not one of the word's fits.
    #[test]
    fn s7_an_identifier_stands_for_an_ambiguous_word() {
        let phone = "phone:+442079460000".to_string();
        let mut thomas = candidate(EntityType::Person, "thomas", &[1]);
        let mut horan = candidate(EntityType::Person, "thomas horan", &[3]);
        thomas
            .identifiers
            .insert(phone.clone(), BTreeSet::from([1]));
        horan.identifiers.insert(phone, BTreeSet::from([3]));
        let cs = vec![
            thomas,
            horan,
            candidate(EntityType::Person, "thomas jefferson", &[2]),
            candidate(EntityType::Person, "thomas wren", &[4]),
        ];
        let out = run(cs);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].b, "person:thomas horan");
        assert_eq!(out[0].reasons.len(), 2);
    }

    /// When two entities match on several names, one that proposes on its own is what stands, even if
    /// a one-word match scores higher and its word turns out to fit others.
    #[test]
    fn s7_a_match_that_stands_goes_before_a_higher_one_word_match() {
        let with_names = |key: &str, names: &[&str], docs: &[i64]| Candidate {
            key: key.into(),
            entity_type: EntityType::Person,
            name_keys: names.iter().map(|n| n.to_string()).collect(),
            documents: docs.to_vec(),
            identifiers: BTreeMap::new(),
        };
        let a = with_names(
            "person:anna berg",
            &["person:anna berg", "person:mr almqvist"],
            &[1],
        );
        let b = with_names(
            "person:anna berg lindqvist larsson",
            &["person:anna berg lindqvist larsson", "person:g almqvist"],
            &[2],
        );
        let other = candidate(EntityType::Person, "gregor almqvist", &[3]);
        let out = run(vec![a, b, other]);
        let found = out
            .iter()
            .find(|p| p.a == "person:anna berg" && p.b == "person:anna berg lindqvist larsson");
        assert!(found.is_some(), "{out:?}");
    }

    /// Numbered records are not similar names.
    #[test]
    fn a_number_makes_no_block() {
        let blocks = |name: &str| {
            let mut out = Vec::new();
            blocks_of(&candidate(EntityType::Person, name, &[1]), &mut out);
            out
        };
        let (a, b) = (blocks("Ada 00001"), blocks("Bob 00001"));
        assert!(!a.is_empty());
        assert!(a.iter().all(|x| !b.contains(x)));
        // Words with a letter still do.
        assert!(
            blocks("Ada 7th")
                .iter()
                .any(|x| blocks("Bob 7th").contains(x))
        );
    }

    #[test]
    fn different_numbers_are_different_names() {
        assert!(org("name 1234", "name 1235").is_none());
        assert!(org("branch 7 north", "branch 7 nort").is_some());
    }

    /// A proposal run stops when it is told to.
    #[test]
    fn cancel_stops_the_run() {
        let cs = vec![
            candidate(EntityType::Org, "kestrel", &[1]),
            candidate(EntityType::Org, "kestrel advisory", &[2]),
        ];
        assert!(proposals(cs, &BTreeSet::new(), THRESHOLD, &AtomicBool::new(true)).is_empty());
    }
}
