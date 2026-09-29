//! Spec 007 — contracts of the composed surface, the shared selection and the services bound to
//! it (AC-007-04 seam; no GUI, no engine, no default session: every host is a fake behind the one
//! `ConnectionsState` hub of the window).
//!
//! Fixture values are distinct on purpose: boots `boot-local-7` / `boot-ssh-3`, sessions
//! `hd007s-local` / `hd007s-remote`, both hosts expose the same pane id `w1:p1`.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SessionPaths, SurfaceGeometry,
};
use herdr_desktop::bridge::agent_commands::{AgentsHost, AgentsState, EventStream};
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshRunner};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{ApiLane, Connected, EndpointLane, HostHub};
use herdr_desktop::connections::profiles::SshProfileDraft;
use herdr_desktop::connections::ssh_options::OpenSshCommand;
use serde_json::{json, Value};

// =======================================================================================
// Fixtures
// =======================================================================================

const LOCAL: &str = "local";
const PANE: &str = "w1:p1";
const WITHIN: Duration = Duration::from_secs(5);

fn geometry() -> SurfaceGeometry {
    SurfaceGeometry {
        cols: 12,
        rows: 2,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn full(boot: &str, revision: u64, text: &str) -> PaneSurfaceFrame {
    let (width, height) = (12u16, 2u16);
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells: text
                .chars()
                .chain(std::iter::repeat(' '))
                .take(usize::from(width * height))
                .map(|c| CellData {
                    symbol: c.to_string(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                })
                .collect(),
            width,
            height,
            cursor: Some(CursorState {
                x: 0,
                y: 0,
                visible: true,
                shape: 0,
            }),
            hyperlinks: vec![],
            graphics: vec![],
        },
        panes: vec![PaneSurfacePane {
            pane_id: PANE.into(),
            content_revision: 1,
            rect,
            inner_rect: rect,
            scrollbar_rect: None,
            scroll: None,
            focused: true,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            alternate_screen_active: false,
            pixel_width: 108,
            pixel_height: 36,
        }],
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

fn snapshot(boot: &str) -> ClientShellSnapshot {
    let mut snap: ClientShellSnapshot = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-snapshot-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    snap.boot_id = boot.into();
    assert_eq!(snap.panes[0].pane_id, PANE, "fixture premise");
    snap
}

/// Everything a fake host received, in order.
#[derive(Debug, Default)]
struct Wire {
    inputs: Vec<(String, Vec<ClientPaneInputEvent>)>,
    focus: Vec<bool>,
    resizes: Vec<SurfaceGeometry>,
    endpoint: Vec<(String, Value)>,
    api: Vec<(String, Value)>,
    detached: u32,
}

/// Blocks the calling thread once: reports `reached`, then waits for `go`.
struct Gate {
    reached: Sender<()>,
    go: Receiver<()>,
}

impl Gate {
    fn new() -> (Gate, Receiver<()>, Sender<()>) {
        let (reached_tx, reached_rx) = channel();
        let (go_tx, go_rx) = channel();
        (
            Gate {
                reached: reached_tx,
                go: go_rx,
            },
            reached_rx,
            go_tx,
        )
    }
    fn pass(self) {
        self.reached.send(()).unwrap();
        self.go.recv_timeout(WITHIN).expect("gate released");
    }
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    wire: Arc<Mutex<Wire>>,
    detach_gate: Arc<Mutex<Option<Gate>>>,
}

fn not_in_fake() -> RuntimeError {
    RuntimeError::new("unsupported_in_fake", "not used by this seam")
}

impl RuntimeGateway for FakeGateway {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn identity(&self) -> Option<LiveIdentity> {
        Some(LiveIdentity {
            endpoint: self.endpoint.clone(),
            session: self.session.clone(),
            connection_generation: 1,
            boot_id: self.boot.clone(),
        })
    }
    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(not_in_fake())
    }
    fn take_events(&mut self) -> Option<std::sync::mpsc::Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        self.wire
            .lock()
            .unwrap()
            .inputs
            .push((target.pane_id.clone(), events));
        Ok(())
    }
    fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        self.wire.lock().unwrap().resizes.push(geometry);
        Ok(())
    }
    fn set_focus(&self, focused: bool) -> Result<(), RuntimeError> {
        self.wire.lock().unwrap().focus.push(focused);
        Ok(())
    }
    fn detach(&mut self) {
        self.wire.lock().unwrap().detached += 1;
        let gate = self.detach_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.pass();
        }
    }
    fn is_connected(&self) -> bool {
        true
    }
}

/// Runs once inside the `n`-th request of `method`, after it was recorded.
type ApiHook = (String, usize, Box<dyn FnOnce() + Send>);

struct FakeApi {
    wire: Arc<Mutex<Wire>>,
    replies: Vec<(String, Value)>,
    hook: Arc<Mutex<Option<ApiHook>>>,
}

impl ApiLane for FakeApi {
    fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.wire
            .lock()
            .unwrap()
            .api
            .push((method.to_owned(), params));
        let calls = self
            .wire
            .lock()
            .unwrap()
            .api
            .iter()
            .filter(|(m, _)| m == method)
            .count();
        let due =
            matches!(&*self.hook.lock().unwrap(), Some((m, n, _)) if m == method && *n == calls);
        if due {
            let (_, _, hook) = self.hook.lock().unwrap().take().unwrap();
            hook();
        }
        Ok(self
            .replies
            .iter()
            .find(|(m, _)| m == method)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| json!({ "type": "ok" })))
    }
}

struct FakeLane {
    wire: Arc<Mutex<Wire>>,
}

impl EndpointLane for FakeLane {
    fn request(&self, _boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.wire
            .lock()
            .unwrap()
            .endpoint
            .push((method.to_owned(), params));
        Ok(json!({ "type": "ok" }))
    }
    fn methods(&self) -> Vec<String> {
        ["pane.focus", "pane.split", "workspace.focus"]
            .iter()
            .map(|m| (*m).to_owned())
            .collect()
    }
}

struct Host {
    wire: Arc<Mutex<Wire>>,
    detach_gate: Arc<Mutex<Option<Gate>>>,
    token: u64,
    api_hook: Arc<Mutex<Option<ApiHook>>>,
}

/// Connects `endpoint` on the hub with fakes (session of its spec), installs its endpoint lane
/// and feeds snapshot + one full frame.
fn connect_host(hub: &HostHub, endpoint: &str, boot: &str, revision: u64, screen: &str) -> Host {
    connect_host_api(hub, endpoint, boot, revision, screen, Vec::new())
}

fn connect_host_api(
    hub: &HostHub,
    endpoint: &str,
    boot: &str,
    revision: u64,
    screen: &str,
    replies: Vec<(String, Value)>,
) -> Host {
    let now = Instant::now();
    let wire = Arc::new(Mutex::new(Wire::default()));
    let detach_gate = Arc::new(Mutex::new(None));
    let api_hook: Arc<Mutex<Option<ApiHook>>> = Arc::default();
    let ticket = hub
        .request_connect(endpoint, now)
        .unwrap()
        .expect("a connect ticket");
    let session = hub.spec(endpoint).unwrap().session;
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: endpoint.into(),
                session,
                boot: boot.into(),
                wire: wire.clone(),
                detach_gate: detach_gate.clone(),
            }),
            api: Arc::new(FakeApi {
                wire: wire.clone(),
                replies,
                hook: api_hook.clone(),
            }),
        }),
        now,
    );
    assert!(hub.install_endpoint_lane(
        endpoint,
        ticket.token,
        Arc::new(FakeLane { wire: wire.clone() })
    ));
    hub.apply_event(
        endpoint,
        ticket.token,
        GatewayEvent::Snapshot(Box::new(snapshot(boot))),
        now,
    );
    hub.apply_event(
        endpoint,
        ticket.token,
        GatewayEvent::Surface(Box::new(full(boot, revision, screen))),
        now,
    );
    Host {
        wire,
        detach_gate,
        token: ticket.token,
        api_hook,
    }
}

