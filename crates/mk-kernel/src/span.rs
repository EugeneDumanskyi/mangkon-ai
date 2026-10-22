use std::fmt;

/// Start and end character positions inside a segment's stored text (domain §3.3).
///
/// Half-open: `start` is included, `end` is not. Always `0 ≤ start < end ≤ len`,
/// so a span is never empty and never runs past its text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Span {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanError {
    /// `start >= end`.
    Empty { start: usize, end: usize },
    /// `end > len`.
    OutOfBounds { end: usize, len: usize },
}

impl fmt::Display for SpanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpanError::Empty { start, end } => write!(f, "empty span: start {start} >= end {end}"),
            SpanError::OutOfBounds { end, len } => {
                write!(f, "span end {end} is past text length {len}")
            }
        }
    }
}

impl std::error::Error for SpanError {}

impl Span {
    /// Checked constructor. `len` is the length of the text the span points into.
    pub fn new(start: usize, end: usize, len: usize) -> Result<Self, SpanError> {
        if start >= end {
            return Err(SpanError::Empty { start, end });
        }
        if end > len {
            return Err(SpanError::OutOfBounds { end, len });
        }
        Ok(Self { start, end })
    }

    pub fn start(&self) -> usize {
        self.start
    }

    pub fn end(&self) -> usize {
        self.end
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Always false: an empty span cannot be built. Present for API symmetry with `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// True if the two spans share at least one position.
    pub fn overlaps(&self, other: &Span) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rejects_empty_and_reversed() {
        assert_eq!(
            Span::new(3, 3, 10),
            Err(SpanError::Empty { start: 3, end: 3 })
        );
        assert_eq!(
            Span::new(5, 2, 10),
            Err(SpanError::Empty { start: 5, end: 2 })
        );
    }

    #[test]
    fn rejects_past_end() {
        assert_eq!(
            Span::new(0, 11, 10),
            Err(SpanError::OutOfBounds { end: 11, len: 10 })
        );
    }

    #[test]
    fn accepts_whole_text() {
        let s = Span::new(0, 10, 10).unwrap();
        assert_eq!((s.start(), s.end(), s.len()), (0, 10, 10));
    }

    proptest! {
        #[test]
        fn constructor_enforces_invariant(start in 0usize..100, end in 0usize..100, len in 0usize..100) {
            match Span::new(start, end, len) {
                Ok(s) => prop_assert!(s.start() < s.end() && s.end() <= len),
                Err(_) => prop_assert!(start >= end || end > len),
            }
        }

        #[test]
        fn overlap_is_symmetric(a in 0usize..50, b in 1usize..50, c in 0usize..50, d in 1usize..50) {
            let x = Span::new(a, a + b, 200).unwrap();
            let y = Span::new(c, c + d, 200).unwrap();
            prop_assert_eq!(x.overlaps(&y), y.overlaps(&x));
        }
    }
}
