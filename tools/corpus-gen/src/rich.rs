//! Rich text: plain text with marked mentions. Document content is written as templates;
//! each marker names a story identifier, entity or negative, so writers know exactly where
//! every mention lands.
//!
//! Template markers:
//! - `{e:ID}` canonical name, `{e:ID|Alias text}` a listed alias.
//! - `{i:ID}` default form, `{i:ID|form}` a named form (e.g. `national`, `compact`).
//! - `{n:ID}` a negative (bad checksum, card-like number, stop-list domain).
//!
//! Options after the main argument, each after `|`: `split=N` (force a line break after
//! character N of the raw text; marks the mention `hard`), `hard`, `#tag` (names the mention
//! for relations and events).

use crate::story::Story;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Ident(String),
    Entity(String),
    Neg(String),
}

#[derive(Clone, Debug)]
pub struct MarkSpec {
    pub target: Target,
    pub raw: String,
    pub hard: bool,
    pub split_at: Option<usize>,
    pub tag: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub text: String,
    pub mark: Option<MarkSpec>,
}

#[derive(Clone, Debug, Default)]
pub struct Rich {
    pub runs: Vec<Run>,
}

impl Rich {
    pub fn plain_text(s: &str) -> Rich {
        Rich {
            runs: vec![Run {
                text: s.to_string(),
                mark: None,
            }],
        }
    }

    pub fn plain(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|r| r.text.is_empty())
    }

    /// Marks with their char ranges in `plain()`.
    pub fn marks(&self) -> Vec<(&MarkSpec, usize, usize)> {
        let mut out = Vec::new();
        let mut pos = 0;
        for r in &self.runs {
            let n = r.text.chars().count();
            if let Some(m) = &r.mark {
                out.push((m, pos, pos + n));
            }
            pos += n;
        }
        out
    }

    pub fn push_str(&mut self, s: &str) {
        self.runs.push(Run {
            text: s.to_string(),
            mark: None,
        });
    }

    /// Splits at `\n` into one rich text per line. Marks never contain a newline.
    pub fn lines(&self) -> Vec<Rich> {
        let mut out = vec![Rich::default()];
        for r in &self.runs {
            if r.mark.is_some() {
                out.last_mut().unwrap().runs.push(r.clone());
                continue;
            }
            let mut parts = r.text.split('\n');
            if let Some(first) = parts.next() {
                out.last_mut().unwrap().push_str(first);
            }
            for p in parts {
                out.push(Rich::default());
                out.last_mut().unwrap().push_str(p);
            }
        }
        out
    }
}

impl Story {
    /// Parses a template. Panics on unknown ids or bad syntax: content errors are bugs.
    pub fn t(&self, template: &str) -> Rich {
        let mut runs = Vec::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                runs.push(Run {
                    text: rest[..open].to_string(),
                    mark: None,
                });
            }
            let close = rest[open..]
                .find('}')
                .unwrap_or_else(|| panic!("unclosed marker in {template:?}"))
                + open;
            runs.push(Run {
                text: String::new(),
                mark: Some(self.marker(&rest[open + 1..close], template)),
            });
            let raw = runs.last().unwrap().mark.as_ref().unwrap().raw.clone();
            runs.last_mut().unwrap().text = raw;
            rest = &rest[close + 1..];
        }
        if !rest.is_empty() {
            runs.push(Run {
                text: rest.to_string(),
                mark: None,
            });
        }
        Rich { runs }
    }

    fn marker(&self, body: &str, template: &str) -> MarkSpec {
        let (kind, args) = body
            .split_once(':')
            .unwrap_or_else(|| panic!("bad marker {{{body}}} in {template:?}"));
        let mut parts = args.split('|');
        let id = parts.next().unwrap().to_string();
        let mut main: Option<&str> = None;
        let mut spec = MarkSpec {
            target: Target::Neg(String::new()),
            raw: String::new(),
            hard: false,
            split_at: None,
            tag: None,
            note: None,
        };
        for p in parts {
            if let Some(n) = p.strip_prefix("split=") {
                spec.split_at = Some(n.parse().expect("split=N"));
                spec.hard = true;
            } else if p == "hard" {
                spec.hard = true;
            } else if let Some(t) = p.strip_prefix('#') {
                spec.tag = Some(t.to_string());
            } else if main.is_none() {
                main = Some(p);
            } else {
                panic!("extra marker argument {p:?} in {template:?}");
            }
        }
        match kind {
            "e" => {
                let e = self.entity(&id);
                let raw = main.unwrap_or(e.canonical);
                assert!(
                    raw == e.canonical || e.aliases.contains(&raw),
                    "{raw:?} is not a name of {id} (add it to the aliases)"
                );
                spec.raw = raw.to_string();
                spec.target = Target::Entity(id);
            }
            "i" => {
                spec.raw = self.ident(&id).form(main).to_string();
                spec.target = Target::Ident(id);
            }
            "n" => {
                spec.raw = self
                    .negs
                    .get(id.as_str())
                    .unwrap_or_else(|| panic!("unknown negative {id}"))
                    .raw()
                    .to_string();
                spec.target = Target::Neg(id);
            }
            _ => panic!("unknown marker kind {kind:?} in {template:?}"),
        }
        if let Some(n) = spec.split_at {
            assert!(
                n > 0 && n < spec.raw.chars().count(),
                "split={n} outside {:?}",
                spec.raw
            );
        }
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markers_and_keeps_plain_text() {
        let s = Story::new(1);
        let r = s.t("Dear {e:daniel|Mr Okonkwo-Hart}, call {i:ph_vpa|national|#p}.");
        assert_eq!(r.plain(), "Dear Mr Okonkwo-Hart, call 020 7946 0102.");
        let marks = r.marks();
        assert_eq!(marks.len(), 2);
        assert_eq!((marks[0].1, marks[0].2), (5, 20));
        assert_eq!(marks[1].0.tag.as_deref(), Some("p"));
    }

    #[test]
    #[should_panic(expected = "is not a name of")]
    fn rejects_unknown_alias() {
        Story::new(1).t("{e:daniel|Danny}");
    }
}
