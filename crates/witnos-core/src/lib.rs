//! witnos-core — domain types, the per-goal JSON store, the auto-watch
//! project registry, the armed marker's shape, and the gate's release
//! condition. No I/O beyond its own files under WITNOS_HOME, no HTTP, no GUI.
//!
//! Design source: README.md (canonical, zh-TW) and docs/schema-v1.md.

pub mod gate;
pub mod marker;
pub mod registry;
pub mod store;
pub mod types;

pub use gate::{evaluate, GateOutcome};
pub use registry::ProjectRegistry;
pub use store::{ItemEdit, NewEvidence, NewItem, Store, StoreError};
pub use types::*;
