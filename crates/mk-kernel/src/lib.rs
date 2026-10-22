//! Shared kernel: evidence value objects used by every context (domain §3.3).
//!
//! No I/O and no async runtime (ADR-0002 rule 1). The `Locator` is drafted in M0b.

mod hash;
mod span;

pub use hash::{ContentHash, ParseHashError};
pub use span::{Span, SpanError};
