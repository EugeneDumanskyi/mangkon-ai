//! Shared kernel: evidence value objects used by every context (domain §3.3).
//!
//! No I/O and no async runtime (ADR-0002 rule 1). Feature `serde` adds the derives the worker
//! protocol needs (ADR-0007); decoding re-checks the [`Span`] and [`Rect`] rules.
//!
//! The [`Locator`] says where a segment is in its document (INV-CO-03). Its [`Span`] always
//! points into the segment's stored text (INV-CO-04). PDF locators hold no boxes: the page view
//! gets a [`PageGeometry`] from the engine that renders the page and maps the span through it
//! (ADR-0011), which fails loudly if that engine no longer extracts the stored text. OCR
//! locators keep their word boxes, taken from the [`OcrGeometry`] built when the page was OCR'd,
//! because OCR is their only source.

mod geometry;
mod hash;
mod identifier;
mod locator;
mod ocr;
mod span;

pub use geometry::{GeometryError, PageGeometry, Rect};
pub use hash::{ContentHash, ParseHashError};
pub use identifier::{IdentifierKey, IdentifierKind, ParseKeyError};
pub use locator::{Column, LineNumber, Locator, PageNumber};
pub use ocr::{ImageSize, OcrGeometry, OcrWord};
pub use span::{Span, SpanError};
