//! SSH bridge: RuntimeGateway over OpenSSH (references: `remote/saved.rs:connect_saved_ssh`,
//! `SavedSshApiBridge`, `remote/attach.rs:SshStdioBridge`).
//!
//! Two independent lanes per host:
//! - visual endpoint lane: one long-lived `ssh … herdr --session S remote-client-bridge`
//!   process speaking the frozen endpoint generation-1 framing over its stdio (subscription,
//!   input and health ping; never used for API commands);
//! - JSON API lane: one short `ssh … herdr --session S remote-api-bridge` process per request,
//!   bounded by a timeout, so a slow action never occupies the visual lane.
//!
//! Before opening the visual lane the connector runs the read-only probe
//! `herdr --session S status server --json`: the remote client bridge starts a daemon when
//! none is listening, and the desktop must not start, install, restart or stop servers.
//! Killing the local ssh process on detach closes only this client; the remote server and its
//! processes continue.

use std::io::{self, Read, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use herdr_client::event_queue::{event_queue, EventSender, EventStream};
use herdr_client::local::{EndpointCorrelator, ENDPOINT_RESPONSE_LIMIT};
use herdr_client::protocol::endpoint::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_SNAPSHOT_KIND,
    ENDPOINT_WELCOME_KIND, HEALTH_PING_KIND,
};
use herdr_client::protocol::wire::{
    ClientMessage, ClientPaneInputEvent, ClientShellSnapshot, ServerMessage, SERVER_MESSAGE_MAX_TAG,
};
use herdr_client::protocol::{decode_message, peek_tag, read_frame, write_message, MAX_FRAME_SIZE};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry,
};
use serde_json::Value;

use crate::connections::failure::{
    check_negotiated, check_server_status, classify_spawn_error, classify_ssh_failure,
    AttentionReason, ConnectFailure,
};
use crate::connections::hub::ApiLane;
use crate::connections::remote_binary::{
    configured_remote_binary, discovery_candidates, known_binary_candidate_script, missing_failure,
    outdated_failure, parse_client_status, ClientStatus, RemoteBinary,
};
use crate::connections::ssh_options::{
    binary_status_client_line, build_ssh_script, build_ssh_with_binary, IsolatedSshConfig,
    OpenSshCommand, RemoteHerdrCommand, SshIdentity, REMOTE_HERDR,
};
use crate::connections::state::{HealthAction, HealthMonitor};

/// Budget of every discovery/status step (the dialog must never wait longer than 15 s).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Budget to start the bridge process itself (the measured window symptom: a `spawn` that
/// never returns left the attempt stuck in `Connecting` with no trace and no child).
pub const BRIDGE_START_TIMEOUT: Duration = Duration::from_secs(15);
/// Budget for the endpoint welcome after the bridge process starts.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Budget of one JSON API request (a timeout makes the result unknown).
pub const API_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait for the correlated reply of one endpoint command.
pub const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
const STDERR_LIMIT: usize = 16 * 1024;
const STDOUT_LIMIT: usize = 4 * 1024 * 1024;
const WRITER_SLOTS: usize = 256;
const HEALTH_TICK: Duration = Duration::from_millis(250);
/// Reader poll while the held control backlog of the event queue is full.
const BACKLOG_POLL: Duration = Duration::from_millis(10);

// ---------------------------------------------------------------------------------------
// Process runner
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// Exit code, `None` when terminated by a signal.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// A running OpenSSH process with piped stdio.
pub trait SshChild: Send {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>>;
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Captured stderr so far (bounded).
    fn stderr_text(&self) -> String;
    /// Waits up to `timeout` for the exit; `Some(code)` when exited.
    fn wait_exit(&mut self, timeout: Duration) -> Option<Option<i32>>;
    fn kill(&mut self);
}

/// Starts OpenSSH processes (real or fake in tests).
pub trait SshRunner: Send + Sync {
    fn output(
        &self,
        command: &OpenSshCommand,
        timeout: Duration,
        stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput>;
    fn spawn(&self, command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>>;
    /// Budget to start one bridge process (the connector always waits with a bound; a test
    /// runner may shorten it).
    fn bridge_start_timeout(&self) -> Duration {
        BRIDGE_START_TIMEOUT
    }
}

/// Real OpenSSH processes.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenSshRunner;

fn read_bounded(mut reader: impl Read, limit: usize, sink: Arc<Mutex<Vec<u8>>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut sink = sink.lock().expect("pipe sink");
                let room = limit.saturating_sub(sink.len());
                sink.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}

struct OsChild {
    child: Child,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl SshChild for OsChild {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.child
            .stdin
            .take()
            .map(|s| Box::new(s) as Box<dyn Write + Send>)
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>)
    }
    fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().expect("stderr")).into_owned()
    }
    fn wait_exit(&mut self, timeout: Duration) -> Option<Option<i32>> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status.code()),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => return None,
            }
        }
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn prepare(command: &OpenSshCommand, stdin: Stdio) -> std::process::Command {
    let mut process = command.to_command();
    process
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    process
}

impl SshRunner for OpenSshRunner {
    fn output(
        &self,
        command: &OpenSshCommand,
        timeout: Duration,
        stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput> {
        let mut child = prepare(
            command,
            if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            },
        )
        .spawn()?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let out_thread = child.stdout.take().map(|pipe| {
            let sink = stdout.clone();
            std::thread::spawn(move || read_bounded(pipe, STDOUT_LIMIT, sink))
        });
        let err_thread = child.stderr.take().map(|pipe| {
            let sink = stderr.clone();
            std::thread::spawn(move || read_bounded(pipe, STDERR_LIMIT, sink))
        });
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // A closed pipe means the process already failed; its exit status tells why.
            let _ = pipe.write_all(bytes).and_then(|_| pipe.flush());
        }
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "ssh command timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if let Some(t) = out_thread {
            let _ = t.join();
        }
        if let Some(t) = err_thread {
            let _ = t.join();
        }
        let stdout = std::mem::take(&mut *stdout.lock().expect("stdout"));
        let stderr = String::from_utf8_lossy(&stderr.lock().expect("stderr")).into_owned();
        Ok(ProcessOutput {
            status: status.code(),
            stdout,
            stderr,
        })
    }

    fn spawn(&self, command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
        let mut child = prepare(command, Stdio::piped()).spawn()?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        if let Some(pipe) = child.stderr.take() {
            let sink = stderr.clone();
            std::thread::Builder::new()
                .name("herdr-desktop-ssh-stderr".into())
                .spawn(move || read_bounded(pipe, STDERR_LIMIT, sink))?;
        }
        Ok(Box::new(OsChild { child, stderr }))
    }
}

