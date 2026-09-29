//! Spec 031 — the SSH bridge must open from the real window like the TUI does.
//!
//! The regression tests drive the **Tauri command path** (`ConnectionsState::connection_*`, the
//! exact futures the window's IPC wrappers await), never the hub directly:
//! - `command_path_opens_the_bridge_for_a_host_without_surface`: a host that is not selected
//!   still opens the bridge (`surface_active=false`, default hub geometry), reaches `Online` and
//!   lists workspaces; selecting it activates the surface with a new hello.
//! - `command_path_bounds_a_stuck_bridge_start`: a bridge start that never returns (the measured
//!   window symptom: probes ok, no `handshake` line, no ssh child, attempt stuck in
//!   `Connecting`) fails with an explicit code inside the 15 s budget.
//! - `ssh_agent_without_socket_fails_before_any_process` and
//!   `unreachable_host_reports_the_timeout_message`: the two explicit failures of AC-031-02.
//!
//! The `#[ignore]` probe is the read-only real-machine proof of AC-031-03:
//! `cargo nextest run -p herdr-desktop --test spec_031_ssh --run-ignored all --no-capture`
//! Only the client bridge attachment and the read-only `workspace.list` run on the host.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use herdr_client::protocol::endpoint::{
    EndpointClientHello, ENDPOINT_HELLO_KIND, ENDPOINT_SNAPSHOT_KIND, ENDPOINT_WELCOME_KIND,
};
use herdr_client::protocol::wire::{ClientMessage, ServerMessage};
use herdr_client::protocol::{decode_message, read_frame, write_message, MAX_FRAME_SIZE};
use herdr_client::SurfaceGeometry;
use herdr_desktop::bridge::ssh::{
    OpenSshRunner, ProcessOutput, SshChild, SshRunner, BRIDGE_START_TIMEOUT,
};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::profiles::{SshProfileDraft, SSH_AGENT_AUTH};
use herdr_desktop::connections::ssh_options::{IsolatedSshConfig, OpenSshCommand};

const TARGET: &str = "ec2-user@mac-mini";
const SESSION: &str = "default";
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(70);
const DISCOVERED: &str = "/Users/ec2-user/.local/bin/herdr";
const CLIENT_OK: &str = r#"{"version":"0.9.0","channel":"stable","protocol":22,"endpoint_protocol_generation":1,"endpoint_capabilities":["surface_interest","presentation_effects_fence","health_check"],"binary":"/Users/ec2-user/.local/bin/herdr","session":null}"#;
const SERVER_OK: &str = r#"{"status":"running","running":true,"version":"0.9.0","protocol":22,"capabilities":{"live_handoff":true,"detached_server_daemon":true,"endpoint_protocol_generation":1,"surface_interest":true,"health_check":true},"compatible":true,"endpoint_compatible":true,"socket":"/Users/ec2-user/.config/herdr/herdr.sock","session":null,"restart_needed":false,"server_binary_stale":false}"#;
const HERDR_MISSING: &str = "zsh:1: command not found: herdr\n";

