//! The evidence rules (decided 2026-11-05), in this order, on a parsed reply:
//!
//! 1. the evidence must be found in the unit after whitespace normalization, else discarded and
//!    counted (INV-AI-01, US-10.2);
//! 2. the name must be inside the evidence, ignoring case, else discarded and counted. The stored
//!    `raw` is the text from the source, never the model's spelling;
//! 3. a name that crosses a segment boundary is discarded and counted. The stored span is the
//!    evidence clipped to the segment holding the name;
//! 4. a name whose own span overlaps a Stage A identifier mention is dropped, with its own count
//!    (`09-model-check.md` §5.3): "John Smith <john@x.example>" keeps John Smith, and "john" from
//!    the address goes;
//! 5. duplicates in one unit (same name key, segment and span) are removed.

use std::collections::HashSet;

use mk_kernel::{EntityType, NameKey};

use crate::extract::Prediction;

/// One stretch of source text in a unit: from `start` (in characters) of `segment`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Piece {
    pub segment: i64,
    pub start: usize,
    pub text: String,
}

/// A Stage A identifier mention's place, in characters of its segment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IdentifierSpan {
    pub segment: i64,
    pub start: usize,
    pub end: usize,
}

/// A mention that passed every rule.
#[derive(Clone, PartialEq, Debug)]
pub struct CheckedMention {
    pub segment: i64,
    pub key: NameKey,
    pub entity_type: EntityType,
    /// The name as the source has it, whitespace runs as one space.
    pub raw: String,
    /// The evidence's place in characters of the segment, clipped to it.
    pub span: (usize, usize),
    pub confidence: f32,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct CheckOutcome {
    pub kept: Vec<CheckedMention>,
    /// "N mentions discarded (no matching text)" (US-10.2 AC2).
    pub discarded: u64,
    /// Names on a Stage A identifier: not an error of the model's evidence.
    pub identifier_dropped: u64,
}

/// The text the model is sent: the pieces joined by one line break.
pub fn unit_text(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for (i, p) in pieces.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&p.text);
    }
    out
}

/// Characters with whitespace runs as one space and the ends trimmed, each with the index of the
/// character it came from.
struct Normalized {
    chars: Vec<char>,
    at: Vec<usize>,
}

fn normalize(chars: impl IntoIterator<Item = char>) -> Normalized {
    let mut out = Normalized {
        chars: Vec::new(),
        at: Vec::new(),
    };
    let mut gap: Option<usize> = None;
    for (i, c) in chars.into_iter().enumerate() {
        if c.is_whitespace() {
            if !out.chars.is_empty() {
                gap.get_or_insert(i);
            }
        } else {
            if let Some(g) = gap.take() {
                out.chars.push(' ');
                out.at.push(g);
            }
            out.chars.push(c);
            out.at.push(i);
        }
    }
    out
}

/// Characters lower-cased, each with the index it came from (a lower case form may be longer).
fn fold(chars: &[char]) -> (Vec<char>, Vec<usize>) {
    let mut folded = Vec::with_capacity(chars.len());
    let mut at = Vec::with_capacity(chars.len());
    for (i, c) in chars.iter().enumerate() {
        for l in c.to_lowercase() {
            folded.push(l);
            at.push(i);
        }
    }
    (folded, at)
}

fn occurrences<'a>(hay: &'a [char], needle: &'a [char]) -> impl Iterator<Item = usize> + 'a {
    let n = needle.len();
    (0..hay.len().saturating_sub(n.saturating_sub(1)))
        .filter(move |&i| n > 0 && hay[i..i + n] == *needle)
}

/// Where the (folded) name is in `chars` (a normalized evidence): the first place that starts and
/// ends on a word boundary, else the first place at all (writing without spaces between words).
/// `(start, end)` in `chars`.
fn name_in(chars: &[char], name: &[char]) -> Option<(usize, usize)> {
    let (hay, at) = fold(chars);
    let mut first = None;
    for i in occurrences(&hay, name) {
        let (start, end) = (at[i], at[i + name.len() - 1] + 1);
        let found = (start, end);
        let before = start == 0 || !chars[start - 1].is_alphanumeric();
        let after = end == chars.len() || !chars[end].is_alphanumeric();
        if before && after {
            return Some(found);
        }
        first.get_or_insert(found);
    }
    first
}

