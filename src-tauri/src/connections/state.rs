//! Connection lifecycle per host with an injected clock: states, backoff, cancellation and
//! endpoint health (port of `client/endpoint/health.rs`, plus the remote bridge idle budget of
//! `platform/remote_bridge.rs`).

use std::time::{Duration, Instant};

use herdr_client::RuntimeError;
use serde::Serialize;

use super::failure::{AttentionReason, ConnectFailure};

/// Health ping cadence when nothing was received (engine: 5 s).
pub const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
/// Probe/initial snapshot budget (engine: 10 s).
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Remote bridge idle reaping (engine `IDLE_TIMEOUT`: 60 s without traffic either way).
pub const BRIDGE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

const BACKOFF_SECS: [u64; 6] = [1, 2, 4, 8, 16, 30];

/// Delay before automatic attempt `attempt` (1-based), capped at 30 s.
pub fn backoff_delay(attempt: u32) -> Duration {
    let index = (attempt.max(1) as usize - 1).min(BACKOFF_SECS.len() - 1);
    Duration::from_secs(BACKOFF_SECS[index])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkPhase {
    Offline,
    Connecting,
    Online,
    Reconnecting,
    Attention,
}

impl LinkPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Offline => "Offline",
            Self::Connecting => "Connecting",
            Self::Online => "Online",
            Self::Reconnecting => "Reconnecting",
            Self::Attention => "Needs attention",
        }
    }
}

/// Lifecycle of one host link. Attempts carry tokens; results of cancelled or superseded
/// attempts are rejected so the caller detaches them.
#[derive(Debug, Clone)]
pub struct LinkMachine {
    phase: LinkPhase,
    attempt: u32,
    retry_at: Option<Instant>,
    error: Option<RuntimeError>,
    attention: Option<AttentionReason>,
    established: bool,
    cancelled: bool,
    next_token: u64,
    in_flight: Option<u64>,
}

impl Default for LinkMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkMachine {
    /// Empty state: offline, nothing scheduled, no process.
    pub fn new() -> Self {
        Self {
            phase: LinkPhase::Offline,
            attempt: 0,
            retry_at: None,
            error: None,
            attention: None,
            established: false,
            cancelled: false,
            next_token: 1,
            in_flight: None,
        }
    }

