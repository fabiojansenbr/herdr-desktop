//! Disposable Local and SSH host fixtures for spec 007 (AC-007-04 flow infrastructure).
//!
//! Each host runs a real engine in its own private namespace (HOME, XDG_*, HERDR_CONFIG_PATH,
//! PATH) with the same pane id `w1:p1` and distinct sessions, boots and project roots. The SSH
//! host is reached through an unprivileged loopback sshd whose `ForceCommand` only accepts the
//! exact command lines the product builds (`build_ssh`) for the fixture's own session, plus the
//! configured SFTP subsystem; everything else is rejected. Every owned process is written to a
//! ledger as soon as it is spawned, so partial startup failures can be verified afterwards.
//! This module returns raw observations; it never evaluates the AC.

#[path = "session.rs"]
pub mod session;

use std::collections::BTreeMap;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub use session::{disposable, may_stop, pair_problems, HostFixture, PREFIX, SHARED_PANE};

use herdr_client::api::ApiClient;
use herdr_client::contracts::{ConnectOptions, SurfaceGeometry};
use herdr_client::{LocalGateway, RuntimeGateway, SessionName};
use herdr_desktop::bridge::ssh::{OpenSshRunner, SshConnector};
use herdr_desktop::connections::ssh_options::{
    remote_command_line, IsolatedSshConfig, ProfileId, RemoteHerdrCommand, SshIdentity,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Prefix for all disposable sessions created by this remote fixture module.
pub const SESSION_PREFIX: &str = "hd007-remote-";
/// Branch of the git repository `local-project` is (spec 010 visual-frame: the status bar shows the
/// branch of the focused workspace; "—" would be indistinguishable from no branch at all).
pub const LOCAL_GIT_BRANCH: &str = "hd010-branch";

/// Path to the reference Herdr engine compiled from commit 03749ae.
pub const REFERENCE_ENGINE_PATH: &str =
    "/home/user/Projects/herdr-desktop/.local/bin/herdr-03749ae";

/// Expected SHA-256 digest of the reference engine binary (exact, 64 hex characters).
pub const REFERENCE_ENGINE_SHA256: &str =
    "e23f88bcdbed155880f68475e6f232217103049baf310e7684016637c4930e1c";

/// Path to the legacy host binary (Herdr 0.9.0 without remote-api-bridge).
pub const LEGACY_ENGINE_PATH: &str = "/usr/bin/herdr";

/// Session variables that must never leak from the caller into fixture commands.
pub const INHERITED_SESSION_VARS: &[&str] = &[
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_SESSION",
    "HERDR_WORKSPACE_ID",
    "HERDR_TAB_ID",
    "HERDR_PANE_ID",
];

/// Conservative bound for a Unix socket path (sun_path is 108 bytes on Linux).
pub const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Default wait for a freshly spawned engine to answer `pane list`.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Longest session suffix this module generates: `<pid:10>-<nonce:6>-<tag:3>`.
const MAX_SESSION_SUFFIX: usize = 21;
/// Longest host directory component used under the workdir.
const LONGEST_HOST_DIR: &str = "h-leg";

// ---------------------------------------------------------------------------------------
// Safe shell quoting and command helpers
// ---------------------------------------------------------------------------------------

/// POSIX single-quote quoting to avoid insecure interpolation.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Constructs a remote execution command line for herdr.
pub fn remote_exec_cmd(session: &str, subcmd: &str) -> String {
    format!("exec herdr --session {} {subcmd}", shell_quote(session))
}

fn current_user() -> Result<String, String> {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return Ok(user);
        }
    }
    let out = Command::new("id")
        .arg("-un")
        .output()
        .map_err(|e| format!("id -un: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn first_existing(candidates: &[&'static str]) -> Option<&'static str> {
    candidates.iter().copied().find(|p| Path::new(p).exists())
}

// ---------------------------------------------------------------------------------------
// SHA-256 verification
// ---------------------------------------------------------------------------------------

/// Computes the SHA-256 digest of a file using `sha256sum`.
pub fn compute_sha256(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Err(format!("file does not exist: {}", path.display()));
    }
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("failed to run sha256sum on {}: {e}", path.display()))?;
    if !out.status.success() {
        return Err(format!(
            "sha256sum failed with exit {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hash = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| "sha256sum produced empty output".to_string())?
        .to_ascii_lowercase();
    Ok(hash)
}

/// Exact comparison against the pinned digest (no prefixes, no empty digests).
pub fn pinned_sha256_matches(actual: &str) -> bool {
    actual.len() == REFERENCE_ENGINE_SHA256.len() && actual == REFERENCE_ENGINE_SHA256
}

/// Verifies that the given reference binary matches the pinned sha256 digest.
pub fn verify_reference_binary(path: &Path) -> Result<(), String> {
    let actual = compute_sha256(path)?;
    if !pinned_sha256_matches(&actual) {
        return Err(format!(
            "reference binary sha256 mismatch at {}: expected {REFERENCE_ENGINE_SHA256}, got {actual}",
            path.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Process tracking via PID and starttime
// ---------------------------------------------------------------------------------------

/// Reads the starttime (field 22) of a process from `/proc/<pid>/stat`.
pub fn get_proc_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(')')?.1;
    tail.split_whitespace().nth(19)?.parse::<u64>().ok()
}

/// Whether a process is alive (not a zombie) and still has the expected starttime.
pub fn is_proc_alive(pid: u32, expected_starttime: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, tail)) = stat.rsplit_once(')') else {
        return false;
    };
    let tokens: Vec<&str> = tail.split_whitespace().collect();
    if tokens.len() < 20 || tokens[0] == "Z" {
        return false;
    }
    tokens[19].parse::<u64>().ok() == Some(expected_starttime)
}

/// Terminates a process only if its starttime matches; returns whether it is gone.
pub fn terminate_proc(pid: u32, expected_starttime: u64) -> bool {
    if !is_proc_alive(pid, expected_starttime) {
        return false;
    }
    for (signal, wait) in [("-TERM", 1500), ("-KILL", 1500)] {
        if !is_proc_alive(pid, expected_starttime) {
            break;
        }
        let _ = Command::new("kill")
            .args([signal, &pid.to_string()])
            .stderr(Stdio::null())
            .status();
        let deadline = Instant::now() + Duration::from_millis(wait);
        while Instant::now() < deadline && is_proc_alive(pid, expected_starttime) {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    !is_proc_alive(pid, expected_starttime)
}

/// Finds child and descendant processes of a given root PID.
pub fn find_descendants(root: u32) -> Vec<u32> {
    let mut table: BTreeMap<u32, u32> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some(tail) = stat.rsplit_once(')').map(|(_, t)| t) else {
            continue;
        };
        if let Some(ppid) = tail
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse::<u32>().ok())
        {
            table.insert(pid, ppid);
        }
    }
    let mut found = vec![root];
    let mut i = 0;
    while i < found.len() {
        let parent = found[i];
        for (&pid, &ppid) in &table {
            if ppid == parent && !found.contains(&pid) {
                found.push(pid);
            }
        }
        i += 1;
    }
    found.remove(0);
    found
}

/// Live descendants of `root` with their starttimes.
pub fn descendants_with_starttime(root: u32, owner: &str) -> Vec<TrackedProcess> {
    find_descendants(root)
        .into_iter()
        .filter_map(|pid| {
            get_proc_starttime(pid).map(|starttime| TrackedProcess {
                pid,
                starttime,
                name: format!("{owner}-descendant-{pid}"),
            })
        })
        .collect()
}

/// Finds a free TCP port on 127.0.0.1.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("free port on 127.0.0.1")
}

/// Checks whether a TCP port is currently listening.
pub fn port_listening(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// Whether a Unix socket accepts connections.
pub fn socket_connectable(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Discovers the standard OpenSSH sftp-server binary.
pub fn find_sftp_server_bin() -> Option<PathBuf> {
    first_existing(&[
        "/usr/lib/ssh/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/sftp-server",
        "/usr/libexec/openssh/sftp-server",
    ])
    .map(PathBuf::from)
}

/// Tracked process record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedProcess {
    pub pid: u32,
    pub starttime: u64,
    pub name: String,
}

/// Kills an owned process when dropped unless defused (startup guard).
pub struct PendingProcessGuard {
    pub pid: u32,
    pub starttime: u64,
    pub defused: bool,
}

impl PendingProcessGuard {
    pub fn new(pid: u32, starttime: u64) -> Self {
        Self {
            pid,
            starttime,
            defused: false,
        }
    }

    pub fn defuse(&mut self) {
        self.defused = true;
    }
}

impl Drop for PendingProcessGuard {
    fn drop(&mut self) {
        if !self.defused {
            for d in descendants_with_starttime(self.pid, "pending") {
                terminate_proc(d.pid, d.starttime);
            }
            terminate_proc(self.pid, self.starttime);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Ledger of owned processes (written at spawn time)
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerEntry {
    Server {
        session: String,
        namespace: PathBuf,
        engine: PathBuf,
        pid: u32,
        starttime: u64,
    },
    Sshd {
        name: String,
        pid: u32,
        starttime: u64,
        port: u16,
    },
    Descendant {
        owner: String,
        pid: u32,
        starttime: u64,
    },
}

/// Append-only JSON-lines ledger inside the fixture workdir.
#[derive(Debug, Clone)]
pub struct Ledger {
    pub path: PathBuf,
}

impl Ledger {
    pub fn in_workdir(workdir: &Path) -> Self {
        Self {
            path: workdir.join("fixture-ledger.jsonl"),
        }
    }

    pub fn append(&self, entry: &LedgerEntry) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("ledger {}: {e}", self.path.display()))?;
        let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        writeln!(file, "{line}").map_err(|e| format!("ledger write: {e}"))
    }

    pub fn read(&self) -> Result<Vec<LedgerEntry>, String> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("ledger {}: {e}", self.path.display()))?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| format!("ledger line {l:?}: {e}")))
            .collect()
    }
}

