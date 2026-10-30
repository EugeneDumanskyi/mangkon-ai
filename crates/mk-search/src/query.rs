//! The keyword query syntax (US-06.1 AC2).
//!
//! - Plain words: all must match (`invoice march`).
//! - `"exact phrase"`: the words in this order. A quote left open runs to the end.
//! - `prefix*`: the word and any word it starts.
//! - `a OR b`: either term. `OR` is upper case only and binds the two terms next to it, so
//!   `x a OR b` is `x` and (`a` or `b`). A lone or trailing `OR` is ignored.
//!
//! Case and diacritics do not matter: the index folds both (`unicode61 remove_diacritics 2`).
//! A term with no letter or digit is dropped; a query left with none is [`QueryError::Empty`].

use std::fmt;

/// One term of a query.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Term {
    /// Words that must appear in this order; one word is a phrase of one.
    Phrase(String),
    /// A word, or the start of one (`invo*`).
    Prefix(String),
}

/// A parsed query: every group must match, and a group matches when any of its terms does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Query {
    groups: Vec<Vec<Term>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QueryError {
    /// Nothing to search for: empty, only operators, or only punctuation.
    Empty,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::Empty => f.write_str("Search needs at least one word"),
        }
    }
}

impl std::error::Error for QueryError {}

enum Token {
    Term(Term),
    Or,
}

fn has_word_char(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

fn tokens(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let phrase: String = chars.by_ref().take_while(|c| *c != '"').collect();
            if has_word_char(&phrase) {
                out.push(Token::Term(Term::Phrase(phrase)));
            }
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '"' {
                    break;
                }
                word.push(c);
                chars.next();
            }
            if word == "OR" {
                out.push(Token::Or);
            } else if let Some(stem) = word.strip_suffix('*') {
                let stem = stem.trim_end_matches('*');
                if has_word_char(stem) {
                    out.push(Token::Term(Term::Prefix(stem.to_string())));
                }
            } else if has_word_char(&word) {
                out.push(Token::Term(Term::Phrase(word)));
            }
        }
    }
    out
}

/// Reads a query as typed (see the module docs).
pub fn parse(input: &str) -> Result<Query, QueryError> {
    let mut groups: Vec<Vec<Term>> = Vec::new();
    let mut or = false;
    for token in tokens(input) {
        match token {
            Token::Or => or = !groups.is_empty(),
            Token::Term(t) => {
                match groups.last_mut() {
                    Some(g) if or => g.push(t),
                    _ => groups.push(vec![t]),
                }
                or = false;
            }
        }
    }
    if groups.is_empty() {
        return Err(QueryError::Empty);
    }
    Ok(Query { groups })
}

/// An FTS5 string: the text in double quotes, inner quotes doubled.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

impl Query {
    pub fn groups(&self) -> &[Vec<Term>] {
        &self.groups
    }

    /// The query as an FTS5 query. Every term is a quoted string (`"abc"`, `"ab"*`), so no
    /// typed text is read as FTS5 syntax (`NEAR`, `col:`, `^`, `NOT`).
    pub fn to_fts5(&self) -> String {
        self.fts5_with(|_| None)
    }

    /// The query for `highlight()` on rows whose texts are `texts`: each prefix term becomes
    /// the words in those texts that start with it, so FTS5 does not expand the prefix over the
    /// whole index once per row (S4: `con*` 407 ms → a few ms). A prefix with no such word,
    /// more than [`MAX_EXPANDED`] of them, or that is not one plain word stays a prefix. A word
    /// the index folds but this does not (`cafe*` and `café`) is not marked; the row still is
    /// a result.
    pub fn highlight_fts5(&self, texts: &[&str]) -> String {
        self.fts5_with(|stem| {
            if !stem.chars().all(char::is_alphanumeric) {
                return None;
            }
            let stem = stem.to_lowercase();
            let mut words: Vec<String> = texts
                .iter()
                .flat_map(|t| t.split(|c: char| !c.is_alphanumeric()))
                .map(str::to_lowercase)
                .filter(|w| w.starts_with(&stem))
                .collect();
            words.sort_unstable();
            words.dedup();
            (!words.is_empty() && words.len() <= MAX_EXPANDED).then_some(words)
        })
    }

