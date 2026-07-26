//! witnos-core — domain types, the per-goal JSON store, and the gate's
//! release condition. No I/O beyond the store's own files, no HTTP, no GUI.
//!
//! Design source: README.md (canonical, zh-TW) and docs/schema-v1.md.

pub mod gate;
pub mod store;
pub mod types;

pub use gate::{evaluate, GateOutcome};
pub use store::{NewEvidence, NewItem, Store, StoreError};
pub use types::*;
