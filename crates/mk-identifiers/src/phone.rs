//! Phone numbers (US-04.2): validated and normalized to E.164 by `phonenumber` (libphonenumber
//! metadata), or kept as digits when no region is set (INV-ID-04).

use std::fmt;
use std::sync::LazyLock;

use mk_kernel::IdentifierKind;
use phonenumber::country::Id;
use regex::Regex;

use crate::{ByteHit, Claims};

/// A phone region: an ISO 3166-1 alpha-2 code the phone metadata knows (US-01.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Region(Id);

impl Region {
    /// `GB`, `de`, … `None` for a code the phone metadata does not have.
    pub fn parse(code: &str) -> Option<Region> {
        code.trim()
            .to_ascii_uppercase()
            .parse::<Id>()
            .ok()
            .map(Region)
    }

    pub fn code(&self) -> String {
        self.0.as_ref().to_string()
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_ref())
    }
}

/// A run of digit groups: an optional `+`, then groups of digits or a bracketed area code,
/// separated by a space, by `.`, `-` or `–` with a space on either side or none, or by a line
/// break. [`find`] splits a run at a line break unless it follows `)` or a dash: a PDF line wraps
/// there (`(202)⏎512–7350`, `418–⏎1665`), while numbers on lines of their own stay apart.
static CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:\+ ?)?(?:\([0-9]{1,5}\)|[0-9]+)",
        r"(?:(?: ?[.\-–] ?|[\-–]\n| |\n)(?:\([0-9]{1,5}\)|[0-9]+))*",
    ))
    .expect("phone pattern compiles")
});
/// Numeric dates: `2025-03-27`, `27.03.2025`, `3/27/25`.
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[0-9]{4}[-./][0-9]{1,2}[-./][0-9]{1,2}|[0-9]{1,2}[-./][0-9]{1,2}[-./][0-9]{2,4})$",
    )
    .expect("date pattern compiles")
});
/// Words just before a number that make it a reference, not a phone (US-04.2 AC4).
static REFERENCE_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\binvoice|\binv|\bno\.|#|\bref\.?)[\s:#.\-]*$")
        .expect("ref pattern compiles")
});
/// Words that say a number is a phone, which outweigh a reference word.
static PHONE_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:tel|phone|telephone|fax|mobile|mob|cell|call)\b")
        .expect("phone word pattern compiles")
});

/// Digits in a phone number (E.164 allows 15; 7 is the shortest the corpus and AC3 use).
const MIN_DIGITS: usize = 7;
const MAX_DIGITS: usize = 15;
/// Characters before a number searched for a reference word.
const CONTEXT_CHARS: usize = 24;

/// One digit group of a candidate, by byte range.
#[derive(Clone, Copy)]
struct Group {
    start: usize,
    end: usize,
    digits: usize,
}

/// The `n` characters of `text` before byte `at`.
fn before(text: &str, at: usize, n: usize) -> &str {
    let from = text[..at]
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map_or(0, |(i, _)| i);
    &text[from..at]
}

/// Whether a candidate run at `start..end` of `text` may hold phones, by its surroundings: not
/// glued to a word or a code (`VC-0418822`, `ID12345678`, `0418-77b`), not a decimal amount or
/// after a currency sign, not after "invoice", "inv", "no.", "#" or "ref" (unless a phone word
/// like "tel" is near too).
fn run_plausible(text: &str, start: usize, end: usize) -> bool {
    let mut prev = text[..start].chars().rev();
    match prev.next() {
        Some(c) if c.is_alphanumeric() || c == '_' => return false,
        Some('-' | '.' | '/' | '–') if prev.next().is_some_and(|c| c.is_alphanumeric()) => {
            return false;
        }
        _ => {}
    }
    let mut next = text[end..].chars();
    match next.next() {
        Some(c) if c.is_alphanumeric() || c == '_' => return false,
        // A decimal tail (`1250.00`, `1.250,00`) or a longer code (`0418-77`, `12/03`).
        Some('.' | ',' | '-' | '/' | '–') if next.next().is_some_and(|c| c.is_ascii_digit()) => {
            return false;
        }
        _ => {}
    }
    let context = before(text, start, CONTEXT_CHARS);
    let tail = context.trim_end();
    if tail.ends_with(['$', '£', '€', '¥'])
        || ["USD", "EUR", "GBP"].iter().any(|c| tail.ends_with(c))
    {
        return false;
    }
    !(REFERENCE_BEFORE.is_match(context) && !PHONE_WORD.is_match(context))
}

