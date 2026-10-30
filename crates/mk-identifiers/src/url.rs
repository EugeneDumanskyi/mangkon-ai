//! URLs and the domain of each (US-04.3 AC1).

use std::sync::LazyLock;

use mk_kernel::IdentifierKind;
use regex::Regex;

use crate::{ByteHit, Claims};

/// `http://`, `https://` or `www.`, then everything up to white space or a character that never
/// appears unescaped in a URL. Trailing punctuation is trimmed afterwards.
static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:https?://|www\.)[^\s<>"'`{}|\\^\[\]]+"#).expect("url pattern compiles")
});

/// A host: dot-separated labels of letters, digits and `-`, the last one letters only (or an
/// IDNA `xn--` label).
fn valid_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let tld = labels.last().copied().unwrap_or("");
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && (tld.chars().all(char::is_alphabetic) || tld.starts_with("xn--"))
        && tld.chars().count() >= 2
}

/// The match without trailing sentence punctuation: `.,;:!?'"`, and a `)` that closes nothing
/// in the URL.
fn trim_end(url: &str) -> &str {
    let mut s = url;
    loop {
        let Some(c) = s.chars().next_back() else {
            return s;
        };
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            _ => false,
        };
        if !drop {
            return s;
        }
        s = &s[..s.len() - c.len_utf8()];
    }
}

/// Each URL not inside an earlier hit, followed by its host's registrable domain at the same
/// span (INV-ID-05). The value is the URL as written, with the scheme and host lower case and
/// `https://` put before a `www.` form (the ground-truth convention, `docs/07-test-corpus.md`
/// §5).
pub(crate) fn find(text: &str, claims: &Claims) -> Vec<ByteHit> {
    let mut out = Vec::new();
    for m in URL.find_iter(text) {
        let raw = trim_end(m.as_str());
        let (start, end) = (m.start(), m.start() + raw.len());
        if claims.overlaps(start, end) {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        let (scheme, rest) = if lower.starts_with("www.") {
            ("https://", raw)
        } else {
            let i = raw.find("://").expect("the pattern has a scheme") + 3;
            (&lower[..i], &raw[i..])
        };
        let host_end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
        let host = rest[..host_end].to_lowercase();
        if !valid_host(&host) {
            continue;
        }
        let value = format!("{scheme}{host}{}", &rest[host_end..]);
        out.push(ByteHit::new(start, end, IdentifierKind::Url, value));
        if let Some(domain) = crate::registrable_domain(&host) {
            out.push(ByteHit::new(start, end, IdentifierKind::Domain, domain));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::Rules;
    use crate::test_util::keys;

    /// US-04.3 AC1 (example as fixed in M4b: PSL wins): a URL and its registrable domain.
    #[test]
    fn us_04_3_ac1_url_and_domain() {
        assert_eq!(
            keys(
                "See https://www.shell-co.example/invoice?id=5.",
                &Rules::default()
            ),
            vec![
                (
                    "url:https://www.shell-co.example/invoice?id=5".into(),
                    "https://www.shell-co.example/invoice?id=5".into()
                ),
                (
                    "domain:shell-co.example".into(),
                    "https://www.shell-co.example/invoice?id=5".into()
                ),
            ]
        );
    }

    #[test]
    fn www_forms_get_https_and_hosts_are_lower_cased() {
        let k = keys(
            "(www.fisheries.noaa.gov/find-species) and HTTP://Edis.USITC.gov/X",
            &Rules::default(),
        );
        assert_eq!(
            k.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            vec![
                "url:https://www.fisheries.noaa.gov/find-species",
                "domain:noaa.gov",
                "url:http://edis.usitc.gov/X",
                "domain:usitc.gov",
            ]
        );
        assert_eq!(k[0].1, "www.fisheries.noaa.gov/find-species");
    }

    #[test]
    fn balanced_parentheses_stay_in() {
        let k = keys(
            "https://en.wikipedia.example/wiki/Foo_(bar), next",
            &Rules::default(),
        );
        assert_eq!(k[0].1, "https://en.wikipedia.example/wiki/Foo_(bar)");
    }

    #[test]
    fn not_urls() {
        for text in [
            "http://localhost/x",
            "www.",
            "https://",
            "ftp://x.example",
            "awww.x.example",
        ] {
            assert_eq!(keys(text, &Rules::default()), vec![], "{text}");
        }
    }
}
