//! Connections of spec 003: Local and SSH hosts side by side.
//!
//! - [`ssh_options`]: validated SSH identity and OpenSSH command construction (seam for 006).
//! - [`failure`]: attention vs transient classification.
//! - [`state`]: link lifecycle, backoff, cancellation and health with an injected clock.
//! - [`profiles`]: the desktop's own SSH profile store; TUI catalog imported read-only.
//! - [`hub`]: routing by endpoint, input gating by reconciled snapshot/surface, action ledger.
//! - [`commands`]: Tauri commands and the background supervisor.
//!
//! The SSH transport lives in `crate::bridge::ssh`. Registering the commands in the window
//! belongs to spec 007.

pub mod commands;
pub mod failure;
pub mod hub;
pub mod profiles;
pub mod remote_binary;
pub mod ssh_options;
pub mod state;
