//! Emails (US-04.1) and the domain of each (US-04.3 AC2).

use std::sync::LazyLock;

use mk_kernel::{IdentifierKind, Span};
use regex::Regex;

use crate::{ByteHit, IdentifierHit};

/// ASCII email addresses: a dot-separated local part, `@`, then at least two domain labels with
/// a letters-only top-level label. Labels never start or end with `-`, so trailing punctuation
/// (`a@b.example.`) stays out of the match. A domain label may go on after a line break that
/// follows a `-` (`accounts@brightwater-⏎logistics.example`, a wrapped PDF line): a label cannot
/// end with `-`, so the break is not the end of the address. Linear time (the `regex` crate has
/// no backtracking).
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"[A-Za-z0-9_%+-]+(?:\.[A-Za-z0-9_%+-]+)*",
        r"@",
        r"(?:[A-Za-z0-9](?:(?:-\n|[A-Za-z0-9-]){0,61}[A-Za-z0-9])?\.)+[A-Za-z]{2,63}\b",
    ))
    .expect("email pattern compiles")
});

/// Normalized email value: lower case, line breaks dropped (US-04.1 AC1). ASSUMPTION: the local
/// part is case-insensitive too, as every common mail provider treats it.
pub fn normalize_email(raw: &str) -> String {
    raw.replace('\n', "").to_ascii_lowercase()
}

/// Each email, followed by the registrable domain of its host at the same span (INV-ID-05).
/// Obfuscated forms (`john [at] example [dot] com`) are not read (US-04.1 AC2).
pub(crate) fn find(text: &str) -> Vec<ByteHit> {
    let mut out = Vec::new();
    for m in EMAIL.find_iter(text) {
        let value = normalize_email(m.as_str());
        let host = value.rsplit_once('@').map(|(_, h)| h.to_string());
        out.push(ByteHit::new(
            m.start(),
            m.end(),
            IdentifierKind::Email,
            value,
        ));
        if let Some(domain) = host.as_deref().and_then(crate::registrable_domain) {
            out.push(ByteHit::new(
                m.start(),
                m.end(),
                IdentifierKind::Domain,
                domain,
            ));
        }
    }
    out
}

/// Every email address in `text`, in text order (US-04.1 AC1), without domains.
pub fn find_emails(text: &str) -> Vec<IdentifierHit> {
    let len = text.chars().count();
    // Matches come in text order, so one cursor converts byte offsets to char offsets.
    let (mut byte, mut chars) = (0usize, 0usize);
    let mut to_chars = |b: usize| {
        chars += text[byte..b].chars().count();
        byte = b;
        chars
    };
    find(text)
        .into_iter()
        .filter(|h| h.key.kind() == IdentifierKind::Email)
        .map(|h| {
            let start = to_chars(h.start);
            let end = to_chars(h.end);
            IdentifierHit {
                span: Span::new(start, end, len).expect("a match is non-empty and inside the text"),
                raw: text[h.start..h.end].to_string(),
                key: h.key,
                confidence: h.confidence,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rules;
    use crate::test_util::keys;

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

    /// US-04.1 AC2: obfuscated forms are not read.
    /// A domain wrapped after its hyphen at a line end keeps the hyphen; a line break anywhere
    /// else ends the address.
    #[test]
    fn domain_wrapped_after_hyphen() {
        let hits = find_emails("sent to accounts@brightwater-\nlogistics.example and");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].key.to_string(),
            "email:accounts@brightwater-logistics.example"
        );
        assert_eq!(hits[0].raw, "accounts@brightwater-\nlogistics.example");
        let hits = find_emails("a@b.example\nc.example");
        assert_eq!(hits[0].raw, "a@b.example");
    }

    #[test]
    fn us_04_1_ac2_obfuscated_email_is_not_detected() {
        for text in [
            "john [at] example [dot] com",
            "john(at)example.com",
            "john at example dot com",
        ] {
            assert_eq!(keys(text, &Rules::default()), vec![], "{text}");
        }
    }

    /// US-04.3 AC2: an email gives the domain of its host.
    #[test]
    fn us_04_3_ac2_domain_from_email() {
        assert_eq!(
            keys("a@shell-co.com", &Rules::default()),
            vec![
                ("email:a@shell-co.com".into(), "a@shell-co.com".into()),
                ("domain:shell-co.com".into(), "a@shell-co.com".into()),
            ]
        );
        assert_eq!(
            keys("Ops@Mail.Shell-Co.CO.UK", &Rules::default())[1].0,
            "domain:shell-co.co.uk"
        );
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
}
