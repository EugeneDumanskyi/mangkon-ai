//! BC-04 Identifiers: validators and extractors (domain §3). Pure functions, no I/O
//! (ADR-0002 rule 1).
//!
//! [`find_all`] runs the built-in finders over one segment's text (US-04.1 – US-04.6), in this
//! order: email (with its domain), URL (with its domain), IBAN, crypto, card-like, phone. A
//! finder never takes characters an earlier one took, so an IBAN is not also read as a phone and
//! a card number never becomes one (INV-ID-06). Domains come only from email and URL hosts
//! (INV-ID-05); a bare domain in text is not read (ASSUMPTION M4b). [`luhn`] (M4a) backs the
//! card-like check.
//!
//! [`find_custom`] (M4c) runs a project's custom patterns (US-04.7), apart from the built-in
//! finders, so either step can run again alone (INV-ID-08).

mod card;
mod crypto;
mod custom;
mod domain;
mod email;
mod iban;
pub mod luhn;
mod phone;
mod url;

use mk_kernel::{IdentifierKey, IdentifierKind, Span};

pub use card::CardLikeHit;
pub use custom::{CustomPattern, NEST_LIMIT, PatternError, SIZE_LIMIT, find_custom};
pub use domain::registrable_domain;
pub use email::{find_emails, normalize_email};
pub use iban::iban_length;
pub use phone::Region;

/// The version of the built-in finders' rules. A project whose mentions were made with an
/// older version gets its identifiers found again from its stored segments when it opens (M4b).
/// Bump on any change to what the finders find or how they normalize.
pub const RULES_VERSION: u32 = 1;

/// One identifier found in a segment's stored text (INV-ID-03).
#[derive(Clone, PartialEq, Debug)]
pub struct IdentifierHit {
    /// Characters of the text the hit was found in (INV-CO-04).
    pub span: Span,
    /// The text exactly as written.
    pub raw: String,
    pub key: IdentifierKey,
    /// 0–1. Text-native hits are 1.0, a Solana address without a nearby cue word 0.7
    /// (US-04.5 AC3); OCR hits also take their word confidence (M3).
    pub confidence: f32,
}

/// The project settings the finders use.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// The region of phone numbers written without a country code (US-04.2 AC2); `None` keeps
    /// them as unnormalized digits (AC3).
    pub phone_region: Option<Region>,
    /// The project's custom patterns (US-04.7), compiled. [`find_all`] does not run them:
    /// [`find_custom`] does.
    pub custom: Vec<CustomPattern>,
}

/// Everything [`find_all`] found in one text.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Found {
    /// Identifier hits in text order. A derived domain follows the email or URL it comes from,
    /// at the same span.
    pub hits: Vec<IdentifierHit>,
    /// Card-like numbers: flags only, never identifiers (INV-ID-06).
    pub cards: Vec<CardLikeHit>,
}

/// A hit with byte offsets, before the offsets are turned into characters.
#[derive(Clone, Debug)]
pub(crate) struct ByteHit {
    pub start: usize,
    pub end: usize,
    pub key: IdentifierKey,
    pub confidence: f32,
}

impl ByteHit {
    pub fn new(start: usize, end: usize, kind: IdentifierKind, value: impl Into<String>) -> Self {
        ByteHit {
            start,
            end,
            key: IdentifierKey::new(kind, value),
            confidence: 1.0,
        }
    }
}

/// Byte ranges already taken by an earlier finder.
#[derive(Default)]
pub(crate) struct Claims(Vec<(usize, usize)>);

impl Claims {
    pub fn overlaps(&self, start: usize, end: usize) -> bool {
        self.0.iter().any(|&(s, e)| start < e && s < end)
    }

    fn take(&mut self, hits: &[ByteHit]) {
        self.0.extend(hits.iter().map(|h| (h.start, h.end)));
    }
}

/// A finder of one or more types, given the spans already taken.
type Finder = dyn Fn(&str, &Claims) -> Vec<ByteHit>;

/// Every built-in identifier and card-like number in `text` (US-04.1 – US-04.6).
pub fn find_all(text: &str, rules: &Rules) -> Found {
    let mut claims = Claims::default();
    let mut hits: Vec<ByteHit> = Vec::new();
    let finders: [&Finder; 4] = [
        &|t, _| email::find(t),
        &url::find,
        &iban::find,
        &crypto::find,
    ];
    for find in finders {
        let found = find(text, &claims);
        claims.take(&found);
        hits.extend(found);
    }
    let cards = card::find(text, &claims);
    claims.0.extend(cards.iter().map(|c| (c.start, c.end)));
    hits.extend(phone::find(text, &claims, rules.phone_region));
    // Text order; a derived domain stays after its email or URL (stable sort).
    hits.sort_by_key(|h| h.start);

    let mut chars = CharOffsets::new(text);
    let len = chars.len;
    let mut bounds: Vec<usize> = hits
        .iter()
        .flat_map(|h| [h.start, h.end])
        .chain(cards.iter().flat_map(|c| [c.start, c.end]))
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let at: std::collections::HashMap<usize, usize> =
        bounds.iter().map(|&b| (b, chars.at(b))).collect();
    let span = |s: usize, e: usize| Span::new(at[&s], at[&e], len).expect("a hit is non-empty");
    Found {
        hits: hits
            .into_iter()
            .map(|h| IdentifierHit {
                span: span(h.start, h.end),
                raw: text[h.start..h.end].to_string(),
                key: h.key,
                confidence: h.confidence,
            })
            .collect(),
        cards: cards
            .into_iter()
            .map(|c| CardLikeHit {
                span: span(c.start, c.end),
                last4: c.last4,
            })
            .collect(),
    }
}

