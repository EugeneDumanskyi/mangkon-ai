//! Custom patterns (US-04.7, INV-ID-07): a project's own regular expressions, named, whose
//! matches are identifiers of the type `custom:<name>`.
//!
//! The `regex` crate runs in time linear in the text, whatever the pattern; it has no
//! look-around and no backreferences, so a pattern using them is refused with a plain reason
//! (AC2). Patterns are also refused when they are too large or can match empty text. Custom
//! matches take no part in the built-in finders' claims: the two steps are independent, so
//! either can run again alone (INV-ID-08, ASSUMPTION M4c).

use std::fmt;

use mk_kernel::{IdentifierKey, MAX_PATTERN_NAME, Span, valid_pattern_name};
use regex::{Regex, RegexBuilder};
use regex_syntax::ast::ErrorKind as AstErrorKind;

use crate::{Claims, IdentifierHit, card};

/// The compiled program size a pattern may take (the `regex` default is 10 MiB).
/// ASSUMPTION (M4c): 1 MiB, far above any pattern a person types.
pub const SIZE_LIMIT: usize = 1 << 20;
/// Groups and repetitions nested deeper than this are refused.
pub const NEST_LIMIT: u32 = 64;

/// A named, compiled custom pattern.
#[derive(Clone, Debug)]
pub struct CustomPattern {
    name: String,
    regex: Regex,
}

/// Why a pattern was refused (US-04.7 AC2). The message never repeats the text it is tested
/// on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PatternError {
    /// The name is empty, longer than 40 characters, or holds `:` or a control character.
    Name,
    Empty,
    LookAround,
    Backreference,
    TooLarge,
    MatchesEmpty,
    /// Any other syntax error, at a 1-based character column of the expression.
    Syntax {
        column: usize,
        message: String,
    },
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternError::Name => write!(
                f,
                "A pattern name has 1 to {MAX_PATTERN_NAME} characters, no \":\" and no spaces \
                 at either end"
            ),
            PatternError::Empty => f.write_str("Enter an expression"),
            PatternError::LookAround => f.write_str(
                "Look-ahead and look-behind, such as (?=…) or (?<!…), are not supported",
            ),
            PatternError::Backreference => {
                f.write_str("Backreferences, such as \\1, are not supported")
            }
            PatternError::TooLarge => {
                f.write_str("The pattern is too large: make it shorter or simpler")
            }
            PatternError::MatchesEmpty => f.write_str(
                "The pattern can match empty text: make it match at least one character",
            ),
            PatternError::Syntax { column, message } => {
                write!(f, "Invalid pattern at column {column}: {message}")
            }
        }
    }
}

impl std::error::Error for PatternError {}