// ---------------------------------------------------------------------------------------
// Connector
// ---------------------------------------------------------------------------------------

/// Key of the remote API bridge capability cache: the chosen binary path and version.
type ApiBridgeKey = (String, Option<String>);

/// Builds connections for one SSH profile.
#[derive(Clone)]
pub struct SshConnector {
    identity: SshIdentity,
    isolated: Option<IsolatedSshConfig>,
    runner: Arc<dyn SshRunner>,
    /// Remote binary chosen by the latest resolution (spec 029); shared by every clone so the
    /// API lane and the event source of this profile use the very same path.
    remote_binary: Arc<Mutex<Option<RemoteBinary>>>,
    /// Whether the chosen remote binary serves the JSON API bridge, once observed (spec 035,
    /// AC-035-03): `Some(false)` is the installed Herdr without `remote-api-bridge` (a capability
    /// of this binary), so every later request is refused without spawning another process. The
    /// binary path+version is the key: an in-place update re-probes once.
    api_bridge: Arc<Mutex<Option<(ApiBridgeKey, bool)>>>,
    /// Configuration override (`HERDR_REMOTE_BINARY`); when set it replaces the candidate list.
    override_binary: Option<String>,
}

impl SshConnector {
    pub fn new(
        identity: SshIdentity,
        isolated: Option<IsolatedSshConfig>,
        runner: Arc<dyn SshRunner>,
    ) -> Self {
        Self::with_override(
            identity,
            isolated,
            runner,
            configured_remote_binary(&|key| std::env::var(key).ok()),
        )
    }

    fn with_override(
        identity: SshIdentity,
        isolated: Option<IsolatedSshConfig>,
        runner: Arc<dyn SshRunner>,
        override_binary: Option<String>,
    ) -> Self {
        Self {
            identity,
            isolated,
            runner,
            remote_binary: Arc::new(Mutex::new(None)),
            api_bridge: Arc::new(Mutex::new(None)),
            override_binary,
        }
    }

    /// Test/configuration seam: a fixed remote binary path (or none) instead of the process
    /// environment. The path must still serve the endpoint requirement.
    pub fn with_remote_binary_override(mut self, path: Option<String>) -> Self {
        self.override_binary = path;
        self
    }

    pub fn identity(&self) -> &SshIdentity {
        &self.identity
    }

    fn endpoint(&self) -> &str {
        self.identity.endpoint_id()
    }

    /// Binary chosen by the latest resolution, when any.
    pub fn remote_binary(&self) -> Option<RemoteBinary> {
        self.remote_binary
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_remote_binary(&self, binary: RemoteBinary) {
        *self
            .remote_binary
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(binary);
    }

    /// One lane command of this profile with the binary chosen so far (spec 029): the API lane
    /// and the event source of a connected host use the same discovered path.
    pub fn command(&self, remote: RemoteHerdrCommand) -> OpenSshCommand {
        let path = self
            .remote_binary()
            .map(|binary| binary.path)
            .unwrap_or_else(|| REMOTE_HERDR.to_owned());
        build_ssh_with_binary(&self.identity, self.isolated.as_ref(), &path, remote)
    }

    /// Read-only status probe of the remote named session, with the binary chosen so far.
    pub fn probe(&self) -> Result<(), ConnectFailure> {
        let binary = self.remote_binary().unwrap_or_else(RemoteBinary::on_path);
        self.probe_binary(&binary)
    }

    fn probe_binary(&self, binary: &RemoteBinary) -> Result<(), ConnectFailure> {
        let command = build_ssh_with_binary(
            &self.identity,
            self.isolated.as_ref(),
            &binary.path,
            RemoteHerdrCommand::ServerStatus,
        );
        let started = Instant::now();
        let output = match self.runner.output(&command, PROBE_TIMEOUT, None) {
            Ok(output) => {
                self.trace(
                    "servidor",
                    started,
                    format!(
                        "exit {}",
                        output
                            .status
                            .map_or_else(|| "signal".into(), |s| s.to_string())
                    ),
                );
                output
            }
            Err(error) => {
                self.trace("servidor", started, format!("erro {}", error.kind()));
                return Err(if error.kind() == io::ErrorKind::TimedOut {
                    ConnectFailure::transient(
                        self.endpoint(),
                        "timeout",
                        "no answer while checking the remote server",
                    )
                } else {
                    classify_spawn_error(self.endpoint(), &error)
                });
            }
        };
        if output.status != Some(0) {
            return Err(classify_ssh_failure(
                self.endpoint(),
                output.status,
                &output.stderr,
            ));
        }
        check_server_status(self.endpoint(), &String::from_utf8_lossy(&output.stdout))
    }

    pub fn api_lane(&self) -> SshApiLane {
        SshApiLane {
            connector: self.clone(),
            timeout: API_TIMEOUT,
        }
    }

    /// Remote binary the API lane commands would use, when one was chosen.
    fn api_bridge_key(&self) -> ApiBridgeKey {
        self.remote_binary()
            .map(|binary| (binary.path, binary.version))
            .unwrap_or_else(|| (REMOTE_HERDR.to_owned(), None))
    }

    /// Whether the current remote binary serves the JSON API bridge (spec 035, AC-035-03):
    /// `Some(false)` means the installed Herdr has no `remote-api-bridge` subcommand, so the
    /// capability stays disabled without spawning a doomed process again.
    pub fn api_bridge_supported(&self) -> Option<bool> {
        let key = self.api_bridge_key();
        self.api_bridge
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(known, _)| known == &key)
            .map(|(_, supported)| *supported)
    }

    /// Records the API bridge capability of the current remote binary (once observed).
    fn record_api_bridge_support(&self, supported: bool) {
        *self
            .api_bridge
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((self.api_bridge_key(), supported));
    }

