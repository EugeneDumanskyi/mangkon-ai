//! BC-02 Corpus: source files, contents, re-scan and presence (domain §3).
//!
//! - [`rescan`]: the pure diff of stored file references against a walk (architecture §5.4).
//! - [`IngestionState`]: content states and their moves (domain §7.1).
//! - [`fs`]: the walk and the BLAKE3 hashing, the crate's only I/O.

pub mod fs;
pub mod rescan;
mod state;

pub use rescan::{
    FileMeta, PathChange, PlanItem, Planned, RescanPlan, Resolved, RootId, StoredRef, WalkedEntry,
    plan,
};
pub use state::{IllegalMove, IngestionState};