/// The identifier a candidate run is, if any (US-04.2 AC1–AC3).
fn classify(raw: &str, region: Option<Region>) -> Option<(IdentifierKind, String)> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if !(MIN_DIGITS..=MAX_DIGITS).contains(&digits.len()) {
        return None;
    }
    let e164 = |region: Option<Id>, number: &str| {
        let n = phonenumber::parse(region, number).ok()?;
        phonenumber::is_valid(&n).then(|| n.format().mode(phonenumber::Mode::E164).to_string())
    };
    if raw.starts_with('+') {
        return e164(None, &format!("+{digits}")).map(|v| (IdentifierKind::Phone, v));
    }
    match region {
        Some(r) => e164(Some(r.0), &digits).map(|v| (IdentifierKind::Phone, v)),
        // ASSUMPTION (M4b): without a region only a formatted national number is read (a leading
        // 0 or an area code in brackets, and a separator), so plain digit runs stay out.
        None => {
            let formatted = (raw.starts_with('0') || raw.starts_with('('))
                && raw.contains([' ', '-', '–', '.', ')']);
            formatted.then_some((IdentifierKind::PhoneUnnormalized, digits))
        }
    }
}

/// Each phone number not inside an earlier hit (US-04.2). A candidate run of digit groups is
/// read as its longest valid stretch, and the rest of the run is searched again, so a number
/// next to other digits (`020 7946 0318 2`) is still found. Only stretches split at spaces are
/// tried: a dash or dot inside a stretch joins its groups.
pub(crate) fn find(text: &str, claims: &Claims, region: Option<Region>) -> Vec<ByteHit> {
    let mut out = Vec::new();
    for (start, end) in CANDIDATE
        .find_iter(text)
        .flat_map(|m| runs(text, m.start(), m.end()))
    {
        if !run_plausible(text, start, end) {
            continue;
        }
        let groups = groups(text, start, end);
        let mut i = 0;
        while i < groups.len() {
            let mut found = None;
            // The longest stretch first, at most MAX_DIGITS digits.
            let mut digits = 0;
            let mut ends = Vec::new();
            for (j, g) in groups.iter().enumerate().skip(i) {
                digits += g.digits;
                if digits > MAX_DIGITS {
                    break;
                }
                ends.push(j);
            }
            for &j in ends.iter().rev() {
                let (start, end) = (groups[i].start, groups[j].end);
                let raw = &text[start..end];
                if claims.overlaps(start, end) || DATE.is_match(&raw.replace('–', "-")) {
                    continue;
                }
                if let Some((kind, value)) = classify(raw, region) {
                    found = Some((j, ByteHit::new(start, end, kind, value)));
                    break;
                }
            }
            match found {
                Some((j, hit)) => {
                    out.push(hit);
                    i = j + 1;
                }
                None => i += 1,
            }
        }
    }
    out
}

/// A candidate split at each line break that does not follow `)` or a dash.
fn runs(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = start;
    for (i, _) in text[start..end].match_indices('\n') {
        let at = start + i;
        if !text[..at].ends_with([')', '-', '–']) {
            out.push((from, at));
            from = at + 1;
        }
    }
    out.push((from, end));
    out
}

