//! The snippet of a search hit (US-06.1 AC1): a window of the segment's text around its first
//! hit, as runs of plain text and hit text. Plain text, never markup (gap A8).

/// How many characters of a segment a snippet shows, at most (ellipses not counted).
pub const SNIPPET_CHARS: usize = 160;

/// Characters shown before the first hit, at most.
const LEAD: usize = 40;

/// How far a cut end moves to land on a space.
const SNAP: usize = 15;

/// A part of a snippet: hit text or not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Run {
    pub text: String,
    pub hit: bool,
}

/// A window of at most `width` characters of `text` around its first hit. `hits` are character
/// spans in `text` (INV-CO-04), in order; parts past the window are cut. Tabs and line ends
/// show as spaces. An end cut inside the text gets `…`.
pub fn snippet(text: &str, hits: &[(usize, usize)], width: usize) -> Vec<Run> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let first = hits.first().map_or(0, |h| h.0.min(n));
    let mut start = first.saturating_sub(LEAD.min(width / 4));
    let mut end = n.min(start + width);
    start = start.min(end.saturating_sub(width));
    if start > 0 {
        let limit = first.min(start + SNAP);
        if let Some(k) = (start..limit).find(|&k| chars[k].is_whitespace()) {
            start = k + 1;
        }
    }
    if end < n {
        let floor = hits
            .iter()
            .filter(|h| h.0 < end)
            .map(|h| h.1.min(end))
            .max()
            .unwrap_or(start)
            .max(end.saturating_sub(SNAP));
        if let Some(k) = (floor..end).rev().find(|&k| chars[k].is_whitespace()) {
            end = k;
        }
    }

    let mut runs: Vec<Run> = Vec::new();
    let mut push = |c: char, hit: bool| match runs.last_mut() {
        Some(r) if r.hit == hit => r.text.push(c),
        _ => runs.push(Run {
            text: c.to_string(),
            hit,
        }),
    };
    if start > 0 {
        push('…', false);
    }
    for (k, &c) in chars.iter().enumerate().take(end).skip(start) {
        let hit = hits.iter().any(|&(s, e)| s <= k && k < e);
        push(
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            },
            hit,
        );
    }
    if end < n {
        push('…', false);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(runs: &[Run]) -> String {
        runs.iter()
            .map(|r| {
                if r.hit {
                    format!("[{}]", r.text)
                } else {
                    r.text.clone()
                }
            })
            .collect()
    }

    #[test]
    fn short_text_is_whole_with_hits_marked() {
        let runs = snippet("Invoice\tfor Zoë", &[(0, 7), (12, 15)], SNIPPET_CHARS);
        assert_eq!(show(&runs), "[Invoice] for [Zoë]");
    }

    #[test]
    fn long_text_is_cut_around_the_first_hit_on_spaces() {
        let text = format!("{} needle {}", "lead ".repeat(40), "tail ".repeat(40));
        let at = text.find("needle").unwrap();
        let runs = snippet(&text, &[(at, at + 6)], 60);
        let s = show(&runs);
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("[needle]"), "{s}");
        assert!(s.chars().count() <= 62, "{s}");
        // Cut on spaces: no partial words at either end.
        assert!(s.starts_with("…lead ") && s.ends_with(" tail…"), "{s}");
    }

    #[test]
    fn hit_at_the_end_keeps_a_full_window() {
        let text = format!("{}end", "x ".repeat(100));
        let runs = snippet(&text, &[(200, 203)], 40);
        let s = show(&runs);
        assert!(s.ends_with("[end]"), "{s}");
        assert!(s.chars().count() >= 30, "{s}");
    }

    #[test]
    fn no_hit_shows_the_start() {
        assert_eq!(show(&snippet("abc", &[], 10)), "abc");
        assert!(snippet("", &[], 10).is_empty());
    }
}