fn elapsed(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

fn geometry() -> SurfaceGeometry {
    SurfaceGeometry {
        cols: 100,
        rows: 30,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn stdout(status: i32, text: &str) -> ProcessOutput {
    ProcessOutput {
        status: Some(status),
        stdout: text.as_bytes().to_vec(),
        stderr: String::new(),
    }
}

fn config(
    prefs: &tempfile::TempDir,
    config_dir: &tempfile::TempDir,
    state_dir: &tempfile::TempDir,
) -> ConnectionsConfig {
    ConnectionsConfig {
        prefs_dir: prefs.path().to_path_buf(),
        herdr_config_dir: config_dir.path().to_path_buf(),
        herdr_state_dir: state_dir.path().to_path_buf(),
        local_session: None,
        local_auto_start: false,
        herdr_bin: PathBuf::from("herdr"),
        isolated_ssh: None,
        geometry: geometry(),
    }
}

fn draft(label: &str, auth: Option<&str>) -> SshProfileDraft {
    SshProfileDraft {
        id: None,
        label: label.into(),
        target: TARGET.into(),
        port: None,
        session: SESSION.into(),
        auth: auth.map(str::to_owned),
    }
}

fn endpoint_of(view: &herdr_desktop::connections::commands::ConnectionsView) -> String {
    view.profiles
        .last()
        .expect("saved profile")
        .id
        .as_str()
        .to_owned()
}

fn host_of(state: &ConnectionsState, endpoint: &str) -> herdr_desktop::connections::hub::HostDto {
    state
        .view()
        .hub
        .hosts
        .iter()
        .find(|host| host.endpoint == endpoint)
        .cloned()
        .expect("host registered")
}

fn wait_for<F>(timeout: Duration, mut done: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------------------
// Fake OpenSSH runner for the command path: scripted probes plus a scripted bridge.
// ---------------------------------------------------------------------------------------

/// Answers like the measured mac-mini: bare `herdr` missing, the discovered binary serving the
/// endpoint, and a live `status server` probe. Each `spawn` consumes one socket pair: the child
/// end goes to the connector, the server end runs a fake remote bridge.
struct ProbeRunner {
    commands: Mutex<Vec<OpenSshCommand>>,
    /// (child end, server end) of the bridge socket pairs, in order.
    bridges: Mutex<
        VecDeque<(
            std::os::unix::net::UnixStream,
            std::os::unix::net::UnixStream,
        )>,
    >,
    /// Hello received by each fake remote bridge, in order.
    hellos: Arc<Mutex<Vec<EndpointClientHello>>>,
}

impl ProbeRunner {
    fn new(
        pairs: Vec<(
            std::os::unix::net::UnixStream,
            std::os::unix::net::UnixStream,
        )>,
    ) -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            bridges: Mutex::new(pairs.into()),
            hellos: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn hellos(&self) -> Vec<EndpointClientHello> {
        self.hellos.lock().unwrap().clone()
    }
}

/// The measured probe answers, shared by every fake runner of this file.
fn probe_output(command: &OpenSshCommand, stdin: Option<&[u8]>) -> io::Result<ProcessOutput> {
    let tail = command.args.last().cloned().unwrap_or_default();
    let input = String::from_utf8_lossy(stdin.unwrap_or_default()).into_owned();
    if tail == "/bin/sh -s" {
        if input.starts_with("command -v herdr") {
            return Ok(stdout(1, ""));
        }
        if input.starts_with("test -x") {
            return Ok(stdout(0, CLIENT_OK));
        }
        return Ok(stdout(0, &format!("{DISCOVERED}\n")));
    }
    if tail.contains("status server --json") {
        if tail.contains(DISCOVERED) {
            return Ok(stdout(0, SERVER_OK));
        }
        return Ok(ProcessOutput {
            status: Some(127),
            stdout: vec![],
            stderr: HERDR_MISSING.into(),
        });
    }
    if tail.contains("status client --json") {
        return Ok(stdout(0, CLIENT_OK));
    }
    if tail.contains("remote-api-bridge") {
        let request: serde_json::Value = serde_json::from_str(input.trim()).unwrap();
        return Ok(stdout(
            0,
            &format!(
                "{}\n",
                serde_json::json!({
                    "id": request["id"],
                    "result": { "workspaces": [ { "workspace_id": "w6", "label": "~" } ] }
                })
            ),
        ));
    }
    Err(io::Error::other(format!("unexpected command: {tail}")))
}

impl SshRunner for ProbeRunner {
    fn output(
        &self,
        command: &OpenSshCommand,
        _timeout: Duration,
        stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput> {
        self.commands.lock().unwrap().push(command.clone());
        probe_output(command, stdin)
    }

    fn spawn(&self, command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
        self.commands.lock().unwrap().push(command.clone());
        let (client, server) = self
            .bridges
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| io::Error::other("no fake bridge left"))?;
        let recorded = self.hellos.clone();
        std::thread::Builder::new()
            .name("spec-031-fake-remote".into())
            .spawn(move || {
                let mut server = server;
                let hello: ClientMessage =
                    decode_message(&read_frame(&mut server, MAX_FRAME_SIZE).unwrap()).unwrap();
                let ClientMessage::EndpointControl { kind, data } = hello else {
                    return;
                };
                assert_eq!(kind, ENDPOINT_HELLO_KIND);
                let hello: EndpointClientHello = serde_json::from_str(&data).unwrap();
                recorded.lock().unwrap().push(hello);
                let mut welcome: herdr_client::protocol::endpoint::EndpointServerWelcome =
                    serde_json::from_str(
                        &std::fs::read_to_string(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../tests/fixtures/endpoint-welcome-v1.json"
                        ))
                        .unwrap(),
                    )
                    .unwrap();
                for capability in ["surface_interest", "health_check"] {
                    if !welcome.capabilities.iter().any(|c| c == capability) {
                        welcome.capabilities.push(capability.to_owned());
                    }
                }
                write_message(
                    &mut server,
                    &ServerMessage::EndpointControl {
                        kind: ENDPOINT_WELCOME_KIND.into(),
                        data: serde_json::to_string(&welcome).unwrap(),
                    },
                )
                .unwrap();
                let mut snapshot: herdr_client::protocol::wire::ClientShellSnapshot =
                    serde_json::from_str(
                        &std::fs::read_to_string(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../tests/fixtures/endpoint-snapshot-v1.json"
                        ))
                        .unwrap(),
                    )
                    .unwrap();
                snapshot.boot_id = "boot-spec-031".into();
                write_message(
                    &mut server,
                    &ServerMessage::EndpointControl {
                        kind: ENDPOINT_SNAPSHOT_KIND.into(),
                        data: serde_json::to_string(&snapshot).unwrap(),
                    },
                )
                .unwrap();
                // Keep the bridge alive: the hub only drops it on detach/loss.
                let mut buf = [0u8; 4096];
                while server.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
            })
            .map_err(io::Error::other)?;
        Ok(Box::new(PipeChild {
            stdin: Some(client.try_clone()?),
            stdout: Some(client.try_clone()?),
            socket: client,
            killed: false,
        }))
    }

    fn bridge_start_timeout(&self) -> Duration {
        Duration::from_secs(1)
    }
}

struct PipeChild {
    socket: std::os::unix::net::UnixStream,
    stdin: Option<std::os::unix::net::UnixStream>,
    stdout: Option<std::os::unix::net::UnixStream>,
    killed: bool,
}

impl SshChild for PipeChild {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.stdin
            .take()
            .map(|s| Box::new(s) as Box<dyn Write + Send>)
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>)
    }
    fn stderr_text(&self) -> String {
        String::new()
    }
    fn wait_exit(&mut self, _timeout: Duration) -> Option<Option<i32>> {
        self.killed.then_some(None)
    }
    fn kill(&mut self) {
        self.killed = true;
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

/// Answers every probe like the measured host, but `spawn` blocks forever (the measured window
/// symptom: probes ok, no handshake, attempt stuck in `Connecting`).
struct StuckRunner {
    hold: Mutex<Option<Receiver<()>>>,
    spawns: Mutex<u32>,
}

impl SshRunner for StuckRunner {
    fn output(
        &self,
        command: &OpenSshCommand,
        _timeout: Duration,
        stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput> {
        probe_output(command, stdin)
    }
    fn spawn(&self, _command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
        *self.spawns.lock().unwrap() += 1;
        let hold = self.hold.lock().unwrap().take().expect("one bridge");
        let _ = hold.recv();
        Err(io::Error::other("never reached"))
    }
    fn bridge_start_timeout(&self) -> Duration {
        Duration::from_millis(250)
    }
}

/// Only records commands; never starts anything.
#[derive(Default)]
struct RecordingRunner {
    commands: Mutex<Vec<OpenSshCommand>>,
}

impl SshRunner for RecordingRunner {
    fn output(
        &self,
        command: &OpenSshCommand,
        _timeout: Duration,
        _stdin: Option<&[u8]>,
    ) -> io::Result<ProcessOutput> {
        self.commands.lock().unwrap().push(command.clone());
        Err(io::Error::other("must not be called"))
    }
    fn spawn(&self, command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
        self.commands.lock().unwrap().push(command.clone());
        Err(io::Error::other("must not be called"))
    }
}

// ---------------------------------------------------------------------------------------
// AC-031-01 — the command path always opens the bridge, surface or not.
// ---------------------------------------------------------------------------------------

#[test]
fn command_path_opens_the_bridge_for_a_host_without_surface() {
    let prefs = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let pairs = (0..2)
        .map(|_| std::os::unix::net::UnixStream::pair().unwrap())
        .collect::<Vec<_>>();
    let runner = Arc::new(ProbeRunner::new(pairs));
    let state =
        ConnectionsState::with_runner(config(&prefs, &config_dir, &state_dir), runner.clone());

    // Register the profile without connecting, then take its surface away: the host is not
    // selected in the window.
    let view = tauri::async_runtime::block_on(
        state.connection_profile_save(draft("spec-031", None), false),
    )
    .expect("profile saved");
    let endpoint = endpoint_of(&view);
    state.set_visible(&endpoint, false).expect("host hidden");

    // Exactly the future `connection_connect` awaits.
    tauri::async_runtime::block_on(state.connection_connect(endpoint.clone()))
        .expect("connect asked");
    assert!(
        wait_for(Duration::from_secs(10), || {
            let host = host_of(&state, &endpoint);
            host.phase == herdr_desktop::connections::state::LinkPhase::Online
                || host.phase == herdr_desktop::connections::state::LinkPhase::Attention
                || host.connection_error.is_some()
        }),
        "the command path reached no outcome: {:?}",
        host_of(&state, &endpoint)
    );
    let host = host_of(&state, &endpoint);
    assert_eq!(
        host.phase,
        herdr_desktop::connections::state::LinkPhase::Online,
        "{:?}",
        host.connection_error
    );
    let first = runner.hellos()[0].clone();
    assert!(
        !first.surface_active,
        "a host without surface connects with surface_active=false"
    );
    assert_eq!(
        (first.surface_size.cols, first.surface_size.rows),
        (100, 30),
        "the default hub geometry travels in the hello"
    );

    // Workspaces through the command path (`connection_workspaces`).
    let workspaces = tauri::async_runtime::block_on(state.connection_workspaces(endpoint.clone()))
        .expect("workspaces");
    assert_eq!(
        workspaces
            .iter()
            .map(|w| (w.workspace_id.as_str(), w.label.as_str()))
            .collect::<Vec<_>>(),
        vec![("w6", "~")]
    );

    // Selecting the host activates the surface on a new connection.
    state.set_visible(&endpoint, true).expect("host selected");
    assert!(
        wait_for(Duration::from_secs(10), || {
            runner.hellos().len() >= 2
                && host_of(&state, &endpoint).phase
                    == herdr_desktop::connections::state::LinkPhase::Online
        }),
        "selection did not renegotiate the surface: {:?}",
        host_of(&state, &endpoint)
    );
    assert!(
        runner.hellos()[1].surface_active,
        "selecting the host activates the surface"
    );
    let spawns: Vec<OpenSshCommand> = runner
        .commands
        .lock()
        .unwrap()
        .iter()
        .filter(|c| {
            c.args
                .last()
                .is_some_and(|a| a.contains("remote-client-bridge"))
        })
        .cloned()
        .collect();
    assert_eq!(spawns.len(), 2, "one bridge per connection: {spawns:?}");
    assert!(spawns
        .iter()
        .all(|c| c.args.last().unwrap().contains(DISCOVERED)));
    state.detach_all();
}

// ---------------------------------------------------------------------------------------
// AC-031-01/02 — a stuck bridge start fails explicitly inside the 15 s budget.
// ---------------------------------------------------------------------------------------

#[test]
fn command_path_bounds_a_stuck_bridge_start() {
    let prefs = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let (hold_tx, hold_rx) = channel();
    let runner = Arc::new(StuckRunner {
        hold: Mutex::new(Some(hold_rx)),
        spawns: Mutex::new(0),
    });
    let state =
        ConnectionsState::with_runner(config(&prefs, &config_dir, &state_dir), runner.clone());

    tauri::async_runtime::block_on(
        state.connection_profile_save(draft("spec-031-stuck", None), true),
    )
    .expect("connect asked");
    let endpoint = endpoint_of(&state.view());
    let bounded = wait_for(Duration::from_secs(10), || {
        let host = host_of(&state, &endpoint);
        host.connection_error
            .as_ref()
            .is_some_and(|error| error.code == "bridge_start_timeout")
    });
    let host = host_of(&state, &endpoint);
    if !bounded {
        state.detach_all();
        drop(hold_tx);
        panic!(
            "the attempt must fail with bridge_start_timeout inside the budget; state={:?}",
            (
                &host.phase,
                &host.connection_error,
                *runner.spawns.lock().unwrap()
            )
        );
    }
    assert_eq!(
        host.phase,
        herdr_desktop::connections::state::LinkPhase::Offline,
        "a transient bounded failure is retried with backoff, not stuck: {:?}",
        host.connection_error
    );
    state.detach_all();
    drop(hold_tx);
    // The production budget is the 15 s of the spec (never more).
    assert_eq!(OpenSshRunner.bridge_start_timeout(), BRIDGE_START_TIMEOUT);
    assert!(BRIDGE_START_TIMEOUT <= Duration::from_secs(15));
}

// ---------------------------------------------------------------------------------------
// AC-031-02 — explicit failures before any process: ssh-agent without socket, unreachable host.
// ---------------------------------------------------------------------------------------

#[test]
fn ssh_agent_without_socket_fails_before_any_process() {
    let prefs = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(RecordingRunner::default());
    let state =
        ConnectionsState::with_runner(config(&prefs, &config_dir, &state_dir), runner.clone());
    let previous = std::env::var_os("SSH_AUTH_SOCK");
    std::env::remove_var("SSH_AUTH_SOCK");

    let error = tauri::async_runtime::block_on(
        state.connection_profile_save(draft("spec-031-agent", Some(SSH_AGENT_AUTH)), true),
    )
    .expect_err("no agent socket must refuse the connect");
    assert_eq!(error.code, "ssh_agent_unavailable");
    assert_eq!(error.message, "ssh-agent is unavailable in this session");
    assert!(
        runner.commands.lock().unwrap().is_empty(),
        "no OpenSSH process runs for an unavailable agent"
    );
    assert!(
        !prefs.path().join("connections.json").exists(),
        "nothing is persisted for a refused auth choice"
    );

    // With the socket present the same draft proceeds to the probes (the runner answers them
    // with its refusal only when a process would really run).
    std::env::set_var("SSH_AUTH_SOCK", "/tmp/spec-031-agent.sock");
    let state =
        ConnectionsState::with_runner(config(&prefs, &config_dir, &state_dir), runner.clone());
    let view = tauri::async_runtime::block_on(
        state.connection_profile_save(draft("spec-031-agent", Some(SSH_AGENT_AUTH)), true),
    )
    .expect("with an agent the connect is asked");
    assert_eq!(view.profiles.len(), 1);
    state.detach_all();
    match previous {
        Some(value) => std::env::set_var("SSH_AUTH_SOCK", value),
        None => std::env::remove_var("SSH_AUTH_SOCK"),
    }
}

#[test]
fn unreachable_host_reports_the_timeout_message() {
    let prefs = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    /// Scripted first probe: OpenSSH gave up after ConnectTimeout=10 with one attempt.
    struct TimeoutRunner {
        commands: Mutex<Vec<OpenSshCommand>>,
    }
    impl SshRunner for TimeoutRunner {
        fn output(
            &self,
            command: &OpenSshCommand,
            _timeout: Duration,
            _stdin: Option<&[u8]>,
        ) -> io::Result<ProcessOutput> {
            self.commands.lock().unwrap().push(command.clone());
            Ok(ProcessOutput {
                status: Some(255),
                stdout: vec![],
                stderr: "ssh: connect to host 172.31.20.215 port 22: Connection timed out\r\n"
                    .into(),
            })
        }
        fn spawn(&self, command: &OpenSshCommand) -> io::Result<Box<dyn SshChild>> {
            self.commands.lock().unwrap().push(command.clone());
            Err(io::Error::other("no bridge for an unreachable host"))
        }
    }
    let runner = Arc::new(TimeoutRunner {
        commands: Mutex::new(Vec::new()),
    });
    let state =
        ConnectionsState::with_runner(config(&prefs, &config_dir, &state_dir), runner.clone());
    let view = tauri::async_runtime::block_on(state.connection_profile_save(
        SshProfileDraft {
            target: "ec2-user@172.31.20.215".into(),
            ..draft("spec-031-unreachable", None)
        },
        true,
    ))
    .expect("connect asked");
    let endpoint = endpoint_of(&view);
    assert!(
        wait_for(Duration::from_secs(5), || {
            host_of(&state, &endpoint)
                .connection_error
                .as_ref()
                .is_some_and(|error| error.code == "ssh_unreachable")
        }),
        "no explicit unreachable error: {:?}",
        host_of(&state, &endpoint)
    );
    let host = host_of(&state, &endpoint);
    let error = host.connection_error.as_ref().unwrap();
    assert_eq!(error.message, "host unreachable (timeout)");
    assert_eq!(
        host.phase,
        herdr_desktop::connections::state::LinkPhase::Offline,
        "transient: retried with backoff, never a stuck spinner"
    );
    // The bound itself: one attempt with the 10 s ConnectTimeout of the engine policy.
    let command = {
        let commands = runner.commands.lock().unwrap();
        commands
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("the unreachable probe ran: {commands:?}"))
    };
    let args = command.args.join(" ");
    assert!(args.contains("-o ConnectTimeout=10"), "{args}");
    assert!(args.contains("-o ConnectionAttempts=1"), "{args}");
    state.detach_all();
}

// ---------------------------------------------------------------------------------------
// AC-031-03 — real read-only probe through the Tauri command path.
// ---------------------------------------------------------------------------------------

#[test]
#[ignore = "real host read-only probe through the Tauri command path (TASK-031-03)"]
fn spec_031_real_command_path_probe() {
    let started = Instant::now();
    let prefs = tempfile::tempdir().expect("temporary desktop preferences");
    let herdr_config = tempfile::tempdir().expect("temporary Herdr config path");
    let herdr_state = tempfile::tempdir().expect("temporary Herdr state path");
    // Window path: no isolated SSH configuration; the same user OpenSSH configuration the
    // desktop uses, with the inherited Herdr session variables removed by the runner.
    let isolated_ssh = std::env::var("HERDR_SPEC031_ISOLATED")
        .ok()
        .map(|identity_file| IsolatedSshConfig {
            identity_file: PathBuf::from(identity_file),
            user_known_hosts_file: PathBuf::from("/home/user/.ssh/known_hosts"),
        });
    let state = ConnectionsState::new(ConnectionsConfig {
        prefs_dir: prefs.path().to_path_buf(),
        herdr_config_dir: herdr_config.path().to_path_buf(),
        herdr_state_dir: herdr_state.path().to_path_buf(),
        local_session: None,
        local_auto_start: false,
        herdr_bin: PathBuf::from("herdr"),
        isolated_ssh,
        geometry: geometry(),
    });

    eprintln!(
        "[031 probe +{}ms] início: alvo={TARGET} sessão={SESSION}; somente leitura, caminho do comando",
        elapsed(started)
    );
    // The exact future the `connection_profile_save` Tauri command awaits.
    let view = tauri::async_runtime::block_on(state.connection_profile_save(
        SshProfileDraft {
            id: None,
            label: "spec-031-probe".into(),
            target: TARGET.into(),
            port: None,
            session: SESSION.into(),
            auth: None,
        },
        true,
    ))
    .unwrap_or_else(|error| {
        panic!(
            "[031 probe +{}ms] salvar/iniciar perfil: {error:?}",
            elapsed(started)
        )
    });
    let endpoint = endpoint_of(&view);
    eprintln!(
        "[031 probe +{}ms] passo 1 solicitado: endpoint={endpoint}",
        elapsed(started)
    );

    let mut online = false;
    let mut last_signature = String::new();
    let deadline = started + OBSERVATION_TIMEOUT;
    while Instant::now() < deadline {
        let host = host_of(&state, &endpoint);
        let signature = format!(
            "phase={:?} attempt={} latency={:?} version={:?} generation={:?} error={:?}",
            host.phase,
            host.attempt,
            host.latency_ms,
            host.server_version,
            host.generation,
            host.connection_error
                .as_ref()
                .map(|error| (&error.code, &error.message)),
        );
        if signature != last_signature {
            eprintln!(
                "[031 probe +{}ms] passo 2 conexão: {signature}",
                elapsed(started)
            );
            last_signature = signature;
        }
        if matches!(
            host.phase,
            herdr_desktop::connections::state::LinkPhase::Online
        ) {
            online = true;
            break;
        }
        if matches!(
            host.phase,
            herdr_desktop::connections::state::LinkPhase::Attention
        ) || (host.connection_error.is_some()
            && matches!(
                host.phase,
                herdr_desktop::connections::state::LinkPhase::Offline
            ))
        {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    if !online {
        state.detach_all();
        panic!("[031 probe +{}ms] parou antes de Online", elapsed(started));
    }
    eprintln!(
        "[031 probe +{}ms] passo 3 handshake/endpoint: Online; iniciando workspace.list",
        elapsed(started)
    );
    let workspace_started = Instant::now();
    let workspace_result =
        tauri::async_runtime::block_on(state.connection_workspaces(endpoint.clone()));
    state.detach_all();
    match workspace_result {
        Ok(workspaces) => {
            assert!(
                !workspaces.is_empty(),
                "[031 probe] the real host must list its workspace"
            );
            eprintln!(
                "[031 probe +{}ms] passo 4 workspaces: {} itens em {}ms: {:?}",
                elapsed(started),
                workspaces.len(),
                workspace_started.elapsed().as_millis(),
                workspaces
                    .iter()
                    .map(|workspace| (&workspace.workspace_id, &workspace.label))
                    .collect::<Vec<_>>()
            );
        }
        Err(error) => {
            eprintln!(
                "[031 probe +{}ms] passo 4 workspaces: ERRO após {}ms: code={} message={}",
                elapsed(started),
                workspace_started.elapsed().as_millis(),
                error.code,
                error.message
            );
            panic!("[031 probe] leitura de workspaces parou no hub: {error:?}");
        }
    }
    eprintln!(
        "[031 probe +{}ms] fim: cliente SSH destacado; servidor/sessão remotos não alterados",
        elapsed(started)
    );
}