/// The whitespace-collapsed source text of `chars`.
fn collapsed(chars: &[char]) -> String {
    let n = normalize(chars.iter().copied());
    n.chars.into_iter().collect()
}

/// Applies the rules to one parsed reply. `identifiers` are the unit's Stage A mentions.
pub fn check_reply(
    pieces: &[Piece],
    identifiers: &[IdentifierSpan],
    predictions: &[Prediction],
) -> CheckOutcome {
    let text: Vec<char> = unit_text(pieces).chars().collect();
    let normal = normalize(text.iter().copied());
    // Each piece's range in `text`.
    let mut ranges = Vec::with_capacity(pieces.len());
    let mut at = 0;
    for p in pieces {
        let len = p.text.chars().count();
        ranges.push((at, at + len));
        at += len + 1;
    }
    let mut out = CheckOutcome::default();
    let mut seen = HashSet::new();
    for pred in predictions {
        let evidence = normalize(pred.evidence.chars()).chars;
        let name = fold(&normalize(pred.name.chars()).chars).0;
        let found: Vec<usize> = occurrences(&normal.chars, &evidence).collect();
        // Rules 1 and 2: the place of the evidence, and of the name inside it.
        let mut places = Vec::new();
        for start in found {
            let quoted = &normal.chars[start..start + evidence.len()];
            if let Some((ns, ne)) = name_in(quoted, &name) {
                places.push((
                    normal.at[start],
                    normal.at[start + evidence.len() - 1] + 1,
                    normal.at[start + ns],
                    normal.at[start + ne - 1] + 1,
                ));
            }
        }
        // Rule 3: the first place where the name lies in one piece.
        let Some((piece, (ev_start, ev_end), (name_start, name_end))) =
            places.into_iter().find_map(|(es, ee, ns, ne)| {
                let piece = ranges.iter().position(|(s, e)| ns >= *s && ne <= *e)?;
                Some((piece, (es, ee), (ns, ne)))
            })
        else {
            out.discarded += 1;
            continue;
        };
        let raw = collapsed(&text[name_start..name_end]);
        let Some(key) = NameKey::new(pred.entity_type, &raw) else {
            out.discarded += 1;
            continue;
        };
        let (range_start, range_end) = ranges[piece];
        let to_segment = |x: usize| pieces[piece].start + x - range_start;
        let segment = pieces[piece].segment;
        // Rule 4: on the name's span, not the evidence's.
        let (seg_name_start, seg_name_end) = (to_segment(name_start), to_segment(name_end));
        if identifiers
            .iter()
            .any(|i| i.segment == segment && i.start < seg_name_end && seg_name_start < i.end)
        {
            out.identifier_dropped += 1;
            continue;
        }
        let span = (
            to_segment(ev_start.max(range_start)),
            to_segment(ev_end.min(range_end)),
        );
        // Rule 5.
        if !seen.insert((key.to_string(), segment, span)) {
            continue;
        }
        out.kept.push(CheckedMention {
            segment,
            key,
            entity_type: pred.entity_type,
            raw,
            span,
            confidence: pred.confidence,
        });
    }
    out
}

