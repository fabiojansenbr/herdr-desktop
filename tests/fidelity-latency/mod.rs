//! Spec 007 latency preparation (input → presentation). Pure evaluator, marker codec, clock and
//! PTY helpers. Not wired to the native dispatcher, window or benchmark yet; see
//! evidencias/007/latency-prep/CONTRACT.md for the integration contract.
pub mod clock;
pub mod marker;
pub mod pty;
pub mod record;
pub mod viewport;
