//! Work units (decided 2026-11-05): adjacent segments of one document are grouped to at most
//! [`MAX_UNIT_CHARS`] characters, so short lines and CSV-like text do not cost one model call
//! each; a longer segment is split at whitespace. Mentions are still stored against one segment
//! and a span in it ([`crate::check`]).
//!
//! The plan depends only on the segments' ids and texts in order, so planning a document twice
//! gives the same units: the store finds what is not covered yet by comparing them.

use crate::check::Piece;

/// M1's prompts were one segment of about this size (`09-model-check.md` §3.1).
pub const MAX_UNIT_CHARS: usize = 2000;

/// One unit: the segments `first..=last` of a document, or for a long segment a window of it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct UnitPlan {
    pub first: i64,
    pub last: i64,
    /// A window `(start, end)` in characters of the one segment `first`.
    pub window: Option<(usize, usize)>,
}

/// Segments of one document in order go in; units come out. Holds only the open unit, so a
/// document of any size plans in O(longest segment) memory.
pub struct UnitPlanner {
    max: usize,
    open: Option<(i64, i64, usize)>,
}

impl UnitPlanner {
    pub fn new(max: usize) -> UnitPlanner {
        UnitPlanner {
            max: max.max(2),
            open: None,
        }
    }

    pub fn push(&mut self, id: i64, text: &str, out: &mut impl FnMut(UnitPlan)) {
        let chars = text.chars().count();
        let blank = text.trim().is_empty();
        if chars > self.max {
            self.finish(out);
            if !blank {
                for (start, end) in windows(text, self.max) {
                    out(UnitPlan {
                        first: id,
                        last: id,
                        window: Some((start, end)),
                    });
                }
            }
            return;
        }
        match self.open {
            // One character joins two segments in the unit's text.
            Some((first, _, used)) if used + 1 + chars <= self.max => {
                self.open = Some((first, id, used + 1 + chars));
            }
            _ if blank => self.finish(out),
            _ => {
                self.finish(out);
                self.open = Some((id, id, chars));
            }
        }
    }

    /// Ends the document.
    pub fn finish(&mut self, out: &mut impl FnMut(UnitPlan)) {
        if let Some((first, last, _)) = self.open.take() {
            out(UnitPlan {
                first,
                last,
                window: None,
            });
        }
    }
}

/// The units of one document's segments, in order.
pub fn plan_units<'a>(segments: impl IntoIterator<Item = (i64, &'a str)>) -> Vec<UnitPlan> {
    let mut out = Vec::new();
    let mut planner = UnitPlanner::new(MAX_UNIT_CHARS);
    for (id, text) in segments {
        planner.push(id, text, &mut |u| out.push(u));
    }
    planner.finish(&mut |u| out.push(u));
    out
}

/// A long text's windows of at most `max` characters: each cut prefers a line break in the
/// window's second half, then other whitespace, else falls at `max` (a text with no space).
/// No overlap (ASSUMPTION: a name cut in two at a window's edge is lost). Blank windows are left
/// out.
fn windows(text: &str, max: usize) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + max).min(chars.len());
        if end < chars.len() {
            let half = start + max / 2;
            let last = |pred: &dyn Fn(char) -> bool| {
                (half..end).rev().find(|&i| pred(chars[i])).map(|i| i + 1)
            };
            end = last(&|c| c == '\n')
                .or_else(|| last(&char::is_whitespace))
                .unwrap_or(end);
        }
        if chars[start..end].iter().any(|c| !c.is_whitespace()) {
            out.push((start, end));
        }
        start = end;
    }
    out
}

