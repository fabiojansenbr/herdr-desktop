//! Support of the single native E2E of spec 007 (`src-tauri/tests/fidelity_native.rs`).
//!
//! Included by that test with `#[path]`; nothing here is compiled into the product binary.
//!
//! - [`plan`]: the phases of the one composed flow, what each must prove, and the evaluator that
//!   refuses to count a pending (placeholder) phase as passed.
//! - [`chunks`]: build chunk map → editor/language modules, and the lazy-editor verdict
//!   (≥ 2000 ms idle with zero editor modules, then a positive control after opening a file).
//! - [`display`]: private headless Wayland display (sway/wlroots, fcitx5) under `env -i`,
//!   reusing the prepared `.local/native-input` and `.local/prep-cjk` resources read-only.
//! - [`session`]: disposable `hd007-*` sessions and the Local/SSH host pair precondition.
//! - [`corpus`]: native input corpus fixture (observed standalone vs proposed).
//! - [`remote`]: private Local/SSH/legacy host fixture (namespaces, loopback sshd, ledger).
//! - [`ssh_flow`]: parent snapshots, ledger and evaluator of the five SSH phases.
//! - [`paste_flow`] / [`mouse_flow`]: parent steps, ledgers and evaluators of the paste-selection
//!   and mouse-scroll-links phases, linked to the same run by [`live`].
//! - [`window`]: the real composed App window (`herdr_desktop::{configure, install, handler}`)
//!   with only `harness_report` routed to the harness.
//! - [`visual_frame`]: literals, parent helpers and evaluator of the spec 010 `visual-frame` phase.
//! - [`visual_agents`]: literals, parent helpers and evaluator of the spec 014 `visual-agents` phase.
//! - [`visual_files`]: literals, parent helpers and evaluator of the spec 015 `visual-files` phase.
//! - [`visual_center`]: the same for the spec 013 `visual-center` phase (header, tabs, pane frames).

pub mod chunks;
pub mod corpus;
pub mod display;
#[cfg(target_os = "linux")]
pub mod geometry;
#[cfg(target_os = "linux")]
pub mod live;
#[cfg(target_os = "linux")]
pub mod mouse_flow;
#[cfg(target_os = "linux")]
pub mod paste_flow;
pub mod plan;
#[cfg(target_os = "linux")]
// `remote.rs` is shared with fidelity_remote_fixture.rs (same allow there): this inclusion does not
// use every re-export nor rename the approved fixture's enums, so the lint is scoped here.
#[allow(unused_imports, clippy::enum_variant_names)]
pub mod remote;
// On Linux `remote` owns the one copy of `session` (loading the file twice duplicates its types).
#[cfg(target_os = "linux")]
pub use remote::session;
#[cfg(not(target_os = "linux"))]
pub mod session;
pub mod ssh_flow;
#[cfg(target_os = "linux")]
pub mod supervisor;
#[cfg(target_os = "linux")]
pub mod view_flow;
#[cfg(target_os = "linux")]
pub mod visual_agents;
pub mod visual_center;
pub mod visual_files;
#[cfg(target_os = "linux")]
pub mod visual_frame;
#[cfg(target_os = "linux")]
pub mod visual_projects;
#[cfg(target_os = "linux")]
pub mod window;
