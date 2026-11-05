//! The extraction prompt (version 1, settled in M1, `09-model-check.md` §3.2 and §11; ported from
//! `spikes/model-check/src/extract.rs`), its JSON schema and the strict parse of a reply.

use mk_kernel::EntityType;
use serde::Deserialize;
use serde_json::{Value, json};

/// Bumped whenever the prompt or schema changes; every run records the one it used.
pub const PROMPT_VERSION: u32 = 1;

pub const SYSTEM_PROMPT: &str = "You find the names of people and organizations in a text.

Return every place where the text names a person or an organization:
- people: full names, an initial with a surname, a title with a surname (Mr, Ms, Dr), a first name or a surname alone;
- organizations: companies, public bodies, banks, newspapers, unions, registries, exchanges, including short forms and acronyms.

For each name give:
- \"name\": the name exactly as written in the text, character for character. Keep titles, initials, legal suffixes (Ltd, LLC, S.A.) and capital letters as printed. Do not expand, correct or translate it.
- \"type\": \"person\" or \"org\".
- \"evidence\": a short exact quote from the text (at most 12 words) that contains the name, copied character for character.
- \"confidence\": from 0 to 1, how sure you are that this is the name of a person or an organization.

List a name once for each different way it is written. Do not list job titles alone, places, products, email addresses, web addresses, domain names, account numbers, reference numbers (invoice, contract, tender) or other identifiers. If the text names no person and no organization, return an empty list.

Reply with compact JSON on one line, without indentation.";

/// The schema's name in `response_format`.
pub const SCHEMA_NAME: &str = "mentions";
/// M1's reply limit (`09-model-check.md` §3.2).
pub const MAX_TOKENS: u32 = 2048;
/// M1's seed, so a re-run of the same model on the same text answers the same (temperature 0).
pub const SEED: u32 = 1;

/// The reply schema (US-10.1 AC2).
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "mentions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "type": {"type": "string", "enum": ["person", "org"]},
                        "evidence": {"type": "string"},
                        "confidence": {"type": "number"}
                    },
                    "required": ["name", "type", "evidence", "confidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["mentions"],
        "additionalProperties": false
    })
}

pub fn user_message(text: &str) -> String {
    format!("Text:\n<<<\n{text}\n>>>")
}

/// What the second call says after a reply that does not parse (INV-AI-03).
pub fn retry_message(error: &str) -> String {
    format!("That reply did not match the JSON schema ({error}). Reply again with JSON only.")
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Person,
    Org,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMention {
    name: String,
    #[serde(rename = "type")]
    kind: Kind,
    evidence: String,
    confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReply {
    mentions: Vec<RawMention>,
}

/// One mention as the model gave it, not checked against the text yet.
#[derive(Clone, PartialEq, Debug)]
pub struct Prediction {
    pub name: String,
    pub entity_type: EntityType,
    pub evidence: String,
    pub confidence: f32,
}

/// Strict parse (INV-AI-03): the schema's shape, no extra fields, confidence in 0..=1, no empty
/// name or evidence. The error text goes into the retry.
pub fn parse_reply(content: &str) -> Result<Vec<Prediction>, String> {
    let reply: RawReply = serde_json::from_str(content.trim()).map_err(|e| e.to_string())?;
    reply
        .mentions
        .into_iter()
        .map(|m| {
            if !(0.0..=1.0).contains(&m.confidence) {
                return Err(format!("confidence {} out of range", m.confidence));
            }
            if m.name.trim().is_empty() || m.evidence.trim().is_empty() {
                return Err("empty name or evidence".to_string());
            }
            Ok(Prediction {
                name: m.name,
                entity_type: match m.kind {
                    Kind::Person => EntityType::Person,
                    Kind::Org => EntityType::Org,
                },
                evidence: m.evidence,
                confidence: m.confidence as f32,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-10.1 AC2, INV-AI-03: the schema's shape and nothing else parses.
    #[test]
    fn us_10_1_ac2_strict_parse() {
        let ok = r#"{"mentions":[{"name":"Corvin Hale","type":"person","evidence":"Mr Corvin Hale","confidence":0.9}]}"#;
        let p = parse_reply(ok).unwrap();
        assert_eq!(p[0].entity_type, EntityType::Person);
        assert_eq!(p[0].name, "Corvin Hale");
        assert_eq!(parse_reply(r#"{"mentions":[]}"#).unwrap(), vec![]);
        assert_eq!(parse_reply("  {\"mentions\":[]}\n").unwrap(), vec![]);
        for bad in [
            "",
            "not json",
            r#"{"mentions":[{"name":"A","type":"place","evidence":"A","confidence":0.5}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A"}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":1.5}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":-0.1}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":0.5,"x":1}]}"#,
            r#"{"mentions":[{"name":" ","type":"org","evidence":"A","confidence":0.5}]}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":" ","confidence":0.5}]}"#,
            r#"{"mentions":[],"extra":true}"#,
            r#"{"mentions":[{"name":"A","type":"org","evidence":"A","confidence":0.5}"#,
        ] {
            assert!(parse_reply(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn schema_matches_parser() {
        let s = schema();
        let item = &s["properties"]["mentions"]["items"];
        assert_eq!(item["required"].as_array().unwrap().len(), 4);
        assert_eq!(item["properties"]["type"]["enum"], json!(["person", "org"]));
        assert_eq!(item["additionalProperties"], json!(false));
    }

    #[test]
    fn messages_carry_the_text_and_the_error() {
        assert_eq!(user_message("a b"), "Text:\n<<<\na b\n>>>");
        assert!(retry_message("missing field").contains("missing field"));
        assert_eq!(PROMPT_VERSION, 1);
    }
}
