use std::fmt;
use std::str::FromStr;

/// Identifier type (US-04.1 – US-04.5). Custom patterns (US-04.7) come in M4c.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IdentifierKind {
    Email,
    /// E.164 (US-04.2 AC1/AC2).
    Phone,
    /// The digits of a national number read without a phone region (US-04.2 AC3). Its own key
    /// space: it never matches a `phone` key (INV-ID-04).
    PhoneUnnormalized,
    Url,
    /// A registrable domain (public suffix list), from an email or URL host (INV-ID-05).
    Domain,
    Iban,
    Btc,
    Evm,
    Sol,
    Tron,
}

impl IdentifierKind {
    pub const ALL: [IdentifierKind; 10] = [
        IdentifierKind::Email,
        IdentifierKind::Phone,
        IdentifierKind::PhoneUnnormalized,
        IdentifierKind::Url,
        IdentifierKind::Domain,
        IdentifierKind::Iban,
        IdentifierKind::Btc,
        IdentifierKind::Evm,
        IdentifierKind::Sol,
        IdentifierKind::Tron,
    ];

    /// Stable name, as stored and as in the ground truth (`docs/07-test-corpus.md` §5).
    pub fn as_str(&self) -> &'static str {
        match self {
            IdentifierKind::Email => "email",
            IdentifierKind::Phone => "phone",
            IdentifierKind::PhoneUnnormalized => "phone_unnormalized",
            IdentifierKind::Url => "url",
            IdentifierKind::Domain => "domain",
            IdentifierKind::Iban => "iban",
            IdentifierKind::Btc => "btc",
            IdentifierKind::Evm => "evm",
            IdentifierKind::Sol => "sol",
            IdentifierKind::Tron => "tron",
        }
    }

    pub fn from_name(name: &str) -> Option<IdentifierKind> {
        IdentifierKind::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

impl fmt::Display for IdentifierKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Identifier type + normalized value: the identity of an identifier (domain §3.3, INV-ID-01).
///
/// The value is normalized by the extractor that found it (`mk-identifiers`); this type only
/// holds it. Displays as `type:value` (for example `email:john.smith@example.com`), which is
/// also its stored form and the anchor of decisions on it (domain §9.5).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IdentifierKey {
    kind: IdentifierKind,
    value: String,
}

impl IdentifierKey {
    pub fn new(kind: IdentifierKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }

    pub fn kind(&self) -> IdentifierKind {
        self.kind
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for IdentifierKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.value)
    }
}

/// A string that is not a `type:value` key. It does not carry the string: a value may be a
/// card-like number or other private text that must not reach a message (NFR-SEC-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeyError;

impl fmt::Display for ParseKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an identifier key")
    }
}

impl std::error::Error for ParseKeyError {}

impl FromStr for IdentifierKey {
    type Err = ParseKeyError;

    /// Accepts the `type:value` form that `Display` produces, with a non-empty value.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (kind, value) = s.split_once(':').ok_or(ParseKeyError)?;
        let kind = IdentifierKind::from_name(kind).ok_or(ParseKeyError)?;
        if value.is_empty() {
            return Err(ParseKeyError);
        }
        Ok(Self::new(kind, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// INV-ID-01: the key is type + normalized value, stored as `type:value`.
    #[test]
    fn inv_id_01_key_display_and_parse() {
        let k = IdentifierKey::new(IdentifierKind::Email, "a.b@example.com");
        assert_eq!(k.to_string(), "email:a.b@example.com");
        assert_eq!("email:a.b@example.com".parse::<IdentifierKey>(), Ok(k));
        assert!("email:".parse::<IdentifierKey>().is_err());
        assert!("bank:GB33".parse::<IdentifierKey>().is_err());
        let iban = "iban:GB33QNMS40112255443322"
            .parse::<IdentifierKey>()
            .unwrap();
        assert_eq!(iban.kind(), IdentifierKind::Iban);
        // INV-ID-04: an unnormalized phone is its own type.
        let p = "phone_unnormalized:02079460318"
            .parse::<IdentifierKey>()
            .unwrap();
        assert_eq!(p.kind(), IdentifierKind::PhoneUnnormalized);
        assert!("no-colon".parse::<IdentifierKey>().is_err());
    }

    /// NFR-SEC-14: the error never repeats the text it was given.
    #[test]
    fn parse_error_does_not_echo_the_value() {
        let e = "card:4111111111111111"
            .parse::<IdentifierKey>()
            .unwrap_err();
        assert!(!e.to_string().contains("4111"));
        assert!(!format!("{e:?}").contains("4111"));
    }

    proptest! {
        #[test]
        fn display_parse_round_trip(
            kind in proptest::sample::select(IdentifierKind::ALL.to_vec()),
            value in "[a-z0-9.@:-]{1,40}",
        ) {
            let k = IdentifierKey::new(kind, value);
            prop_assert_eq!(k.to_string().parse::<IdentifierKey>().unwrap(), k);
        }
    }
}
