use std::fmt;
use std::str::FromStr;

use crate::ContentHash;

/// The start of an entity key made by a split decision (domain §9.5): `split:<hash>`.
pub const SPLIT_PREFIX: &str = "split:";

/// Whether `key` is the key of a split-out entity.
pub fn is_split_key(key: &str) -> bool {
    key.strip_prefix(SPLIT_PREFIX)
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// MentionKey (domain §3.3): the identity of one mention that survives a re-run, when the same text
/// is found at the same place: content hash, document position, locator (kind, position, reference
/// and span) and evidence text. Decisions keep its text form (INV-CU-02); it is never a column of a
/// mention, so a project without splits pays nothing for it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MentionKey {
    pub content: ContentHash,
    /// The document's position in its file (`document.position`).
    pub document: String,
    pub loc_kind: String,
    pub loc_pos: i64,
    pub loc_ref: Option<String>,
    pub span: (u32, u32),
    /// The mention's text as written.
    pub evidence: String,
}

impl MentionKey {
    /// The key of the split group these mentions move into: `split:` and a BLAKE3 hash of their
    /// sorted keys, so the same mentions always give the same anchor. `None` with no mentions.
    pub fn split_key<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
        let mut sorted: Vec<&str> = keys.into_iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.is_empty() {
            return None;
        }
        Some(format!(
            "{SPLIT_PREFIX}{}",
            ContentHash::of(sorted.join("\n").as_bytes())
        ))
    }
}

fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            _ => return None,
        }
    }
    Some(out)
}

/// One line of text: tab-separated fields, with `\`, tab and newline escaped, and an empty field
/// for no locator reference.
impl fmt::Display for MentionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        out.push_str(&self.content.to_hex());
        out.push('\t');
        push_escaped(&mut out, &self.document);
        out.push('\t');
        push_escaped(&mut out, &self.loc_kind);
        out.push('\t');
        out.push_str(&self.loc_pos.to_string());
        out.push('\t');
        if let Some(r) = &self.loc_ref {
            push_escaped(&mut out, r);
        }
        out.push('\t');
        out.push_str(&format!("{}\t{}\t", self.span.0, self.span.1));
        push_escaped(&mut out, &self.evidence);
        f.write_str(&out)
    }
}

/// Text that is not a [`MentionKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseMentionKeyError;

impl fmt::Display for ParseMentionKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a mention key")
    }
}

impl std::error::Error for ParseMentionKeyError {}

impl FromStr for MentionKey {
    type Err = ParseMentionKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let fields: Vec<&str> = s.split('\t').collect();
        let [
            content,
            document,
            loc_kind,
            loc_pos,
            loc_ref,
            start,
            end,
            evidence,
        ] = fields[..]
        else {
            return Err(ParseMentionKeyError);
        };
        Ok(MentionKey {
            content: content.parse().map_err(|_| ParseMentionKeyError)?,
            document: unescape(document).ok_or(ParseMentionKeyError)?,
            loc_kind: unescape(loc_kind).ok_or(ParseMentionKeyError)?,
            loc_pos: loc_pos.parse().map_err(|_| ParseMentionKeyError)?,
            loc_ref: if loc_ref.is_empty() {
                None
            } else {
                Some(unescape(loc_ref).ok_or(ParseMentionKeyError)?)
            },
            span: (
                start.parse().map_err(|_| ParseMentionKeyError)?,
                end.parse().map_err(|_| ParseMentionKeyError)?,
            ),
            evidence: unescape(evidence).ok_or(ParseMentionKeyError)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(evidence: &str, loc_ref: Option<&str>) -> MentionKey {
        MentionKey {
            content: ContentHash::of(b"file"),
            document: "whole".into(),
            loc_kind: "email_header".into(),
            loc_pos: 3,
            loc_ref: loc_ref.map(str::to_string),
            span: (4, 9),
            evidence: evidence.into(),
        }
    }

    /// Domain §3.3: the text form is one line and reads back, whatever the evidence holds.
    #[test]
    fn mention_key_text_form_reads_back() {
        for k in [
            key("Anna Berg", None),
            key("a\tb\nc\\d", Some("From")),
            key("", Some("x\ty")),
        ] {
            let text = k.to_string();
            assert!(!text.contains('\n'), "{text}");
            assert_eq!(text.parse::<MentionKey>(), Ok(k));
        }
        assert!("not a key".parse::<MentionKey>().is_err());
    }

    /// A split's anchor does not depend on the order the mentions were picked in.
    #[test]
    fn split_key_is_stable_and_marked() {
        let (a, b) = (key("A", None).to_string(), key("B", None).to_string());
        let one = MentionKey::split_key([a.as_str(), b.as_str()]).unwrap();
        let two = MentionKey::split_key([b.as_str(), a.as_str(), b.as_str()]).unwrap();
        assert_eq!(one, two);
        assert!(is_split_key(&one));
        assert_ne!(Some(one), MentionKey::split_key([a.as_str()]));
        assert_eq!(MentionKey::split_key([]), None);
        for not in ["split:", "split:xyz", "person:split", "email:a@b.example"] {
            assert!(!is_split_key(not), "{not}");
        }
    }
}
