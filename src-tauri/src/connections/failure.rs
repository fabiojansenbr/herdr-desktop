//! Classification of connection failures into "precisa de atenção" (explicit configuration,
//! never retried automatically) and transient failures (retried with backoff).
//!
//! Stderr is inspected only to pick a stable code; it is never echoed to the WebView and an
//! unknown message is never treated as success.

use std::io;

use herdr_client::protocol::endpoint::{
    ENDPOINT_PROTOCOL_GENERATION, HEALTH_CHECK_CAPABILITY, SURFACE_INTEREST_CAPABILITY,
};
use herdr_client::{Negotiated, RuntimeError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    HostKeyUnknown,
    HostKeyChanged,
    AuthenticationRequired,
    SshUnavailable,
    HerdrMissing,
    /// A candidate exists but does not serve the endpoint requirement (spec 029 edge case).
    HerdrOutdated,
    ServerNotRunning,
    ServerIncompatible,
}

impl AttentionReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::HostKeyUnknown => "ssh_host_key_unknown",
            Self::HostKeyChanged => "ssh_host_key_changed",
            Self::AuthenticationRequired => "ssh_authentication_required",
            Self::SshUnavailable => "ssh_unavailable",
            Self::HerdrMissing => "remote_herdr_missing",
            Self::HerdrOutdated => "remote_herdr_outdated",
            Self::ServerNotRunning => "remote_server_not_running",
            Self::ServerIncompatible => "remote_server_incompatible",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::HostKeyUnknown => "this host's key is not known",
            Self::HostKeyChanged => "this host's key changed since the last connection",
            Self::AuthenticationRequired => {
                "the host requires interactive authentication (password/MFA) or refused the key"
            }
            Self::SshUnavailable => "the OpenSSH client was not found on this computer",
            Self::HerdrMissing => "Herdr was not found on the remote host",
            Self::HerdrOutdated => {
                "the host's Herdr is outdated and does not serve endpoint generation 1"
            }
            Self::ServerNotRunning => {
                "this session's Herdr server is not running on the remote host"
            }
            Self::ServerIncompatible => {
                "the remote Herdr server is not compatible with endpoint generation 1"
            }
        }
    }

    /// Explicit configuration offered to the user. The desktop performs none of it.
    pub fn guidance(self) -> &'static str {
        match self {
            Self::HostKeyUnknown => "Confirm the fingerprint with the administrator and add the key to known_hosts in a terminal (for example, by connecting once with ssh). Then use Try again.",
            Self::HostKeyChanged => "Check whether the key change is expected before updating known_hosts by hand. The desktop does not accept the new key.",
            Self::AuthenticationRequired => "Load the key into ssh-agent or complete the MFA in a terminal (for example, with ControlMaster configured). Background connections never ask for a password.",
            Self::SshUnavailable => "Install the OpenSSH client and make sure ssh is in PATH.",
            Self::HerdrMissing => "Install or update Herdr on the host with herdr --remote in an interactive terminal.",
            Self::HerdrOutdated => "Update Herdr on the host with herdr --remote in an interactive terminal and use Try again. The desktop neither installs nor updates it.",
            Self::ServerNotRunning => "Start the session on the host (herdr --session <name> server) and use Try again. The desktop does not start remote servers.",
            Self::ServerIncompatible => "Update or restart the remote Herdr server explicitly in a terminal. The desktop neither installs nor restarts servers.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFailure {
    Attention {
        reason: AttentionReason,
        error: RuntimeError,
    },
    Transient(RuntimeError),
}

impl ConnectFailure {
    pub fn attention(endpoint: &str, reason: AttentionReason) -> Self {
        Self::attention_message(endpoint, reason, reason.message().to_owned())
    }

    /// Attention with a message that carries a measured fact (e.g. the outdated version);
    /// stderr is never echoed.
    pub fn attention_message(endpoint: &str, reason: AttentionReason, message: String) -> Self {
        Self::Attention {
            reason,
            error: RuntimeError::new(reason.code(), message).with_endpoint(endpoint),
        }
    }

    /// Attention reason of this failure, when any.
    pub fn reason(&self) -> Option<AttentionReason> {
        match self {
            Self::Attention { reason, .. } => Some(*reason),
            Self::Transient(_) => None,
        }
    }

    pub fn transient(endpoint: &str, code: &str, message: &str) -> Self {
        Self::Transient(
            RuntimeError::new(code, message)
                .retryable()
                .with_endpoint(endpoint),
        )
    }

    pub fn error(&self) -> &RuntimeError {
        match self {
            Self::Attention { error, .. } | Self::Transient(error) => error,
        }
    }
}