/// Whether a stored mention still shows its evidence (NFR-QA-11, `check_health`): the span lies
/// in the segment's text and holds the name, ignoring case and whitespace runs.
pub fn verify_mention(segment_text: &str, span: (usize, usize), raw: &str) -> bool {
    let chars: Vec<char> = segment_text.chars().collect();
    let (start, end) = span;
    if start >= end || end > chars.len() {
        return false;
    }
    let shown = normalize(chars[start..end].iter().copied()).chars;
    let name = fold(&normalize(raw.chars()).chars).0;
    !name.is_empty() && occurrences(&fold(&shown).0, &name).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn pred(name: &str, kind: EntityType, evidence: &str, confidence: f32) -> Prediction {
        Prediction {
            name: name.into(),
            entity_type: kind,
            evidence: evidence.into(),
            confidence,
        }
    }

    fn person(name: &str, evidence: &str) -> Prediction {
        pred(name, EntityType::Person, evidence, 0.9)
    }

    fn one(text: &str) -> Vec<Piece> {
        vec![Piece {
            segment: 1,
            start: 0,
            text: text.into(),
        }]
    }

    fn span_text(text: &str, span: (usize, usize)) -> String {
        text.chars().skip(span.0).take(span.1 - span.0).collect()
    }

    /// US-10.1 AC1: name as written, type, evidence span, confidence.
    #[test]
    fn us_10_1_ac1_mention_fields() {
        let text = "Paid by Mr Corvin Hale on 2 May.";
        let out = check_reply(
            &one(text),
            &[],
            &[person("Corvin Hale", "by Mr Corvin Hale on")],
        );
        assert_eq!((out.discarded, out.identifier_dropped), (0, 0));
        let m = &out.kept[0];
        assert_eq!(m.raw, "Corvin Hale");
        assert_eq!(m.entity_type, EntityType::Person);
        assert_eq!(m.key.to_string(), "person:corvin hale");
        assert_eq!(span_text(text, m.span), "by Mr Corvin Hale on");
        assert_eq!(m.confidence, 0.9);
    }

    /// INV-AI-01: found only after whitespace normalization; case and letters are not.
    #[test]
    fn inv_ai_01_evidence_whitespace() {
        let text = "Bill to\nVelmora Port   Authority\nAccounts Payable";
        let kept = |e: &str| {
            check_reply(
                &one(text),
                &[],
                &[pred("Velmora Port Authority", EntityType::Org, e, 0.8)],
            )
        };
        let out = kept("to Velmora\tPort Authority ");
        assert_eq!(out.kept.len(), 1);
        assert_eq!(
            span_text(text, out.kept[0].span),
            "to\nVelmora Port   Authority"
        );
        assert_eq!(out.kept[0].raw, "Velmora Port Authority");
        for bad in [
            "velmora port authority",
            "Velmora Port Authority Ltd",
            "   ",
            "Authority of Velmora Port",
        ] {
            let out = kept(bad);
            assert_eq!((out.kept.len(), out.discarded), (0, 1), "{bad}");
        }
    }

    /// The stored `raw` comes from the source, and the name must be in the evidence. M1's saved
    /// outputs had both kinds of miss.
    #[test]
    fn name_must_be_inside_its_evidence() {
        let text = "Regards, Tomas Berg, Daniel Okonkwo at Harbor.";
        // Spelled differently: found by case, stored as the source has it.
        let out = check_reply(&one(text), &[], &[person("TOMAS berg", "Tomas Berg,")]);
        assert_eq!(out.kept[0].raw, "Tomas Berg");
        // Invented name with real evidence, and a misspelling.
        let out = check_reply(
            &one(text),
            &[],
            &[
                person("Daniel Okonkwo-Hart", "Daniel Okonkwo at"),
                person("toomas", "Tomas Berg,"),
            ],
        );
        assert_eq!((out.kept.len(), out.discarded), (0, 2));
    }

    #[test]
    fn a_whole_word_is_preferred_to_part_of_one() {
        let text = "Johnson met John Smith.";
        let out = check_reply(
            &one(text),
            &[],
            &[person("John", "Johnson met John Smith.")],
        );
        assert_eq!(out.kept[0].raw, "John");
        assert_eq!(span_text(text, out.kept[0].span), text);
        // Writing without spaces still matches.
        let out = check_reply(&one("由张三签名"), &[], &[person("张三", "由张三签名")]);
        assert_eq!(out.kept[0].raw, "张三");
    }

    /// Rule 3: spans are per segment. Evidence may cross a boundary and is clipped; a name may
    /// not.
    #[test]
    fn segment_boundaries() {
        let pieces = vec![
            Piece {
                segment: 10,
                start: 0,
                text: "Signed by Anna".into(),
            },
            Piece {
                segment: 11,
                start: 0,
                text: "Berg for Harbor Ltd today".into(),
            },
        ];
        let out = check_reply(&pieces, &[], &[person("Anna Berg", "by Anna Berg for")]);
        assert_eq!((out.kept.len(), out.discarded), (0, 1));

        let out = check_reply(
            &pieces,
            &[],
            &[pred(
                "Harbor Ltd",
                EntityType::Org,
                "Anna Berg for Harbor Ltd",
                0.7,
            )],
        );
        let m = &out.kept[0];
        assert_eq!(m.segment, 11);
        assert_eq!(span_text(&pieces[1].text, m.span), "Berg for Harbor Ltd");
    }

    /// A window of a long segment gives spans in the segment, not the window.
    #[test]
    fn windows_map_to_the_segment() {
        let pieces = vec![Piece {
            segment: 5,
            start: 100,
            text: "wrote Anna Berg here".into(),
        }];
        let out = check_reply(&pieces, &[], &[person("Anna Berg", "Anna Berg")]);
        assert_eq!(out.kept[0].span, (106, 115));
    }

    /// `09-model-check.md` §5.3: the name's own span decides.
    #[test]
    fn identifier_overlap_is_on_the_names_span() {
        let text = "John Smith <john@x.example> called";
        let ids = [IdentifierSpan {
            segment: 1,
            start: 12,
            end: 26,
        }];
        let out = check_reply(
            &one(text),
            &ids,
            &[
                person("John Smith", "John Smith <john@x.example>"),
                person("john", "<john@x.example>"),
            ],
        );
        assert_eq!(out.identifier_dropped, 1);
        assert_eq!(out.discarded, 0);
        assert_eq!(out.kept.len(), 1);
        assert_eq!(out.kept[0].raw, "John Smith");
        assert_eq!(
            span_text(text, out.kept[0].span),
            "John Smith <john@x.example>"
        );
        // Another segment's identifier does not count.
        let other = [IdentifierSpan {
            segment: 2,
            start: 0,
            end: 40,
        }];
        let out = check_reply(&one(text), &other, &[person("John Smith", "John Smith")]);
        assert_eq!(out.kept.len(), 1);
    }

    #[test]
    fn duplicates_in_a_unit_go() {
        let out = check_reply(
            &one("Anna Berg and more"),
            &[],
            &[
                person("Anna Berg", "Anna Berg"),
                person("anna berg", "Anna Berg"),
                person("Anna Berg", "Anna Berg and"),
            ],
        );
        // The third has another span.
        assert_eq!(out.kept.len(), 2);
    }

    #[test]
    fn a_name_of_only_punctuation_has_no_key() {
        let out = check_reply(&one("see --- here"), &[], &[person("---", "---")]);
        assert_eq!((out.kept.len(), out.discarded), (0, 1));
    }

    #[test]
    fn empty_reply_keeps_nothing() {
        assert_eq!(check_reply(&one("x"), &[], &[]), CheckOutcome::default());
        assert_eq!(check_reply(&[], &[], &[]), CheckOutcome::default());
    }

    #[test]
    fn verify_mention_rules() {
        assert!(verify_mention(
            "Mr  Corvin\nHale wrote",
            (0, 15),
            "corvin hale"
        ));
        assert!(!verify_mention(
            "Mr Corvin Hale wrote",
            (0, 5),
            "Corvin Hale"
        ));
        assert!(!verify_mention("short", (2, 90), "o"));
        assert!(!verify_mention("short", (2, 2), "o"));
    }

    fn piece_strategy() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::vec("[a-zA-ZéİßΣ漢 \t\n\u{00a0}\u{2003}]{0,40}", 1..5)
    }

    proptest! {
        /// INV-AI-01 over Unicode and whitespace, whatever the model replies: every kept mention's
        /// span is in its segment and holds the name, and its name is in the source.
        #[test]
        fn inv_ai_01_kept_is_always_in_the_source(
            texts in piece_strategy(),
            name in "[a-zA-ZéİßΣ漢 ]{1,10}",
            evidence in "[a-zA-ZéİßΣ漢 ]{1,20}",
            quote in any::<bool>(),
            from in 0usize..30,
            len in 1usize..15,
        ) {
            let pieces: Vec<Piece> = texts.iter().enumerate().map(|(i, t)| Piece {
                segment: i as i64 + 1,
                start: 0,
                text: t.clone(),
            }).collect();
            // Half the time the reply really quotes the unit.
            let joined: Vec<char> = unit_text(&pieces).chars().collect();
            let (evidence, name) = if quote && !joined.is_empty() {
                let s = from.min(joined.len() - 1);
                let e = (s + len).min(joined.len());
                let q: String = joined[s..e].iter().collect();
                (q.clone(), q)
            } else {
                (evidence, name)
            };
            if evidence.trim().is_empty() || name.trim().is_empty() {
                return Ok(());
            }
            let out = check_reply(&pieces, &[], &[person(&name, &evidence)]);
            prop_assert_eq!(out.kept.len() as u64 + out.discarded, 1);
            for m in &out.kept {
                let piece = &pieces[m.segment as usize - 1];
                prop_assert!(verify_mention(&piece.text, m.span, &m.raw),
                    "{:?} not in {:?} at {:?}", m.raw, piece.text, m.span);
            }
        }
    }
}