/// Independent post-cleanup verification of every ledger entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LedgerVerification {
    pub entries: usize,
    pub servers: usize,
    pub sshd: usize,
    pub descendants: usize,
    pub alive: Vec<String>,
    pub listening_ports: Vec<u16>,
    pub running_sessions: Vec<String>,
    pub errors: Vec<String>,
}

impl LedgerVerification {
    pub fn is_clean(&self) -> bool {
        self.alive.is_empty()
            && self.listening_ports.is_empty()
            && self.running_sessions.is_empty()
            && self.errors.is_empty()
    }
}

pub fn verify_ledger_clean(ledger: &Ledger) -> LedgerVerification {
    let mut v = LedgerVerification::default();
    let entries = match ledger.read() {
        Ok(entries) => entries,
        Err(e) => {
            v.errors.push(e);
            return v;
        }
    };
    v.entries = entries.len();
    for entry in entries {
        match entry {
            LedgerEntry::Server {
                session,
                namespace,
                engine,
                pid,
                starttime,
            } => {
                v.servers += 1;
                if is_proc_alive(pid, starttime) {
                    v.alive.push(format!("server {session} pid {pid}"));
                }
                let ns = HostNamespace::new(namespace, engine);
                match ns.session_running(&session) {
                    Ok(false) => {}
                    Ok(true) => v.running_sessions.push(session),
                    Err(e) => v.errors.push(e),
                }
            }
            LedgerEntry::Sshd {
                name,
                pid,
                starttime,
                port,
            } => {
                v.sshd += 1;
                if is_proc_alive(pid, starttime) {
                    v.alive.push(format!("sshd {name} pid {pid}"));
                }
                if port_listening(port) {
                    v.listening_ports.push(port);
                }
            }
            LedgerEntry::Descendant {
                owner,
                pid,
                starttime,
            } => {
                v.descendants += 1;
                if is_proc_alive(pid, starttime) {
                    v.alive.push(format!("descendant of {owner} pid {pid}"));
                }
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------

/// Named points where `start_pair` can be made to fail on purpose (partial startup proof).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStage {
    AfterLocalSession,
    AfterSshd,
    AfterSshSession,
}

#[derive(Debug, Clone)]
pub struct RemoteFixtureConfig {
    pub workdir: PathBuf,
    pub reference_binary: PathBuf,
    pub legacy_binary: PathBuf,
    pub session_prefix: String,
    pub sftp_server_binary: Option<PathBuf>,
    pub agent_script: PathBuf,
    pub ready_timeout: Duration,
    pub induced_failure: Option<StartupStage>,
}

/// The deterministic helper shipped with the fixture (installed as `pi` in each namespace).
pub fn default_agent_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fidelity-native/remote-fixtures/deterministic-agent.sh")
}

impl RemoteFixtureConfig {
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
            reference_binary: PathBuf::from(REFERENCE_ENGINE_PATH),
            legacy_binary: PathBuf::from(LEGACY_ENGINE_PATH),
            session_prefix: SESSION_PREFIX.to_string(),
            sftp_server_binary: find_sftp_server_bin(),
            agent_script: default_agent_script(),
            ready_timeout: DEFAULT_READY_TIMEOUT,
            induced_failure: None,
        }
    }

    pub fn with_reference_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.reference_binary = path.into();
        self
    }

    pub fn with_legacy_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.legacy_binary = path.into();
        self
    }

    pub fn with_session_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.session_prefix = prefix.into();
        self
    }

    pub fn with_sftp_server_binary(mut self, path: Option<PathBuf>) -> Self {
        self.sftp_server_binary = path;
        self
    }

    pub fn with_agent_script(mut self, script: impl Into<PathBuf>) -> Self {
        self.agent_script = script.into();
        self
    }

    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    pub fn with_induced_failure(mut self, stage: StartupStage) -> Self {
        self.induced_failure = Some(stage);
        self
    }

    /// Longest API socket path any host of this config can produce.
    pub fn longest_socket_path_bytes(&self) -> usize {
        self.workdir.as_os_str().len()
            + format!("/{LONGEST_HOST_DIR}/cfg/herdr/sessions/").len()
            + self.session_prefix.len()
            + MAX_SESSION_SUFFIX
            + "/herdr.sock".len()
    }

    /// Structural validation (no machine prerequisites).
    pub fn validate_structure(&self) -> Result<(), String> {
        if !self.workdir.is_absolute() {
            return Err(format!(
                "workdir must be absolute: {}",
                self.workdir.display()
            ));
        }
        if self
            .workdir
            .to_string_lossy()
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"' || c == '\\')
        {
            return Err(format!(
                "workdir must not contain whitespace or quotes: {}",
                self.workdir.display()
            ));
        }
        if !self.session_prefix.starts_with(SESSION_PREFIX) {
            return Err(format!(
                "session_prefix must start with '{SESSION_PREFIX}', got '{}'",
                self.session_prefix
            ));
        }
        if self
            .session_prefix
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        {
            return Err(format!(
                "session_prefix contains invalid characters: '{}'",
                self.session_prefix
            ));
        }
        let longest = self.longest_socket_path_bytes();
        if longest > MAX_SOCKET_PATH_BYTES {
            return Err(format!(
                "workdir too long: socket paths could reach {longest} bytes (max {MAX_SOCKET_PATH_BYTES})"
            ));
        }
        Ok(())
    }

    /// Full preflight: structure, pinned reference digest, legacy engine and helper present.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_structure()?;
        if !self.reference_binary.exists() {
            return Err(format!(
                "reference binary does not exist at {}",
                self.reference_binary.display()
            ));
        }
        verify_reference_binary(&self.reference_binary)?;
        if !self.legacy_binary.exists() {
            return Err(format!(
                "legacy binary does not exist at {}",
                self.legacy_binary.display()
            ));
        }
        if !self.agent_script.is_file() {
            return Err(format!(
                "agent helper does not exist at {}",
                self.agent_script.display()
            ));
        }
        Ok(())
    }

    fn fail_if(&self, stage: StartupStage) -> Result<(), String> {
        if self.induced_failure == Some(stage) {
            return Err(format!("induced startup failure at {stage:?}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Private host namespace
// ---------------------------------------------------------------------------------------

/// A host's private HOME/XDG/config/PATH directory bound to one engine binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostNamespace {
    pub dir: PathBuf,
    pub engine: PathBuf,
}

impl HostNamespace {
    pub fn new(dir: impl Into<PathBuf>, engine: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            engine: engine.into(),
        }
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.dir.join("bin")
    }
    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }
    pub fn config_home(&self) -> PathBuf {
        self.dir.join("cfg")
    }
    pub fn herdr_config_dir(&self) -> PathBuf {
        self.dir.join("cfg/herdr")
    }
    pub fn config_file(&self) -> PathBuf {
        self.dir.join("cfg/herdr/config.toml")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.dir.join("log")
    }
    pub fn agent_helper(&self) -> PathBuf {
        self.bin_dir().join("pi")
    }
    pub fn agent_log(&self) -> PathBuf {
        self.log_dir().join("pi-agent.log")
    }
    pub fn forced_command_script(&self) -> PathBuf {
        self.dir.join("sshd-forced-command.sh")
    }
    pub fn forced_command_log(&self) -> PathBuf {
        self.log_dir().join("forced-commands.log")
    }
    pub fn socket_path(&self, session: &str) -> PathBuf {
        self.herdr_config_dir()
            .join("sessions")
            .join(session)
            .join("herdr.sock")
    }

    /// The complete environment of every engine process of this host (nothing inherited).
    pub fn env(&self) -> Result<Vec<(String, String)>, String> {
        let user = current_user()?;
        let shell = first_existing(&["/usr/bin/bash", "/bin/bash", "/bin/sh"])
            .ok_or("no POSIX shell found")?;
        let d = |p: PathBuf| p.display().to_string();
        Ok(vec![
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin", d(self.bin_dir())),
            ),
            ("HOME".into(), d(self.home())),
            ("USER".into(), user.clone()),
            ("LOGNAME".into(), user),
            ("SHELL".into(), shell.into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("XDG_CONFIG_HOME".into(), d(self.config_home())),
            ("XDG_DATA_HOME".into(), d(self.dir.join("data"))),
            ("XDG_STATE_HOME".into(), d(self.dir.join("state"))),
            ("HERDR_CONFIG_PATH".into(), d(self.config_file())),
            ("HD007_AGENT_LOG".into(), d(self.agent_log())),
        ])
    }

    /// Engine command with exactly this namespace's environment.
    pub fn command(&self) -> Result<Command, String> {
        let mut cmd = Command::new(&self.engine);
        cmd.env_clear().envs(self.env()?).stdin(Stdio::null());
        Ok(cmd)
    }

    /// Creates the namespace, installs the helper as `pi` and `herdr` -> engine, before any
    /// engine process of this host starts.
    pub fn prepare(&self, agent_script: &Path) -> Result<(), String> {
        for dir in [
            self.bin_dir(),
            self.home(),
            self.herdr_config_dir(),
            self.dir.join("data"),
            self.dir.join("state"),
            self.log_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(
            self.config_file(),
            "[update]\nversion_check = false\nmanifest_check = false\n\n[experimental]\nallow_nested = true\n",
        )
        .map_err(|e| e.to_string())?;

        let helper = self.agent_helper();
        let source = std::fs::read(agent_script)
            .map_err(|e| format!("agent helper {}: {e}", agent_script.display()))?;
        std::fs::write(&helper, &source).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;

        let link = self.bin_dir().join("herdr");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&self.engine, &link).map_err(|e| e.to_string())?;
        let resolved = std::fs::canonicalize(&link).map_err(|e| {
            format!(
                "namespace herdr link {} does not resolve to an engine: {e}",
                link.display()
            )
        })?;
        let engine = std::fs::canonicalize(&self.engine).map_err(|e| e.to_string())?;
        if resolved != engine {
            return Err(format!(
                "namespace herdr link resolves to {}, expected {}",
                resolved.display(),
                engine.display()
            ));
        }
        Ok(())
    }

    /// `session list --json` of this namespace.
    pub fn session_list_json(&self) -> Result<Value, String> {
        let out = self
            .command()?
            .args(["session", "list", "--json"])
            .output()
            .map_err(|e| format!("session list in {}: {e}", self.dir.display()))?;
        if !out.status.success() {
            return Err(format!(
                "session list in {} exited {:?}: {}",
                self.dir.display(),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| format!("session list json: {e}"))
    }

    /// Running according to the engine's own listing of this namespace, or a live socket.
    pub fn session_running(&self, session: &str) -> Result<bool, String> {
        let list = self.session_list_json()?;
        let listed_running = list["sessions"]
            .as_array()
            .ok_or("session list without sessions array")?
            .iter()
            .any(|s| s["name"] == session && s["running"] == true);
        Ok(listed_running || socket_connectable(&self.socket_path(session)))
    }
}

/// `ForceCommand` of the disposable sshd: only the exact product command lines for `session`
/// (status probe, client bridge, API bridge) and the SFTP subsystem are executed, each with the
/// namespace environment; any other request is rejected with exit 126.
pub fn forced_command_script(
    ns: &HostNamespace,
    session: &SessionName,
    sftp_server: Option<&Path>,
) -> Result<String, String> {
    let env_bin = first_existing(&["/usr/bin/env", "/bin/env"]).ok_or("env not found")?;
    let assignments = ns
        .env()?
        .iter()
        .map(|(k, v)| shell_quote(&format!("{k}={v}")))
        .collect::<Vec<_>>()
        .join(" ");
    let engine = shell_quote(&ns.engine.display().to_string());
    let s = shell_quote(session.as_str());
    let mut script = format!(
        "#!/bin/sh\n# hd007 remote fixture: sshd ForceCommand (generated per run).\nlog={}\ncase \"${{SSH_ORIGINAL_COMMAND-}}\" in\n",
        shell_quote(&ns.forced_command_log().display().to_string())
    );
    for (label, remote, tail) in [
        (
            "status",
            RemoteHerdrCommand::ServerStatus,
            "status server --json",
        ),
        (
            "client-bridge",
            RemoteHerdrCommand::ClientBridge,
            "remote-client-bridge",
        ),
        (
            "api-bridge",
            RemoteHerdrCommand::ApiBridge,
            "remote-api-bridge",
        ),
    ] {
        script.push_str(&format!(
            "  {})\n    printf 'accept {label}\\n' >>\"$log\"\n    exec {env_bin} -i {assignments} {engine} --session {s} {tail} ;;\n",
            shell_quote(&remote_command_line(session, remote)),
        ));
    }
    if let Some(sftp) = sftp_server {
        script.push_str(&format!(
            "  {q})\n    printf 'accept sftp\\n' >>\"$log\"\n    exec {env_bin} -i PATH=/usr/bin:/bin {q} ;;\n",
            q = shell_quote(&sftp.display().to_string()),
        ));
    }
    script.push_str(
        "  *)\n    printf 'reject %s\\n' \"${SSH_ORIGINAL_COMMAND-}\" >>\"$log\"\n    echo 'hd007 fixture: command not allowed' >&2\n    exit 126 ;;\nesac\n",
    );
    Ok(script)
}

// ---------------------------------------------------------------------------------------
// SSH Keys and Ephemeral sshd
// ---------------------------------------------------------------------------------------

/// Disposable SSH keys (host and client ed25519) stored in a private directory.
#[derive(Debug)]
pub struct SshKeys {
    pub base: PathBuf,
    pub host_key: PathBuf,
    pub host_pub: PathBuf,
    pub client_key: PathBuf,
    pub client_pub: PathBuf,
    pub authorized_keys: PathBuf,
    pub known_hosts: PathBuf,
}

impl SshKeys {
    pub fn create(base: &Path) -> Result<Self, String> {
        let etc_dir = base.join("etc");
        let keys_dir = base.join("keys");
        std::fs::create_dir_all(&etc_dir).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&keys_dir).map_err(|e| e.to_string())?;

        let host_key = etc_dir.join("ssh_host_ed25519_key");
        let client_key = keys_dir.join("id_ed25519");
        for (key_path, comment) in [(&host_key, "hd007-host"), (&client_key, "hd007-client")] {
            let out = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
                .arg(key_path)
                .output()
                .map_err(|e| format!("ssh-keygen spawn failed: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "ssh-keygen failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
        let authorized_keys = etc_dir.join("authorized_keys");
        let client_pub = keys_dir.join("id_ed25519.pub");
        std::fs::copy(&client_pub, &authorized_keys)
            .map_err(|e| format!("failed to copy authorized_keys: {e}"))?;

        Ok(Self {
            base: base.to_path_buf(),
            host_pub: etc_dir.join("ssh_host_ed25519_key.pub"),
            host_key,
            client_key,
            client_pub,
            authorized_keys,
            known_hosts: etc_dir.join("known_hosts"),
        })
    }

    /// Appends the host key for `127.0.0.1:<port>` to the private `known_hosts` file.
    pub fn trust_port(&self, port: u16) -> Result<(), String> {
        let pub_raw = std::fs::read_to_string(&self.host_pub)
            .map_err(|e| format!("could not read host pubkey: {e}"))?;
        let parts: Vec<&str> = pub_raw.split_whitespace().take(2).collect();
        if parts.len() < 2 {
            return Err("invalid public key format in host_key.pub".to_string());
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.known_hosts)
            .map_err(|e| format!("could not open known_hosts: {e}"))?;
        writeln!(file, "[127.0.0.1]:{port} {} {}", parts[0], parts[1])
            .map_err(|e| format!("failed to write known_hosts: {e}"))
    }

    /// Converts to the product's `IsolatedSshConfig`.
    pub fn to_isolated_config(&self) -> IsolatedSshConfig {
        IsolatedSshConfig {
            identity_file: self.client_key.clone(),
            user_known_hosts_file: self.known_hosts.clone(),
        }
    }
}

impl Drop for SshKeys {
    fn drop(&mut self) {
        for key in [&self.client_key, &self.host_key] {
            if key.exists() {
                let _ = Command::new("shred").arg("-u").arg(key).status();
            }
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Disposable unprivileged OpenSSH daemon instance.
#[derive(Debug)]
pub struct SshdInstance {
    pub name: String,
    pub base: PathBuf,
    pub port: u16,
    pub config_file: PathBuf,
    pub process: Option<TrackedProcess>,
}

impl SshdInstance {
    fn binary() -> Result<&'static str, String> {
        first_existing(&["/usr/sbin/sshd", "/usr/bin/sshd"])
            .ok_or_else(|| "sshd binary not found in /usr/sbin or /usr/bin".to_string())
    }

    pub fn create(
        name: &str,
        keys: &SshKeys,
        forced_command: &Path,
        sftp_server: Option<&Path>,
    ) -> Result<Self, String> {
        let base = keys.base.join(name);
        std::fs::create_dir_all(base.join("run")).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(base.join("log")).map_err(|e| e.to_string())?;
        let port = free_port();
        let user = current_user()?;
        let sftp_line = sftp_server
            .map(|p| format!("Subsystem sftp {}\n", p.display()))
            .unwrap_or_default();
        let cfg = format!(
            "Port {port}\nListenAddress 127.0.0.1\nAddressFamily inet\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nAllowUsers {user}\nPubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nUsePAM no\nStrictModes no\nX11Forwarding no\nAllowAgentForwarding no\nAllowTcpForwarding no\nPermitTunnel no\nPermitUserRC no\nPermitUserEnvironment no\nLoginGraceTime 30\nLogLevel VERBOSE\nPerSourcePenalties no\n{sftp_line}ForceCommand {}\n",
            keys.host_key.display(),
            base.join("run/sshd.pid").display(),
            keys.authorized_keys.display(),
            forced_command.display(),
        );
        let config_file = base.join("sshd_config");
        std::fs::write(&config_file, cfg).map_err(|e| e.to_string())?;
        keys.trust_port(port)?;
        let out = Command::new(Self::binary()?)
            .args(["-t", "-f"])
            .arg(&config_file)
            .output()
            .map_err(|e| format!("sshd -t execution failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "sshd config check failed for {}: {}",
                config_file.display(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(Self {
            name: name.to_string(),
            base,
            port,
            config_file,
            process: None,
        })
    }

    pub fn start(&mut self, ledger: &Ledger) -> Result<(), String> {
        let _ = std::fs::remove_file(self.base.join("run/sshd.pid"));
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.base.join("log/sshd.log"))
            .map_err(|e| format!("could not open sshd.log: {e}"))?;
        let mut child = Command::new("/usr/bin/setsid")
            .arg(Self::binary()?)
            .args(["-D", "-e", "-f"])
            .arg(&self.config_file)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("failed to spawn sshd: {e}"))?;
        let pid = child.id();
        let Some(starttime) = get_proc_starttime(pid) else {
            let status = child.wait();
            return Err(format!("sshd {} exited at once: {status:?}", self.name));
        };
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let mut pending = PendingProcessGuard::new(pid, starttime);
        ledger.append(&LedgerEntry::Sshd {
            name: self.name.clone(),
            pid,
            starttime,
            port: self.port,
        })?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !port_listening(self.port) {
            if Instant::now() >= deadline || !is_proc_alive(pid, starttime) {
                return Err(format!(
                    "sshd {} did not listen on port {} within 10s",
                    self.name, self.port
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        pending.defuse();
        self.process = Some(TrackedProcess {
            pid,
            starttime,
            name: format!("sshd-{}", self.name),
        });
        Ok(())
    }

    /// Stops the daemon and its connection children; Ok only when all are gone and the port
    /// is closed.
    pub fn stop(&mut self, ledger: Option<&Ledger>) -> Result<Vec<TrackedProcess>, String> {
        let Some(proc) = self.process.take() else {
            return Ok(Vec::new());
        };
        let children = descendants_with_starttime(proc.pid, &proc.name);
        if let Some(ledger) = ledger {
            for c in &children {
                let _ = ledger.append(&LedgerEntry::Descendant {
                    owner: proc.name.clone(),
                    pid: c.pid,
                    starttime: c.starttime,
                });
            }
        }
        terminate_proc(proc.pid, proc.starttime);
        for c in &children {
            terminate_proc(c.pid, c.starttime);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while port_listening(self.port) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut problems = Vec::new();
        if is_proc_alive(proc.pid, proc.starttime) {
            problems.push(format!("sshd pid {} alive", proc.pid));
        }
        for c in &children {
            if is_proc_alive(c.pid, c.starttime) {
                problems.push(format!("sshd child pid {} alive", c.pid));
            }
        }
        if port_listening(self.port) {
            problems.push(format!("port {} still listening", self.port));
        }
        if !problems.is_empty() {
            return Err(format!("sshd {}: {}", self.name, problems.join("; ")));
        }
        let mut stopped = vec![proc];
        stopped.extend(children);
        Ok(stopped)
    }

    pub fn is_listening(&self) -> bool {
        port_listening(self.port)
    }
}

// ---------------------------------------------------------------------------------------
// Managed Disposable Engine Session
// ---------------------------------------------------------------------------------------

/// How `pi` resolved inside the real pane shell before any agent action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentResolution {
    pub helper: PathBuf,
    pub resolved: String,
    pub pane_path: String,
}

/// Observed result of stopping one session (only verified states are reported as stopped).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SessionStopEvidence {
    pub session: String,
    pub alive_before: bool,
    pub stop_exit: Option<i32>,
    pub stop_output: String,
    pub forced_server_kill: bool,
    pub descendants: Vec<TrackedProcess>,
    pub forced_descendant_kills: usize,
    pub delete_exit: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct TrackedSession {
    pub name: String,
    pub namespace: HostNamespace,
    pub project_root: PathBuf,
    pub log_file: PathBuf,
    pub pid: Option<u32>,
    pub starttime: Option<u64>,
    pub boot_id: String,
    pub pane_id: String,
    pub agent_resolution: AgentResolution,
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(10)..].join("\n")
}

impl TrackedSession {
    /// Prepares the namespace (helper first), spawns `engine --session S server`, records it in
    /// the ledger, waits for `w1:p1`, proves `pi` resolves to the private helper inside the pane
    /// shell and observes the boot id. Any error after spawn kills the owned server.
    pub fn start(
        session_name: &str,
        namespace: &HostNamespace,
        project_root: &Path,
        agent_script: &Path,
        ledger: &Ledger,
        ready_timeout: Duration,
    ) -> Result<Self, String> {
        disposable(session_name)?;
        let socket = namespace.socket_path(session_name);
        if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
            return Err(format!(
                "socket path too long ({} bytes): {}",
                socket.as_os_str().len(),
                socket.display()
            ));
        }
        namespace.prepare(agent_script)?;

        let log_file = namespace
            .log_dir()
            .join(format!("server-{session_name}.log"));
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| e.to_string())?;
        let mut child = Command::new("/usr/bin/setsid")
            .arg(&namespace.engine)
            .env_clear()
            .envs(namespace.env()?)
            .env("HERDR_STARTUP_CWD", project_root)
            .current_dir(project_root)
            .args(["--session", session_name, "server"])
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("failed to spawn engine server: {e}"))?;
        let pid = child.id();
        let Some(starttime) = get_proc_starttime(pid) else {
            let status = child.wait();
            return Err(format!(
                "engine server {session_name} exited at once ({status:?}): {}",
                log_tail(&log_file)
            ));
        };
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let mut pending = PendingProcessGuard::new(pid, starttime);
        ledger.append(&LedgerEntry::Server {
            session: session_name.to_string(),
            namespace: namespace.dir.clone(),
            engine: namespace.engine.clone(),
            pid,
            starttime,
        })?;

        let cli = |args: &[&str]| -> Result<std::process::Output, String> {
            namespace
                .command()?
                .arg("--session")
                .arg(session_name)
                .args(args)
                .output()
                .map_err(|e| format!("engine cli {args:?}: {e}"))
        };

        let deadline = Instant::now() + ready_timeout;
        let panes = loop {
            if !is_proc_alive(pid, starttime) {
                return Err(format!(
                    "engine server {session_name} exited before ready: {}",
                    log_tail(&log_file)
                ));
            }
            if let Ok(out) = cli(&["pane", "list"]) {
                if out.status.success() {
                    if let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) {
                        break value;
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "session {session_name} not ready within {ready_timeout:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let has_shared = |v: &Value| {
            v["result"]["panes"]
                .as_array()
                .is_some_and(|a| a.iter().any(|p| p["pane_id"] == SHARED_PANE))
        };
        if !has_shared(&panes) {
            let root = project_root.display().to_string();
            let out = cli(&["workspace", "create", "--focus", "--cwd", &root])?;
            if !out.status.success() {
                return Err(format!(
                    "workspace create failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            let listed = cli(&["pane", "list"])?;
            let value: Value = serde_json::from_slice(&listed.stdout).unwrap_or(Value::Null);
            if !has_shared(&value) {
                return Err(format!(
                    "session {session_name} has no {SHARED_PANE}: {value}"
                ));
            }
        }
        let _ = cli(&[
            "pane",
            "wait-output",
            "--timeout",
            "5000",
            "--regex",
            "[$#>%] ?$",
            SHARED_PANE,
        ]);

        // Resolution of `pi` in the real PTY shell, before any agent action (never a real Pi).
        let which_file = namespace.log_dir().join("which-pi.txt");
        let path_file = namespace.log_dir().join("pane-path.txt");
        let _ = std::fs::remove_file(&which_file);
        let _ = std::fs::remove_file(&path_file);
        let probe = format!(
            "printf '%s' \"$PATH\" > {}; command -v pi > {}; clear\n",
            shell_quote(&path_file.display().to_string()),
            shell_quote(&which_file.display().to_string()),
        );
        let sent = cli(&["pane", "send-text", SHARED_PANE, &probe])?;
        if !sent.status.success() {
            return Err(format!(
                "pane send-text probe failed: {}",
                String::from_utf8_lossy(&sent.stderr)
            ));
        }
        let helper = namespace.agent_helper();
        let probe_deadline = Instant::now() + Duration::from_secs(10);
        let resolved = loop {
            if let Ok(text) = std::fs::read_to_string(&which_file) {
                if text.ends_with('\n') || !text.is_empty() {
                    break text.trim().to_string();
                }
            }
            if Instant::now() >= probe_deadline {
                return Err(format!(
                    "session {session_name}: pane never reported how pi resolves"
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if resolved != helper.display().to_string() {
            return Err(format!(
                "session {session_name}: pi resolves to {resolved:?}, not the private helper {}; refusing to continue",
                helper.display()
            ));
        }
        let agent_resolution = AgentResolution {
            helper,
            resolved,
            pane_path: std::fs::read_to_string(&path_file).unwrap_or_default(),
        };

        // Boot id announced by this namespace's endpoint.
        let session = SessionName::parse(session_name).map_err(|e| e.code)?;
        let mut gateway = LocalGateway::new(&namespace.herdr_config_dir(), session);
        gateway
            .connect(ConnectOptions {
                geometry: SurfaceGeometry {
                    cols: 80,
                    rows: 24,
                    cell_width_px: 9,
                    cell_height_px: 18,
                },
                surface_active: false,
            })
            .map_err(|e| format!("gateway connect to {session_name} failed: {e}"))?;
        if let Some(events) = gateway.take_event_stream() {
            std::thread::spawn(move || while events.recv().is_ok() {});
        }
        let boot_deadline = Instant::now() + Duration::from_secs(10);
        let boot_id = loop {
            if let Some(id) = gateway.identity() {
                break id.boot_id;
            }
            if Instant::now() >= boot_deadline {
                gateway.detach();
                return Err(format!("session {session_name} did not announce boot_id"));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        gateway.detach();

        pending.defuse();
        Ok(Self {
            name: session_name.to_string(),
            namespace: namespace.clone(),
            project_root: project_root.to_path_buf(),
            log_file,
            pid: Some(pid),
            starttime: Some(starttime),
            boot_id,
            pane_id: SHARED_PANE.to_string(),
            agent_resolution,
        })
    }

    /// Stops the session in its own namespace. Ok only when the engine accepted the stop (or
    /// the server was already gone), and the server, its descendants, the listing and the socket
    /// all confirm it is no longer running.
    pub fn stop(&mut self, ledger: Option<&Ledger>) -> Result<SessionStopEvidence, String> {
        disposable(&self.name)?;
        let mut ev = SessionStopEvidence {
            session: self.name.clone(),
            ..Default::default()
        };
        let owned = self.pid.zip(self.starttime);
        if let Some((pid, st)) = owned {
            ev.alive_before = is_proc_alive(pid, st);
            if ev.alive_before {
                ev.descendants = descendants_with_starttime(pid, &self.name);
            }
        }
        if let Some(ledger) = ledger {
            for d in &ev.descendants {
                let _ = ledger.append(&LedgerEntry::Descendant {
                    owner: self.name.clone(),
                    pid: d.pid,
                    starttime: d.starttime,
                });
            }
        }
        match self.namespace.command().and_then(|mut c| {
            c.args(["session", "stop", &self.name])
                .output()
                .map_err(|e| e.to_string())
        }) {
            Ok(out) => {
                ev.stop_exit = out.status.code();
                ev.stop_output = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
                .trim()
                .to_string();
            }
            Err(e) => ev.stop_output = format!("spawn failed: {e}"),
        }
        if let Some((pid, st)) = owned {
            let deadline = Instant::now() + Duration::from_secs(5);
            while is_proc_alive(pid, st) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if is_proc_alive(pid, st) {
                ev.forced_server_kill = true;
                terminate_proc(pid, st);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while ev
            .descendants
            .iter()
            .any(|d| is_proc_alive(d.pid, d.starttime))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        for d in &ev.descendants {
            if is_proc_alive(d.pid, d.starttime) {
                ev.forced_descendant_kills += 1;
                terminate_proc(d.pid, d.starttime);
            }
        }

        let mut problems = Vec::new();
        if let Some((pid, st)) = owned {
            if is_proc_alive(pid, st) {
                problems.push(format!("server pid {pid} alive"));
            }
        }
        for d in &ev.descendants {
            if is_proc_alive(d.pid, d.starttime) {
                problems.push(format!("descendant pid {} alive", d.pid));
            }
        }
        match self.namespace.session_running(&self.name) {
            Ok(false) => {}
            Ok(true) => problems.push("namespace still lists it running".into()),
            Err(e) => problems.push(format!("namespace listing unavailable: {e}")),
        }
        if ev.alive_before && ev.stop_exit != Some(0) {
            problems.push(format!(
                "engine refused stop (exit {:?}): {}",
                ev.stop_exit, ev.stop_output
            ));
        }
        if problems.is_empty() {
            ev.delete_exit = self.namespace.command().ok().and_then(|mut c| {
                c.args(["session", "delete", &self.name])
                    .output()
                    .ok()
                    .and_then(|o| o.status.code())
            });
        }
        self.pid = None;
        self.starttime = None;
        if problems.is_empty() {
            Ok(ev)
        } else {
            Err(format!(
                "session {} not verified stopped: {} ({ev:?})",
                self.name,
                problems.join("; ")
            ))
        }
    }

    pub fn to_host_fixture(&self) -> HostFixture {
        HostFixture {
            session: self.name.clone(),
            pane_id: self.pane_id.clone(),
            root: self.project_root.clone(),
            boot_id: self.boot_id.clone(),
        }
    }

    pub fn tracked_process(&self, label: &str) -> Option<TrackedProcess> {
        Some(TrackedProcess {
            pid: self.pid?,
            starttime: self.starttime?,
            name: format!("herdr-{label}-{}", self.name),
        })
    }
}

// ---------------------------------------------------------------------------------------
// Guard and Idempotent Cleanup
// ---------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct CleanupReport {
    pub stopped_sessions: Vec<String>,
    pub session_evidence: Vec<SessionStopEvidence>,
    pub killed_pids: Vec<u32>,
    pub ports_closed: Vec<u16>,
    pub failures: Vec<String>,
}

/// Owns every process/session/listener the fixture started.
#[derive(Debug)]
pub struct RemoteFixtureGuard {
    pub workdir: PathBuf,
    pub ledger: Ledger,
    pub tracked_processes: Vec<TrackedProcess>,
    pub created_sessions: Vec<TrackedSession>,
    pub sshd_instances: Vec<SshdInstance>,
    pub cleaned: bool,
}

impl RemoteFixtureGuard {
    pub fn new(workdir: PathBuf) -> Self {
        Self {
            ledger: Ledger::in_workdir(&workdir),
            workdir,
            tracked_processes: Vec::new(),
            created_sessions: Vec::new(),
            sshd_instances: Vec::new(),
            cleaned: false,
        }
    }

    pub fn register_session(&mut self, session: TrackedSession, label: &str) {
        if let Some(p) = session.tracked_process(label) {
            self.tracked_processes.push(p);
        }
        self.created_sessions.push(session);
    }

    /// Stops only sessions it created, then sshd listeners, then any tracked PID still alive.
    /// Reports stopped/closed/killed only after verification; everything else is a failure.
    /// A second call returns an empty report.
    pub fn cleanup(&mut self) -> CleanupReport {
        if self.cleaned {
            return CleanupReport::default();
        }
        self.cleaned = true;
        let mut report = CleanupReport::default();
        let created: Vec<String> = self
            .created_sessions
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for s in &mut self.created_sessions {
            if !may_stop(&s.name, &created) {
                report
                    .failures
                    .push(format!("refused to stop foreign session {}", s.name));
                continue;
            }
            match s.stop(Some(&self.ledger)) {
                Ok(ev) => {
                    report.stopped_sessions.push(ev.session.clone());
                    report.session_evidence.push(ev);
                }
                Err(e) => report.failures.push(e),
            }
        }
        for sshd in &mut self.sshd_instances {
            match sshd.stop(Some(&self.ledger)) {
                Ok(stopped) => {
                    report.ports_closed.push(sshd.port);
                    report.killed_pids.extend(stopped.iter().map(|p| p.pid));
                }
                Err(e) => report.failures.push(e),
            }
        }
        for p in &self.tracked_processes {
            if is_proc_alive(p.pid, p.starttime) {
                if terminate_proc(p.pid, p.starttime) {
                    report.killed_pids.push(p.pid);
                } else {
                    report
                        .failures
                        .push(format!("{} pid {} survived termination", p.name, p.pid));
                }
            }
        }
        report
    }
}

impl Drop for RemoteFixtureGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// ---------------------------------------------------------------------------------------
// Complete Remote Fixture
// ---------------------------------------------------------------------------------------

/// SSH profile metadata compatible with ConnectionsState and connections.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshProfileInfo {
    pub id: String,
    pub label: String,
    pub target: String,
    pub port: u16,
    pub session: String,
}

impl SshProfileInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "target": self.target,
            "port": self.port,
            "session": self.session,
        })
    }

    pub fn to_identity(&self) -> SshIdentity {
        SshIdentity::new(
            ProfileId::parse(&self.id).expect("generated profile id"),
            &self.target,
            Some(self.port),
            &self.session,
        )
        .expect("valid fixture ssh identity")
    }
}

/// Raw observations captured from the fixture (never an AC verdict).
#[derive(Debug, Clone, Serialize)]
pub struct RawObservations {
    pub local_session: String,
    pub local_boot: String,
    pub local_pane: String,
    pub local_root: PathBuf,
    pub local_namespace: PathBuf,
    pub ssh_session: String,
    pub ssh_boot: String,
    pub ssh_pane: String,
    pub ssh_root: PathBuf,
    pub ssh_namespace: PathBuf,
    pub ssh_port: u16,
    pub legacy_session: Option<String>,
    pub legacy_boot: Option<String>,
    pub legacy_port: Option<u16>,
    pub agent_resolution: BTreeMap<String, AgentResolution>,
    pub processes: Vec<TrackedProcess>,
    pub agent_logs: BTreeMap<String, String>,
    pub forced_command_logs: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct RemoteFixture {
    /// Declared first: dropped (processes stopped) before the keys are shredded.
    pub guard: RemoteFixtureGuard,
    pub config: RemoteFixtureConfig,
    pub keys: SshKeys,
    pub local_session: TrackedSession,
    pub ssh_session: TrackedSession,
    pub legacy_session: Option<TrackedSession>,
    pub local_root: PathBuf,
    pub ssh_root: PathBuf,
    pub legacy_root: PathBuf,
    pub ssh_profile: SshProfileInfo,
    pub legacy_ssh_profile: Option<SshProfileInfo>,
}

struct PairParts {
    keys: SshKeys,
    local_session: TrackedSession,
    ssh_session: TrackedSession,
    local_root: PathBuf,
    ssh_root: PathBuf,
    legacy_root: PathBuf,
    ssh_profile: SshProfileInfo,
}

fn write_files(root: &Path, files: &[(&str, &str)]) -> Result<(), String> {
    std::fs::create_dir_all(root.join("sub")).map_err(|e| e.to_string())?;
    for (name, content) in files {
        std::fs::write(root.join(name), content).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

/// Makes `root` a git repository on `branch` before the engine starts there. The git dir lives
/// next to `root` (`<root>.git`, `--separate-git-dir`): only a static `.git` file enters the tree
/// the SSH phases digest. No commit, no user or system git config.
fn init_git_branch(root: &Path, branch: &str) -> Result<(), String> {
    let git_dir = root.with_extension("git");
    let out = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("init")
        .arg("--quiet")
        .arg("--template=")
        .arg("-b")
        .arg(branch)
        .arg("--separate-git-dir")
        .arg(&git_dir)
        .arg(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git init {}: {e}", root.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git init {}: {} {}",
            root.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let head = std::fs::read_to_string(git_dir.join("HEAD")).map_err(|e| e.to_string())?;
    if head.trim() != format!("ref: refs/heads/{branch}") {
        return Err(format!("git init {}: HEAD is {head:?}", root.display()));
    }
    Ok(())
}

fn run_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{:06x}", (nanos / 1000) % 0x100_0000)
}

impl RemoteFixture {
    /// Starts the Local and SSH hosts. On any error everything already started is cleaned up
    /// and the cleanup report is part of the error.
    pub fn start_pair(config: RemoteFixtureConfig) -> Result<Self, String> {
        config.validate()?;
        std::fs::create_dir_all(&config.workdir).map_err(|e| e.to_string())?;
        let mut guard = RemoteFixtureGuard::new(config.workdir.clone());
        match Self::start_pair_parts(&config, &mut guard) {
            Ok(p) => Ok(Self {
                guard,
                config,
                keys: p.keys,
                local_session: p.local_session,
                ssh_session: p.ssh_session,
                legacy_session: None,
                local_root: p.local_root,
                ssh_root: p.ssh_root,
                legacy_root: p.legacy_root,
                ssh_profile: p.ssh_profile,
                legacy_ssh_profile: None,
            }),
            Err(error) => {
                let report = guard.cleanup();
                Err(format!(
                    "{error}; cleanup: stopped={:?} ports_closed={:?} killed={:?} failures={:?}",
                    report.stopped_sessions,
                    report.ports_closed,
                    report.killed_pids,
                    report.failures
                ))
            }
        }
    }

    fn start_pair_parts(
        config: &RemoteFixtureConfig,
        guard: &mut RemoteFixtureGuard,
    ) -> Result<PairParts, String> {
        let fixtures_dir = config.workdir.join("fixtures");
        let local_root = fixtures_dir.join("local-project");
        let ssh_root = fixtures_dir.join("ssh-project");
        let legacy_root = fixtures_dir.join("legacy-project");
        write_files(
            &local_root,
            &[
                ("notas.txt", "conteudo LOCAL do projeto herdr-desktop\n"),
                (
                    "diff-target.txt",
                    "linha 1: base comum\nlinha 2: apenas versao local\nlinha 3: encerramento\n",
                ),
                (
                    "utf8-sample.txt",
                    "Local UTF-8: Olá mundo, café e código 📁\n",
                ),
                ("local-only.txt", "arquivo exclusivo do host local\n"),
                ("sub/data.json", "{\"host\": \"local\", \"active\": true}\n"),
            ],
        )?;
        init_git_branch(&local_root, LOCAL_GIT_BRANCH)?;
        write_files(
            &ssh_root,
            &[
                (
                    "notas.txt",
                    "conteudo REMOTO do projeto via SSH (referencia 03749ae)\n",
                ),
                (
                    "diff-target.txt",
                    "linha 1: base comum\nlinha 2: MODIFICADA no host SSH remoto\nlinha 3: encerramento\nlinha 4: extra remota\n",
                ),
                ("utf8-sample.txt", "SSH UTF-8: Ações remotas, diff e rede 🌐\n"),
                ("remote-only.txt", "arquivo exclusivo do host SSH remoto\n"),
                ("sub/data.json", "{\"host\": \"ssh\", \"active\": true}\n"),
            ],
        )?;
        write_files(
            &legacy_root,
            &[(
                "notas.txt",
                "conteudo LEGADO via host SSH antigo 0.9.0 sem bridge\n",
            )],
        )?;

        let stem = format!(
            "{}{}-{}",
            config.session_prefix,
            std::process::id(),
            run_nonce()
        );
        let ledger = guard.ledger.clone();

        let local_ns = HostNamespace::new(config.workdir.join("h-loc"), &config.reference_binary);
        let local_session = TrackedSession::start(
            &format!("{stem}-loc"),
            &local_ns,
            &local_root,
            &config.agent_script,
            &ledger,
            config.ready_timeout,
        )?;
        guard.register_session(local_session.clone(), "local");
        config.fail_if(StartupStage::AfterLocalSession)?;

        let keys = SshKeys::create(&config.workdir.join("ssh"))?;
        let ssh_ns = HostNamespace::new(config.workdir.join("h-ssh"), &config.reference_binary);
        let ssh_session_name = format!("{stem}-ssh");
        let (sshd, ssh_port) = start_host_sshd(
            "sshd-ssh",
            &keys,
            &ssh_ns,
            &ssh_session_name,
            config.sftp_server_binary.as_deref(),
            &ledger,
        )?;
        guard.sshd_instances.push(sshd);
        config.fail_if(StartupStage::AfterSshd)?;

        let ssh_session = TrackedSession::start(
            &ssh_session_name,
            &ssh_ns,
            &ssh_root,
            &config.agent_script,
            &ledger,
            config.ready_timeout,
        )?;
        guard.register_session(ssh_session.clone(), "ssh");
        config.fail_if(StartupStage::AfterSshSession)?;

        let problems = pair_problems(
            &local_session.to_host_fixture(),
            &ssh_session.to_host_fixture(),
        );
        if !problems.is_empty() {
            return Err(format!("pair problems detected: {}", problems.join("; ")));
        }
        let ssh_profile = SshProfileInfo {
            id: ProfileId::generate().as_str().to_string(),
            label: "Host SSH (Referencia)".to_string(),
            target: format!("{}@127.0.0.1", current_user()?),
            port: ssh_port,
            session: ssh_session_name,
        };
        Ok(PairParts {
            keys,
            local_session,
            ssh_session,
            local_root,
            ssh_root,
            legacy_root,
            ssh_profile,
        })
    }

    /// Starts a legacy SSH host using the legacy engine binary (`/usr/bin/herdr` 0.9.0).
    pub fn start_legacy_host(&mut self) -> Result<(), String> {
        if self.legacy_session.is_some() {
            return Ok(());
        }
        let ns = HostNamespace::new(
            self.config.workdir.join("h-leg"),
            &self.config.legacy_binary,
        );
        let name = format!(
            "{}{}-{}-leg",
            self.config.session_prefix,
            std::process::id(),
            run_nonce()
        );
        let ledger = self.guard.ledger.clone();
        let (sshd, port) = start_host_sshd(
            "sshd-leg",
            &self.keys,
            &ns,
            &name,
            self.config.sftp_server_binary.as_deref(),
            &ledger,
        )?;
        self.guard.sshd_instances.push(sshd);
        let session = TrackedSession::start(
            &name,
            &ns,
            &self.legacy_root,
            &self.config.agent_script,
            &ledger,
            self.config.ready_timeout,
        )?;
        self.guard.register_session(session.clone(), "legacy");
        self.legacy_ssh_profile = Some(SshProfileInfo {
            id: ProfileId::generate().as_str().to_string(),
            label: "Host SSH Legado (0.9.0)".to_string(),
            target: format!("{}@127.0.0.1", current_user()?),
            port,
            session: name,
        });
        self.legacy_session = Some(session);
        Ok(())
    }

    pub fn local_fixture(&self) -> HostFixture {
        self.local_session.to_host_fixture()
    }

    pub fn ssh_fixture(&self) -> HostFixture {
        self.ssh_session.to_host_fixture()
    }

    pub fn legacy_fixture(&self) -> Option<HostFixture> {
        self.legacy_session.as_ref().map(|s| s.to_host_fixture())
    }

    pub fn isolated_ssh_config(&self) -> IsolatedSshConfig {
        self.keys.to_isolated_config()
    }

    /// Product connector (real OpenSSH) for the reference SSH host.
    pub fn ssh_connector(&self) -> SshConnector {
        SshConnector::new(
            self.ssh_profile.to_identity(),
            Some(self.isolated_ssh_config()),
            std::sync::Arc::new(OpenSshRunner),
        )
    }

    /// Product connector (real OpenSSH) for the legacy SSH host, once started.
    pub fn legacy_connector(&self) -> Option<SshConnector> {
        self.legacy_ssh_profile.as_ref().map(|p| {
            SshConnector::new(
                p.to_identity(),
                Some(self.isolated_ssh_config()),
                std::sync::Arc::new(OpenSshRunner),
            )
        })
    }

    fn host(&self, label: &str) -> Result<&TrackedSession, String> {
        match label {
            "local" => Ok(&self.local_session),
            "ssh" => Ok(&self.ssh_session),
            "legacy" => self
                .legacy_session
                .as_ref()
                .ok_or_else(|| "legacy host not started".to_string()),
            other => Err(format!("unknown host label '{other}'")),
        }
    }

    /// Observer on a host's own API socket (never used to perform the SSH actions under test).
    pub fn host_api(&self, label: &str) -> Result<ApiClient, String> {
        let s = self.host(label)?;
        Ok(ApiClient::new(&s.namespace.socket_path(&s.name), label))
    }

    /// Deterministic agent log of a host (empty when the helper never ran there).
    pub fn read_agent_log(&self, label: &str) -> Result<String, String> {
        let path = self.host(label)?.namespace.agent_log();
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Lines written by the sshd forced command of a host (empty when never reached).
    pub fn read_forced_command_log(&self, label: &str) -> Result<String, String> {
        let path = self.host(label)?.namespace.forced_command_log();
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// All live owned processes (servers, sshd) and their descendants, with starttimes.
    pub fn process_snapshot(&self) -> Vec<TrackedProcess> {
        let mut roots: Vec<TrackedProcess> = self.guard.tracked_processes.clone();
        roots.extend(
            self.guard
                .sshd_instances
                .iter()
                .filter_map(|s| s.process.clone()),
        );
        let mut all = Vec::new();
        for root in roots {
            if !is_proc_alive(root.pid, root.starttime) {
                continue;
            }
            all.extend(descendants_with_starttime(root.pid, &root.name));
            all.push(root);
        }
        all
    }

    /// JSON parameters for the future window harness.
    pub fn harness_params_json(&self) -> Value {
        json!({
            "root_local": self.local_root.display().to_string(),
            "root_ssh": self.ssh_root.display().to_string(),
            "session_local": self.local_session.name,
            "session_ssh": self.ssh_session.name,
            "boot_local": self.local_session.boot_id,
            "boot_ssh": self.ssh_session.boot_id,
            "shared_pane": SHARED_PANE,
            "local_xdg_config_home": self.local_session.namespace.config_home().display().to_string(),
            "local_herdr_config_dir": self.local_session.namespace.herdr_config_dir().display().to_string(),
            "ssh_profile": self.ssh_profile.to_json(),
            "ssh_identity": self.keys.client_key.display().to_string(),
            "ssh_known_hosts": self.keys.known_hosts.display().to_string(),
            "agent_kind": "pi",
            "agent_logs": {
                "local": self.local_session.namespace.agent_log().display().to_string(),
                "ssh": self.ssh_session.namespace.agent_log().display().to_string(),
            },
            "legacy_profile": self.legacy_ssh_profile.as_ref().map(SshProfileInfo::to_json),
        })
    }

    pub fn raw_observations(&self) -> RawObservations {
        let mut agent_logs = BTreeMap::new();
        let mut forced = BTreeMap::new();
        let mut resolution = BTreeMap::new();
        for label in ["local", "ssh", "legacy"] {
            let Ok(host) = self.host(label) else { continue };
            resolution.insert(label.to_string(), host.agent_resolution.clone());
            agent_logs.insert(
                label.to_string(),
                self.read_agent_log(label).unwrap_or_default(),
            );
            forced.insert(
                label.to_string(),
                self.read_forced_command_log(label).unwrap_or_default(),
            );
        }
        RawObservations {
            local_session: self.local_session.name.clone(),
            local_boot: self.local_session.boot_id.clone(),
            local_pane: self.local_session.pane_id.clone(),
            local_root: self.local_root.clone(),
            local_namespace: self.local_session.namespace.dir.clone(),
            ssh_session: self.ssh_session.name.clone(),
            ssh_boot: self.ssh_session.boot_id.clone(),
            ssh_pane: self.ssh_session.pane_id.clone(),
            ssh_root: self.ssh_root.clone(),
            ssh_namespace: self.ssh_session.namespace.dir.clone(),
            ssh_port: self.ssh_profile.port,
            legacy_session: self.legacy_session.as_ref().map(|s| s.name.clone()),
            legacy_boot: self.legacy_session.as_ref().map(|s| s.boot_id.clone()),
            legacy_port: self.legacy_ssh_profile.as_ref().map(|p| p.port),
            agent_resolution: resolution,
            processes: self.process_snapshot(),
            agent_logs,
            forced_command_logs: forced,
        }
    }

    pub fn cleanup(&mut self) -> CleanupReport {
        self.guard.cleanup()
    }
}

/// Writes the host's forced command and starts its sshd (registered in the ledger at spawn).
fn start_host_sshd(
    name: &str,
    keys: &SshKeys,
    ns: &HostNamespace,
    session: &str,
    sftp_server: Option<&Path>,
    ledger: &Ledger,
) -> Result<(SshdInstance, u16), String> {
    let session_name = SessionName::parse(session).map_err(|e| e.code)?;
    std::fs::create_dir_all(ns.log_dir()).map_err(|e| e.to_string())?;
    let script = forced_command_script(ns, &session_name, sftp_server)?;
    let path = ns.forced_command_script();
    std::fs::write(&path, script).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    let mut sshd = SshdInstance::create(name, keys, &path, sftp_server)?;
    sshd.start(ledger)?;
    let port = sshd.port;
    Ok((sshd, port))
}
