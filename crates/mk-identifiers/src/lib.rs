//! BC-04 Identifiers: validators and extractors (domain §3). Pure functions, no I/O
//! (ADR-0002 rule 1).
//!
//! M2b has email ([`find_emails`]). The other identifier types, domains derived from emails
//! (INV-ID-05) and custom patterns come in M4. [`luhn`] (M4a) backs the card-like checks.

pub mod luhn;

use std::sync::LazyLock;

use mk_kernel::{IdentifierKey, IdentifierKind, Span};
use regex::Regex;

/// One identifier found in a segment's stored text (INV-ID-03).
#[derive(Clone, PartialEq, Debug)]
pub struct IdentifierHit {
    /// Characters of the text the hit was found in (INV-CO-04).
    pub span: Span,
    /// The text exactly as written.
    pub raw: String,
    pub key: IdentifierKey,
    /// 0–1. Text-native hits are 1.0; OCR hits take their word confidence (M3).
    pub confidence: f32,
}

/// ASCII email addresses: a dot-separated local part, `@`, then at least two domain labels with
/// a letters-only top-level label. Labels never start or end with `-`, so trailing punctuation
/// (`a@b.example.`) stays out of the match. Linear time (the `regex` crate has no backtracking).
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"[A-Za-z0-9_%+-]+(?:\.[A-Za-z0-9_%+-]+)*",
        r"@",
        r"(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+[A-Za-z]{2,63}\b",
    ))
    .expect("email pattern compiles")
});

/// Normalized email value: lower case (US-04.1 AC1). ASSUMPTION: the local part is
/// case-insensitive too, as every common mail provider treats it.
pub fn normalize_email(raw: &str) -> String {
    raw.to_ascii_lowercase()
}

/// Every email address in `text`, in text order (US-04.1 AC1).
pub fn find_emails(text: &str) -> Vec<IdentifierHit> {
    let len = text.chars().count();
    // Matches come in text order, so one cursor converts byte offsets to char offsets.
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut to_chars = |b: usize| {
        chars += text[byte..b].chars().count();
        byte = b;
        chars
    };
    EMAIL
        .find_iter(text)
        .map(|m| {
            let start = to_chars(m.start());
            let end = to_chars(m.end());
            let raw = m.as_str();
            IdentifierHit {
                span: Span::new(start, end, len).expect("a match is non-empty and inside the text"),
                raw: raw.to_string(),
                key: IdentifierKey::new(IdentifierKind::Email, normalize_email(raw)),
                confidence: 1.0,
            }
        })
        .collect()
}

/// Every built-in identifier in `text`. In M2b: emails.
pub fn find_all(text: &str) -> Vec<IdentifierHit> {
    find_emails(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn values(text: &str) -> Vec<(usize, usize, String)> {
        find_emails(text)
            .into_iter()
            .map(|h| (h.span.start(), h.span.end(), h.key.value().to_string()))
            .collect()
    }

    /// US-04.1 AC1, INV-ID-01: the key is the lower-cased address.
    #[test]
    fn us_04_1_ac1_email_is_lower_cased() {
        let hits = find_emails("Write to John.Smith@Example.COM today");
        assert_eq!(hits.len(), 1);
        let h = &hits[0];
        assert_eq!(h.raw, "John.Smith@Example.COM");
        assert_eq!(h.key.to_string(), "email:john.smith@example.com");
        assert_eq!((h.span.start(), h.span.end()), (9, 31));
        assert_eq!(h.confidence, 1.0);
    }

    #[test]
    fn punctuation_around_the_address_is_left_out() {
        assert_eq!(
            values("(mail: <a.b@x.example>, or c@y.example.)"),
            vec![
                (8, 21, "a.b@x.example".into()),
                (27, 38, "c@y.example".into()),
            ]
        );
        assert_eq!(
            values("mailto:priya.ramanathan-cole@velmora-port.example;"),
            vec![(7, 49, "priya.ramanathan-cole@velmora-port.example".into())]
        );
    }

    #[test]
    fn not_emails() {
        for text in [
            "user@localhost",
            "@example.com",
            "a@-x.example",
            "a@x.c",
            "a@x.123",
            "a..b",
            "price 5@3.50",
        ] {
            assert_eq!(values(text), vec![], "{text}");
        }
    }

    /// INV-CO-04: offsets count characters, not bytes, so text before the hit may be non-ASCII.
    #[test]
    fn inv_co_04_offsets_are_characters() {
        let text = "Größe — €5 an j@x.example und k@y.example";
        let hits = find_emails(text);
        assert_eq!(hits.len(), 2);
        for h in hits {
            let got: String = text
                .chars()
                .skip(h.span.start())
                .take(h.span.len())
                .collect();
            assert_eq!(got, h.raw);
        }
    }

    proptest! {
        /// INV-CO-04, INV-ID-03: every span slices back to its raw text, and the key is the
        /// normalized raw text.
        #[test]
        fn inv_co_04_span_slices_back_to_raw(
            parts in proptest::collection::vec(
                prop_oneof![
                    "[ a-zé€,;:<>()\n]{0,8}",
                    "[A-Za-z0-9._%+-]{1,10}@[a-z0-9-]{1,8}\\.[A-Za-z]{2,6}",
                ],
                0..8,
            )
        ) {
            let text: String = parts.concat();
            for h in find_emails(&text) {
                let got: String = text.chars().skip(h.span.start()).take(h.span.len()).collect();
                prop_assert_eq!(&got, &h.raw);
                prop_assert_eq!(h.key.value(), normalize_email(&h.raw));
            }
        }
    }
}