/// OpenSSH of the window: probes always fail (a renegotiated SSH connection never comes back),
/// each spawned process takes the next socket and records its remote command.
#[derive(Default)]
struct BridgeRunner {
    spawned: Mutex<Vec<OpenSshCommand>>,
    sockets: Mutex<VecDeque<UnixStream>>,
}

struct BridgeChild {
    socket: UnixStream,
}

impl SshChild for BridgeChild {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        Some(Box::new(self.socket.try_clone().ok()?))
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        Some(Box::new(self.socket.try_clone().ok()?))
    }
    fn stderr_text(&self) -> String {
        String::new()
    }
    fn wait_exit(&mut self, _timeout: Duration) -> Option<Option<i32>> {
        None
    }
    fn kill(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

impl SshRunner for BridgeRunner {
    fn output(
        &self,
        _command: &OpenSshCommand,
        _timeout: Duration,
        _stdin: Option<&[u8]>,
    ) -> std::io::Result<ProcessOutput> {
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "fake host",
        ))
    }
    fn spawn(&self, command: &OpenSshCommand) -> std::io::Result<Box<dyn SshChild>> {
        self.spawned.lock().unwrap().push(command.clone());
        let socket =
            self.sockets.lock().unwrap().pop_front().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no fake bridge")
            })?;
        Ok(Box::new(BridgeChild { socket }))
    }
}

struct Composed {
    dirs: Vec<tempfile::TempDir>,
    connections: ConnectionsState,
    selection: SelectionState,
    runner: Arc<BridgeRunner>,
    ssh: String,
    api_socket: std::path::PathBuf,
}

impl Composed {
    fn hub(&self) -> &HostHub {
        self.connections.hub()
    }
    fn visible(&self) -> Vec<String> {
        self.hub()
            .snapshot(Instant::now())
            .hosts
            .into_iter()
            .filter(|h| h.visible)
            .map(|h| h.endpoint)
            .collect()
    }
    fn bridges(&self) -> usize {
        self.runner
            .spawned
            .lock()
            .unwrap()
            .iter()
            .filter(|c| format!("{c:?}").contains("remote-api-bridge"))
            .count()
    }
}

/// Local session `hd007s-local` plus one saved SSH profile (`hd007s-remote`), nothing connected.
fn composed() -> Composed {
    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd007s-local").unwrap();
    let paths = SessionPaths::for_session(herdr_config.path(), &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let runner = Arc::new(BridgeRunner::default());
    let connections = ConnectionsState::with_runner(
        ConnectionsConfig {
            prefs_dir: prefs.path().to_path_buf(),
            herdr_config_dir: herdr_config.path().to_path_buf(),
            herdr_state_dir: herdr_state.path().to_path_buf(),
            local_session: Some(session),
            local_auto_start: false,
            herdr_bin: "herdr".into(),
            isolated_ssh: None,
            geometry: geometry(),
        },
        runner.clone(),
    );
    let view = connections
        .save_profile(
            SshProfileDraft {
                id: None,
                label: "remoto".into(),
                target: "tester@127.0.0.1".into(),
                port: None,
                session: "hd007s-remote".into(),
                auth: None,
            },
            false,
        )
        .unwrap();
    let ssh = view.profiles[0].id.as_str().to_owned();
    let selection = SelectionState::new(connections.clone());
    Composed {
        dirs: vec![prefs, herdr_config, herdr_state],
        connections,
        selection,
        runner,
        ssh,
        api_socket: paths.api_socket,
    }
}

fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WITHIN;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn phase(c: &Composed, endpoint: &str) -> String {
    let host = c
        .hub()
        .snapshot(Instant::now())
        .hosts
        .into_iter()
        .find(|h| h.endpoint == endpoint)
        .unwrap();
    serde_json::to_value(host.phase)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

// =======================================================================================
// A — watcher identity and serialized selection
// =======================================================================================

/// Agents host of the window with a barrier on the first call made by the agents' event
/// watcher thread; records the identity each stream was requested for and its outcome.
struct WatcherBarrier {
    inner: Arc<dyn AgentsHost>,
    gate: Mutex<Option<Gate>>,
    opened: Mutex<Sender<(LiveIdentity, Result<(), String>)>>,
}

impl WatcherBarrier {
    fn on_watcher_thread(&self) {
        let watcher = std::thread::current().name() == Some("herdr-desktop-agent-events");
        if watcher {
            let gate = self.gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.pass();
            }
        }
    }
}

impl AgentsHost for WatcherBarrier {
    fn endpoint(&self) -> String {
        self.on_watcher_thread();
        self.inner.endpoint()
    }
    fn gateway(&self) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        self.on_watcher_thread();
        self.inner.gateway()
    }
    fn endpoint_methods(&self) -> Result<Vec<String>, RuntimeError> {
        self.on_watcher_thread();
        self.inner.endpoint_methods()
    }
    fn surface(&self) -> Option<PaneSurfaceFrame> {
        self.on_watcher_thread();
        self.inner.surface()
    }
    fn event_stream(&self, attached: &LiveIdentity) -> Result<Box<dyn EventStream>, RuntimeError> {
        self.on_watcher_thread();
        let result = self.inner.event_stream(attached);
        let outcome = match &result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.code.clone()),
        };
        let _ = self
            .opened
            .lock()
            .unwrap()
            .send((attached.clone(), outcome));
        result
    }
}

fn read_line(stream: &mut UnixStream) -> String {
    stream.set_read_timeout(Some(WITHIN)).unwrap();
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
        line.push(byte[0]);
    }
    String::from_utf8(line).unwrap()
}

/// Would catch: the watcher resolving the host lazily inside its thread (subscribing on whatever
/// host is selected when the thread runs — here Local, with the same pane `w1:p1`), a stream
/// opener that ignores the attached identity, or one that opens/writes after the attached host
/// was deselected. Positive control: without a selection change the stream is the SSH bridge
/// and carries the subscription.
#[test]
fn agents_watcher_subscribes_only_on_the_attached_identity_even_if_selection_changes_before_open() {
    let c = composed();
    let ssh = c.ssh.clone();
    let listener = UnixListener::bind(&c.api_socket).unwrap();
    listener.set_nonblocking(true).unwrap();

    // Positive control: SSH selected and online, no barrier.
    c.selection.select(&ssh).unwrap();
    let remote = connect_host(c.hub(), &ssh, "boot-ssh-3", 9, "remote$");
    let (bridge_near, bridge_far) = UnixStream::pair().unwrap();
    c.runner.sockets.lock().unwrap().push_back(bridge_near);
    let (opened_tx, opened_rx) = channel();
    let host = Arc::new(WatcherBarrier {
        inner: c.selection.agents_host(),
        gate: Mutex::new(None),
        opened: Mutex::new(opened_tx),
    });
    let agents = AgentsState::hosted(host.clone());
    agents.attach_hosted(None).unwrap();
    let (identity, outcome) = opened_rx.recv_timeout(WITHIN).unwrap();
    assert_eq!(identity.endpoint, ssh);
    assert_eq!(identity.boot_id, "boot-ssh-3");
    assert_eq!(outcome, Ok(()));
    let mut far = bridge_far;
    assert!(read_line(&mut far).contains("events.subscribe"));
    assert_eq!(c.bridges(), 1);
    agents.detach();

    // Race: the watcher thread is held before it opens; Local becomes selected and online.
    let (gate, reached, go) = Gate::new();
    *host.gate.lock().unwrap() = Some(gate);
    agents.attach_hosted(None).unwrap();
    reached
        .recv_timeout(WITHIN)
        .expect("watcher reached barrier");
    c.selection.select(LOCAL).unwrap();
    eventually("renegotiation attempts settle", || {
        phase(&c, LOCAL) != "connecting" && phase(&c, &ssh) != "connecting"
    });
    let local = connect_host(c.hub(), LOCAL, "boot-local-7", 5, "local$");
    assert!(c.hub().live_identity(LOCAL).is_ok(), "Local online premise");
    go.send(()).unwrap();

    let (identity, outcome) = opened_rx.recv_timeout(WITHIN).unwrap();
    assert_eq!(
        identity.endpoint, ssh,
        "the opener carries the attached identity"
    );
    assert_eq!(identity.boot_id, "boot-ssh-3");
    let code = outcome.expect_err("no stream after the attached host was deselected");
    assert!(
        ["selection_changed", "host_offline", "host_reconnecting"].contains(&code.as_str()),
        "{code}"
    );
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "nothing connected to Local's API socket"
    );
    assert_eq!(c.bridges(), 1, "no second SSH bridge process");
    let local_wire = local.wire.lock().unwrap();
    assert!(local_wire.api.is_empty() && local_wire.endpoint.is_empty());
    drop(local_wire);
    assert_eq!(remote.wire.lock().unwrap().endpoint.len(), 0);
    agents.detach();
}

