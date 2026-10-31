use std::fmt;
use std::str::FromStr;

/// Identifier type (US-04.1 – US-04.5, US-04.7).
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
    /// A match of a project's custom pattern (US-04.7, INV-ID-07). The key's value is
    /// `<pattern>:<match>`, so the key reads `custom:<pattern>:<match>` and each pattern is its
    /// own type ([`IdentifierKey::type_name`]).
    Custom,
}

impl IdentifierKind {
    /// The built-in types: what `mk-identifiers` finds without a project's patterns.
    pub const BUILTIN: [IdentifierKind; 10] = [
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

    pub const ALL: [IdentifierKind; 11] = [
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
        IdentifierKind::Custom,
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
            IdentifierKind::Custom => "custom",
        }
    }

    pub fn from_name(name: &str) -> Option<IdentifierKind> {
        IdentifierKind::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

/// Longest custom pattern name, in characters (INV-ID-07). ASSUMPTION (M4c).
pub const MAX_PATTERN_NAME: usize = 40;

/// Whether `name` can name a custom pattern: 1–[`MAX_PATTERN_NAME`] characters, no `:` (it ends
/// the name in a key) and no control characters, not starting or ending with a space.
pub fn valid_pattern_name(name: &str) -> bool {
    let n = name.chars().count();
    (1..=MAX_PATTERN_NAME).contains(&n)
        && name.trim() == name
        && !name.chars().any(|c| c == ':' || c.is_control())
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

    /// The key of a match of custom pattern `name` (US-04.7); `None` if `name` is not a valid
    /// pattern name or `value` is empty.
    pub fn custom(name: &str, value: &str) -> Option<Self> {
        (valid_pattern_name(name) && !value.is_empty())
            .then(|| Self::new(IdentifierKind::Custom, format!("{name}:{value}")))
    }

    /// The pattern of a custom key; `None` for a built-in one.
    pub fn pattern(&self) -> Option<&str> {
        match self.kind {
            IdentifierKind::Custom => self.value.split_once(':').map(|(p, _)| p),
            _ => None,
        }
    }

    /// The value as people read it: a custom key's match without its pattern name.
    pub fn display_value(&self) -> &str {
        match self.kind {
            IdentifierKind::Custom => self.value.split_once(':').map_or("", |(_, v)| v),
            _ => &self.value,
        }
    }

    /// The type as people and the lists name it: `custom:<pattern>` for a custom key, else the
    /// kind's name.
    pub fn type_name(&self) -> String {
        match self.pattern() {
            Some(p) => format!("custom:{p}"),
            None => self.kind.as_str().to_string(),
        }
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
        if kind == IdentifierKind::Custom {
            let (name, v) = value.split_once(':').ok_or(ParseKeyError)?;
            return Self::custom(name, v).ok_or(ParseKeyError);
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

    /// INV-ID-07: a custom match's key is `custom:<pattern>:<value>`; its type is named after
    /// the pattern. Names are 1–40 characters without `:` or control characters.
    #[test]
    fn inv_id_07_custom_key_form() {
        let k = IdentifierKey::custom("tender_ref", "VPA-TND-2024-017").unwrap();
        assert_eq!(k.to_string(), "custom:tender_ref:VPA-TND-2024-017");
        assert_eq!(k.pattern(), Some("tender_ref"));
        assert_eq!(k.display_value(), "VPA-TND-2024-017");
        assert_eq!(k.type_name(), "custom:tender_ref");
        assert_eq!(k.to_string().parse::<IdentifierKey>(), Ok(k));
        // A value may hold `:`; the name ends at the first one.
        let v = IdentifierKey::custom("case", "A:1").unwrap();
        assert_eq!(v.to_string().parse::<IdentifierKey>(), Ok(v.clone()));
        assert_eq!(v.display_value(), "A:1");
        for bad in ["", "a:b", " lead", "tab\there", &"x".repeat(41)] {
            assert!(IdentifierKey::custom(bad, "v").is_none(), "{bad:?}");
        }
        assert!(IdentifierKey::custom(&"é".repeat(40), "v").is_some());
        assert!(IdentifierKey::custom("n", "").is_none());
        assert!("custom:novalue".parse::<IdentifierKey>().is_err());
        assert!("custom::v".parse::<IdentifierKey>().is_err());
        let email = IdentifierKey::new(IdentifierKind::Email, "a@b.example");
        assert_eq!((email.pattern(), email.type_name()), (None, "email".into()));
        assert_eq!(email.display_value(), "a@b.example");
    }

    proptest! {
        #[test]
        fn custom_round_trip(name in "[A-Za-z_][A-Za-z0-9 _-]{0,38}[A-Za-z0-9_]", value in "[ -~]{1,30}") {
            let k = IdentifierKey::custom(&name, &value).unwrap();
            prop_assert_eq!(k.to_string().parse::<IdentifierKey>().unwrap(), k);
        }

        #[test]
        fn display_parse_round_trip(
            kind in proptest::sample::select(IdentifierKind::BUILTIN.to_vec()),
            value in "[a-z0-9.@:-]{1,40}",
        ) {
            let k = IdentifierKey::new(kind, value);
            prop_assert_eq!(k.to_string().parse::<IdentifierKey>().unwrap(), k);
        }
    }
}
