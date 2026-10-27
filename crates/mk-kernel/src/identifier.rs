use std::fmt;
use std::str::FromStr;

/// Identifier type. M2b has email only; the other types come in M4 (US-04.2 – US-04.7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IdentifierKind {
    Email,
}

impl IdentifierKind {
    pub const ALL: [IdentifierKind; 1] = [IdentifierKind::Email];

    /// Stable name, as stored and as in the ground truth (`docs/07-test-corpus.md` §5).
    pub fn as_str(&self) -> &'static str {
        match self {
            IdentifierKind::Email => "email",
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeyError(pub String);

impl fmt::Display for ParseKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not an identifier key: {:?}", self.0)
    }
}

impl std::error::Error for ParseKeyError {}

impl FromStr for IdentifierKey {
    type Err = ParseKeyError;

    /// Accepts the `type:value` form that `Display` produces, with a non-empty value.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseKeyError(s.to_string());
        let (kind, value) = s.split_once(':').ok_or_else(err)?;
        let kind = IdentifierKind::ALL
            .into_iter()
            .find(|k| k.as_str() == kind)
            .ok_or_else(err)?;
        if value.is_empty() {
            return Err(err());
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
        assert!("iban:GB33".parse::<IdentifierKey>().is_err());
        assert!("no-colon".parse::<IdentifierKey>().is_err());
    }

    proptest! {
        #[test]
        fn display_parse_round_trip(value in "[a-z0-9.@:-]{1,40}") {
            let k = IdentifierKey::new(IdentifierKind::Email, value);
            prop_assert_eq!(k.to_string().parse::<IdentifierKey>().unwrap(), k);
        }
    }
}