/// The stretches a candidate can start and end at: each group with what binds it to the next
/// (`-`, `.`, `–`) merged in, so a stretch only splits at a plain space. A leading `+` belongs to
/// the first group.
fn groups(text: &str, start: usize, end: usize) -> Vec<Group> {
    let s = &text[start..end];
    let mut out: Vec<Group> = Vec::new();
    let mut glued = false;
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < s.len() {
        let c = s[i..].chars().next().expect("in bounds");
        if c.is_ascii_digit() || c == '(' || (c == '+' && i == 0) {
            // A group: a `(…)` area code, a digit run, or `+` and the first digit run.
            let g_start = i;
            if c == '+' {
                i += 1;
                if bytes.get(i) == Some(&b' ') {
                    i += 1;
                }
            }
            if bytes.get(i) == Some(&b'(') {
                i = s[i..].find(')').map_or(s.len(), |p| i + p + 1);
            } else {
                while i < s.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let digits = s[g_start..i].bytes().filter(u8::is_ascii_digit).count();
            match out.last_mut() {
                Some(last) if glued => {
                    last.end = start + i;
                    last.digits += digits;
                }
                _ => out.push(Group {
                    start: start + g_start,
                    end: start + i,
                    digits,
                }),
            }
            glued = false;
        } else {
            // A separator: glued unless it is spaces only.
            let sep_start = i;
            while i < s.len() {
                let c = s[i..].chars().next().expect("in bounds");
                if c.is_ascii_digit() || c == '(' {
                    break;
                }
                i += c.len_utf8();
            }
            glued = s[sep_start..i].chars().any(|c| c != ' ');
            // `+44 (0)20` style or a bracketed area code after a space: glue a `(`… group to a
            // `+` country code before it.
            if !glued && bytes.get(i) == Some(&b'(') && out.len() == 1 && s.starts_with('+') {
                glued = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{gb, keys};
    use crate::{Rules, find_all};

    fn region(code: &str) -> Rules {
        Rules {
            phone_region: Region::parse(code),
        }
    }

    fn phones(text: &str, rules: &Rules) -> Vec<String> {
        keys(text, rules).into_iter().map(|(k, _)| k).collect()
    }

    /// US-04.2 AC1: an international number is stored as E.164, with or without a region.
    #[test]
    fn us_04_2_ac1_international_format() {
        for rules in [Rules::default(), gb(), region("US")] {
            assert_eq!(
                keys("Call +44 20 7946 0958 now", &rules),
                vec![("phone:+442079460958".into(), "+44 20 7946 0958".into())]
            );
        }
        for (text, want) in [
            ("+1 (202) 555-0147", "phone:+12025550147"),
            ("+1 202-555-0147", "phone:+12025550147"),
            ("+61 491 570 156", "phone:+61491570156"),
            ("+441134960461", "phone:+441134960461"),
        ] {
            assert_eq!(phones(text, &Rules::default()), vec![want], "{text}");
        }
    }

    /// US-04.2 AC2: a national number is read in the project's region.
    #[test]
    fn us_04_2_ac2_national_format_with_region() {
        assert_eq!(
            keys("Tel: 020 7946 0958.", &gb()),
            vec![("phone:+442079460958".into(), "020 7946 0958".into())]
        );
        for (text, want) in [
            ("(202) 245–0333", "phone:+12022450333"),
            ("202–418– 7400", "phone:+12024187400"),
            ("(202) 418– 1665", "phone:+12024181665"),
            ("877–925–7773", "phone:+18779257773"),
            ("202.555.0147", "phone:+12025550147"),
        ] {
            assert_eq!(phones(text, &region("US")), vec![want], "{text}");
        }
        // The same digits in another region are another number, or none.
        assert_eq!(phones("(202) 245–0333", &gb()), Vec::<String>::new());
    }

    /// US-04.2 AC3, INV-ID-04: without a region a formatted national number keeps its digits,
    /// in its own key space.
    #[test]
    fn us_04_2_ac3_national_format_without_region() {
        assert_eq!(
            keys("Tel 020 7946 0958", &Rules::default()),
            vec![(
                "phone_unnormalized:02079460958".into(),
                "020 7946 0958".into()
            )]
        );
        assert_eq!(
            phones("(202) 245-0333", &Rules::default()),
            vec!["phone_unnormalized:2022450333"]
        );
        // Plain digit runs are not read without a region.
        assert_eq!(
            phones("2022450333 or 7946 0958", &Rules::default()),
            Vec::<String>::new()
        );
    }

    /// US-04.2 AC4: invalid numbers, dates, amounts and references are not phones.
    #[test]
    fn us_04_2_ac4_no_false_phones() {
        for rules in [gb(), region("US"), Rules::default()] {
            for text in [
                "2025-03-27",
                "27.03.2025",
                "05/14/2025",
                "3/27/25",
                "£1,250.00",
                "1250.00",
                "€ 1.250.000",
                "USD 12 500 000",
                "Invoice 2025 0418 822",
                "INV-2025-0418",
                "inv. 020 7946 0318",
                "Ref: 0207 946 0318",
                "No. 020 7946 0318",
                "# 020 7946 0318",
                "VC-0418822",
                "ID02079460318",
                "020 7946 0318x",
                "1234567",
            ] {
                assert_eq!(phones(text, &rules), Vec::<String>::new(), "{text}");
            }
        }
        // Not valid in the region.
        assert_eq!(
            phones("(123) 456-7890", &region("US")),
            Vec::<String>::new()
        );
        // A phone word outweighs "no.".
        assert_eq!(
            phones("Phone no. 020 7946 0318", &gb()),
            vec!["phone:+442079460318"]
        );
    }

    /// The longest valid stretch of a run is read, and the rest searched again.
    #[test]
    fn runs_are_split_at_spaces() {
        assert_eq!(
            phones("020 7946 0318 2 times, 020 7946 0102 020 7946 0555", &gb()),
            vec![
                "phone:+442079460318",
                "phone:+442079460102",
                "phone:+442079460555"
            ]
        );
    }

    #[test]
    fn region_codes() {
        assert_eq!(Region::parse("gb").map(|r| r.code()).as_deref(), Some("GB"));
        assert_eq!(
            Region::parse(" DE ").map(|r| r.to_string()),
            Some("DE".into())
        );
        assert!(Region::parse("UK").is_none());
        assert!(Region::parse("XX").is_none());
        assert!(Region::parse("").is_none());
    }

    /// A PDF line wraps after the area code or a dash; a number on a line of its own is not
    /// joined to the one above.
    #[test]
    fn wrapped_line_after_bracket_or_dash() {
        let us = region("US");
        for text in [
            "call (202)\n512–7350.",
            "Colombo, (202) 418–\n1665, or",
            "or 202–418–\n7400.",
        ] {
            let k = keys(text, &us);
            assert_eq!(k.len(), 1, "{text:?}: {k:?}");
            assert!(k[0].0.starts_with("phone:+1202"), "{k:?}");
        }
        assert_eq!(
            keys("Tel 020 7946 0318\n2024 was the year", &gb()),
            [("phone:+442079460318".into(), "020 7946 0318".into())]
        );
        assert_eq!(keys("0113\n496\n0461", &gb()), []);
    }

    #[test]
    fn phones_have_full_confidence() {
        let f = find_all("+44 20 7946 0958", &Rules::default());
        assert_eq!(f.hits[0].confidence, 1.0);
    }
}
