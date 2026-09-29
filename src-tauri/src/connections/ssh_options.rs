//! OpenSSH identity and command construction — the seam shared with spec 006 (SFTP).
//!
//! Everything that starts an OpenSSH process for a saved host goes through this module:
//! validated target (port of `remote/args.rs:validate_remote_target` plus the catalog rules
//! of `client/endpoint/catalog.rs`), validated session (`session::validate_name`; `default` is a
//! valid target, spec 034),
//! non-interactive options with strict host key checking (`remote/attach.rs:
//! apply_noninteractive_ssh_options`) and, for tests only, an isolated configuration that
//! never reads the user's `~/.ssh`. No option here accepts an unknown host key, prompts for a
//! password or lets the WebView pick arbitrary OpenSSH options or remote commands.

use std::fmt;
use std::path::PathBuf;
use std::process::Command;

use herdr_client::bootstrap::INHERITED_SESSION_VARS;
use herdr_client::{RuntimeError, SessionName};
use serde::{Deserialize, Serialize};

/// Program name of Herdr on the remote host (resolved by the remote login shell's PATH).
pub const REMOTE_HERDR: &str = "herdr";
/// Maximum target length (catalog rule).
pub const MAX_TARGET_BYTES: usize = 1024;
/// `ConnectTimeout` in seconds (engine saved-SSH policy).
pub const CONNECT_TIMEOUT_SECS: u32 = 10;

// ---------------------------------------------------------------------------------------
// Profile id
// ---------------------------------------------------------------------------------------

/// Endpoint profile id: 32 lowercase hex characters (same shape as the engine's ProfileId),
/// used as the `endpoint` of every qualified target of this host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileId(String);

