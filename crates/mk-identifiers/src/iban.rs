//! IBAN (US-04.4): country length and mod-97 (ISO 13616).

use std::sync::LazyLock;

use mk_kernel::IdentifierKind;
use regex::Regex;

use crate::{ByteHit, Claims};

/// Country code, two check digits, then the BBAN compact or in groups separated by a single space
/// or line break. Groups of any length: OCR joins and splits them (`0417 339821`), and a PDF line
/// can break inside one; the country length and mod-97 decide.
static IBAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z]{2}[0-9]{2}(?:[ \n]?[A-Z0-9]+)+\b").expect("iban pattern compiles")
});

/// IBAN length by country (SWIFT IBAN registry). ASSUMPTION: the registry's countries as of
/// 2025; a country not listed is not read.
pub fn iban_length(country: &str) -> Option<usize> {
    Some(match country {
        "NO" => 15,
        "BE" => 16,
        "DK" | "FI" | "FO" | "GL" | "NL" | "SD" | "FK" => 18,
        "MK" | "SI" => 19,
        "AT" | "BA" | "EE" | "KZ" | "LT" | "LU" | "XK" | "MN" => 20,
        "CH" | "HR" | "LI" | "LV" => 21,
        "BG" | "BH" | "CR" | "DE" | "GB" | "GE" | "IE" | "ME" | "RS" | "VA" => 22,
        "AE" | "GI" | "IL" | "IQ" | "OM" | "SO" | "TL" => 23,
        "AD" | "CZ" | "ES" | "MD" | "PK" | "RO" | "SA" | "SE" | "SK" | "TN" | "VG" => 24,
        "LY" | "PT" | "ST" => 25,
        "IS" | "TR" => 26,
        "BI" | "DJ" | "FR" | "GR" | "IT" | "MC" | "MR" | "SM" => 27,
        "AL" | "AZ" | "BY" | "CY" | "DO" | "GT" | "HU" | "LB" | "NI" | "PL" | "SV" => 28,
        "BR" | "EG" | "PS" | "QA" | "UA" => 29,
        "JO" | "KW" | "MU" | "YE" => 30,
        "MT" | "SC" => 31,
        "LC" => 32,
        "RU" => 33,
        _ => return None,
    })
}

/// Mod-97 of the IBAN with its first four characters moved to the end and letters as numbers
/// (A = 10 … Z = 35). Expects upper-case letters and digits only.
fn mod97(iban: &str) -> u32 {
    iban[4..]
        .bytes()
        .chain(iban[..4].bytes())
        .fold(0u32, |rem, c| {
            if c.is_ascii_digit() {
                (rem * 10 + u32::from(c - b'0')) % 97
            } else {
                (rem * 100 + u32::from(c - b'A') + 10) % 97
            }
        })
}

/// Each valid IBAN not inside an earlier hit, stored compact and upper case (US-04.4 AC1). A
/// match longer than its country's length is cut after that many characters, if a group ends
/// there; one that is shorter, or fails mod-97, is not stored (AC2, INV-ID-02). The search goes
/// on after a found IBAN, so one that follows it in the same match is found too.
pub(crate) fn find(text: &str, claims: &Claims) -> Vec<ByteHit> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(m) = IBAN.find_at(text, from) {
        // Not found here: try again after the country code.
        from = m.start() + 2;
        let Some(len) = iban_length(&m.as_str()[..2]) else {
            continue;
        };
        // The byte where the `len`-th character ends: separators are not counted.
        let mut taken = 0;
        let mut end = None;
        for (i, c) in m.as_str().char_indices() {
            if !matches!(c, ' ' | '\n') {
                taken += 1;
                if taken == len {
                    end = Some(i + 1);
                    break;
                }
            }
        }
        let Some(end) = end else { continue };
        let raw = &m.as_str()[..end];
        if m.as_str()[end..]
            .chars()
            .next()
            .is_some_and(|c| !matches!(c, ' ' | '\n'))
        {
            continue;
        }
        let compact: String = raw.chars().filter(|c| !matches!(c, ' ' | '\n')).collect();
        let (start, end) = (m.start(), m.start() + end);
        if mod97(&compact) == 1 && !claims.overlaps(start, end) {
            out.push(ByteHit::new(start, end, IdentifierKind::Iban, compact));
            from = end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rules;
    use crate::test_util::keys;

    /// US-04.4 AC1: spaced or compact, stored compact after the length and mod-97 checks.
    #[test]
    fn us_04_4_ac1_valid_iban_is_stored_compact() {
        assert_eq!(
            keys("IBAN: DE89 3704 0044 0532 0130 00.", &Rules::default()),
            vec![(
                "iban:DE89370400440532013000".into(),
                "DE89 3704 0044 0532 0130 00".into()
            )]
        );
        for text in [
            "GB86QMTB60991248213377",
            "MT55 QSAL 1234 5000 0000 0001 2345 678",
            "NL42 QKES 0417 3398 21 EUR",
        ] {
            assert_eq!(keys(text, &Rules::default()).len(), 1, "{text}");
        }
    }

    /// US-04.4 AC2: a failed checksum, a wrong length or an unknown country is not stored.
    #[test]
    fn us_04_4_ac2_invalid_iban_is_not_stored() {
        for text in [
            "DE89 3704 0044 0532 0130 01",
            "GB86 QMTB 6099 1248 2133 70",
            "DE89 3704 0044 0532 0130",
            "ZZ89 3704 0044 0532 0130 00",
            "GB86 QMTB 6099 1248 2133 77X",
        ] {
            assert_eq!(keys(text, &Rules::default()), vec![], "{text}");
        }
    }

    /// A following group of four is not taken into a complete IBAN.
    #[test]
    fn group_after_the_iban_is_left_out() {
        let k = keys(
            "NL42 QKES 0417 3398 21 and DE89 3704 0044 0532 0130 00 2024",
            &Rules::default(),
        );
        assert_eq!(k.len(), 2);
        assert_eq!(k[1].1, "DE89 3704 0044 0532 0130 00");
    }

    /// OCR joins or splits groups and a PDF line can break inside an IBAN; the length and
    /// mod-97 still decide. An IBAN right after another is found too.
    #[test]
    fn regrouped_and_wrapped_iban() {
        for text in [
            "NL42 QKES 0417 339821",
            "M55 x MT55 QSAL 1234 5000 0000 0001 2345678",
            "IBAN\nGB86 QMTB 6099\n1248 2133 77, quoting",
        ] {
            assert_eq!(keys(text, &Rules::default()).len(), 1, "{text:?}");
        }
        let k = keys(
            "NL42 QKES 0417 3398 21 DE89 3704 0044 0532 0130 00",
            &Rules::default(),
        );
        assert_eq!(k.len(), 2, "{k:?}");
        assert_eq!(keys("GB86 QMTB 6099\n1248 2133 70", &Rules::default()), []);
    }

    #[test]
    fn country_lengths_match_mk_bench() {
        for (c, n) in [("GB", 22), ("NL", 18), ("MT", 31), ("DE", 22), ("LU", 20)] {
            assert_eq!(iban_length(c), Some(n));
        }
    }
}
