//! Card-like numbers (US-04.6): flagged on their document, never identifiers (INV-ID-06).

use std::sync::LazyLock;

use mk_kernel::Span;
use regex::Regex;

use crate::{Claims, luhn};

/// 13–19 digits written compact, in groups of four (the last shorter), or as 4-6-5 (American
/// Express), with spaces or dashes between groups.
static CARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(?:[0-9]{13,19}",
        r"|[0-9]{4}(?:[ -][0-9]{4}){2,3}(?:[ -][0-9]{1,3})?",
        r"|[0-9]{4}[ -][0-9]{6}[ -][0-9]{4,5})\b",
    ))
    .expect("card pattern compiles")
});

/// A card-like number in a segment's text. Holds only its place and last four digits: the
/// number itself is never kept (NFR-SEC-14).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CardLikeHit {
    pub span: Span,
    pub last4: String,
}

impl CardLikeHit {
    /// The masked form the app shows: `**** **** **** 1234` (US-04.6 AC1).
    pub fn masked(&self) -> String {
        mask(&self.last4)
    }
}

/// `**** **** **** <last4>`.
pub fn mask(last4: &str) -> String {
    format!("**** **** **** {last4}")
}

pub(crate) struct ByteCard {
    pub start: usize,
    pub end: usize,
    pub last4: String,
}

/// Every 13–19 digit number passing Luhn that no earlier finder took (US-04.6 AC1).
pub(crate) fn find(text: &str, claims: &Claims) -> Vec<ByteCard> {
    CARD.find_iter(text)
        .filter(|m| !claims.overlaps(m.start(), m.end()))
        .filter_map(|m| {
            let digits: Vec<u8> = m.as_str().bytes().filter(u8::is_ascii_digit).collect();
            luhn::is_card_like(&digits).then(|| ByteCard {
                start: m.start(),
                end: m.end(),
                last4: String::from_utf8_lossy(&digits[digits.len() - 4..]).into_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{Rules, find_all};

    /// US-04.6 AC1: flagged with the last four digits and shown masked; AC2 / INV-ID-06: never
    /// an identifier.
    #[test]
    fn us_04_6_ac1_card_like_is_flagged_masked() {
        for (text, last4) in [
            ("card 4111 1111 1111 1111 ok", "1111"),
            ("5555555555554444", "4444"),
            ("3782-822463-10005", "0005"),
            ("4222 2222 2222 2", "2222"),
        ] {
            let found = find_all(text, &Rules::default());
            assert_eq!(found.cards.len(), 1, "{text}");
            assert_eq!(found.cards[0].last4, last4);
            assert_eq!(found.cards[0].masked(), format!("**** **** **** {last4}"));
            assert!(found.hits.is_empty(), "{text}: {:?}", found.hits);
        }
    }

    /// US-04.6 AC2: a card-like number is never a key, not even a phone.
    #[test]
    fn us_04_6_ac2_card_like_is_never_a_key() {
        let found = find_all("+4111111111111111", &Rules::default());
        assert!(found.hits.is_empty());
    }

    #[test]
    fn not_card_like() {
        for text in [
            "4111 1111 1111 1112",
            "411111111111",
            "41111111111111111111",
            "020 7946 0318 0207",
        ] {
            assert!(find_all(text, &Rules::default()).cards.is_empty(), "{text}");
        }
    }
}