    pub fn phase(&self) -> LinkPhase {
        self.phase
    }
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at
    }
    pub fn error(&self) -> Option<&RuntimeError> {
        self.error.as_ref()
    }
    pub fn attention(&self) -> Option<AttentionReason> {
        self.attention
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }
    pub fn in_flight(&self) -> Option<u64> {
        self.in_flight
    }

    fn begin(&mut self) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        self.in_flight = Some(token);
        self.retry_at = None;
        self.phase = if self.established {
            LinkPhase::Reconnecting
        } else {
            LinkPhase::Connecting
        };
        token
    }

    /// Explicit user request (connect, or retry after configuring). `None` while an attempt
    /// is already in flight or the link is online.
    pub fn request(&mut self, _now: Instant) -> Option<u64> {
        if self.in_flight.is_some() || self.phase == LinkPhase::Online {
            return None;
        }
        self.cancelled = false;
        self.attention = None;
        Some(self.begin())
    }

    /// Renegotiates an online link (e.g. switching to metadata-only). The previous
    /// connection must be dropped by the caller.
    pub fn renegotiate(&mut self, _now: Instant) -> Option<u64> {
        if self.phase != LinkPhase::Online || self.in_flight.is_some() {
            return None;
        }
        Some(self.begin())
    }

    /// Whether an automatic retry is due. Attention and cancellation never retry.
    pub fn due(&self, now: Instant) -> bool {
        !self.cancelled
            && self.in_flight.is_none()
            && matches!(self.phase, LinkPhase::Offline | LinkPhase::Reconnecting)
            && self.retry_at.is_some_and(|at| now >= at)
    }

    pub fn start_retry(&mut self, now: Instant) -> Option<u64> {
        if !self.due(now) {
            return None;
        }
        Some(self.begin())
    }

    pub fn succeeded(&mut self, token: u64, _now: Instant) -> bool {
        if self.in_flight != Some(token) {
            return false;
        }
        self.in_flight = None;
        self.phase = LinkPhase::Online;
        self.attempt = 0;
        self.retry_at = None;
        self.error = None;
        self.attention = None;
        self.established = true;
        true
    }

    pub fn failed(&mut self, token: u64, failure: &ConnectFailure, now: Instant) -> bool {
        if self.in_flight != Some(token) {
            return false;
        }
        self.in_flight = None;
        match failure {
            ConnectFailure::Attention { reason, error } => {
                self.phase = LinkPhase::Attention;
                self.attention = Some(*reason);
                self.error = Some(error.clone());
                self.retry_at = None;
            }
            ConnectFailure::Transient(error) => {
                self.attempt += 1;
                self.error = Some(error.clone());
                self.retry_at = Some(now + backoff_delay(self.attempt));
                self.phase = if self.established {
                    LinkPhase::Reconnecting
                } else {
                    LinkPhase::Offline
                };
            }
        }
        true
    }

    /// An online link dropped: input disabled, first retry after the first backoff step.
    pub fn lost(&mut self, error: RuntimeError, now: Instant) {
        self.established = true;
        self.in_flight = None;
        self.attempt = 1;
        self.error = Some(error);
        self.retry_at = Some(now + backoff_delay(1));
        self.phase = if self.cancelled {
            LinkPhase::Offline
        } else {
            LinkPhase::Reconnecting
        };
    }

    /// User cancellation: no retries, pending attempt results discarded, and no stale failure
    /// reason kept (spec 029: an explicit disconnect reads as disconnected, not as a failure).
    /// The remote server and its sessions are not touched.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.in_flight = None;
        self.retry_at = None;
        self.error = None;
        self.attention = None;
        self.phase = LinkPhase::Offline;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthAction {
    None,
    Ping,
    Expired,
}

/// Health of one negotiated endpoint connection.
#[derive(Debug, Clone)]
pub struct HealthMonitor {
    connected_at: Instant,
    last_received: Instant,
    last_sent: Instant,
    ping_sent_at: Option<Instant>,
    ready: bool,
}

impl HealthMonitor {
    pub fn new(now: Instant) -> Self {
        Self {
            connected_at: now,
            last_received: now,
            last_sent: now,
            ping_sent_at: None,
            ready: false,
        }
    }

    /// Any message received satisfies an outstanding probe.
    pub fn received(&mut self, now: Instant) {
        self.last_received = self.last_received.max(now);
        self.ping_sent_at = None;
    }

    /// Any message written by the client.
    pub fn sent(&mut self, now: Instant) {
        self.last_sent = self.last_sent.max(now);
    }

    /// The initial snapshot arrived.
    pub fn ready(&mut self) {
        self.ready = true;
    }

    pub fn ping_sent(&mut self, now: Instant) {
        self.ping_sent_at = Some(now);
        self.sent(now);
    }

    pub fn action(&self, now: Instant) -> HealthAction {
        let initial_expired =
            !self.ready && now.saturating_duration_since(self.connected_at) >= HEALTH_TIMEOUT;
        let probe_expired = self
            .ping_sent_at
            .is_some_and(|at| now.saturating_duration_since(at) >= HEALTH_TIMEOUT);
        if initial_expired || probe_expired {
            HealthAction::Expired
        } else if self.ping_sent_at.is_none()
            && now.saturating_duration_since(self.last_received) >= HEALTH_INTERVAL
        {
            HealthAction::Ping
        } else {
            HealthAction::None
        }
    }

    /// Time since the last traffic in either direction.
    pub fn traffic_silence(&self, now: Instant) -> std::time::Duration {
        now.saturating_duration_since(self.last_received.max(self.last_sent))
    }

    /// Whether a remote bridge with the idle option would have reaped this connection.
    pub fn bridge_idle_expired(&self, now: Instant) -> bool {
        self.traffic_silence(now) >= BRIDGE_IDLE_TIMEOUT
    }
}