/// The pieces of a unit, from the segments `first..=last` in order, for the call and the checks.
pub fn unit_pieces(plan: &UnitPlan, segments: Vec<(i64, String)>) -> Vec<Piece> {
    match plan.window {
        Some((start, end)) => segments
            .into_iter()
            .filter(|(id, _)| *id == plan.first)
            .map(|(id, text)| Piece {
                segment: id,
                start,
                text: text.chars().skip(start).take(end - start).collect(),
            })
            .collect(),
        None => segments
            .into_iter()
            .map(|(id, text)| Piece {
                segment: id,
                start: 0,
                text,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn unit(first: i64, last: i64) -> UnitPlan {
        UnitPlan {
            first,
            last,
            window: None,
        }
    }

    #[test]
    fn adjacent_segments_group_up_to_the_limit() {
        let a = "a".repeat(900);
        let b = "b".repeat(900);
        let c = "c".repeat(300);
        // 900 + 1 + 900 = 1801; 300 more with the join is 2102.
        let plan = plan_units([(1, a.as_str()), (2, b.as_str()), (3, c.as_str())]);
        assert_eq!(plan, vec![unit(1, 2), unit(3, 3)]);
        // Exactly at the limit stays one unit.
        let d = "d".repeat(1099);
        assert_eq!(
            plan_units([(1, a.as_str()), (2, d.as_str())]),
            vec![unit(1, 2)]
        );
    }

    #[test]
    fn blank_segments_start_no_unit() {
        assert_eq!(plan_units([(1, ""), (2, "   \n")]), vec![]);
        assert_eq!(plan_units([(1, " "), (2, "x"), (3, " ")]), vec![unit(2, 3)]);
        assert_eq!(plan_units([]), vec![]);
    }

    #[test]
    fn a_long_segment_is_cut_at_line_breaks_then_whitespace() {
        let line = format!("{}\n", "w ".repeat(400)); // 801 characters
        let text = line.repeat(4); // 3204 characters
        let plan = plan_units([(7, "before"), (8, text.as_str()), (9, "after")]);
        assert_eq!(plan[0], unit(7, 7));
        assert_eq!(plan.last(), Some(&unit(9, 9)));
        let windows: Vec<_> = plan[1..plan.len() - 1]
            .iter()
            .map(|u| u.window.unwrap())
            .collect();
        // Cuts fall right after a line break.
        assert_eq!(windows, vec![(0, 1602), (1602, 3204)]);

        let spaced = "word ".repeat(1000);
        let plan = plan_units([(1, spaced.as_str())]);
        assert!(plan.iter().all(|u| {
            let (s, e) = u.window.unwrap();
            e - s <= MAX_UNIT_CHARS && (s == 0 || spaced.chars().nth(s - 1) == Some(' '))
        }));

        // No whitespace at all: cut at the limit.
        let solid = "x".repeat(4500);
        let plan = plan_units([(1, solid.as_str())]);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].window, Some((0, 2000)));
    }

    #[test]
    fn pieces_rebuild_the_text() {
        let plan = unit(1, 2);
        let p = unit_pieces(&plan, vec![(1, "one".into()), (2, "two".into())]);
        assert_eq!(
            (p[1].segment, p[1].start, p[1].text.as_str()),
            (2, 0, "two")
        );
        let plan = UnitPlan {
            first: 5,
            last: 5,
            window: Some((2, 5)),
        };
        let p = unit_pieces(&plan, vec![(5, "héllo wörld".into())]);
        assert_eq!((p[0].start, p[0].text.as_str()), (2, "llo"));
    }

    proptest! {
        /// Every non-blank character is in exactly one unit, no unit is over the limit, and
        /// planning again gives the same units.
        #[test]
        fn units_cover_every_segment_once(
            segments in proptest::collection::vec("[a-zé \n]{0,700}", 0..40),
            max in 10usize..600,
        ) {
            let indexed: Vec<(i64, &str)> = segments
                .iter()
                .enumerate()
                .map(|(i, s)| (i as i64 + 1, s.as_str()))
                .collect();
            let run = || {
                let mut out = Vec::new();
                let mut p = UnitPlanner::new(max);
                for (id, text) in &indexed {
                    p.push(*id, text, &mut |u| out.push(u));
                }
                p.finish(&mut |u| out.push(u));
                out
            };
            let plan = run();
            prop_assert_eq!(&plan, &run());
            let mut seen: Vec<Vec<bool>> =
                segments.iter().map(|s| vec![false; s.chars().count()]).collect();
            for u in &plan {
                match u.window {
                    Some((s, e)) => {
                        prop_assert_eq!(u.first, u.last);
                        prop_assert!(e - s <= max);
                        for c in s..e {
                            let row = &mut seen[u.first as usize - 1];
                            prop_assert!(!row[c]);
                            row[c] = true;
                        }
                    }
                    None => {
                        let len: usize = (u.first..=u.last)
                            .map(|i| segments[i as usize - 1].chars().count())
                            .sum::<usize>()
                            + (u.last - u.first) as usize;
                        prop_assert!(len <= max, "{} over {}", len, max);
                        for i in u.first..=u.last {
                            let row = &mut seen[i as usize - 1];
                            for c in row.iter_mut() {
                                prop_assert!(!*c);
                                *c = true;
                            }
                        }
                    }
                }
            }
            for (s, row) in segments.iter().zip(&seen) {
                for (c, covered) in s.chars().zip(row) {
                    prop_assert!(*covered || c.is_whitespace(), "lost {:?} in {:?}", c, s);
                }
            }
        }
    }
}
