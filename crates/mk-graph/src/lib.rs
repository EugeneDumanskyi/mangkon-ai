//! BC-07, BC-08 and BC-09: derived identifier and connection rows, rebuilt together per touched
//! key (domain §3, ADR-0005). No I/O (ADR-0002 rule 1).
//!
//! Empty in M2a. M2b adds the per-key rebuild.