/// Would catch: the watcher asking the host for a fresh identity when it starts instead of
/// using the one the agents were discovered on — after the same SSH endpoint reconnected
/// (same boot, new generation) between discovery and the watcher start, the old core would be
/// fed events of the new connection. The reconnect runs inside the last discovery request
/// (`tab.list` of `refresh_tabs`, whose failure attach ignores), so no timing is involved.
#[test]
fn agents_watcher_keeps_the_discovery_generation_when_the_host_reconnects_before_it_starts() {
    let c = composed();
    let ssh = c.ssh.clone();
    c.selection.select(&ssh).unwrap();
    let remote = connect_host(c.hub(), &ssh, "boot-ssh-3", 9, "remote$");
    let original = c.hub().live_identity(&ssh).unwrap();
    // A bridge is available: a watcher on the renewed connection would open successfully.
    let (bridge_near, _bridge_far) = UnixStream::pair().unwrap();
    c.runner.sockets.lock().unwrap().push_back(bridge_near);
    let renewed: Arc<Mutex<Option<Host>>> = Arc::default();
    {
        let connections = c.connections.clone();
        let ssh = ssh.clone();
        let renewed = renewed.clone();
        *remote.api_hook.lock().unwrap() = Some((
            "tab.list".into(),
            2,
            Box::new(move || {
                connections.hub().cancel(&ssh).unwrap();
                let host = connect_host(connections.hub(), &ssh, "boot-ssh-3", 10, "novo$");
                *renewed.lock().unwrap() = Some(host);
            }),
        ));
    }
    let (opened_tx, opened_rx) = channel();
    let host = Arc::new(WatcherBarrier {
        inner: c.selection.agents_host(),
        gate: Mutex::new(None),
        opened: Mutex::new(opened_tx),
    });
    let agents = AgentsState::hosted(host);
    let overview = agents.attach_hosted(None).unwrap();
    assert!(
        renewed.lock().unwrap().is_some(),
        "reconnect happened during attach"
    );
    let live = c.hub().live_identity(&ssh).unwrap();
    assert!(
        live.connection_generation > original.connection_generation,
        "premise: same endpoint and boot, new generation"
    );
    assert_eq!(overview.identity.as_ref(), Some(&original));

    let (identity, outcome) = opened_rx.recv_timeout(WITHIN).unwrap();
    assert_eq!(
        identity, original,
        "the watcher carries the discovery identity"
    );
    let code = outcome.expect_err("no stream of the renewed connection for the old core");
    assert_eq!(code, "target_generation_stale");
    assert_eq!(c.bridges(), 0, "no bridge process opened");
    agents.detach();
}

/// Would catch: two `select` calls interleaving (both hosts left with a surface, or the one that
/// lost the race re-enabling its host after the other finished), or a failed selection changing
/// visibility. The first transition is held inside hiding the SSH connection.
#[test]
fn concurrent_selection_transitions_leave_exactly_the_selected_host_visible() {
    let c = composed();
    let ssh = c.ssh.clone();
    c.selection.select(&ssh).unwrap();
    let _local = connect_host(c.hub(), LOCAL, "boot-local-7", 5, "local$");
    let remote = connect_host(c.hub(), &ssh, "boot-ssh-3", 9, "remote$");
    assert_eq!(c.visible(), vec![ssh.clone()]);

    let (gate, reached, go) = Gate::new();
    *remote.detach_gate.lock().unwrap() = Some(gate);
    let first = {
        let selection = c.selection.clone();
        std::thread::spawn(move || selection.select(LOCAL))
    };
    reached
        .recv_timeout(WITHIN)
        .expect("first transition hides SSH");
    let second = {
        let selection = c.selection.clone();
        let ssh = ssh.clone();
        std::thread::spawn(move || selection.select(&ssh))
    };
    // Give an unserialized second transition the chance to run into the first one; a
    // serialized implementation simply keeps waiting (bounded, not the proof itself).
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline && c.selection.selected().as_deref() != Some(ssh.as_str()) {
        std::thread::sleep(Duration::from_millis(2));
    }
    go.send(()).unwrap();
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();

    assert_eq!(c.selection.selected().as_deref(), Some(ssh.as_str()));
    assert_eq!(c.visible(), vec![ssh.clone()], "exactly the selected host");

    // Barrier-started rounds of racing transitions (hosts now offline: flips are lock-short).
    for round in 0..400 {
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let racers: Vec<_> = [LOCAL.to_owned(), ssh.clone(), LOCAL.to_owned()]
            .into_iter()
            .map(|endpoint| {
                let selection = c.selection.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    selection.select(&endpoint)
                })
            })
            .collect();
        for racer in racers {
            racer.join().unwrap().unwrap();
        }
        let selected = c.selection.selected().unwrap();
        assert_eq!(c.visible(), vec![selected], "round {round}");
    }
    c.selection.select(&ssh).unwrap();

    let error = c.selection.select("nao-existe").unwrap_err();
    assert_eq!(error.code, "endpoint_unknown");
    assert_eq!(c.selection.selected().as_deref(), Some(ssh.as_str()));
    assert_eq!(c.visible(), vec![ssh.clone()]);
    drop(c.dirs);
}

/// Would catch: a connection attempt started while the host was visible (surface requested)
/// installing an active surface after the window already hid the host, or the reverse — the
/// selected host staying metadata-only because its in-flight attempt predates the selection.
#[test]
fn an_in_flight_attempt_from_an_old_visibility_is_renegotiated_with_the_current_one() {
    let hub = HostHub::new();
    hub.add_host(herdr_desktop::connections::hub::HostSpec {
        endpoint: LOCAL.into(),
        label: "Este computador".into(),
        kind: herdr_desktop::connections::hub::HostKind::Local,
        session: "hd007s-local".into(),
        target: None,
        visible: true,
    })
    .unwrap();
    let now = Instant::now();
    let ticket = hub.request_connect(LOCAL, now).unwrap().unwrap();
    assert!(ticket.surface_active);
    assert!(matches!(
        hub.set_visible(LOCAL, false, now).unwrap(),
        herdr_desktop::connections::hub::VisibilityOutcome::Interest(_)
    ));
    let wire = Arc::new(Mutex::new(Wire::default()));
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: LOCAL.into(),
                session: "hd007s-local".into(),
                boot: "boot-local-7".into(),
                wire: wire.clone(),
                detach_gate: Arc::new(Mutex::new(None)),
            }),
            api: Arc::new(FakeApi {
                wire: wire.clone(),
                replies: vec![],
                hook: Arc::default(),
            }),
        }),
        now,
    );
    let renegotiate = hub
        .settle_visibility(&ticket)
        .expect("stale surface mode is renegotiated");
    assert!(!renegotiate.surface_active);
    assert_ne!(renegotiate.token, ticket.token);
    assert_eq!(wire.lock().unwrap().detached, 1, "old connection dropped");
    assert!(
        hub.settle_visibility(&renegotiate).is_none(),
        "not installed yet: nothing to settle"
    );

    // Matching mode: nothing to do.
    let hub2 = HostHub::new();
    hub2.add_host(hub.spec(LOCAL).unwrap()).unwrap();
    let ticket = hub2.request_connect(LOCAL, now).unwrap().unwrap();
    assert!(!ticket.surface_active);
    connect_ticket(&hub2, &ticket);
    assert!(hub2.settle_visibility(&ticket).is_none());
}