impl ProfileId {
    pub fn parse(value: &str) -> Result<Self, RuntimeError> {
        if value.len() == 32
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(RuntimeError::new(
                "profile_id_invalid",
                "invalid profile identifier",
            ))
        }
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProfileId {
    type Error = RuntimeError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ProfileId> for String {
    fn from(value: ProfileId) -> Self {
        value.0
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------------------
// Target
// ---------------------------------------------------------------------------------------

/// OpenSSH destination (`host`, `user@host`, alias or `ssh://user@host:port`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SshTarget(String);

impl SshTarget {
    pub fn parse(target: &str) -> Result<Self, RuntimeError> {
        if target.is_empty() {
            return Err(RuntimeError::new("ssh_target_empty", "enter the SSH host"));
        }
        if target.starts_with('-') {
            return Err(RuntimeError::new(
                "ssh_target_option_like",
                "the SSH host cannot start with '-'",
            ));
        }
        if target.len() > MAX_TARGET_BYTES
            || target.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(RuntimeError::new(
                "ssh_target_invalid",
                "the SSH host contains invalid characters",
            ));
        }
        let authority = target.strip_prefix("ssh://").unwrap_or(target);
        if authority
            .rsplit_once('@')
            .is_some_and(|(userinfo, _)| userinfo.contains(':'))
        {
            return Err(RuntimeError::new(
                "ssh_target_password",
                "the SSH host cannot carry a password",
            ));
        }
        Ok(Self(target.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SshTarget {
    type Error = RuntimeError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SshTarget> for String {
    fn from(value: SshTarget) -> Self {
        value.0
    }
}

// ---------------------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------------------

/// Everything that identifies one SSH endpoint: profile id, target, optional port and the
/// remote named session. Constructed only through validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshIdentity {
    pub profile_id: ProfileId,
    pub target: SshTarget,
    pub port: Option<u16>,
    pub session: SessionName,
}

impl SshIdentity {
    pub fn new(
        profile_id: ProfileId,
        target: &str,
        port: Option<u16>,
        session: &str,
    ) -> Result<Self, RuntimeError> {
        let target = SshTarget::parse(target)?;
        if port == Some(0) {
            return Err(RuntimeError::new(
                "ssh_port_invalid",
                "the SSH port must be between 1 and 65535",
            ));
        }
        let session = SessionName::parse(session)?;
        Ok(Self {
            profile_id,
            target,
            port,
            session,
        })
    }

    /// Endpoint id used by qualified targets of this host.
    pub fn endpoint_id(&self) -> &str {
        self.profile_id.as_str()
    }
}

/// Test-only isolation: explicit identity and known_hosts, no user or system config, no agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolatedSshConfig {
    pub identity_file: PathBuf,
    pub user_known_hosts_file: PathBuf,
}

// ---------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------

/// The only remote commands the desktop runs. None of them installs, updates, restarts or
/// stops a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteHerdrCommand {
    /// Read-only probe: `herdr --session S status server --json`.
    ServerStatus,
    /// Visual endpoint lane over stdio.
    ClientBridge,
    /// One JSON API request per process over stdio.
    ApiBridge,
}

/// A fully built OpenSSH invocation (program + argv, no shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl OpenSshCommand {
    /// `std::process::Command` without the caller's Herdr session variables.
    pub fn to_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        for var in INHERITED_SESSION_VARS {
            command.env_remove(var);
        }
        command
    }
}

/// Non-interactive options shared by ssh and sftp.
pub fn noninteractive_options() -> Vec<String> {
    let mut args = Vec::new();
    for option in [
        "BatchMode=yes".to_owned(),
        "NumberOfPasswordPrompts=0".to_owned(),
        "StrictHostKeyChecking=yes".to_owned(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
        "ConnectionAttempts=1".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "ServerAliveCountMax=4".to_owned(),
    ] {
        args.push("-o".to_owned());
        args.push(option);
    }
    args
}

fn isolated_options(isolated: Option<&IsolatedSshConfig>) -> Vec<String> {
    let Some(isolated) = isolated else {
        return Vec::new();
    };
    vec![
        "-F".into(),
        "none".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        "IdentityAgent=none".into(),
        "-o".into(),
        "GlobalKnownHostsFile=none".into(),
        "-o".into(),
        format!(
            "UserKnownHostsFile={}",
            isolated.user_known_hosts_file.display()
        ),
        "-i".into(),
        isolated.identity_file.display().to_string(),
    ]
}

/// `ssh <options> -T [-p port] -- <target> <remote command line>`.
pub fn build_ssh(
    identity: &SshIdentity,
    isolated: Option<&IsolatedSshConfig>,
    remote: RemoteHerdrCommand,
) -> OpenSshCommand {
    build_ssh_with_binary(identity, isolated, REMOTE_HERDR, remote)
}

/// Same as [`build_ssh`] with the binary chosen by discovery (spec 029): the remote command line
/// runs `exec <path>` instead of the bare `herdr`.
pub fn build_ssh_with_binary(
    identity: &SshIdentity,
    isolated: Option<&IsolatedSshConfig>,
    binary_path: &str,
    remote: RemoteHerdrCommand,
) -> OpenSshCommand {
    build_ssh_line(
        identity,
        isolated,
        &remote_command_line_with(binary_path, &identity.session, remote),
    )
}

/// `ssh <options> -T [-p port] -- <target> <internally built script>`: discovery probes only.
/// The script never comes from the WebView.
pub fn build_ssh_script(
    identity: &SshIdentity,
    isolated: Option<&IsolatedSshConfig>,
    _script: &str,
) -> OpenSshCommand {
    // Match the TUI's `Ssh::sh_output`: the login shell only receives the fixed `/bin/sh -s`
    // command, while the discovery script itself is written to stdin. This is important for
    // saved profiles whose login shell is zsh: an unmatched glob must be interpreted by POSIX sh,
    // not by zsh before `/bin/sh` gets a chance to run it.
    build_ssh_line(identity, isolated, "/bin/sh -s")
}

fn build_ssh_line(
    identity: &SshIdentity,
    isolated: Option<&IsolatedSshConfig>,
    line: &str,
) -> OpenSshCommand {
    let mut args = noninteractive_options();
    args.extend(isolated_options(isolated));
    args.push("-T".into());
    if let Some(port) = identity.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    args.push("--".into());
    args.push(identity.target.as_str().to_owned());
    args.push(line.to_owned());
    OpenSshCommand {
        program: "ssh".into(),
        args,
    }
}

/// `sftp <options> [-P port] -- <target>` (consumed by spec 006; batch input is its concern).
pub fn build_sftp(identity: &SshIdentity, isolated: Option<&IsolatedSshConfig>) -> OpenSshCommand {
    let mut args = noninteractive_options();
    args.extend(isolated_options(isolated));
    if let Some(port) = identity.port {
        args.push("-P".into());
        args.push(port.to_string());
    }
    args.push("--".into());
    args.push(identity.target.as_str().to_owned());
    OpenSshCommand {
        program: "sftp".into(),
        args,
    }
}

/// `ssh <options> -T [-p port] -s -- <target> sftp`: the SFTP subsystem channel of spec 006.
///
/// Same non-interactive/strict host key options (and test isolation) as the Herdr lanes; the
/// remote side runs the `sftp` subsystem configured in its sshd, never a shell command line.
/// A working SSH terminal does not imply this subsystem exists on the host.
pub fn build_sftp_subsystem(
    identity: &SshIdentity,
    isolated: Option<&IsolatedSshConfig>,
) -> OpenSshCommand {
    let mut args = noninteractive_options();
    args.extend(isolated_options(isolated));
    args.push("-T".into());
    if let Some(port) = identity.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    args.push("-s".into());
    args.push("--".into());
    args.push(identity.target.as_str().to_owned());
    args.push("sftp".into());
    OpenSshCommand {
        program: "ssh".into(),
        args,
    }
}

/// Remote shell line. The session name is validated and quoted; the remote side does not
/// inherit the local XDG/Herdr environment, so the session is always explicit.
pub fn remote_command_line(session: &SessionName, command: RemoteHerdrCommand) -> String {
    remote_command_line_with(REMOTE_HERDR, session, command)
}

/// [`remote_command_line`] with the binary chosen by discovery (spec 029). The path was produced
/// by the desktop's own discovery (never by the WebView) and is quoted.
pub fn remote_command_line_with(
    binary_path: &str,
    session: &SessionName,
    command: RemoteHerdrCommand,
) -> String {
    let tail = match command {
        RemoteHerdrCommand::ServerStatus => "status server --json",
        RemoteHerdrCommand::ClientBridge => "remote-client-bridge",
        RemoteHerdrCommand::ApiBridge => "remote-api-bridge",
    };
    // The bare program name stays unquoted exactly as before; a discovered absolute path is
    // always quoted.
    let program = if binary_path == REMOTE_HERDR {
        REMOTE_HERDR.to_owned()
    } else {
        shell_quote(binary_path)
    };
    format!(
        "exec {program} --session {} {tail}",
        shell_quote(session.as_str())
    )
}

/// Endpoint requirement probe of one candidate (port of
/// `remote/attach.rs:RemoteExecutable::status_client_command`): the file must be executable and
/// answer `status client --json`. Read-only; no server is started.
pub fn binary_status_client_line(binary_path: &str) -> String {
    let quoted = shell_quote(binary_path);
    format!("test -x {quoted} && {quoted} status client --json")
}

/// POSIX single-quote quoting.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}
