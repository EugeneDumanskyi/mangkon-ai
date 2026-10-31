use std::fmt;
use std::str::FromStr;

/// Entity type (domain §3.3). M4c makes entities only from confirmed table columns (US-05.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum EntityType {
    Person,
    Org,
}

impl EntityType {
    pub const ALL: [EntityType; 2] = [EntityType::Person, EntityType::Org];

    /// Stable name, as stored, as a name key's prefix and as in the ground truth.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityType::Person => "person",
            EntityType::Org => "org",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Legal suffixes dropped from organization names (US-11.1 AC1: "Ltd", "LLC", "GmbH" …).
/// ASSUMPTION: the M0a list of the ground truth (`mk-bench` `keys.rs`), kept in step by a test.
pub const LEGAL_SUFFIXES: &[&str] = &[
    "ltd",
    "limited",
    "llc",
    "llp",
    "inc",
    "incorporated",
    "gmbh",
    "sa",
    "plc",
    "corp",
    "co",
];

/// NameKey: entity type + normalized name, the identity entity mentions are grouped by
/// (domain §3.3, INV-ER-02) and the anchor of decisions on an entity (domain §9.5). Displays as
/// `person:<name>` or `org:<name>`, its stored form.
///
/// Normalized (US-11.1): lower case, every character that is not a letter or digit becomes a
/// space, spaces collapsed; for organizations, trailing legal suffixes are removed ("S.A." as
/// well as "SA"), but never the last word.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct NameKey {
    entity_type: EntityType,
    name: String,
}

impl NameKey {
    /// The key of `raw` as an entity of `entity_type`; `None` if nothing is left of it.
    pub fn new(entity_type: EntityType, raw: &str) -> Option<Self> {
        let name = normalize(entity_type, raw);
        (!name.is_empty()).then_some(Self { entity_type, name })
    }

    pub fn entity_type(&self) -> EntityType {
        self.entity_type
    }

    /// The normalized name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether `raw` ends in a legal suffix, so reads as an organization ("Kestrel Advisory LLP",
    /// "Saltmarsh Holdings S.A.").
    pub fn has_legal_suffix(raw: &str) -> bool {
        let words = words(raw);
        let n = words.len();
        n >= 2
            && (LEGAL_SUFFIXES.contains(&words[n - 1].as_str())
                || (words[n - 1].chars().count() == 1
                    && words[n - 2].chars().count() == 1
                    && n >= 3
                    && LEGAL_SUFFIXES
                        .contains(&format!("{}{}", words[n - 2], words[n - 1]).as_str())))
    }
}

fn words(raw: &str) -> Vec<String> {
    let mut cleaned = String::with_capacity(raw.len());
    for c in raw.chars().flat_map(char::to_lowercase) {
        cleaned.push(if c.is_alphanumeric() { c } else { ' ' });
    }
    cleaned.split_whitespace().map(str::to_string).collect()
}

fn normalize(entity_type: EntityType, raw: &str) -> String {
    let mut words = words(raw);
    if entity_type == EntityType::Org {
        // "S.A." splits into "s" "a"; join single letters at the end first.
        while words.len() >= 2
            && words[words.len() - 1].chars().count() == 1
            && words[words.len() - 2].chars().count() == 1
        {
            let joined = format!("{}{}", words[words.len() - 2], words[words.len() - 1]);
            if LEGAL_SUFFIXES.contains(&joined.as_str()) {
                words.truncate(words.len() - 2);
            } else {
                break;
            }
        }
        while words.len() > 1 && LEGAL_SUFFIXES.contains(&words[words.len() - 1].as_str()) {
            words.pop();
        }
    }
    words.join(" ")
}

impl fmt::Display for NameKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.entity_type, self.name)
    }
}

/// A string that is not a `person:` / `org:` name key in normalized form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseNameKeyError;

impl fmt::Display for ParseNameKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a name key")
    }
}

impl std::error::Error for ParseNameKeyError {}

impl FromStr for NameKey {
    type Err = ParseNameKeyError;

    /// Accepts what `Display` produces: a type, `:`, and a name already normalized.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (t, name) = s.split_once(':').ok_or(ParseNameKeyError)?;
        let entity_type = EntityType::from_name(t).ok_or(ParseNameKeyError)?;
        match NameKey::new(entity_type, name) {
            Some(k) if k.name == name => Ok(k),
            _ => Err(ParseNameKeyError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: EntityType, raw: &str) -> String {
        NameKey::new(t, raw).unwrap().to_string()
    }

    /// US-11.1 AC1: case, punctuation and legal suffixes do not change an organization's key.
    #[test]
    fn org_suffixes_and_case() {
        assert_eq!(
            key(EntityType::Org, "BRIGHTWATER LOGISTICS LTD"),
            key(EntityType::Org, "Brightwater Logistics Limited")
        );
        assert_eq!(
            key(EntityType::Org, "Saltmarsh Holdings S.A."),
            "org:saltmarsh holdings"
        );
        // The last word stays, even when it is a suffix.
        assert_eq!(key(EntityType::Org, "Co"), "org:co");
        // Suffixes are not dropped from people.
        assert_eq!(key(EntityType::Person, "Ann Co"), "person:ann co");
    }

    #[test]
    fn person_punctuation_and_parse() {
        let k = NameKey::new(EntityType::Person, "Daniel  Okonkwo-Hart").unwrap();
        assert_eq!(k.to_string(), "person:daniel okonkwo hart");
        assert_eq!(k.to_string().parse::<NameKey>(), Ok(k));
        assert_ne!(
            key(EntityType::Person, "Gregor Almqvist"),
            key(EntityType::Person, "Gregory Almquist")
        );
        assert!(NameKey::new(EntityType::Person, " — ").is_none());
        assert!("person:Daniel".parse::<NameKey>().is_err());
        assert!("org:acme ltd".parse::<NameKey>().is_err());
        assert!("email:a@b.example".parse::<NameKey>().is_err());
        assert!("org:".parse::<NameKey>().is_err());
    }

    #[test]
    fn legal_suffix_check() {
        for org in [
            "Kestrel Advisory LLP",
            "SALTMARSH HOLDINGS S.A.",
            "Acme GmbH",
        ] {
            assert!(NameKey::has_legal_suffix(org), "{org}");
        }
        for person in ["Corvin Hale", "C. Hale", "LLP", "VASQUEZ-LINDQVIST, Irene"] {
            assert!(!NameKey::has_legal_suffix(person), "{person}");
        }
    }
}