/// Byte offset → character offset, for offsets asked in ascending order.
struct CharOffsets<'a> {
    text: &'a str,
    byte: usize,
    chars: usize,
    len: usize,
}

impl<'a> CharOffsets<'a> {
    fn new(text: &'a str) -> Self {
        CharOffsets {
            text,
            byte: 0,
            chars: 0,
            len: text.chars().count(),
        }
    }

    fn at(&mut self, byte: usize) -> usize {
        if byte < self.byte {
            (self.byte, self.chars) = (0, 0);
        }
        self.chars += self.text[self.byte..byte].chars().count();
        self.byte = byte;
        self.chars
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    /// `(type:value, raw)` of every hit, in order.
    pub fn keys(text: &str, rules: &Rules) -> Vec<(String, String)> {
        find_all(text, rules)
            .hits
            .into_iter()
            .map(|h| (h.key.to_string(), h.raw))
            .collect()
    }

    pub fn gb() -> Rules {
        Rules {
            phone_region: Region::parse("GB"),
            ..Rules::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use proptest::prelude::*;

    /// No span is claimed twice: an IBAN is not also a phone, a card is not a phone.
    #[test]
    fn finders_never_share_a_span() {
        let text = "Pay GB86 QMTB 6099 1248 2133 77, card 4111 1111 1111 1111, tel 020 7946 0318";
        let found = find_all(text, &gb());
        assert_eq!(
            found
                .hits
                .iter()
                .map(|h| h.key.to_string())
                .collect::<Vec<_>>(),
            vec!["iban:GB86QMTB60991248213377", "phone:+442079460318"]
        );
        assert_eq!(found.cards.len(), 1);
        assert_eq!(found.cards[0].last4, "1111");
    }

    /// INV-ID-05: the email and its domain share one span.
    #[test]
    fn derived_domain_follows_its_source() {
        let found = find_all("x a@b.shell-co.example y", &Rules::default());
        assert_eq!(found.hits.len(), 2);
        assert_eq!(found.hits[0].key.kind(), IdentifierKind::Email);
        assert_eq!(found.hits[1].key.to_string(), "domain:shell-co.example");
        assert_eq!(found.hits[0].span, found.hits[1].span);
        assert_eq!(found.hits[0].raw, found.hits[1].raw);
    }

    /// INV-CO-04: offsets count characters, not bytes.
    #[test]
    fn inv_co_04_offsets_are_characters() {
        let text = "Größe — €5: +44 20 7946 0318, 0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed ✓ \
                    4111111111111111";
        let found = find_all(text, &Rules::default());
        assert_eq!(found.hits.len(), 2);
        for h in &found.hits {
            let got: String = text
                .chars()
                .skip(h.span.start())
                .take(h.span.len())
                .collect();
            assert_eq!(got, h.raw);
        }
        let c = &found.cards[0];
        let got: String = text
            .chars()
            .skip(c.span.start())
            .take(c.span.len())
            .collect();
        assert_eq!(got, "4111111111111111");
    }

    fn part() -> impl Strategy<Value = String> {
        prop_oneof![
            "[ a-zé€,;:<>()\n/.#-]{0,8}",
            "[A-Za-z0-9._%+-]{1,10}@[a-z0-9-]{1,8}\\.[A-Za-z]{2,6}",
            "(https?://|www\\.)[a-z0-9-]{1,8}\\.[a-z]{2,6}(/[A-Za-z0-9._-]{0,8})?",
            "\\+?[0-9]{1,4}([ .–-]?\\(?[0-9]{1,4}\\)?){1,4}",
            "[A-Z]{2}[0-9]{2}( ?[A-Z0-9]{4}){2,6}",
            "(0x[0-9a-fA-F]{40}|[13T][1-9A-HJ-NP-Za-km-z]{25,40}|bc1[02-9ac-hj-np-z]{20,40})",
            Just("GB86 QMTB 6099 1248 2133 77".to_string()),
            Just("+44 20 7946 0318".to_string()),
            Just("4111 1111 1111 1111".to_string()),
            Just("TH2fjNgATy9Hi1e4NtJHzukziwPxX2Vb9r".to_string()),
            Just("H34gx4D9ADcYkon9B9uTUpsxPUkKmXTL6Y82nyGg8srm".to_string()),
        ]
    }

    proptest! {
        /// INV-CO-04, INV-ID-03: every span of every finder slices back to its raw text, spans
        /// of different identifiers never overlap, and cards never become keys.
        #[test]
        fn inv_co_04_span_slices_back_to_raw(
            parts in proptest::collection::vec(part(), 0..8),
            region in proptest::option::of(proptest::sample::select(vec!["GB", "US", "DE"])),
        ) {
            let text: String = parts.concat();
            let rules = Rules { phone_region: region.and_then(Region::parse), ..Rules::default() };
            let found = find_all(&text, &rules);
            let slice = |s: Span| -> String { text.chars().skip(s.start()).take(s.len()).collect() };
            let mut last_end = 0;
            let mut last_span = None;
            for h in &found.hits {
                prop_assert_eq!(&slice(h.span), &h.raw);
                if h.key.kind() == IdentifierKind::Domain && last_span == Some(h.span) {
                    continue;
                }
                prop_assert!(h.span.start() >= last_end, "overlap in {:?}", text);
                last_end = h.span.end();
                last_span = Some(h.span);
                prop_assert!((0.0..=1.0).contains(&h.confidence));
            }
            for c in &found.cards {
                let raw = slice(c.span);
                let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
                prop_assert!(digits.ends_with(&c.last4));
                for h in &found.hits {
                    prop_assert!(!h.key.value().contains(&digits));
                }
            }
        }
    }
}
