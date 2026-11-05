//! BC-10 Search. Keyword part (M4a, US-06.1): the query syntax, the snippet of a hit and the
//! masking of card-like numbers in it (NFR-SEC-14). Meaning part (M6b, US-06.2): [`meaning`].
//! Pure functions, no I/O (ADR-0002 rule 1); the keyword index is SQLite FTS5 in `mk-store`
//! (ADR-0010).
//!
//! - [`parse`] reads what a person types; [`Query::to_fts5`] writes it as an FTS5 query where
//!   every term is a quoted string, so typed text is never FTS5 syntax.
//! - [`mask_card_like`] hides all but the last four digits of card-like numbers, keeping the
//!   text's length in characters so hit spans stay valid (INV-CO-04).
//! - [`snippet`] cuts a window of a segment's text around its first hit, as runs.

mod mask;
pub mod meaning;
mod query;
mod snippet;

pub use mask::mask_card_like;
pub use query::{Query, QueryError, Term, parse};
pub use snippet::{Run, SNIPPET_CHARS, snippet};