fn connect_ticket(hub: &HostHub, ticket: &herdr_desktop::connections::hub::ConnectTicket) {
    let wire = Arc::new(Mutex::new(Wire::default()));
    hub.finish_connect(
        ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: ticket.endpoint.clone(),
                session: "hd007s-local".into(),
                boot: "boot-local-7".into(),
                wire: wire.clone(),
                detach_gate: Arc::new(Mutex::new(None)),
            }),
            api: Arc::new(FakeApi {
                wire,
                replies: vec![],
                hook: Arc::default(),
            }),
        }),
        Instant::now(),
    );
}

// =======================================================================================
// C — project gateways of the window
// =======================================================================================

mod project_hosts {
    use super::*;
    use herdr_client::{ProjectRef, RuntimeBinding};
    use herdr_desktop::bridge::project_hosts::HostedProjectGateways;
    use herdr_desktop::files::local::FilesState;
    use herdr_desktop::files::sftp::{OpenSshSftpConnector, RemoteFilesConfig, RemoteFilesState};
    use herdr_desktop::project_store::{
        ProjectDraft, ProjectGateways, ProjectStore, ProjectsState,
    };
    use std::collections::BTreeMap;

    fn engine_replies() -> Vec<(String, Value)> {
        vec![
            ("workspace.list".into(), json!({ "workspaces": [] })),
            (
                "workspace.create".into(),
                json!({ "workspace": { "workspace_id": "w1" },
                        "root_pane": { "pane_id": PANE, "workspace_id": "w1" } }),
            ),
        ]
    }

    struct Window {
        c: Composed,
        local: Host,
        remote: Host,
        files: FilesState,
        remote_files: RemoteFilesState,
        gateways: Arc<HostedProjectGateways>,
        projects: ProjectsState,
        local_project: ProjectRef,
        ssh_project: ProjectRef,
        local_root: tempfile::TempDir,
    }

    fn api_calls(host: &Host) -> Vec<String> {
        host.wire
            .lock()
            .unwrap()
            .api
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }

    fn endpoint_calls(host: &Host) -> Vec<(String, Value)> {
        host.wire.lock().unwrap().endpoint.clone()
    }

    fn remote_roots(w: &Window) -> Vec<String> {
        w.remote_files
            .hosts()
            .hosts
            .into_iter()
            .find(|h| h.endpoint == w.c.ssh)
            .map(|h| h.roots)
            .unwrap_or_default()
    }

    /// SSH selected; both hosts online on the one hub (Local metadata-only), same pane `w1:p1`.
    fn window() -> Window {
        let c = composed();
        let ssh = c.ssh.clone();
        c.selection.select(&ssh).unwrap();
        let local = connect_host_api(c.hub(), LOCAL, "boot-local-7", 5, "l$", engine_replies());
        let remote = connect_host_api(c.hub(), &ssh, "boot-ssh-3", 9, "r$", engine_replies());
        let files = FilesState::empty();
        let remote_files = RemoteFilesState::new(RemoteFilesConfig {
            links: Arc::new(c.connections.clone()),
            connector: Arc::new(OpenSshSftpConnector::new(None)),
            roots: BTreeMap::new(),
            deadline: Duration::from_secs(5),
        })
        .unwrap();
        let gateways = Arc::new(HostedProjectGateways::new(
            c.selection.clone(),
            files.clone(),
            remote_files.root_authorizer(),
        ));
        let local_root = tempfile::tempdir().unwrap();
        let prefs = tempfile::tempdir().unwrap();
        let herdr_config = c.dirs[1].path().to_path_buf();
        let mut store = ProjectStore::open(prefs.path(), &herdr_config).unwrap();
        let local_project = store
            .create_project(ProjectDraft {
                label: "api local".into(),
                endpoint_profile_id: LOCAL.into(),
                session_name: "hd007s-local".into(),
                root: local_root.path().to_string_lossy().into_owned(),
            })
            .unwrap();
        let ssh_project = store
            .create_project(ProjectDraft {
                label: "api prod".into(),
                endpoint_profile_id: ssh.clone(),
                session_name: "hd007s-remote".into(),
                root: "/home/deploy/api".into(),
            })
            .unwrap();
        drop(store);
        let projects = ProjectsState::with_gateways(
            prefs.path().to_path_buf(),
            herdr_config,
            gateways.clone(),
        );
        let mut c = c;
        c.dirs.push(prefs);
        Window {
            c,
            local,
            remote,
            files,
            remote_files,
            gateways,
            projects,
            local_project,
            ssh_project,
            local_root,
        }
    }

    /// Would catch: the adapter routing by the selected host instead of the project's endpoint,
    /// focusing a workspace on the other host (same `w1`), authorizing an SSH root in the local
    /// provider or a Local root remotely, or focusing/authorizing more than once per open.
    #[test]
    fn projects_open_focus_and_authorize_only_on_their_own_host() {
        let w = window();
        let opened = w.projects.open(&w.local_project.id).unwrap();
        assert_eq!(opened.result.binding.boot_id, "boot-local-7");
        assert_eq!(
            api_calls(&w.local),
            [
                "workspace.list",
                "workspace.create",
                "workspace.report_metadata"
            ]
        );
        assert_eq!(
            endpoint_calls(&w.local),
            vec![(
                "workspace.focus".to_owned(),
                json!({ "workspace_id": "w1" })
            )]
        );
        assert!(api_calls(&w.remote).is_empty() && endpoint_calls(&w.remote).is_empty());
        assert_eq!(
            w.files.authorized_roots(),
            vec![std::fs::canonicalize(w.local_root.path()).unwrap()]
        );
        assert!(
            remote_roots(&w).is_empty(),
            "no Local root on the SSH provider"
        );

        let opened = w.projects.open(&w.ssh_project.id).unwrap();
        assert_eq!(opened.result.binding.boot_id, "boot-ssh-3");
        assert_eq!(
            endpoint_calls(&w.remote),
            vec![(
                "workspace.focus".to_owned(),
                json!({ "workspace_id": "w1" })
            )]
        );
        assert_eq!(
            endpoint_calls(&w.local).len(),
            1,
            "Local untouched by SSH open"
        );
        assert_eq!(remote_roots(&w), vec!["/home/deploy/api".to_owned()]);
        assert_eq!(w.files.authorized_roots().len(), 1, "no SSH root locally");

        // Reopen: reuse, one more focus, idempotent authorization.
        w.projects.open(&w.ssh_project.id).unwrap();
        assert_eq!(endpoint_calls(&w.remote).len(), 2);
        assert_eq!(remote_roots(&w), vec!["/home/deploy/api".to_owned()]);
    }

    /// Would catch: a gateway for a project whose session differs from the host's (sending on
    /// the wrong session), a Local fallback for an unknown or offline endpoint, or an automatic
    /// bootstrap/connection attempt of a missing server.
    #[test]
    fn mismatched_unknown_or_offline_hosts_send_nothing_and_never_fall_back_to_local() {
        let w = window();
        let mut wrong_session = w.ssh_project.clone();
        wrong_session.session_name = "outra-sessao".into();
        let error = w.gateways.gateway(&wrong_session).err().unwrap();
        assert_eq!(error.code, "target_session_mismatch");

        let mut unknown = w.ssh_project.clone();
        unknown.endpoint_profile_id = "0007ffff0007ffff0007ffff0007ffff".into();
        assert_eq!(
            w.gateways.gateway(&unknown).err().unwrap().code,
            "endpoint_unknown"
        );
        assert_eq!(
            w.gateways.authorize_root(&unknown).unwrap_err().code,
            "endpoint_unknown"
        );

        w.c.hub().cancel(&w.c.ssh).unwrap();
        let error = w.projects.open(&w.ssh_project.id).unwrap_err();
        assert_eq!(error.code, "host_offline");
        assert_eq!(error.endpoint.as_deref(), Some(w.c.ssh.as_str()));
        assert!(api_calls(&w.local).is_empty() && endpoint_calls(&w.local).is_empty());
        assert!(api_calls(&w.remote).is_empty());
        assert!(remote_roots(&w).is_empty() && w.files.authorized_roots().is_empty());
        assert_eq!(
            phase(&w.c, &w.c.ssh),
            "offline",
            "no automatic reconnection or bootstrap"
        );
    }

