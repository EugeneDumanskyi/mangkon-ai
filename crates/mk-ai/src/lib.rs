//! BC-06 AI extraction (domain §3, §6.6, ADR-0005, ADR-0006): everything about Stage B that needs
//! no I/O. `mk-store` persists runs and mentions; `mk-core` runs the lane and makes the calls
//! through the egress gate (ADR-0009).
//!
//! - [`extract`]: the M1 prompt and schema, the strict parse and the retry text (INV-AI-03).
//! - [`units`]: adjacent segments of one document grouped into work units.
//! - [`check`]: the evidence rules on what a model replied (INV-AI-01, NFR-QA-11).
//! - [`run`]: a run's identity and states, and the "Experimental" build flag (US-10.3).

pub mod check;
pub mod extract;
pub mod run;
pub mod units;

pub use check::{
    CheckOutcome, CheckedMention, IdentifierSpan, Piece, check_reply, unit_text, verify_mention,
};
pub use extract::{
    MAX_TOKENS, PROMPT_VERSION, Prediction, SCHEMA_NAME, SEED, SYSTEM_PROMPT, parse_reply,
    retry_message, schema, user_message,
};
pub use run::{RunIdentity, RunState, experimental, experimental_from};
pub use units::{MAX_UNIT_CHARS, UnitPlan, UnitPlanner, plan_units, unit_pieces};
