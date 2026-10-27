//! Parser worker (ADR-0001, ADR-0007): the framed protocol between the core and a worker, and
//! the loop a worker runs. A worker is the app binary started with `--worker`; it reads one
//! content at a time, read-only (NFR-REL-10), parses it, finds identifiers and streams the result
//! back in chunks. It never opens the project database, never writes files and never uses the
//! network.
//!
//! - [`frame`]: `u32` little-endian length + `postcard` payload, with a size guard.
//! - [`protocol`]: the messages, versioned by [`PROTOCOL`].
//! - [`extract`]: a parsed content as chunks of segments and mentions (US-04.1).
//! - [`serve`] / [`run_stdio`]: the worker loop, with heartbeats; the stdio entry also sets the
//!   Linux address-space cap.
//! - [`fault`]: test faults (memory bomb, endless loop, crash), honoured only when the core asks
//!   for them in [`Settings::test_faults`].
//!
//! The supervisor (spawn, deadline, memory watchdog, recycling) is in `mk-core`.

pub mod extract;
pub mod fault;
pub mod frame;
pub mod protocol;
mod serve;

pub use protocol::{
    Chunk, FromWorker, Limits, Mention, PROTOCOL, ProcessContent, Segment, Settings, ToWorker,
    WorkStage,
};
pub use serve::{HEARTBEAT, ServeOptions, run_stdio, serve};