    /// One internally built remote line through ssh (discovery probes only; never from the
    /// WebView).
    fn output_line(&self, line: &str, step: &str) -> Result<ProcessOutput, ConnectFailure> {
        let started = Instant::now();
        let command = build_ssh_script(&self.identity, self.isolated.as_ref(), line);
        match self
            .runner
            .output(&command, PROBE_TIMEOUT, Some(line.as_bytes()))
        {
            Ok(output) => {
                self.trace(
                    step,
                    started,
                    format!(
                        "exit {}",
                        output
                            .status
                            .map_or_else(|| "signal".into(), |s| s.to_string())
                    ),
                );
                Ok(output)
            }
            Err(error) => {
                self.trace(step, started, format!("erro {}", error.kind()));
                if error.kind() == io::ErrorKind::TimedOut {
                    Err(ConnectFailure::transient(
                        self.endpoint(),
                        "timeout",
                        match step {
                            "descoberta" => "no answer while discovering the remote Herdr",
                            "compatibilidade" => {
                                "no answer while checking the remote Herdr's compatibility"
                            }
                            _ => "no answer while running the SSH step",
                        },
                    ))
                } else {
                    Err(classify_spawn_error(self.endpoint(), &error))
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    fn trace(&self, step: &str, started: Instant, result: String) {
        eprintln!(
            "[conn] {} {} {}ms {}",
            self.identity.target.as_str(),
            step,
            started.elapsed().as_millis(),
            result
        );
    }

    #[cfg(not(debug_assertions))]
    fn trace(&self, _step: &str, _started: Instant, _result: String) {}

    /// Same candidates and order as the TUI (`command -v herdr`, then the known-path script);
    /// the configuration override replaces the list.
    fn candidates(&self) -> Result<Vec<String>, ConnectFailure> {
        if let Some(path) = &self.override_binary {
            return Ok(vec![path.clone()]);
        }
        let path_output = match self.output_line("command -v herdr", "descoberta") {
            Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
            _ => String::new(),
        };
        let script_output = match self.output_line(&known_binary_candidate_script(), "descoberta") {
            Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
            Err(failure) if path_output.is_empty() => return Err(failure),
            Err(_) => String::new(),
        };
        Ok(discovery_candidates(&path_output, &script_output))
    }

    /// Endpoint requirement of one candidate: `test -x <path> && <path> status client --json`
    /// (the TUI's `remote_client_status`). A candidate that cannot run is skipped.
    fn client_status(&self, path: &str) -> Result<Option<ClientStatus>, ConnectFailure> {
        let output = self.output_line(&binary_status_client_line(path), "compatibilidade")?;
        if output.status == Some(255) {
            return Err(classify_ssh_failure(
                self.endpoint(),
                output.status,
                &output.stderr,
            ));
        }
        if output.status != Some(0) {
            return Ok(None);
        }
        Ok(parse_client_status(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// Discovery among the candidates (AC-029-01): the first that serves is stored and returned;
    /// no candidate at all -> `HerdrMissing`; none serving -> outdated with the version found.
    pub fn discover_binary(&self) -> Result<RemoteBinary, ConnectFailure> {
        let candidates = self.candidates()?;
        let mut found_version: Option<String> = None;
        for candidate in &candidates {
            match self.client_status(candidate) {
                Ok(Some(status)) if status.supports_endpoint_requirement() => {
                    let binary = RemoteBinary {
                        path: candidate.clone(),
                        version: status.version,
                    };
                    self.set_remote_binary(binary.clone());
                    return Ok(binary);
                }
                Ok(Some(status)) => {
                    if found_version.is_none() {
                        found_version = status.version;
                    }
                }
                Ok(None) => {}
                Err(failure) => return Err(failure),
            }
        }
        if candidates.is_empty() {
            Err(missing_failure(self.endpoint()))
        } else {
            Err(outdated_failure(self.endpoint(), found_version.as_deref()))
        }
    }

    /// Probe, open the visual lane and negotiate endpoint generation 1.
    pub fn connect(&self, options: ConnectOptions) -> Result<SshGateway, ConnectFailure> {
        if let Some(path) = &self.override_binary {
            // The override replaces the candidate list and must still serve the endpoint.
            let binary = match self.client_status(path) {
                Ok(Some(status)) if status.supports_endpoint_requirement() => RemoteBinary {
                    path: path.clone(),
                    version: status.version,
                },
                Ok(status) => {
                    return Err(outdated_failure(
                        self.endpoint(),
                        status.and_then(|s| s.version).as_deref(),
                    ))
                }
                Err(failure) => return Err(failure),
            };
            self.set_remote_binary(binary.clone());
            self.probe_binary(&binary)?;
            return self.open_bridge(binary, options);
        }
        match self.probe() {
            Ok(()) => {
                // A path discovered by an earlier attempt is kept; the PATH candidate is the
                // generic fallback (candidate 1 of the TUI).
                let binary = self.remote_binary().unwrap_or_else(RemoteBinary::on_path);
                self.set_remote_binary(binary.clone());
                self.open_bridge(binary, options)
            }
            Err(failure)
                if matches!(
                    failure.reason(),
                    Some(AttentionReason::HerdrMissing | AttentionReason::ServerIncompatible)
                ) =>
            {
                let binary = self.discover_binary()?;
                // Judge the server with the chosen binary too.
                self.probe_binary(&binary)?;
                self.open_bridge(binary, options)
            }
            Err(failure) => Err(failure),
        }
    }

    /// Starts the bridge process bounded by [`SshRunner::bridge_start_timeout`]. A `spawn` that
    /// never returns (measured in the real window: no trace, no child, attempt stuck) fails with
    /// an explicit error instead of blocking the attempt forever; a child that arrives after the
    /// bound is killed.
    fn start_bridge(
        &self,
        command: OpenSshCommand,
        handshake_started: Instant,
    ) -> Result<Box<dyn SshChild>, ConnectFailure> {
        let endpoint = self.endpoint().to_owned();
        let timeout = self.runner.bridge_start_timeout();
        let budget = if timeout.as_secs() >= 1 {
            format!("{} s", timeout.as_secs())
        } else {
            format!("{} ms", timeout.as_millis())
        };
        let runner = self.runner.clone();
        let (started_tx, started_rx) = sync_channel::<io::Result<Box<dyn SshChild>>>(1);
        std::thread::Builder::new()
            .name("herdr-desktop-ssh-start".into())
            .spawn(move || {
                let result = runner.spawn(&command);
                if let Err(send_error) = started_tx.send(result) {
                    // Nobody is waiting any more: the late child is terminated here.
                    if let Ok(mut child) = send_error.0 {
                        child.kill();
                    }
                }
            })
            .map_err(|_| {
                self.trace(
                    "ponte",
                    handshake_started,
                    "falha ao iniciar thread de abertura".into(),
                );
                ConnectFailure::transient(&endpoint, "ssh_failed", "bridge opening thread")
            })?;
        match started_rx.recv_timeout(timeout) {
            Ok(Ok(child)) => {
                self.trace("ponte", handshake_started, "iniciada".into());
                Ok(child)
            }
            Ok(Err(error)) => {
                self.trace(
                    "ponte",
                    handshake_started,
                    format!("erro {}: {}", error.kind(), error),
                );
                Err(classify_spawn_error(&endpoint, &error))
            }
            Err(_) => {
                self.trace(
                    "ponte",
                    handshake_started,
                    format!("timeout de {budget} ao iniciar"),
                );
                let message = format!("the ssh process did not start within {budget}");
                Err(ConnectFailure::transient(
                    &endpoint,
                    "bridge_start_timeout",
                    &message,
                ))
            }
        }
    }

    /// Spawns the visual lane with `binary` and negotiates endpoint generation 1.
    fn open_bridge(
        &self,
        binary: RemoteBinary,
        options: ConnectOptions,
    ) -> Result<SshGateway, ConnectFailure> {
        let endpoint = self.endpoint().to_owned();
        let handshake_started = Instant::now();
        // Decision point between the last probe and the bridge: which geometry/surface mode the
        // hello carries (never silent).
        self.trace(
            "ponte",
            handshake_started,
            format!(
                "abrindo surface={} {}x{}",
                options.surface_active, options.geometry.cols, options.geometry.rows
            ),
        );
        let command = build_ssh_with_binary(
            &self.identity,
            self.isolated.as_ref(),
            &binary.path,
            RemoteHerdrCommand::ClientBridge,
        );
        let mut child = self.start_bridge(command, handshake_started)?;
        let (Some(mut stdin), Some(stdout)) = (child.take_stdin(), child.take_stdout()) else {
            self.trace("handshake", handshake_started, "stdio ausente".into());
            child.kill();
            return Err(ConnectFailure::transient(
                &endpoint,
                "ssh_failed",
                "the ssh process exposed no stdio",
            ));
        };
        let exit_failure = |child: &mut Box<dyn SshChild>| {
            let status = child.wait_exit(Duration::from_secs(2));
            let stderr = child.stderr_text();
            child.kill();
            match status {
                Some(code) => classify_ssh_failure(&endpoint, code, &stderr),
                None => ConnectFailure::transient(
                    &endpoint,
                    "handshake_timeout",
                    "the remote server did not answer the handshake",
                ),
            }
        };

        let geometry = options.geometry;
        let mut hello = EndpointClientHello::generation_one(
            geometry.surface_size(),
            geometry.cell_width_px,
            geometry.cell_height_px,
        );
        hello.surface_active = options.surface_active;
        let hello = ClientMessage::EndpointControl {
            kind: ENDPOINT_HELLO_KIND.into(),
            data: serde_json::to_string(&hello).expect("hello serializes"),
        };
        let handshake_rtt_start = Instant::now();
        if write_message(&mut stdin, &hello).is_err() {
            self.trace(
                "handshake",
                handshake_started,
                "falha ao enviar hello".into(),
            );
            return Err(exit_failure(&mut child));
        }

        // The first frame must be the welcome; read it on a thread to bound the wait.
        let (first_tx, first_rx) = sync_channel(1);
        std::thread::Builder::new()
            .name("herdr-desktop-ssh-handshake".into())
            .spawn(move || {
                let mut stdout = stdout;
                let frame = read_frame(&mut stdout, MAX_FRAME_SIZE);
                let _ = first_tx.send((stdout, frame));
            })
            .map_err(|_| {
                self.trace(
                    "handshake",
                    handshake_started,
                    "falha ao iniciar reader".into(),
                );
                ConnectFailure::transient(&endpoint, "ssh_failed", "handshake thread")
            })?;
        let (stdout, frame) = match first_rx.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok((stdout, Ok(frame))) => {
                self.trace("handshake", handshake_started, "ok".into());
                (stdout, frame)
            }
            Ok((_, Err(_))) => {
                self.trace("handshake", handshake_started, "frame inválido".into());
                return Err(exit_failure(&mut child));
            }
            Err(_) => {
                self.trace("handshake", handshake_started, "timeout".into());
                return Err(exit_failure(&mut child));
            }
        };
        let incompatible =
            || ConnectFailure::attention(&endpoint, AttentionReason::ServerIncompatible);
        let welcome = match decode_message::<ServerMessage>(&frame) {
            Ok(ServerMessage::EndpointControl { kind, data }) if kind == ENDPOINT_WELCOME_KIND => {
                serde_json::from_str::<EndpointServerWelcome>(&data).map_err(|_| incompatible())
            }
            _ => Err(incompatible()),
        };
        let welcome = match welcome {
            Ok(welcome) if welcome.is_generation_one_core() => welcome,
            _ => {
                child.kill();
                return Err(incompatible());
            }
        };
        let negotiated = Negotiated::from_welcome(
            LiveIdentity {
                endpoint: endpoint.clone(),
                session: self.identity.session.as_str().to_owned(),
                connection_generation: 1,
                boot_id: String::new(),
            },
            &welcome,
        );
        if let Err(failure) = check_negotiated(&endpoint, &negotiated) {
            child.kill();
            return Err(failure);
        }
        let handshake_latency_ms = handshake_rtt_start.elapsed().as_millis() as u64;
        Ok(SshGateway::start(
            endpoint,
            self.identity.session.as_str().to_owned(),
            Negotiation {
                negotiated,
                remote_binary: binary,
                handshake_latency_ms,
            },
            child,
            stdin,
            stdout,
            self.api_lane(),
        ))
    }
}

// ---------------------------------------------------------------------------------------
// API lane
// ---------------------------------------------------------------------------------------

static NEXT_API_ID: AtomicU64 = AtomicU64::new(1);

/// The installed remote Herdr has no `remote-api-bridge` subcommand: the JSON API is unavailable
/// on this host (the connection and its visual lane are not affected). Not retryable.
fn remote_api_unsupported(endpoint: &str) -> RuntimeError {
    RuntimeError::new(
        "remote_api_unsupported",
        "the remote Herdr does not offer the JSON API over SSH (remote-api-bridge); update Herdr on the host",
    )
    .with_endpoint(endpoint)
}

/// One `remote-api-bridge` process per request.
#[derive(Clone)]
pub struct SshApiLane {
    connector: SshConnector,
    timeout: Duration,
}

impl ApiLane for SshApiLane {
    fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let endpoint = self.connector.endpoint().to_owned();
        let step = if method == "workspace.list" {
            "workspaces"
        } else {
            "api"
        };
        let started = Instant::now();
        // Spec 035 (AC-035-03): the missing subcommand is a capability of the installed binary,
        // not a per-call failure. Once seen, nothing is spawned for this binary again.
        if self.connector.api_bridge_supported() == Some(false) {
            self.connector
                .trace(step, started, "sem suporte (remote-api-bridge)".into());
            return Err(remote_api_unsupported(&endpoint));
        }
        let id = format!(
            "desktop-ssh:{}",
            NEXT_API_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut line =
            serde_json::json!({ "id": id, "method": method, "params": params }).to_string();
        line.push('\n');
        let output = match self.connector.runner.output(
            &self.connector.command(RemoteHerdrCommand::ApiBridge),
            self.timeout,
            Some(line.as_bytes()),
        ) {
            Ok(output) => {
                if output.status != Some(0)
                    && output.stderr.contains("unknown command: remote-api-bridge")
                {
                    // Older remote Herdr (e.g. 0.9.0): the action is unavailable; the connection
                    // is not, and no other request of this binary tries again.
                    self.connector.record_api_bridge_support(false);
                    self.connector
                        .trace(step, started, "sem suporte (remote-api-bridge)".into());
                    return Err(remote_api_unsupported(&endpoint));
                }
                if output.status == Some(0) {
                    self.connector.record_api_bridge_support(true);
                }
                self.connector.trace(
                    step,
                    started,
                    format!(
                        "exit {}",
                        output
                            .status
                            .map_or_else(|| "signal".into(), |s| s.to_string())
                    ),
                );
                output
            }
            Err(error) => {
                self.connector
                    .trace(step, started, format!("erro {}", error.kind()));
                return Err(if error.kind() == io::ErrorKind::TimedOut {
                    RuntimeError::new(
                        "timeout",
                        if method == "workspace.list" {
                            "no answer while reading workspaces"
                        } else {
                            "the remote API did not answer in time"
                        },
                    )
                    .retryable()
                    .with_endpoint(&endpoint)
                } else {
                    classify_spawn_error(&endpoint, &error).error().clone()
                });
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let Some(response) = stdout.lines().find(|l| !l.trim().is_empty()) else {
            return Err(match output.status {
                Some(0) => {
                    RuntimeError::new("empty_response", "empty response from the remote API")
                        .retryable()
                        .with_endpoint(&endpoint)
                }
                status => classify_ssh_failure(&endpoint, status, &output.stderr)
                    .error()
                    .clone(),
            });
        };
        let value: Value = serde_json::from_str(response).map_err(|_| {
            RuntimeError::new(
                "protocol_error",
                "the remote API response is not valid JSON",
            )
            .with_endpoint(&endpoint)
        })?;
        let Some(got) = value.get("id").and_then(Value::as_str) else {
            return Err(RuntimeError::new(
                "protocol_error",
                "the remote API response has no textual id",
            )
            .with_endpoint(&endpoint));
        };
        let same_id = got == id;
        // Same contract as the Local `ApiClient`: an error is kept even when its id diverges (the
        // engine answers requests it cannot deserialize with `"id": ""`), marked `:id_mismatch`
        // so it never reads as this request's own error; a success needs the exact id.
        if let Some(error) = value.get("error") {
            let mut runtime = RuntimeError::new(
                error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("api_error"),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("remote API error"),
            )
            .with_endpoint(&endpoint);
            if !same_id {
                runtime.code = format!("{}:id_mismatch", runtime.code);
            }
            return Err(runtime);
        }
        if !same_id {
            return Err(RuntimeError::new(
                "response_id_mismatch",
                "the response id does not match the request",
            )
            .with_endpoint(&endpoint));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

// ---------------------------------------------------------------------------------------
// Gateway
// ---------------------------------------------------------------------------------------

/// Connections opened by this process; part of endpoint request ids (the sequence inside the
/// correlator already keeps ids unique).
static SSH_CONNECTIONS: AtomicU64 = AtomicU64::new(1);

struct Shared {
    /// Boot served by the connection and correlation of endpoint command replies.
    endpoint: Arc<EndpointCorrelator>,
    health: Mutex<HealthMonitor>,
    stop: AtomicBool,
    failed: AtomicBool,
    /// Cause of the loss, reported once as `Disconnected` by whoever takes it first.
    lost: Mutex<Option<RuntimeError>>,
}

/// Negotiated welcome and the binary that produced it (one `SshGateway::start` argument).
struct Negotiation {
    negotiated: Negotiated,
    remote_binary: RemoteBinary,
    handshake_latency_ms: u64,
}

/// Visual endpoint lane of one SSH host.
pub struct SshGateway {
    endpoint: String,
    session: String,
    negotiated: Negotiated,
    /// Binary chosen for this connection (spec 029); the tooltip projects it.
    remote_binary: RemoteBinary,
    shared: Arc<Shared>,
    writer: Option<SyncSender<ClientMessage>>,
    events: Option<EventStream>,
    child: Arc<Mutex<Box<dyn SshChild>>>,
    api: SshApiLane,
    handshake_latency_ms: u64,
}

fn lost_error(endpoint: &str, code: &str, message: &str) -> RuntimeError {
    RuntimeError::new(code, message)
        .retryable()
        .with_endpoint(endpoint)
}

/// Records the loss once, fails the pending endpoint request and kills the local ssh process
/// (which ends the reader). Never blocks on the event queue.
fn fail(shared: &Shared, child: &Mutex<Box<dyn SshChild>>, error: RuntimeError) {
    {
        let mut lost = shared.lost.lock().unwrap_or_else(|p| p.into_inner());
        if shared.stop.load(Ordering::Acquire) || shared.failed.swap(true, Ordering::AcqRel) {
            return;
        }
        *lost = Some(error);
    }
    shared.endpoint.close();
    if let Ok(mut child) = child.lock() {
        child.kill();
    }
}

/// Delivers the recorded loss once (held in order if the queue is full), unless detaching.
fn report_loss(shared: &Shared, events: &EventSender) {
    if shared.stop.load(Ordering::Acquire) {
        return;
    }
    let error = shared.lost.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(error) = error {
        events.control(GatewayEvent::Disconnected(error));
    }
}

impl SshGateway {
    fn start(
        endpoint: String,
        session: String,
        negotiation: Negotiation,
        child: Box<dyn SshChild>,
        stdin: Box<dyn Write + Send>,
        stdout: Box<dyn Read + Send>,
        api: SshApiLane,
    ) -> Self {
        let Negotiation {
            negotiated,
            remote_binary,
            handshake_latency_ms,
        } = negotiation;
        let now = Instant::now();
        let shared = Arc::new(Shared {
            endpoint: Arc::new(EndpointCorrelator::new(
                endpoint.clone(),
                SSH_CONNECTIONS.fetch_add(1, Ordering::AcqRel),
            )),
            health: Mutex::new(HealthMonitor::new(now)),
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            lost: Mutex::new(None),
        });
        let child = Arc::new(Mutex::new(child));
        // Only the reader and the health thread produce events: the writer may outlive the
        // connection through a kept endpoint lane and must not keep the stream open.
        let (event_tx, event_rx) = event_queue();
        let (writer_tx, writer_rx) = sync_channel::<ClientMessage>(WRITER_SLOTS);

        {
            let (shared, child, endpoint) = (shared.clone(), child.clone(), endpoint.clone());
            let _ = std::thread::Builder::new()
                .name("herdr-desktop-ssh-writer".into())
                .spawn(move || {
                    let mut stdin = stdin;
                    while let Ok(message) = writer_rx.recv() {
                        let detach = matches!(message, ClientMessage::Detach);
                        if write_message(&mut stdin, &message).is_err() || stdin.flush().is_err() {
                            fail(
                                &shared,
                                &child,
                                lost_error(
                                    &endpoint,
                                    "connection_lost",
                                    "the SSH connection was closed",
                                ),
                            );
                            return;
                        }
                        shared.health.lock().expect("health").sent(Instant::now());
                        if detach {
                            return;
                        }
                    }
                });
        }
        {
            let (shared, child, events, endpoint) = (
                shared.clone(),
                child.clone(),
                event_tx.clone(),
                endpoint.clone(),
            );
            let _ = std::thread::Builder::new()
                .name("herdr-desktop-ssh-reader".into())
                .spawn(move || reader_loop(stdout, &shared, &child, events, &endpoint));
        }
        {
            let (shared, child, events, endpoint, writer) = (
                shared.clone(),
                child.clone(),
                event_tx,
                endpoint.clone(),
                writer_tx.clone(),
            );
            let _ = std::thread::Builder::new()
                .name("herdr-desktop-ssh-health".into())
                .spawn(move || loop {
                    std::thread::sleep(HEALTH_TICK);
                    if shared.stop.load(Ordering::Acquire) || shared.failed.load(Ordering::Acquire)
                    {
                        return;
                    }
                    let now = Instant::now();
                    let action = shared.health.lock().expect("health").action(now);
                    match action {
                        HealthAction::None => {}
                        HealthAction::Ping => {
                            let ping = ClientMessage::EndpointControl {
                                kind: HEALTH_PING_KIND.into(),
                                data: String::new(),
                            };
                            if writer.try_send(ping).is_ok() {
                                shared.health.lock().expect("health").ping_sent(now);
                            }
                        }
                        HealthAction::Expired => {
                            fail(
                                &shared,
                                &child,
                                lost_error(
                                    &endpoint,
                                    "health_timeout",
                                    "the remote server did not answer the health check",
                                ),
                            );
                            report_loss(&shared, &events);
                            return;
                        }
                    }
                });
        }

        Self {
            endpoint,
            session,
            negotiated,
            remote_binary,
            shared,
            writer: Some(writer_tx),
            events: Some(event_rx),
            child,
            api,
            handshake_latency_ms,
        }
    }

    pub fn handshake_latency_ms(&self) -> u64 {
        self.handshake_latency_ms
    }

    pub fn negotiated(&self) -> &Negotiated {
        &self.negotiated
    }

    /// Remote binary chosen by discovery for this connection (spec 029).
    pub fn remote_binary(&self) -> &RemoteBinary {
        &self.remote_binary
    }

    pub fn api_lane(&self) -> SshApiLane {
        self.api.clone()
    }

    fn enqueue(&self, message: ClientMessage) -> Result<(), RuntimeError> {
        if self.shared.failed.load(Ordering::Acquire) {
            return Err(lost_error(
                &self.endpoint,
                "connection_lost",
                "the SSH connection was closed",
            ));
        }
        let Some(writer) = self.writer.as_ref() else {
            return Err(lost_error(
                &self.endpoint,
                "not_connected",
                "disconnected from the SSH host",
            ));
        };
        writer.try_send(message).map_err(|error| match error {
            TrySendError::Full(_) => lost_error(
                &self.endpoint,
                "input_queue_full",
                "this host's send queue is full; the input was not sent",
            ),
            TrySendError::Disconnected(_) => lost_error(
                &self.endpoint,
                "connection_lost",
                "the SSH connection was closed",
            ),
        })
    }

    /// Endpoint command lane of this connection; `None` once detached.
    pub fn endpoint_lane(&self) -> Option<SshEndpointLane> {
        if self.shared.stop.load(Ordering::Acquire) {
            return None;
        }
        Some(SshEndpointLane {
            endpoint: self.endpoint.clone(),
            writer: self.writer.clone()?,
            shared: self.shared.clone(),
            methods: Arc::new(self.negotiated.methods.clone()),
            timeout: ENDPOINT_TIMEOUT,
            limit: ENDPOINT_RESPONSE_LIMIT,
        })
    }
}

/// Endpoint command lane of one SSH connection: requests go through the connection's writer
/// and replies are correlated by the reader, so a caller waits without holding any lock of
/// the gateway's owner. Detach or loss fails the pending request at once.
#[derive(Clone)]
pub struct SshEndpointLane {
    endpoint: String,
    writer: SyncSender<ClientMessage>,
    shared: Arc<Shared>,
    methods: Arc<Vec<String>>,
    timeout: Duration,
    limit: usize,
}

impl SshEndpointLane {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_response_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Methods announced by the remote server's welcome.
    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    /// One announced command for `boot_id`, correlated with its reply.
    pub fn request(
        &self,
        boot_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, RuntimeError> {
        self.request_admitted(boot_id, method, params, &|| Ok(()))
    }

    /// [`Self::request`] whose `admit` runs once this requester holds the lane's turn (after the
    /// request ahead of it finished) and before the write; a refusal writes nothing. After an
    /// accepted admission the correlator state and the writer queue are still acquired before the
    /// bytes go out: facts admitted there (focus, attachment) may change in that residual window.
    pub fn request_admitted(
        &self,
        boot_id: &str,
        method: &str,
        params: Value,
        admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        if !self.methods.iter().any(|m| m == method) {
            return Err(RuntimeError::new(
                "unsupported_method",
                format!("the {method} method is not available on this host"),
            )
            .with_endpoint(&self.endpoint));
        }
        self.shared.endpoint.request_guarded(
            boot_id,
            method,
            params,
            self.timeout,
            self.limit,
            admit,
            |message| {
                if self.shared.stop.load(Ordering::Acquire)
                    || self.shared.failed.load(Ordering::Acquire)
                {
                    return Err(lost_error(
                        &self.endpoint,
                        "not_connected",
                        "disconnected from the SSH host",
                    ));
                }
                // Not sent when the queue is full or gone: the result is known (nothing ran).
                self.writer.try_send(message).map_err(|error| match error {
                    TrySendError::Full(_) => lost_error(
                        &self.endpoint,
                        "request_queue_full",
                        "this host's send queue is full; the command was not sent",
                    ),
                    TrySendError::Disconnected(_) => lost_error(
                        &self.endpoint,
                        "not_connected",
                        "disconnected from the SSH host",
                    ),
                })
            },
        )
    }
}

impl crate::connections::hub::EndpointLane for SshEndpointLane {
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        SshEndpointLane::request(self, boot_id, method, params)
    }

    fn request_admitted(
        &self,
        boot_id: &str,
        method: &str,
        params: Value,
        admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<Value, RuntimeError> {
        SshEndpointLane::request_admitted(self, boot_id, method, params, admit)
    }

    fn methods(&self) -> Vec<String> {
        SshEndpointLane::methods(self).to_vec()
    }
}

fn reader_loop(
    mut stdout: Box<dyn Read + Send>,
    shared: &Shared,
    child: &Mutex<Box<dyn SshChild>>,
    events: EventSender,
    endpoint: &str,
) {
    // Whatever ends the reader fails the pending endpoint request at once.
    struct CloseOnExit<'a>(&'a EndpointCorrelator);
    impl Drop for CloseOnExit<'_> {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = CloseOnExit(&shared.endpoint);
    loop {
        // Never blocks on the queue: while held controls fill the backlog, stop reading
        // (server backpressure) but keep observing detach and loss.
        loop {
            if shared.stop.load(Ordering::Acquire) || !events.flush() {
                return;
            }
            if shared.failed.load(Ordering::Acquire) {
                report_loss(shared, &events);
                return;
            }
            if !events.backlogged() {
                break;
            }
            std::thread::sleep(BACKLOG_POLL);
        }
        let frame = match read_frame(&mut stdout, MAX_FRAME_SIZE) {
            Ok(frame) => frame,
            Err(_) => {
                if shared.stop.load(Ordering::Acquire) {
                    return;
                }
                if shared.failed.load(Ordering::Acquire) {
                    report_loss(shared, &events);
                    return;
                }
                let (status, stderr) = child
                    .lock()
                    .map(|mut c| (c.wait_exit(Duration::from_secs(1)), c.stderr_text()))
                    .unwrap_or((None, String::new()));
                let mut error = match status {
                    Some(code) => classify_ssh_failure(endpoint, code, &stderr)
                        .error()
                        .clone(),
                    None => {
                        lost_error(endpoint, "connection_lost", "the SSH connection was closed")
                    }
                };
                // Any loss of the visual lane is retried; attention is decided by the probe.
                error.retryable = true;
                fail(shared, child, error);
                report_loss(shared, &events);
                return;
            }
        };
        shared
            .health
            .lock()
            .expect("health")
            .received(Instant::now());
        let message: ServerMessage = match decode_message(&frame) {
            Ok(message) => message,
            Err(_) => {
                let tag = peek_tag(&frame).unwrap_or(u32::MAX);
                if tag > SERVER_MESSAGE_MAX_TAG {
                    if !events.control(GatewayEvent::Unsupported { tag }) {
                        return;
                    }
                    continue;
                }
                fail(
                    shared,
                    child,
                    lost_error(endpoint, "protocol_error", "invalid server message"),
                );
                report_loss(shared, &events);
                return;
            }
        };
        let event = match message {
            ServerMessage::EndpointControl { kind, data } => {
                if kind != ENDPOINT_SNAPSHOT_KIND {
                    // Health pong and optional named controls: receipt already recorded.
                    continue;
                }
                match serde_json::from_str::<ClientShellSnapshot>(&data) {
                    Ok(snapshot) => {
                        shared.endpoint.observe_boot(&snapshot.boot_id);
                        shared.health.lock().expect("health").ready();
                        GatewayEvent::Snapshot(Box::new(snapshot))
                    }
                    Err(_) => GatewayEvent::ShellError("invalid endpoint snapshot".into()),
                }
            }
            ServerMessage::ClientShellSnapshot(snapshot) => {
                shared.endpoint.observe_boot(&snapshot.boot_id);
                shared.health.lock().expect("health").ready();
                GatewayEvent::Snapshot(snapshot)
            }
            ServerMessage::PaneSurface(surface) => GatewayEvent::Surface(Box::new(surface)),
            ServerMessage::PaneSurfacePatch(patch) => GatewayEvent::Patch(Box::new(patch)),
            ServerMessage::ClientShellEndpointResponseChunk {
                boot_id,
                request_id,
                final_chunk,
                data,
            } => {
                // Replies go straight to the requester; late or foreign ids are ignored.
                let _ = shared
                    .endpoint
                    .chunk(&boot_id, &request_id, final_chunk, &data);
                continue;
            }
            ServerMessage::ClientShellError { message } => GatewayEvent::ShellError(message),
            ServerMessage::ServerShutdown { reason } => GatewayEvent::Shutdown(reason),
            _ => continue,
        };
        // Frames are bounded by bytes and count, dropped with one notice arranged at once;
        // controls are held in order. Neither waits for the consumer.
        let delivered = if matches!(event, GatewayEvent::Surface(_) | GatewayEvent::Patch(_)) {
            events.frame(frame.len(), event)
        } else {
            events.control(event)
        };
        if !delivered {
            return;
        }
    }
}

impl RuntimeGateway for SshGateway {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn identity(&self) -> Option<LiveIdentity> {
        let boot_id = self.shared.endpoint.boot_id()?;
        Some(LiveIdentity {
            endpoint: self.endpoint.clone(),
            session: self.session.clone(),
            connection_generation: 1,
            boot_id,
        })
    }

    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(RuntimeError::new(
            "use_connector",
            "SSH connections are opened by SshConnector",
        )
        .with_endpoint(&self.endpoint))
    }

    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        self.events.take()?.into_receiver()
    }

    fn api_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.api.request(method, params)
    }

    fn endpoint_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let Some(lane) = self.endpoint_lane() else {
            return Err(lost_error(
                &self.endpoint,
                "not_connected",
                "disconnected from the SSH host",
            ));
        };
        let boot_id = self.shared.endpoint.boot_id().ok_or_else(|| {
            RuntimeError::new("boot_unknown", "the server identity is not established yet")
                .retryable()
                .with_endpoint(&self.endpoint)
        })?;
        lane.request(&boot_id, method, params)
    }

    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        let live = self.identity().ok_or_else(|| {
            RuntimeError::new("boot_unknown", "the server identity is not established yet")
                .retryable()
                .with_endpoint(&self.endpoint)
        })?;
        target.validate(&live)?;
        if events.is_empty() {
            return Ok(());
        }
        self.enqueue(ClientMessage::ClientShellPaneInput {
            pane_id: target.pane_id.clone(),
            events,
        })
    }

    fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        self.enqueue(ClientMessage::ClientShellResize {
            cell_width_px: geometry.cell_width_px,
            cell_height_px: geometry.cell_height_px,
            surface_size: geometry.surface_size(),
            pixel_mouse: false,
        })
    }

    fn set_focus(&self, focused: bool) -> Result<(), RuntimeError> {
        self.enqueue(ClientMessage::ClientShellFocus { focused })
    }

    fn set_host_theme(
        &self,
        updates: &[herdr_client::protocol::wire::ClientHostThemeUpdate],
    ) -> Result<(), RuntimeError> {
        for update in updates {
            self.enqueue(ClientMessage::ClientShellHostTheme {
                update: update.clone(),
            })?;
        }
        Ok(())
    }

    fn detach(&mut self) {
        if self.shared.stop.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shared.endpoint.close();
        if let Some(writer) = self.writer.take() {
            let _ = writer.try_send(ClientMessage::Detach);
        }
        let child = self.child.clone();
        // Give the Detach a moment to reach the server, then close only the local ssh client.
        let _ = std::thread::Builder::new()
            .name("herdr-desktop-ssh-detach".into())
            .spawn(move || {
                if let Ok(mut child) = child.lock() {
                    if child.wait_exit(Duration::from_millis(300)).is_none() {
                        child.kill();
                    }
                }
            });
        self.events = None;
    }

    fn is_connected(&self) -> bool {
        !self.shared.stop.load(Ordering::Acquire) && !self.shared.failed.load(Ordering::Acquire)
    }
}

impl Drop for SshGateway {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(test)]
mod host_theme_tests {
    use super::*;
    use crate::connections::ssh_options::ProfileId;
    use herdr_client::protocol::wire::{
        ClientHostAppearance, ClientHostColor as Color, ClientHostDefaultColorKind as Kind,
        ClientHostThemeUpdate as Update,
    };

    struct UnusedChild;
    impl SshChild for UnusedChild {
        fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
            panic!("no process")
        }
        fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
            panic!("no process")
        }
        fn stderr_text(&self) -> String {
            panic!("no process")
        }
        fn wait_exit(&mut self, _: Duration) -> Option<Option<i32>> {
            panic!("no process")
        }
        fn kill(&mut self) {
            panic!("no process")
        }
    }

    #[test]
    fn ssh_host_theme_enqueues_four_ordered_messages_and_nothing_else() {
        let identity = SshIdentity::new(
            ProfileId::parse("00000000000040008000000000000066").unwrap(),
            "fake.example",
            None,
            "spec066-fake",
        )
        .unwrap();
        let connector = SshConnector::new(identity, None, Arc::new(OpenSshRunner));
        let (writer, queue) = sync_channel(16);
        let gateway = SshGateway {
            endpoint: "spec066".into(),
            session: "spec066-fake".into(),
            negotiated: Negotiated {
                identity: LiveIdentity {
                    endpoint: "spec066".into(),
                    session: "spec066-fake".into(),
                    connection_generation: 1,
                    boot_id: "fake".into(),
                },
                generation: 1,
                server_version: "fake".into(),
                methods: vec![],
                capabilities: vec![],
            },
            remote_binary: RemoteBinary::on_path(),
            shared: Arc::new(Shared {
                endpoint: Arc::new(EndpointCorrelator::new("spec066", 1)),
                health: Mutex::new(HealthMonitor::new(Instant::now())),
                stop: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                lost: Mutex::new(None),
            }),
            writer: Some(writer),
            events: None,
            child: Arc::new(Mutex::new(Box::new(UnusedChild))),
            api: connector.api_lane(),
            handshake_latency_ms: 0,
        };
        let updates = vec![
            Update::DefaultColor {
                kind: Kind::Background,
                color: Color {
                    r: 15,
                    g: 15,
                    b: 15,
                },
            },
            Update::DefaultColor {
                kind: Kind::Foreground,
                color: Color {
                    r: 237,
                    g: 237,
                    b: 237,
                },
            },
            Update::PaletteColors((0..16).map(|i| (i, Color { r: i, g: i, b: i })).collect()),
            Update::Appearance(ClientHostAppearance::Dark),
        ];
        let result = gateway.set_host_theme(&updates);
        // No child process exists in this queue seam, including on Drop.
        gateway.shared.stop.store(true, Ordering::Release);
        result.unwrap();
        let messages: Vec<_> = queue.try_iter().collect();
        assert_eq!(
            messages,
            updates
                .into_iter()
                .map(|update| ClientMessage::ClientShellHostTheme { update })
                .collect::<Vec<_>>()
        );
    }
}
