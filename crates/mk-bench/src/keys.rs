//! Key normalization as the ground truth records it.
//!
//! ASSUMPTION: this is the M0a reading of US-11.1 (NameKey) and US-04.x (IdentifierKey).
//! `mk-identifiers` owns the production rules from M4; if they differ, the ground truth is
//! regenerated, not the rules bent to fit.

use crate::model::EntityKind;

// `mk_kernel::NameKey` is the production form of these rules; a test there checks that the two
// agree on every TD-1 table name.

/// Legal suffixes dropped from organization names (US-11.1 AC1: "Ltd", "LLC", "GmbH" …).
const LEGAL_SUFFIXES: &[&str] = &[
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

/// NameKey: entity kind + normalized name. Lowercase, punctuation becomes space, spaces
/// collapsed, trailing legal suffixes removed (organizations only).
pub fn name_key(kind: EntityKind, raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for c in raw.chars().flat_map(char::to_lowercase) {
        cleaned.push(if c.is_alphanumeric() { c } else { ' ' });
    }
    let mut words: Vec<&str> = cleaned.split_whitespace().collect();
    let prefix = match kind {
        EntityKind::Person => "person",
        EntityKind::Org => {
            // "S.A." splits into "s" "a"; join single letters at the end first.
            while words.len() >= 2
                && words[words.len() - 1].len() == 1
                && words[words.len() - 2].len() == 1
            {
                let joined = format!("{}{}", words[words.len() - 2], words[words.len() - 1]);
                if LEGAL_SUFFIXES.contains(&joined.as_str()) {
                    words.truncate(words.len() - 2);
                } else {
                    break;
                }
            }
            while words.len() > 1 && LEGAL_SUFFIXES.contains(words.last().unwrap()) {
                words.pop();
            }
            "org"
        }
    };
    format!("{prefix}:{}", words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_suffixes_and_case() {
        assert_eq!(
            name_key(EntityKind::Org, "BRIGHTWATER LOGISTICS LTD"),
            name_key(EntityKind::Org, "Brightwater Logistics Limited")
        );
        assert_eq!(
            name_key(EntityKind::Org, "Saltmarsh Holdings S.A."),
            "org:saltmarsh holdings"
        );
    }

    #[test]
    fn person_punctuation() {
        assert_eq!(
            name_key(EntityKind::Person, "Daniel  Okonkwo-Hart"),
            "person:daniel okonkwo hart"
        );
        assert_ne!(
            name_key(EntityKind::Person, "Gregor Almqvist"),
            name_key(EntityKind::Person, "Gregory Almquist")
        );
    }
}