    /// The FTS5 query, a prefix term replaced by the words `expand` gives for its stem.
    fn fts5_with(&self, expand: impl Fn(&str) -> Option<Vec<String>>) -> String {
        let terms = |t: &Term| match t {
            Term::Phrase(p) => vec![quoted(p)],
            Term::Prefix(p) => match expand(p) {
                Some(words) => words.iter().map(|w| quoted(w)).collect(),
                None => vec![format!("{}*", quoted(p))],
            },
        };
        self.groups
            .iter()
            .map(
                |g| match g.iter().flat_map(terms).collect::<Vec<_>>().as_slice() {
                    [one] => one.clone(),
                    many => format!("({})", many.join(" OR ")),
                },
            )
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}

/// Most words a prefix is replaced by in a highlight query.
pub const MAX_EXPANDED: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    fn fts(q: &str) -> String {
        parse(q)
            .map(|q| q.to_fts5())
            .unwrap_or_else(|e| e.to_string())
    }

    /// US-06.1 AC2: words, phrases, prefixes and OR.
    #[test]
    fn us_06_1_ac2_syntax() {
        let cases = [
            ("invoice", r#""invoice""#),
            ("invoice march", r#""invoice" AND "march""#),
            (r#""harbour view" lease"#, r#""harbour view" AND "lease""#),
            ("invo*", r#""invo"*"#),
            ("invo**", r#""invo"*"#),
            ("a OR b", r#"("a" OR "b")"#),
            ("x a OR b", r#""x" AND ("a" OR "b")"#),
            ("a OR b OR c d", r#"("a" OR "b" OR "c") AND "d""#),
            ("a or b", r#""a" AND "or" AND "b""#),
            ("OR a", r#""a""#),
            ("a OR", r#""a""#),
            ("a OR OR b", r#"("a" OR "b")"#),
            (r#"say "open quote"#, r#""say" AND "open quote""#),
            (r#"a"b"#, r#""a" AND "b""#),
            ("Zoë", r#""Zoë""#),
            ("a@x.example", r#""a@x.example""#),
        ];
        for (input, want) in cases {
            assert_eq!(fts(input), want, "{input}");
        }
    }

    /// US-06.1 AC2: FTS5 syntax typed by a person stays text.
    #[test]
    fn us_06_1_ac2_no_fts5_syntax_gets_through() {
        let cases = [
            ("NEAR(a b)", r#""NEAR(a" AND "b)""#),
            ("text:secret", r#""text:secret""#),
            ("^start", r#""^start""#),
            ("a NOT b", r#""a" AND "NOT" AND "b""#),
            ("a AND b", r#""a" AND "AND" AND "b""#),
            ("{text}: x", r#""{text}:" AND "x""#),
        ];
        for (input, want) in cases {
            assert_eq!(fts(input), want, "{input}");
        }
    }

    /// S4: a prefix is marked through the words of the page's texts, not expanded per row.
    #[test]
    fn highlight_query_expands_prefixes_from_the_texts() {
        let q = |s: &str, texts: &[&str]| parse(s).unwrap().highlight_fts5(texts);
        let texts = ["Contract for Consulting", "the CONTRACT, and a cone."];
        assert_eq!(
            q("con*", &texts),
            r#"("cone" OR "consulting" OR "contract")"#
        );
        assert_eq!(
            q("x con* OR lease", &texts),
            r#""x" AND ("cone" OR "consulting" OR "contract" OR "lease")"#
        );
        assert_eq!(q("zzz*", &texts), r#""zzz"*"#);
        assert_eq!(q("a-b*", &texts), r#""a-b"*"#);
        assert_eq!(q(r#""for consulting""#, &texts), r#""for consulting""#);
        let many: Vec<String> = (0..=MAX_EXPANDED).map(|i| format!("con{i}")).collect();
        let many = [many.join(" ")];
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(q("con*", &many), r#""con"*"#);
    }

    /// US-06.1 AC2: nothing to search for.
    #[test]
    fn us_06_1_ac2_empty_queries() {
        for q in [
            "", "   ", "OR", "OR OR", "*", "\"\"", "\"  \"", "-- !!", "\"",
        ] {
            assert_eq!(parse(q), Err(QueryError::Empty), "{q:?}");
        }
        assert_eq!(
            QueryError::Empty.to_string(),
            "Search needs at least one word"
        );
    }
}