/// Failure of an OpenSSH process (exit status + captured stderr).
pub fn classify_ssh_failure(endpoint: &str, exit: Option<i32>, stderr: &str) -> ConnectFailure {
    let text = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    if has(&["remote host identification has changed"]) {
        return ConnectFailure::attention(endpoint, AttentionReason::HostKeyChanged);
    }
    if has(&[
        "host key verification failed",
        "no matching host key",
        "host key is known for",
    ]) {
        return ConnectFailure::attention(endpoint, AttentionReason::HostKeyUnknown);
    }
    if has(&[
        "permission denied",
        "keyboard-interactive",
        "too many authentication failures",
        "verification code",
        "password:",
    ]) {
        return ConnectFailure::attention(endpoint, AttentionReason::AuthenticationRequired);
    }
    if exit == Some(127) || (has(&["herdr"]) && has(&["command not found", "not found"])) {
        return ConnectFailure::attention(endpoint, AttentionReason::HerdrMissing);
    }
    if has(&[
        "needs one final update",
        "unsupported remote client bridge option",
        "install or update",
    ]) {
        return ConnectFailure::attention(endpoint, AttentionReason::ServerIncompatible);
    }
    match exit {
        None => {
            ConnectFailure::transient(endpoint, "ssh_terminated", "the ssh process was terminated")
        }
        Some(0) => ConnectFailure::transient(
            endpoint,
            "ssh_unexpected_exit",
            "the ssh process exited without completing the connection",
        ),
        Some(255)
            if has(&[
                "connection refused",
                "timed out",
                "could not resolve",
                "no route to host",
                "network is unreachable",
                "connection closed",
                "connection reset",
                "broken pipe",
            ]) =>
        {
            // Explicit result of ConnectTimeout=10 with ConnectionAttempts=1 (AC-031-02).
            ConnectFailure::transient(endpoint, "ssh_unreachable", "host unreachable (timeout)")
        }
        Some(255) => ConnectFailure::transient(endpoint, "ssh_failed", "the SSH connection failed"),
        Some(_) => ConnectFailure::transient(
            endpoint,
            "remote_command_failed",
            "the remote Herdr command failed",
        ),
    }
}

/// Failure to start the local OpenSSH process.
pub fn classify_spawn_error(endpoint: &str, error: &io::Error) -> ConnectFailure {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
            ConnectFailure::attention(endpoint, AttentionReason::SshUnavailable)
        }
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            ConnectFailure::transient(endpoint, "timeout", "the SSH host did not answer in time")
        }
        _ => ConnectFailure::transient(endpoint, "ssh_failed", "could not run ssh"),
    }
}

#[derive(Debug, Deserialize)]
struct ServerStatusJson {
    running: bool,
    #[serde(default)]
    capabilities: Option<ServerCapabilitiesJson>,
    #[serde(default)]
    endpoint_compatible: Option<bool>,
    #[serde(default)]
    restart_needed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ServerCapabilitiesJson {
    #[serde(default)]
    endpoint_protocol_generation: Option<u32>,
    #[serde(default)]
    surface_interest: bool,
    #[serde(default)]
    health_check: bool,
}

/// Output of `herdr --session S status server --json`. A stopped server is attention: the
/// client bridge would otherwise start a daemon on its own.
pub fn check_server_status(endpoint: &str, stdout: &str) -> Result<(), ConnectFailure> {
    let parsed = stdout
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .find_map(|l| serde_json::from_str::<ServerStatusJson>(l.trim()).ok())
        .or_else(|| serde_json::from_str::<ServerStatusJson>(stdout.trim()).ok());
    let Some(status) = parsed else {
        return Err(ConnectFailure::attention(
            endpoint,
            AttentionReason::ServerIncompatible,
        ));
    };
    if !status.running {
        return Err(ConnectFailure::attention(
            endpoint,
            AttentionReason::ServerNotRunning,
        ));
    }
    let capable = status.capabilities.as_ref().is_some_and(|c| {
        c.endpoint_protocol_generation == Some(ENDPOINT_PROTOCOL_GENERATION)
            && c.surface_interest
            && c.health_check
    });
    if !capable || status.endpoint_compatible == Some(false) || status.restart_needed == Some(true)
    {
        return Err(ConnectFailure::attention(
            endpoint,
            AttentionReason::ServerIncompatible,
        ));
    }
    Ok(())
}

/// Remote endpoints must negotiate generation 1 with surface interest and health check.
pub fn check_negotiated(endpoint: &str, negotiated: &Negotiated) -> Result<(), ConnectFailure> {
    let has = |cap: &str| negotiated.capabilities.iter().any(|c| c == cap);
    if negotiated.generation != ENDPOINT_PROTOCOL_GENERATION
        || !has(SURFACE_INTEREST_CAPABILITY)
        || !has(HEALTH_CHECK_CAPABILITY)
    {
        return Err(ConnectFailure::attention(
            endpoint,
            AttentionReason::ServerIncompatible,
        ));
    }
    Ok(())
}
