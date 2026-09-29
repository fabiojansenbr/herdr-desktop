//! Resource bench of spec 007 (AC-007-03 preparation), included by
//! `src-tauri/tests/fidelity_bench.rs` with `#[path]`; nothing here reaches the product binary.
//!
//! - [`plan`]: the 1/15 PTY × 1/2 GUI × visible/hidden matrix, the idle baselines, the run modes
//!   (smoke ≤ 3 s, acceptance ≥ 5 s warmup + ≥ 60 s, memory 1800 s with fixed checkpoints) and
//!   the explicit CLI parameters.
//! - [`evidence`]: pure mapping of real readings (engine JSON, /proc, generator counter, probe).
//! - [`live`]: the 1-GUI/1-PTY release runner (private display, disposable engine, collector).
//! - [`verdict`]: validation of one run's evidence. The best outcome is
//!   `MeasuredPendingReview` (acceptance) or `InsufficientForAcceptance` (smoke); there is no PASS.

pub mod evidence;
#[cfg(target_os = "linux")]
pub mod live;
pub mod plan;
pub mod verdict;
