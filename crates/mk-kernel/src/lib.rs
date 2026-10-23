//! Shared kernel: evidence value objects used by every context (domain §3.3).
//!
//! No I/O and no async runtime (ADR-0002 rule 1).
//!
//! The [`Locator`] says where a segment is in its document (INV-CO-03). Its [`Span`] always
//! points into the segment's stored text (INV-CO-04). PDF locators hold no boxes: the page view
//! gets a [`PageGeometry`] from the engine that renders the page and maps the span through it
//! (ADR-0011), which fails loudly if that engine no longer extracts the stored text.

mod geometry;
mod hash;
mod locator;
mod span;

pub use geometry::{GeometryError, PageGeometry, Rect};
pub use hash::{ContentHash, ParseHashError};
pub use locator::{Locator, PageNumber};
pub use span::{Span, SpanError};
