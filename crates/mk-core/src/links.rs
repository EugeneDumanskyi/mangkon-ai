//! Links in shown text (US-08.4 AC3): `http`, `https` and `mailto` URLs found in the visible
//! text of a line, cell, paragraph or header. Display only: they create no identifier (M4), and
//! HTML `href`s are not kept (gap A8). The desktop opens one only after the user confirms its
//! full URL, and checks it again with [`openable_url`].

/// Longest URL shown as a link or opened (the desktop refuses longer ones).
pub const MAX_URL_CHARS: usize = 2_048;

const SCHEMES: &[&str] = &["https://", "http://", "mailto:"];

/// Characters that end a URL in running text.
fn ends_url(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '\\' | '^')
}

/// Punctuation trimmed from a URL's end: it more likely ends the sentence.
fn trailing(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | ';' | ':' | '!' | '?' | '\'' | ')' | ']' | '*'
    )
}

/// The URLs in `text` as byte ranges, in order.
pub fn find_urls(text: &str) -> Vec<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while from < text.len() {
        let Some((start, scheme)) = SCHEMES
            .iter()
            .filter_map(|s| lower[from..].find(s).map(|i| (from + i, *s)))
            .min_by_key(|(i, _)| *i)
        else {
            break;
        };
        // A scheme glued to a word (`xhttp://`) is not a link.
        let glued = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = &text[start..];
        let mut end = start + rest.find(ends_url).unwrap_or(rest.len());
        // Trim trailing punctuation; a `)` stays when it closes a `(` in the URL.
        while let Some(c) = text[start..end].chars().next_back() {
            let open = text[start..end].matches('(').count();
            let close = text[start..end].matches(')').count();
            if trailing(c) && !(c == ')' && close <= open) {
                end -= c.len_utf8();
            } else {
                break;
            }
        }
        let body = &text[start + scheme.len()..end];
        let ok = !glued
            && !body.is_empty()
            && text[start..end].chars().count() <= MAX_URL_CHARS
            && (scheme != "mailto:" || body.contains('@'));
        if ok {
            out.push((start, end));
            from = end;
        } else {
            from = start + scheme.len();
        }
    }
    out
}

/// Whether `url` may be handed to the system browser or mail app: an `http`, `https` or
/// `mailto` URL with something after the scheme, no control characters or spaces, and at most
/// [`MAX_URL_CHARS`] characters. Anything else is refused.
pub fn openable_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let Some(scheme) = SCHEMES.iter().find(|s| lower.starts_with(**s)) else {
        return false;
    };
    let body = &url[scheme.len()..];
    let host_ok = match *scheme {
        "mailto:" => body.contains('@'),
        _ => body
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '['),
    };
    host_ok
        && url.chars().count() <= MAX_URL_CHARS
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text)
            .into_iter()
            .map(|(s, e)| &text[s..e])
            .collect()
    }

    /// US-08.4 AC3: the URLs of running text, trailing punctuation trimmed.
    #[test]
    fn us_08_4_ac3_finds_urls_in_text() {
        assert_eq!(
            urls("See https://example.org/a?b=1. Or (http://x.example/p) now!"),
            ["https://example.org/a?b=1", "http://x.example/p"]
        );
        assert_eq!(
            urls("wiki https://en.example/wiki/Foo_(bar), ok"),
            ["https://en.example/wiki/Foo_(bar)"]
        );
        assert_eq!(
            urls("Write to <mailto:anna@x.example>; HTTPS://UP.EXAMPLE"),
            ["mailto:anna@x.example", "HTTPS://UP.EXAMPLE"]
        );
        assert_eq!(urls("Zoë → https://ü.example/ö…"), ["https://ü.example/ö…"]);
        assert!(urls("xhttp://glued.example mailto:nobody http:// https:").is_empty());
        assert_eq!(urls("x https://a.example\tb"), ["https://a.example"]);
    }

    /// The desktop's check before opening (US-08.4 AC3).
    #[test]
    fn us_08_4_ac3_only_web_and_mail_urls_open() {
        for ok in [
            "https://example.org/x",
            "http://127.0.0.1:8080/",
            "HTTPS://EXAMPLE.ORG",
            "mailto:a@b.example?subject=Hi",
            "https://[::1]/",
        ] {
            assert!(openable_url(ok), "{ok}");
        }
        let long = format!("https://a.example/{}", "x".repeat(MAX_URL_CHARS));
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://example.org",
            "smb://host/share",
            "https://",
            "https:///x",
            "mailto:nobody",
            "https://a.example/\nx",
            "https://a.example/ x",
            "https://a.example/\u{7}",
            " https://a.example",
            long.as_str(),
        ] {
            assert!(!openable_url(bad), "{bad:?}");
        }
    }
}