    /// Would catch: `focus_workspace` using the host's live identity instead of the binding's
    /// boot/generation (focusing on a rebooted server), or refusing a workspace the engine
    /// created just before its snapshot arrived instead of waiting (bounded) for it.
    #[test]
    fn focus_uses_the_binding_identity_and_waits_for_a_new_workspace_in_the_snapshot() {
        let w = window();
        let binding = |boot: &str, generation: u64, workspace: &str| RuntimeBinding {
            project_id: w.ssh_project.id.clone(),
            connection_generation: generation,
            boot_id: boot.into(),
            workspace_id: workspace.into(),
        };
        let live = w.c.hub().live_identity(&w.c.ssh).unwrap();
        let stale_boot = w
            .gateways
            .focus_workspace(
                &w.ssh_project,
                &binding("boot-ssh-0", live.connection_generation, "w1"),
            )
            .unwrap_err();
        assert_eq!(stale_boot.code, "target_boot_stale");
        let stale_generation = w
            .gateways
            .focus_workspace(
                &w.ssh_project,
                &binding("boot-ssh-3", live.connection_generation + 7, "w1"),
            )
            .unwrap_err();
        assert_eq!(stale_generation.code, "target_generation_stale");
        assert!(endpoint_calls(&w.remote).is_empty(), "nothing sent");

        let pending = {
            let gateways = w.gateways.clone();
            let project = w.ssh_project.clone();
            let binding = binding("boot-ssh-3", live.connection_generation, "w9");
            std::thread::spawn(move || gateways.focus_workspace(&project, &binding))
        };
        let mut snap = snapshot("boot-ssh-3");
        let mut extra = snap.workspaces[0].clone();
        extra.workspace_id = "w9".into();
        snap.workspaces.push(extra);
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            endpoint_calls(&w.remote).is_empty(),
            "not sent before it exists"
        );
        w.c.hub().apply_event(
            &w.c.ssh,
            w.remote.token,
            GatewayEvent::Snapshot(Box::new(snap)),
            Instant::now(),
        );
        pending.join().unwrap().unwrap();
        assert_eq!(
            endpoint_calls(&w.remote),
            vec![(
                "workspace.focus".to_owned(),
                json!({ "workspace_id": "w9" })
            )]
        );
        assert!(endpoint_calls(&w.local).is_empty());
    }
}

// =======================================================================================
// B — composed surface IPC seam (selection_*/surface_*), observer and identity fences
// =======================================================================================

mod surface {
    use super::*;
    use herdr_client::protocol::wire::{PaneSurfacePatch, PaneSurfacePatchRow};
    use herdr_desktop::bridge::composition::{
        ComposedSurface, FrameSink, SurfaceConfig, SurfaceIdentityDto,
    };
    use herdr_desktop::connections::hub::{HostNotice, HubObserver, NoticeOrigin};
    use herdr_desktop::terminal::{FrameEvent, InputDto};

    #[derive(Default)]
    struct Sink {
        events: Mutex<Vec<Value>>,
    }

    impl FrameSink for Sink {
        fn send(&self, event: FrameEvent) -> bool {
            self.events
                .lock()
                .unwrap()
                .push(serde_json::to_value(&event).unwrap());
            true
        }
    }

    impl Sink {
        fn all(&self) -> Vec<Value> {
            self.events.lock().unwrap().clone()
        }
        fn kinds(&self) -> Vec<String> {
            self.all()
                .iter()
                .map(|e| e["type"].as_str().unwrap().to_owned())
                .collect()
        }
    }

    fn config(c: &Composed, trace: Option<std::path::PathBuf>) -> SurfaceConfig {
        SurfaceConfig {
            local_config_dir: c.dirs[1].path().to_path_buf(),
            local_session: Some(SessionName::parse("hd007s-local").unwrap()),
            local_auto_start: false,
            surface_trace: trace,
        }
    }

    fn text(t: &str) -> Vec<InputDto> {
        vec![serde_json::from_value(json!({ "kind": "text", "text": t })).unwrap()]
    }

    fn expected(endpoint: &str, session: &str, generation: u64, boot: &str) -> SurfaceIdentityDto {
        serde_json::from_value(json!({
            "endpoint": endpoint, "session": session, "connection_generation": generation,
            "boot_id": boot, "pane_id": PANE,
        }))
        .unwrap()
    }

    fn patch(boot: &str, base: u64, revision: u64, row: &str) -> PaneSurfacePatch {
        let frame = full(boot, 1, row);
        let cells = frame.frame.cells[..12].to_vec();
        PaneSurfacePatch {
            boot_id: boot.into(),
            projection_revision: 1,
            base_surface_revision: base,
            surface_revision: revision,
            rows: vec![PaneSurfacePatchRow { x: 0, y: 0, cells }],
            panes: frame.panes,
            cursor: None,
        }
    }

