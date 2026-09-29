//! Remote Herdr binary discovery — the same candidates, in the same order, as the TUI
//! (`remote/attach.rs:remote_binary_candidates`, `known_remote_binary_candidate_script`,
//! `remote_binary_on_path_any`, `remote_binary_override_path`,
//! `remote_binary_supports_endpoint_requirement`).
//!
//! Candidate order: the binary on PATH, then the known install paths of the platform (including
//! `$HOME/.local/bin/herdr`), with the configuration override replacing the list when set. Each
//! candidate is tested against the endpoint requirement (`<binary> status client --json`:
//! generation 1 with surface interest, presentation effects fence and health check); the first
//! that serves is used. `HerdrMissing` exists only when no candidate exists; candidates that do
//! not serve are reported as an outdated Herdr with the version found.
//!
//! Nothing here installs, updates, starts or stops anything: the desktop only reads.

use herdr_client::protocol::endpoint::{
    ENDPOINT_PROTOCOL_GENERATION, HEALTH_CHECK_CAPABILITY, PRESENTATION_EFFECTS_FENCE_CAPABILITY,
    SURFACE_INTEREST_CAPABILITY,
};
use serde::{Deserialize, Serialize};

use super::failure::{AttentionReason, ConnectFailure};

/// Same override variable as the TUI (`remote/attach.rs:REMOTE_BINARY_ENV_VAR`).
pub const REMOTE_BINARY_ENV_VAR: &str = "HERDR_REMOTE_BINARY";
const MAX_BINARY_PATH_BYTES: usize = 1024;

/// The remote binary chosen for one host, as the host tooltip shows it (`spec 029`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteBinary {
    /// Absolute shell path used in the remote command lines (quoted when composed).
    pub path: String,
    /// Version reported by `status client --json`; `None` when it could not be read.
    pub version: Option<String>,
}

impl RemoteBinary {
    /// The TUI's own first candidate: `herdr` resolved by the remote login shell's PATH.
    pub fn on_path() -> Self {
        Self {
            path: super::ssh_options::REMOTE_HERDR.to_owned(),
            version: None,
        }
    }
}

/// Configuration override, never from the WebView: an absolute path without spaces or control
/// characters. An invalid value is not a remote command and simply does not take part.
pub fn configured_remote_binary(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let value = env(REMOTE_BINARY_ENV_VAR)?;
    if value.is_empty()
        || value.len() > MAX_BINARY_PATH_BYTES
        || !value.starts_with('/')
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    Some(value)
}

/// Candidate paths from the PATH probe and the known-path script, in the TUI's order: absolute
/// paths only, mise shims skipped (the TUI filters them), each path once.
pub fn discovery_candidates(path_output: &str, script_output: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for line in path_output.lines().chain(script_output.lines()) {
        let path = line.trim();
        if !path.starts_with('/') || path.ends_with("/mise/shims/herdr") {
            continue;
        }
        if !candidates.iter().any(|candidate| candidate == path) {
            candidates.push(path.to_owned());
        }
    }
    candidates
}

/// Port of `remote/attach.rs:known_remote_binary_candidate_script`: only emits executable files,
/// `$HOME/.local/bin/herdr` first, then the platform paths chosen by `uname`, the mise install
/// versions (wildcard: the desktop does not know the engine version of the remote host), nix and
/// per-user profiles. The platform check runs inside the same remote shell, so the SSH round
/// trips stay the same regardless of the remote OS.
pub fn known_binary_candidate_script() -> String {
    r#"# zsh may run the ssh command before the POSIX interpreter receives it. Keep this harmless
# under sh and disable zsh's unmatched-glob failure for callers that inline this script.
setopt NULL_GLOB 2>/dev/null || :
home=${HOME:-}
user=${USER:-}
emit() {
    path=$1
    if [ -n "$path" ] && [ -x "$path" ]; then
        printf '%s\n' "$path"
    fi
}
if [ -n "$home" ]; then
    emit "$home/.local/bin/herdr"
fi
case "$(uname -s 2>/dev/null)" in
    Darwin)
        emit "/opt/homebrew/bin/herdr"
        emit "/usr/local/bin/herdr"
        ;;
    Linux)
        emit "/home/linuxbrew/.linuxbrew/bin/herdr"
        ;;
esac
if [ -n "$home" ]; then
    for candidate in \
        "$home"/.local/share/mise/installs/herdr/*/bin/herdr \
        "$home"/.local/share/mise/installs/herdr/*/herdr \
        "$home"/.local/share/mise/installs/github-ogulcancelik-herdr/*/herdr \
        "$home/.nix-profile/bin/herdr"
    do
        emit "$candidate"
    done
fi
if [ -n "$user" ]; then
    emit "/etc/profiles/per-user/$user/bin/herdr"
fi
emit "/nix/var/nix/profiles/default/bin/herdr"
emit "/run/current-system/sw/bin/herdr"
"#
    .to_owned()
}

/// Client capabilities announced by `<binary> status client --json` (the TUI's
/// `RemoteClientStatusJson`), reduced to what the endpoint requirement reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientStatus {
    pub version: Option<String>,
    pub generation: Option<u32>,
    pub capabilities: Vec<String>,
}

impl ClientStatus {
    /// Port of `RemoteClientStatusJson::supports_endpoint_requirement` for POSIX hosts (the
    /// remote Windows target is out of scope for the desktop, spec 029).
    pub fn supports_endpoint_requirement(&self) -> bool {
        self.generation == Some(ENDPOINT_PROTOCOL_GENERATION)
            && [
                SURFACE_INTEREST_CAPABILITY,
                PRESENTATION_EFFECTS_FENCE_CAPABILITY,
                HEALTH_CHECK_CAPABILITY,
            ]
            .iter()
            .all(|required| {
                self.capabilities
                    .iter()
                    .any(|capability| capability == required)
            })
    }
}

#[derive(Debug, Deserialize)]
struct ClientStatusJson {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    endpoint_protocol_generation: Option<u32>,
    #[serde(default)]
    endpoint_capabilities: Vec<String>,
}

/// Last parsable status line with any capability evidence (the TUI's
/// `parse_client_status_json`); a message without any of the fields is not a status.
pub fn parse_client_status(stdout: &str) -> Option<ClientStatus> {
    stdout
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<ClientStatusJson>(line).ok())
        .find(|status| {
            status.version.is_some()
                || status.endpoint_protocol_generation.is_some()
                || !status.endpoint_capabilities.is_empty()
        })
        .map(|status| ClientStatus {
            version: status.version.filter(|v| !v.trim().is_empty()),
            generation: status.endpoint_protocol_generation,
            capabilities: status.endpoint_capabilities,
        })
}

/// No candidate exists at all (`remote/attach.rs` semantics: nothing to run on the host).
pub fn missing_failure(endpoint: &str) -> ConnectFailure {
    ConnectFailure::attention(endpoint, AttentionReason::HerdrMissing)
}

/// Candidates exist but none serves the endpoint requirement; the version of the first readable
/// candidate travels in the message.
pub fn outdated_failure(endpoint: &str, version: Option<&str>) -> ConnectFailure {
    let message = match version {
        Some(version) => format!(
            "the host's Herdr is outdated (version {version}) and does not serve endpoint generation 1"
        ),
        None => "the host's Herdr is outdated and does not serve endpoint generation 1".to_owned(),
    };
    ConnectFailure::attention_message(endpoint, AttentionReason::HerdrOutdated, message)
}
