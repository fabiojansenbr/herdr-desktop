//! Start-up classification and detached session start.
//!
//! Fatal bootstrap failures (invalid configuration, injected failure) make the executable
//! exit non-zero with the single line [`FATAL_LINE`] plus a stable code; no stack trace,
//! no environment values. A missing server is *not* fatal: the window opens in a
//! recoverable "desconectado" state.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::contracts::RuntimeError;
use crate::session::{herdr_config_dir, SessionName, SessionPaths, DEFAULT_SESSION_NAME};

/// Literal line printed to stderr on fatal bootstrap failure.
pub const FATAL_LINE: &str = "herdr-desktop: falha ao iniciar";

pub const ENV_SESSION: &str = "HERDR_DESKTOP_SESSION";
pub const ENV_FAIL_BOOTSTRAP: &str = "HERDR_DESKTOP_FAIL_BOOTSTRAP";
pub const ENV_SURFACE_TRACE: &str = "HERDR_DESKTOP_SURFACE_TRACE";
pub const ENV_HERDR_BIN: &str = "HERDR_DESKTOP_HERDR_BIN";
/// Overrides the engine configuration directory (default: the CLI's own resolution).
pub const ENV_CONFIG_DIR: &str = "HERDR_DESKTOP_CONFIG_DIR";

/// Wait budget of one detached session start (AC-016-01). The failure line names it.
pub const SESSION_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Literal cause shown when the engine binary is not on `PATH` (AC-016-02).
pub const HERDR_NOT_FOUND_LINE: &str = "herdr was not found in PATH — install Herdr";

/// Environment variables that must never leak from the desktop into a spawned engine,
/// so the new server cannot address the caller's session or panes.
pub const INHERITED_SESSION_VARS: &[&str] = &[
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_SESSION",
    "HERDR_WORKSPACE_ID",
    "HERDR_TAB_ID",
    "HERDR_PANE_ID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub config_dir: PathBuf,
    /// Session the window attaches to. The product always resolves one: `HERDR_DESKTOP_SESSION`
    /// when set, the engine's `default` session otherwise. `None` remains only for harnesses
    /// that mount the window without a Local host.
    pub session: Option<SessionName>,
    /// Zero-config mode: an absent session is started detached before connecting, like the TUI
    /// (AC-016-01). False whenever `HERDR_DESKTOP_SESSION` selected a named session.
    pub auto_start: bool,
    pub surface_trace: Option<PathBuf>,
    pub herdr_bin: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    /// Test hook: `HERDR_DESKTOP_FAIL_BOOTSTRAP=1`.
    InjectedFailure,
    /// `HERDR_DESKTOP_SESSION` failed validation (value intentionally not echoed).
    InvalidSession { code: String },
}

impl BootstrapError {
    pub fn code(&self) -> &str {
        match self {
            BootstrapError::InjectedFailure => "bootstrap_injected_failure",
            BootstrapError::InvalidSession { code } => code,
        }
    }

    /// Exit status for the process. Non-zero by contract.
    pub fn exit_code(&self) -> i32 {
        2
    }

    /// The full stderr line: literal prefix + stable code, nothing else.
    pub fn fatal_line(&self) -> String {
        format!("{FATAL_LINE} ({})", self.code())
    }
}

/// Reads the bootstrap configuration from an environment lookup.
pub fn bootstrap_from_env(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<BootstrapConfig, BootstrapError> {
    if env(ENV_FAIL_BOOTSTRAP).as_deref() == Some("1") {
        return Err(BootstrapError::InjectedFailure);
    }
    // AC-016-01: zero configuration attaches to the engine's default session, like the TUI;
    // an explicit override keeps the named-session behavior (including the detached start).
    let explicit = env(ENV_SESSION).filter(|raw| !raw.trim().is_empty());
    let auto_start = explicit.is_none();
    let session = match explicit {
        Some(raw) => Some(
            SessionName::parse(raw.trim())
                .map_err(|error| BootstrapError::InvalidSession { code: error.code })?,
        ),
        None => Some(SessionName::parse(DEFAULT_SESSION_NAME).expect("default is a valid name")),
    };
    let config_dir = env(ENV_CONFIG_DIR)
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| herdr_config_dir(env));
    let surface_trace = env(ENV_SURFACE_TRACE)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let herdr_bin = env(ENV_HERDR_BIN)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("herdr"));
    Ok(BootstrapConfig {
        config_dir,
        session,
        auto_start,
        surface_trace,
        herdr_bin,
    })
}

/// Whether the session's client socket currently accepts a connection.
pub fn session_available(paths: &SessionPaths) -> bool {
    crate::local::probe_socket(&paths.client_socket)
}

/// Starts the session's server detached (own session id on Unix, DETACHED_PROCESS on Windows)
/// with stdio to null and the caller's session variables removed. Closing the desktop later
/// does not affect the spawned engine. Same form as the TUI bootstrap: `herdr server` for the
/// default session (no argument, which resolves to the default paths) and
/// `herdr --session <name> server` for a named one. A missing binary is the literal line of
/// AC-016-02.
pub fn start_session_detached(
    herdr_bin: &Path,
    session: &SessionName,
    startup_cwd: Option<&Path>,
) -> Result<u32, RuntimeError> {
    let mut command = Command::new(herdr_bin);
    if !session.is_default() {
        command.arg("--session").arg(session.as_str());
    }
    command
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for var in INHERITED_SESSION_VARS {
        command.env_remove(var);
    }
    if let Some(cwd) = startup_cwd {
        command.env("HERDR_STARTUP_CWD", cwd);
    }
    detach(&mut command);
    let child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            RuntimeError::new("herdr_not_found", HERDR_NOT_FOUND_LINE)
                .retryable()
                .with_endpoint(crate::contracts::LOCAL_ENDPOINT)
        } else {
            RuntimeError::from_io_kind(error.kind(), "could not start the Herdr server")
                .with_endpoint(crate::contracts::LOCAL_ENDPOINT)
        }
    })?;
    Ok(child.id())
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid only touches the child's own session; no allocation in the child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

/// Timeout error of [`wait_for_session`] / [`ensure_session_running`]: stable code plus the
/// literal cause naming the budget actually used (AC-016-02).
pub fn server_timeout_error(timeout: Duration) -> RuntimeError {
    RuntimeError::new(
        "server_start_timeout",
        format!(
            "the server did not answer within {} s",
            timeout.as_secs().max(1)
        ),
    )
    .retryable()
    .with_endpoint(crate::contracts::LOCAL_ENDPOINT)
}

/// Polls until the client socket accepts a connection or the timeout elapses.
pub fn wait_for_session(paths: &SessionPaths, timeout: Duration) -> Result<(), RuntimeError> {
    let deadline = Instant::now() + timeout;
    loop {
        if session_available(paths) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(server_timeout_error(timeout));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Zero-config bootstrap (AC-016-01/02): connects to the session's server if it is already
/// accepting, otherwise starts it detached (same form as the TUI) and waits up to `timeout`.
/// The failure is retryable and names the literal cause; nothing else is attempted.
pub fn ensure_session_running(
    herdr_bin: &Path,
    config_dir: &Path,
    session: &SessionName,
    startup_cwd: Option<&Path>,
    timeout: Duration,
) -> Result<(), RuntimeError> {
    let paths = SessionPaths::for_session(config_dir, session);
    if session_available(&paths) {
        return Ok(());
    }
    start_session_detached(herdr_bin, session, startup_cwd)?;
    wait_for_session(&paths, timeout)
}

/// Convenience used by the executable: env lookup backed by the process environment.
pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}