impl CustomPattern {
    /// Checks and compiles `expression` under `name` (US-04.7 AC1/AC2, INV-ID-07).
    pub fn compile(name: &str, expression: &str) -> Result<Self, PatternError> {
        if !valid_pattern_name(name) {
            return Err(PatternError::Name);
        }
        if expression.is_empty() {
            return Err(PatternError::Empty);
        }
        let ast = regex_syntax::ast::parse::ParserBuilder::new()
            .nest_limit(NEST_LIMIT)
            .build()
            .parse(expression)
            .map_err(|e| match e.kind() {
                AstErrorKind::UnsupportedLookAround => PatternError::LookAround,
                AstErrorKind::UnsupportedBackreference => PatternError::Backreference,
                AstErrorKind::NestLimitExceeded(_) => PatternError::TooLarge,
                kind => PatternError::Syntax {
                    column: e.span().start.column,
                    message: kind.to_string(),
                },
            })?;
        let hir = regex_syntax::hir::translate::Translator::new()
            .translate(expression, &ast)
            .map_err(|e| PatternError::Syntax {
                column: e.span().start.column,
                message: e.kind().to_string(),
            })?;
        if hir.properties().minimum_len().is_none_or(|n| n == 0) {
            return Err(PatternError::MatchesEmpty);
        }
        let regex = RegexBuilder::new(expression)
            .size_limit(SIZE_LIMIT)
            .dfa_size_limit(SIZE_LIMIT)
            .nest_limit(NEST_LIMIT)
            .build()
            .map_err(|e| match e {
                regex::Error::CompiledTooBig(_) => PatternError::TooLarge,
                other => PatternError::Syntax {
                    column: 1,
                    message: other.to_string(),
                },
            })?;
        Ok(CustomPattern {
            name: name.to_string(),
            regex,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn expression(&self) -> &str {
        self.regex.as_str()
    }
}

/// Every match of `patterns` in `text`, in text order (then pattern order), as identifier hits
/// of type `custom:<name>` (US-04.7 AC3). Spans count characters (INV-CO-04); the value is the
/// match with spaces trimmed at both ends, case kept. A match holding a card-like number is
/// dropped: a card number never becomes an identifier (INV-ID-06, NFR-SEC-14).
pub fn find_custom(text: &str, patterns: &[CustomPattern]) -> Vec<IdentifierHit> {
    let mut byte_hits: Vec<(usize, usize, usize)> = Vec::new();
    for (p, pattern) in patterns.iter().enumerate() {
        for m in pattern.regex.find_iter(text) {
            let raw = m.as_str();
            let lead = raw.len() - raw.trim_start().len();
            let trimmed = raw.trim();
            if trimmed.is_empty() || !card::find(trimmed, &Claims::default()).is_empty() {
                continue;
            }
            let start = m.start() + lead;
            byte_hits.push((start, start + trimmed.len(), p));
        }
    }
    byte_hits.sort_by_key(|&(s, _, p)| (s, p));
    let mut bounds: Vec<usize> = byte_hits.iter().flat_map(|h| [h.0, h.1]).collect();
    bounds.sort_unstable();
    bounds.dedup();
    let mut chars = crate::CharOffsets::new(text);
    let len = chars.len;
    let at: std::collections::HashMap<usize, usize> =
        bounds.iter().map(|&b| (b, chars.at(b))).collect();
    byte_hits
        .into_iter()
        .filter_map(|(s, e, p)| {
            let raw = &text[s..e];
            Some(IdentifierHit {
                span: Span::new(at[&s], at[&e], len).ok()?,
                raw: raw.to_string(),
                key: IdentifierKey::custom(&patterns[p].name, raw)?,
                confidence: 1.0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn tender() -> CustomPattern {
        CustomPattern::compile("tender_ref", r"VPA-TND-\d{4}-\d{3}").unwrap()
    }

    fn values(text: &str, patterns: &[CustomPattern]) -> Vec<String> {
        find_custom(text, patterns)
            .into_iter()
            .map(|h| h.key.to_string())
            .collect()
    }

    /// US-04.7 AC1: matches come out with their pattern's name as type, at character spans.
    #[test]
    fn us_04_7_ac1_matches_named_after_the_pattern() {
        let text = "Größe: VPA-TND-2024-017 and VPA-TND-2024-018.";
        let hits = find_custom(text, &[tender()]);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].key.to_string(),
            "custom:tender_ref:VPA-TND-2024-017"
        );
        assert_eq!(hits[0].key.type_name(), "custom:tender_ref");
        for h in &hits {
            let got: String = text
                .chars()
                .skip(h.span.start())
                .take(h.span.len())
                .collect();
            assert_eq!(got, h.raw);
        }
    }

    /// D14: values are trimmed, case kept; patterns are independent and may overlap.
    #[test]
    fn values_trimmed_case_kept_patterns_independent() {
        let a = CustomPattern::compile("Case", r" ?[A-Z]{2}-\d+ ?").unwrap();
        let b = CustomPattern::compile("num", r"\d+").unwrap();
        assert_eq!(
            values("see AB-12 now", &[a, b]),
            vec!["custom:Case:AB-12", "custom:num:12"]
        );
    }

    /// INV-ID-06: a match holding a card-like number is dropped.
    #[test]
    fn inv_id_06_card_like_match_is_dropped() {
        let p = CustomPattern::compile("acct", r"ACC-[\d ]+").unwrap();
        assert!(values("ACC-4111 1111 1111 1111", std::slice::from_ref(&p)).is_empty());
        assert_eq!(
            values("ACC-4111 1111", &[p]),
            vec!["custom:acct:ACC-4111 1111"]
        );
    }

    /// US-04.7 AC2: look-around, backreferences, oversize, empty matches and bad syntax are
    /// refused, each with a plain reason.
    #[test]
    fn us_04_7_ac2_invalid_patterns_are_refused() {
        let err = |e: &str| CustomPattern::compile("p", e).unwrap_err();
        assert_eq!(err(r"VPA(?=-TND)"), PatternError::LookAround);
        assert_eq!(err(r"(?<!x)VPA"), PatternError::LookAround);
        assert_eq!(err(r"(a)\1"), PatternError::Backreference);
        assert_eq!(err(r"\w{1000}\w{1000}\w{1000}"), PatternError::TooLarge);
        let deep = format!("{}a{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(err(&deep), PatternError::TooLarge);
        assert_eq!(err("a*"), PatternError::MatchesEmpty);
        assert_eq!(err(r"\b"), PatternError::MatchesEmpty);
        assert_eq!(err(""), PatternError::Empty);
        match err("abc[") {
            PatternError::Syntax { column, .. } => assert_eq!(column, 4),
            other => panic!("{other:?}"),
        }
        let e = err("ab(c");
        assert!(
            e.to_string().starts_with("Invalid pattern at column 3"),
            "{e}"
        );
        assert!(err(r"(?=x)").to_string().contains("not supported"));
        assert_eq!(
            CustomPattern::compile("a:b", "x").unwrap_err(),
            PatternError::Name
        );
    }

    /// Linear time: a pattern that backtracks exponentially elsewhere stays fast here.
    #[test]
    fn us_04_7_ac2_linear_time() {
        let p = CustomPattern::compile("evil", "(a*)*b").unwrap();
        let text = "a".repeat(100_000);
        let t = Instant::now();
        assert!(find_custom(&text, &[p]).is_empty());
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "{:?}",
            t.elapsed()
        );
    }
}
