//! Herdr Desktop runtime gateway.
//!
//! Shared contracts for every feature front (002–006) plus the Linux/Unix local
//! transport used by the 001 proof. Nothing here imports Tauri, CodeMirror or SSH;
//! remote adapters implement the same traits in their own crates.
//!
//! Modules:
//! - [`contracts`]: `QualifiedTarget`, `RuntimeError`, `RuntimeGateway`, `FileProvider`, `ProjectRef`.
//! - [`event_queue`]: bounded gateway event queue shared by the local and SSH transports.
//! - [`frame_store`]: full/patch/revision handling with stale rejection and input gating.
//! - [`session`]: session-name validation and socket path resolution (ports of the engine rules).
//! - [`bootstrap`]: fatal vs recoverable start-up classification and detached session start.
//! - [`local`]: `LocalGateway` over the engine's local socket (Unix path / Windows namespaced).
//! - [`api`]: newline-delimited JSON API client for runtime actions.

pub mod api;
pub mod bootstrap;
pub mod contracts;
pub mod event_queue;
pub mod frame_store;
pub mod local;
pub mod session;

pub use contracts::*;
pub use frame_store::{ApplyOutcome, FrameStore, RecoveryAction, StaleReason, SurfaceState};
pub use herdr_protocol as protocol;
pub use local::LocalGateway;
pub use session::{SessionName, SessionPaths};