    /// Would catch: an automatic Local server start or connect attempt on attach, a silent fallback
    /// to another host, an attach that errors the window instead of a recoverable status, or a
    /// trace written when not configured.
    #[test]
    fn local_is_selected_only_when_configured_and_an_absent_session_is_recoverable_without_autostart(
    ) {
        let c = composed();
        let trace = c.dirs[0].path().join("surface-trace.json");
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, Some(trace.clone())));
        let selection = surface.selection_dto();
        assert_eq!(selection.endpoint.as_deref(), Some(LOCAL));
        assert_eq!(selection.session.as_deref(), Some("hd007s-local"));
        assert!(!selection.online);
        let sink = Arc::new(Sink::default());
        let status = surface
            .attach(geometry(), sink.clone())
            .expect("recoverable status");
        assert_eq!(status.state, "disconnected");
        assert!(!status.session_available && !status.connected);
        assert_eq!(
            status.last_error.as_ref().unwrap().code,
            "server_unavailable"
        );
        assert!(status.last_error.as_ref().unwrap().retryable);
        assert_eq!(
            phase(&c, LOCAL),
            "offline",
            "no connect attempt without a session"
        );
        assert!(!c.api_socket.exists(), "nothing started");
        assert_eq!(sink.kinds(), ["state"]);
        assert_eq!(sink.all()[0]["state"], "disconnected");
        let traced: Value = serde_json::from_slice(&std::fs::read(&trace).unwrap()).unwrap();
        assert_eq!(traced["state"], "disconnected");
        assert_eq!(traced["input_enabled"], false);
        assert_eq!(traced["last_error"]["code"], "server_unavailable");

        // Without a configured Local session nothing is selected and nothing is traced.
        let c2 = composed();
        let untraced = c2.dirs[0].path().join("untraced.json");
        let mut cfg = config(&c2, None);
        cfg.local_session = None;
        let bare = ComposedSurface::new(c2.selection.clone(), cfg);
        assert_eq!(bare.selection_dto().endpoint, None);
        let error = bare
            .attach(geometry(), Arc::new(Sink::default()))
            .unwrap_err();
        assert_eq!(error.code, "no_selection");
        assert_eq!(bare.status().state, "empty");
        assert!(!untraced.exists());
    }

    /// Would catch: frames of the unselected host with the same pane id reaching the channel, an
    /// attach that does not replay the committed surface, metadata missing after full/patch, or
    /// a requested geometry not kept by the hub.
    #[test]
    fn attach_replays_the_selected_host_and_forwards_only_its_committed_frames() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let ssh = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let sink = Arc::new(Sink::default());
        let wanted = SurfaceGeometry {
            cols: 40,
            rows: 10,
            cell_width_px: 8,
            cell_height_px: 16,
        };
        let status = surface.attach(wanted, sink.clone()).unwrap();
        assert_eq!(status.state, "live");
        assert_eq!(status.boot_id.as_deref(), Some("boot-ssh-3"));
        assert_eq!(status.pane_id.as_deref(), Some(PANE));
        assert_eq!(status.session.as_deref(), Some("hd007s-remote"));
        assert_eq!(c.hub().geometry(&c.ssh), Some(wanted));
        assert_eq!(ssh.wire.lock().unwrap().resizes.last(), Some(&wanted));
        assert_eq!(sink.kinds(), ["identity", "full", "metadata", "state"]);
        let events = sink.all();
        assert_eq!(events[0]["boot_id"], "boot-ssh-3");
        assert_eq!(events[0]["pane_id"], PANE);
        assert_eq!(events[1]["revision"], 4);
        assert_eq!(events[3]["state"], "live");
        let generation = events[0]["connection_generation"].as_u64().unwrap();

        // The Local host (same pane id, other boot) is hidden: nothing of it reaches the sink.
        let now = Instant::now();
        c.hub().apply_event(
            &c.ssh,
            ssh.token,
            GatewayEvent::Patch(Box::new(patch("boot-ssh-3", 4, 5, "ok"))),
            now,
        );
        assert_eq!(sink.kinds()[4..], ["patch", "metadata"]);
        assert_eq!(sink.all()[4]["revision"], 5);
        // A rejected patch never reaches the renderer; stale is reported instead.
        c.hub().apply_event(
            &c.ssh,
            ssh.token,
            GatewayEvent::Patch(Box::new(patch("boot-ssh-3", 1, 9, "bad"))),
            now,
        );
        assert_eq!(sink.kinds()[6..], ["state"]);
        assert_eq!(sink.all()[6]["state"], "stale");
        assert!(
            surface
                .input(
                    &expected(&c.ssh, "hd007s-remote", generation, "boot-ssh-3"),
                    &text("x")
                )
                .is_err(),
            "stale surface blocks input"
        );
        assert!(ssh.wire.lock().unwrap().inputs.is_empty());
    }

    /// Would catch: input accepted for another endpoint, session, connection generation or boot,
    /// for a pane that is not the engine's confirmed focus, or queued/replayed after refusal.
    #[test]
    fn input_requires_the_current_identity_and_the_confirmed_focused_pane() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let ssh = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let sink = Arc::new(Sink::default());
        surface.attach(geometry(), sink.clone()).unwrap();
        let generation = sink.all()[0]["connection_generation"].as_u64().unwrap();
        let good = expected(&c.ssh, "hd007s-remote", generation, "boot-ssh-3");
        let refusals = [
            (
                expected(LOCAL, "hd007s-local", generation, "boot-ssh-3"),
                "local endpoint",
            ),
            (
                expected(&c.ssh, "hd007s-other", generation, "boot-ssh-3"),
                "other session",
            ),
            (
                expected(&c.ssh, "hd007s-remote", generation + 1, "boot-ssh-3"),
                "other generation",
            ),
            (
                expected(&c.ssh, "hd007s-remote", generation, "boot-ssh-9"),
                "other boot",
            ),
        ];
        for (identity, what) in &refusals {
            assert!(
                surface.input(identity, &text("no")).is_err(),
                "{what} refused"
            );
        }
        let mut unfocused = good.clone();
        unfocused.pane_id = "w1:p2".into();
        let error = surface.input(&unfocused, &text("no")).unwrap_err();
        assert_eq!(error.code, "pane_not_focused");
        assert!(
            ssh.wire.lock().unwrap().inputs.is_empty(),
            "nothing sent on refusal"
        );
        surface.input(&good, &text("sim")).unwrap();
        let inputs = ssh.wire.lock().unwrap().inputs.clone();
        assert_eq!(inputs.len(), 1, "sent once, refusals never replayed");
        assert_eq!(inputs[0].0, PANE);
        assert_eq!(
            inputs[0].1,
            vec![ClientPaneInputEvent::TextCommit("sim".into())]
        );

        // Detached: the same identity is refused, nothing is sent.
        surface.detach();
        assert_eq!(
            surface.input(&good, &text("tarde")).unwrap_err().code,
            "not_attached"
        );
        assert_eq!(ssh.wire.lock().unwrap().inputs.len(), 1);
    }

    /// Would catch: a replaced or detached channel that keeps receiving frames, or a selection
    /// change that leaves the old channel bound to the new host.
    #[test]
    fn replaced_detached_or_reselected_channels_receive_nothing_more() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let ssh = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let old = Arc::new(Sink::default());
        let new = Arc::new(Sink::default());
        surface.attach(geometry(), old.clone()).unwrap();
        surface.attach(geometry(), new.clone()).unwrap();
        let before = old.all().len();
        let now = Instant::now();
        c.hub().apply_event(
            &c.ssh,
            ssh.token,
            GatewayEvent::Patch(Box::new(patch("boot-ssh-3", 4, 5, "a"))),
            now,
        );
        assert_eq!(old.all().len(), before, "replaced channel is silent");
        assert_eq!(new.kinds()[new.kinds().len() - 2..], ["patch", "metadata"]);
        let status = surface.detach();
        assert_eq!(status.state, "live", "detach keeps the hub connection");
        assert_eq!(phase(&c, &c.ssh), "online");
        let after = new.all().len();
        c.hub().apply_event(
            &c.ssh,
            ssh.token,
            GatewayEvent::Patch(Box::new(patch("boot-ssh-3", 5, 6, "b"))),
            now,
        );
        assert_eq!(new.all().len(), after, "detached channel is silent");
        assert_eq!(
            ssh.wire.lock().unwrap().detached,
            0,
            "engine connection kept"
        );

        let third = Arc::new(Sink::default());
        surface.attach(geometry(), third.clone()).unwrap();
        let bound = third.all().len();
        let _ = surface.select(LOCAL);
        assert_eq!(surface.selection_dto().endpoint.as_deref(), Some(LOCAL));
        // Local now streams the same pane id: the channel attached to the SSH host gets nothing.
        let local = connect_host(c.hub(), LOCAL, "boot-local-7", 3, "local$");
        c.hub().apply_event(
            LOCAL,
            local.token,
            GatewayEvent::Patch(Box::new(patch("boot-local-7", 3, 4, "l"))),
            now,
        );
        assert_eq!(
            third.all().len(),
            bound,
            "selection change detaches the channel"
        );
    }

    /// Delays the first notice matching `hold` between the hub lock release and the composed
    /// observer.
    struct Barrier {
        inner: Arc<dyn HubObserver>,
        hold: Box<dyn Fn(&HostNotice) -> bool + Send + Sync>,
        gate: Mutex<Option<Gate>>,
    }

    impl HubObserver for Barrier {
        fn notice(&self, origin: &NoticeOrigin, notice: &HostNotice) {
            if (self.hold)(notice) {
                let gate = self.gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    gate.pass();
                }
            }
            self.inner.notice(origin, notice);
        }
    }

    /// Would catch: an observer that labels a late notice with the host's current connection
    /// (same pane id and boot after a reconnect), letting the old screen replace the new one
    /// under the new identity; and stale input with the old generation.
    #[test]
    fn a_late_notice_of_the_previous_connection_is_never_relabelled_after_reconnect() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let first = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let sink = Arc::new(Sink::default());
        surface.attach(geometry(), sink.clone()).unwrap();
        let old_generation = sink.all()[0]["connection_generation"].as_u64().unwrap();
        let (gate, reached, go) = Gate::new();
        c.hub().set_observer(Arc::new(Barrier {
            inner: surface.observer(),
            hold: Box::new(|n| matches!(n, HostNotice::Full(f) if f.surface_revision == 9)),
            gate: Mutex::new(Some(gate)),
        }));
        let hub_state = c.connections.clone();
        let endpoint = c.ssh.clone();
        let token = first.token;
        let late = std::thread::spawn(move || {
            hub_state.hub().apply_event(
                &endpoint,
                token,
                GatewayEvent::Surface(Box::new(full("boot-ssh-3", 9, "velho"))),
                Instant::now(),
            );
        });
        reached.recv_timeout(WITHIN).expect("late notice held");
        // The connection is lost and renewed: same boot, same pane id, new generation.
        c.hub().apply_event(
            &c.ssh,
            token,
            GatewayEvent::Disconnected(RuntimeError::new("connection_lost", "x").retryable()),
            Instant::now(),
        );
        let second = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 2, "novo$");
        assert_ne!(second.token, first.token);
        go.send(()).unwrap();
        late.join().unwrap();

        let events = sink.all();
        assert!(
            !events
                .iter()
                .any(|e| e["type"] == "full" && e["revision"] == 9),
            "late frame of the old connection delivered: {events:?}"
        );
        let identities: Vec<u64> = events
            .iter()
            .filter(|e| e["type"] == "identity")
            .map(|e| e["connection_generation"].as_u64().unwrap())
            .collect();
        let new_generation = *identities.last().unwrap();
        assert!(new_generation > old_generation, "{identities:?}");
        let last_full = events.iter().rev().find(|e| e["type"] == "full").unwrap();
        assert_eq!(last_full["revision"], 2);
        assert!(events
            .iter()
            .any(|e| e["type"] == "state" && e["state"] == "disconnected"));
        assert_eq!(events.last().unwrap()["state"], "live");

        let old = expected(&c.ssh, "hd007s-remote", old_generation, "boot-ssh-3");
        assert!(surface.input(&old, &text("velho")).is_err());
        assert!(second.wire.lock().unwrap().inputs.is_empty());
        let current = expected(&c.ssh, "hd007s-remote", new_generation, "boot-ssh-3");
        surface.input(&current, &text("novo")).unwrap();
        assert_eq!(second.wire.lock().unwrap().inputs.len(), 1);
    }

    /// Would catch: a reboot on reconnect keeping the previous boot usable for input, or no
    /// identity for the new boot reaching the channel.
    #[test]
    fn reboot_invalidates_the_old_boot_and_reattach_announces_the_new_one() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let first = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let sink = Arc::new(Sink::default());
        surface.attach(geometry(), sink.clone()).unwrap();
        let generation = sink.all()[0]["connection_generation"].as_u64().unwrap();
        c.hub().apply_event(
            &c.ssh,
            first.token,
            GatewayEvent::Shutdown(None),
            Instant::now(),
        );
        assert_eq!(surface.status().state, "disconnected");
        let second = connect_host(c.hub(), &c.ssh, "boot-ssh-4", 1, "reboot$");
        let again = Arc::new(Sink::default());
        let status = surface.attach(geometry(), again.clone()).unwrap();
        assert_eq!(status.boot_id.as_deref(), Some("boot-ssh-4"));
        assert_eq!(again.all()[0]["boot_id"], "boot-ssh-4");
        let new_generation = status.connection_generation;
        assert!(surface
            .input(
                &expected(&c.ssh, "hd007s-remote", generation, "boot-ssh-3"),
                &text("a")
            )
            .is_err());
        assert!(
            surface
                .input(
                    &expected(&c.ssh, "hd007s-remote", new_generation, "boot-ssh-3"),
                    &text("b")
                )
                .is_err(),
            "new generation with the previous boot is refused"
        );
        assert!(second.wire.lock().unwrap().inputs.is_empty());
        surface
            .input(
                &expected(&c.ssh, "hd007s-remote", new_generation, "boot-ssh-4"),
                &text("c"),
            )
            .unwrap();
        assert_eq!(second.wire.lock().unwrap().inputs.len(), 1);
    }

    /// Would catch: a second hub or selection in the composed services (surface, agents and
    /// projects diverging from the connections the window shows), an engine config dir used for
    /// preferences, or a connection attempt/server start while building the window.
    #[test]
    fn desktop_services_share_one_hub_and_selection_and_connect_nothing() {
        use herdr_desktop::{DesktopConfig, DesktopServices, DEFAULT_GEOMETRY};
        let dirs: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let bootstrap = herdr_client::bootstrap::BootstrapConfig {
            config_dir: dirs[0].path().to_path_buf(),
            session: Some(SessionName::parse("hd007s-local").unwrap()),
            auto_start: false,
            surface_trace: None,
            herdr_bin: "/nonexistent/herdr-must-not-run".into(),
        };
        let env =
            |key: &str| (key == "XDG_STATE_HOME").then(|| dirs[1].path().display().to_string());
        let config = DesktopConfig::new(bootstrap, &env);
        assert_eq!(config.prefs_dir, None, "resolved by Tauri at setup");
        assert_eq!(config.herdr_state_dir, dirs[1].path().join("herdr"));
        assert_eq!(config.geometry, DEFAULT_GEOMETRY);
        let services = DesktopServices::new(&config, dirs[2].path().to_path_buf()).unwrap();
        let hub = services.connections.hub();
        assert!(std::ptr::eq(hub, services.selection.connections().hub()));
        assert!(std::ptr::eq(
            hub,
            services.surface.selection().connections().hub()
        ));
        assert_eq!(services.selection.selected().as_deref(), Some(LOCAL));
        assert_eq!(
            services.surface.selection_dto().endpoint.as_deref(),
            Some(LOCAL)
        );
        // Selecting through the surface is the selection agents use.
        let host = services.selection.agents_host();
        assert_eq!(host.endpoint(), LOCAL);
        let sink = Arc::new(Sink::default());
        let status = services.surface.attach(geometry(), sink).unwrap();
        assert_eq!(status.state, "disconnected");
        assert_eq!(hub.snapshot(Instant::now()).hosts.len(), 1);
        assert_eq!(phase_of(hub, LOCAL), "offline");
        assert!(!dirs[0].path().join("sessions").exists(), "nothing started");
    }

    fn phase_of(hub: &HostHub, endpoint: &str) -> String {
        let host = hub
            .snapshot(Instant::now())
            .hosts
            .into_iter()
            .find(|h| h.endpoint == endpoint)
            .unwrap();
        serde_json::to_value(host.phase)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Snapshot of `boot` with panes `w1:p1` and `w1:p2` where only `focused` has the focus.
    fn snapshot_focused(boot: &str, focused: &str) -> ClientShellSnapshot {
        let mut snap = snapshot(boot);
        let mut second = snap.panes[0].clone();
        second.pane_id = "w1:p2".into();
        snap.panes.push(second);
        for pane in &mut snap.panes {
            pane.focused = pane.pane_id == focused;
        }
        snap.focused_pane_id = Some(focused.into());
        snap
    }

    /// Would catch: focus checked under one hub lock and input sent under another, so a snapshot
    /// moving the focus in between delivers input to the pane that lost it; and the legacy
    /// explicit-pane path being used for the window's focused input.
    #[test]
    fn focused_input_is_validated_and_sent_under_the_same_lock_as_the_focus() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let ssh = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        surface
            .attach(geometry(), Arc::new(Sink::default()))
            .unwrap();
        let live = c.hub().live_identity(&c.ssh).unwrap();

        // The conversion runs inside the send path; a focus change is attempted meanwhile.
        let (gate, reached, go) = Gate::new();
        let hub_state = c.connections.clone();
        let identity = live.clone();
        let sender = std::thread::spawn(move || {
            hub_state.hub().send_focused_input(&identity, PANE, |pane| {
                assert_eq!(pane.map(|p| p.pane_id.as_str()), Some(PANE));
                gate.pass();
                Ok(vec![ClientPaneInputEvent::TextCommit("durante".into())])
            })
        });
        reached.recv_timeout(WITHIN).expect("inside the send path");
        let (moved_tx, moved_rx) = channel();
        let hub_state = c.connections.clone();
        let endpoint = c.ssh.clone();
        let token = ssh.token;
        let mover = std::thread::spawn(move || {
            hub_state.hub().apply_event(
                &endpoint,
                token,
                GatewayEvent::Snapshot(Box::new(snapshot_focused("boot-ssh-3", "w1:p2"))),
                Instant::now(),
            );
            moved_tx.send(()).unwrap();
        });
        let moved_before_send = moved_rx.recv_timeout(Duration::from_millis(200)).is_ok();
        go.send(()).unwrap();
        let sent = sender.join().unwrap();
        mover.join().unwrap();
        let inputs = ssh.wire.lock().unwrap().inputs.clone();
        if moved_before_send {
            assert!(
                sent.is_err() && inputs.is_empty(),
                "sent after focus moved: {inputs:?}"
            );
        } else {
            assert!(sent.is_ok());
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].0, PANE);
        }

        // Focus is now w1:p2: the old focused pane gets nothing, through either API level.
        let error = c
            .hub()
            .send_focused_input(&live, PANE, |_| {
                Ok(vec![ClientPaneInputEvent::TextCommit("x".into())])
            })
            .unwrap_err();
        assert_eq!(error.code, "pane_not_focused");
        let old = expected(
            &c.ssh,
            "hd007s-remote",
            live.connection_generation,
            "boot-ssh-3",
        );
        assert_eq!(
            surface.input(&old, &text("y")).unwrap_err().code,
            "pane_not_focused"
        );
        let before = ssh.wire.lock().unwrap().inputs.len();
        assert_eq!(before, usize::from(!moved_before_send));
        // Mouse is converted against the pane committed for this connection (read under the send
        // lock): a click on a pane without mouse reporting is refused as such, nothing sent.
        let click: Vec<InputDto> = vec![serde_json::from_value(json!({
            "kind": "mouse", "action": "down", "button": "left", "column": 1, "row": 0
        }))
        .unwrap()];
        let mut now_focused = old.clone();
        now_focused.pane_id = "w1:p2".into();
        // w1:p2 is focused but absent from the committed surface: mouse has no pane to target.
        assert_eq!(
            surface.input(&now_focused, &click).unwrap_err().code,
            "no_target"
        );
        c.hub().apply_event(
            &c.ssh,
            ssh.token,
            GatewayEvent::Snapshot(Box::new(snapshot_focused("boot-ssh-3", PANE))),
            Instant::now(),
        );
        assert_eq!(
            surface.input(&old, &click).unwrap_err().code,
            "mouse_not_reporting",
            "converted against the locked pane of the surface"
        );
        assert_eq!(ssh.wire.lock().unwrap().inputs.len(), before);
        c.hub().apply_event(
            &c.ssh,
            ssh.token,
            GatewayEvent::Snapshot(Box::new(snapshot_focused("boot-ssh-3", "w1:p2"))),
            Instant::now(),
        );
        surface.input(&now_focused, &text("z")).unwrap();
        let inputs = ssh.wire.lock().unwrap().inputs.clone();
        assert_eq!(inputs.len(), before + 1);
        assert_eq!(inputs.last().unwrap().0, "w1:p2");
        // A wrong identity is refused before any conversion runs.
        let mut stale = live.clone();
        stale.boot_id = "boot-ssh-9".into();
        let converted = std::sync::atomic::AtomicBool::new(false);
        assert!(c
            .hub()
            .send_focused_input(&stale, "w1:p2", |_| {
                converted.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![])
            })
            .is_err());
        assert!(!converted.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(ssh.wire.lock().unwrap().inputs.len(), before + 1);
    }

    /// Would catch: cancelling the attached host leaving the channel live (no invalidation), a
    /// cancel of another host touching it, the window not recovering afterwards, and a late
    /// invalidation of the old connection turning the renewed connection off.
    #[test]
    fn cancelling_the_attached_host_invalidates_its_channel_only_and_stays_recoverable() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let _local = connect_host(c.hub(), LOCAL, "boot-local-7", 2, "local$");
        let ssh = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let sink = Arc::new(Sink::default());
        surface.attach(geometry(), sink.clone()).unwrap();
        assert_eq!(sink.all().last().unwrap()["state"], "live");
        let generation = sink.all()[0]["connection_generation"].as_u64().unwrap();

        let count = sink.all().len();
        c.connections.cancel(LOCAL).unwrap();
        assert_eq!(
            sink.all().len(),
            count,
            "another host's cancel is not this channel's"
        );
        assert_eq!(surface.status().state, "live");

        c.connections.cancel(&c.ssh).unwrap();
        let last = sink.all().last().unwrap().clone();
        assert_eq!(last["type"], "state");
        assert_eq!(last["state"], "disconnected", "{last}");
        assert_eq!(last["error"]["code"], "connection_cancelled");
        assert_eq!(surface.status().state, "disconnected");
        let old = expected(&c.ssh, "hd007s-remote", generation, "boot-ssh-3");
        assert!(surface.input(&old, &text("no")).is_err());
        assert!(ssh.wire.lock().unwrap().inputs.is_empty());

        // Recoverable: a new connection of the same host becomes live on the same channel.
        let renewed = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 1, "renewed$");
        assert_eq!(sink.all().last().unwrap()["state"], "live");

        // A late invalidation of the renewed connection's predecessor never turns it off.
        let (gate, reached, go) = Gate::new();
        c.hub().set_observer(Arc::new(Barrier {
            inner: surface.observer(),
            hold: Box::new(|n| matches!(n, HostNotice::Invalidated(_))),
            gate: Mutex::new(Some(gate)),
        }));
        let hub_state = c.connections.clone();
        let endpoint = c.ssh.clone();
        let late = std::thread::spawn(move || hub_state.cancel(&endpoint).map(|_| ()));
        reached.recv_timeout(WITHIN).expect("invalidation held");
        let newest = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 7, "newest$");
        assert_ne!(newest.token, renewed.token);
        go.send(()).unwrap();
        late.join().unwrap().unwrap();
        let last = sink.all().last().unwrap().clone();
        assert_eq!(
            last["state"], "live",
            "old invalidation turned the new connection off"
        );
        let current = sink
            .all()
            .iter()
            .rev()
            .find(|e| e["type"] == "identity")
            .unwrap()["connection_generation"]
            .as_u64()
            .unwrap();
        surface
            .input(
                &expected(&c.ssh, "hd007s-remote", current, "boot-ssh-3"),
                &text("ok"),
            )
            .unwrap();
        assert_eq!(newest.wire.lock().unwrap().inputs.len(), 1);
    }

    /// Would catch: hiding/renegotiating the attached host without telling its channel, so it
    /// keeps showing a live surface of a dropped connection.
    #[test]
    fn renegotiating_the_attached_host_invalidates_the_channel_until_the_new_connection() {
        let c = composed();
        let surface = ComposedSurface::new(c.selection.clone(), config(&c, None));
        surface.select(&c.ssh).unwrap();
        let _ssh = connect_host(c.hub(), &c.ssh, "boot-ssh-3", 4, "remote$");
        let sink = Arc::new(Sink::default());
        surface.attach(geometry(), sink.clone()).unwrap();
        c.connections.set_visible(&c.ssh, false).unwrap();
        // The renegotiation attempt may fail right after (fake SSH): judge the invalidation itself
        // and that the channel never returns to live.
        let events = sink.all();
        let invalidated = events
            .iter()
            .position(|e| e["error"]["code"] == "surface_renegotiating")
            .expect("invalidation delivered");
        assert_ne!(events[invalidated]["state"], "live");
        assert!(
            events[invalidated..].iter().all(|e| e["state"] != "live"),
            "{events:?}"
        );
        assert_ne!(surface.status().state, "live");
    }
}
