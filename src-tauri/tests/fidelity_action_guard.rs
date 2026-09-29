//! Spec 007 — admission guard of terminal actions (focus, scroll, copy selection, open link).
//!
//! An action captures, at the call, the attached channel binding and input epoch of the composed
//! surface. After waiting (blocking pool, endpoint lane serialization) and before the lane writes,
//! the admission runs: selection, connection, confirmed focus, live surface/interest and the same
//! binding/epoch. A stale action is refused unsent; a late reply of a changed attachment causes no
//! native effect; nothing is retried. The admission is not atomic with the write (the lane still
//! takes its state/writer); these contracts prove the admission point, not remote focus after it.
//!
//! Part A drives the true composition (`ComposedSurface` + `TerminalActions::composed` + hub) with
//! fake hosts whose lane serializes like the product lane. Part B drives the product lanes
//! (`LocalEndpointLane`, `SshEndpointLane`) over socket pairs through the hub's `EndpointLane` trait.
//! No GUI, no engine, no default session, no real clipboard/browser.
//!
//! Fixture values are distinct on purpose: Local and SSH both expose `w1:p1` (focused) and
//! `w1:p2`; boots `boot-local-7` / `boot-ssh-3`; scroll offsets 3 / 5 / 7 differ per action.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfaceScrollMetrics, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SessionPaths, SurfaceGeometry, SurfaceState,
};
use herdr_desktop::bridge::composition::{
    ComposedSurface, FrameSink, SurfaceConfig, SurfaceIdentityDto,
};
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshRunner};
use herdr_desktop::bridge::terminal_actions::{
    ActionReceipt, NativeEffects, TerminalAction, TerminalActions,
};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{ApiLane, Connected, EndpointLane, HostHub};
use herdr_desktop::connections::profiles::SshProfileDraft;
use herdr_desktop::connections::ssh_options::OpenSshCommand;
use herdr_desktop::terminal::{
    FocusRequestDto, FrameEvent, LinkRequestDto, ScrollRequestDto, SelectionRequestDto,
    TextPointDto,
};
use serde_json::{json, Value};

// =======================================================================================
// Fixtures (host values, frames and fakes as in fidelity_actions)
// =======================================================================================

const LOCAL: &str = "local";
const FOCUSED: &str = "w1:p1";
const OTHER: &str = "w1:p2";
const WITHIN: Duration = Duration::from_secs(5);
const ALL_METHODS: &[&str] = &[
    "pane.focus",
    "pane.scroll",
    "pane.selection.read",
    "workspace.focus",
];

/// Per-host values that an action must never mix up.
#[derive(Clone, Copy)]
struct HostValues {
    boot: &'static str,
    content_revision: u64,
    max_scroll: u64,
    link: &'static str,
    screen: &'static str,
}

const LOCAL_VALUES: HostValues = HostValues {
    boot: "boot-local-7",
    content_revision: 11,
    max_scroll: 60,
    link: "https://local.example/a",
    screen: "local$",
};

const SSH_VALUES: HostValues = HostValues {
    boot: "boot-ssh-3",
    content_revision: 40,
    max_scroll: 120,
    link: "https://remote.example/b",
    screen: "remote$",
};

const UNSAFE_LINK: &str = "file:///etc/passwd";

fn geometry() -> SurfaceGeometry {
    SurfaceGeometry {
        cols: 24,
        rows: 4,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn rect(x: u16, width: u16) -> SurfaceRect {
    SurfaceRect {
        x,
        y: 0,
        width,
        height: 4,
    }
}

fn cell(symbol: char, hyperlink: Option<u32>) -> CellData {
    CellData {
        symbol: symbol.to_string(),
        fg: 0,
        bg: 0,
        modifier: 0,
        skip: false,
        hyperlink,
    }
}

/// 24×4 surface: `w1:p1` (focused, no scrollback) on columns 0..12, `w1:p2` (not focused, with
/// scrollback) on 12..24. `w1:p2` row 1 col 1 carries the host's web link, col 3 a `file:` link.
fn full(values: HostValues, revision: u64) -> PaneSurfaceFrame {
    let (width, height) = (24u16, 4u16);
    let mut cells: Vec<CellData> = values
        .screen
        .chars()
        .chain(std::iter::repeat(' '))
        .take(usize::from(width * height))
        .map(|c| cell(c, None))
        .collect();
    cells[usize::from(width) + 12 + 1] = cell('L', Some(0));
    cells[usize::from(width) + 12 + 3] = cell('F', Some(1));
    let pane =
        |pane_id: &str, x: u16, focused: bool, content_revision: u64, scroll| PaneSurfacePane {
            pane_id: pane_id.into(),
            content_revision,
            rect: rect(x, 12),
            inner_rect: rect(x, 12),
            scrollbar_rect: None,
            scroll,
            focused,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            alternate_screen_active: false,
            pixel_width: 108,
            pixel_height: 72,
        };
    PaneSurfaceFrame {
        boot_id: values.boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells,
            width,
            height,
            cursor: Some(CursorState {
                x: 0,
                y: 0,
                visible: true,
                shape: 0,
            }),
            hyperlinks: vec![values.link.into(), UNSAFE_LINK.into()],
            graphics: vec![],
        },
        panes: vec![
            pane(FOCUSED, 0, true, 3, None),
            pane(
                OTHER,
                12,
                false,
                values.content_revision,
                Some(PaneSurfaceScrollMetrics {
                    offset_from_bottom: 0,
                    max_offset_from_bottom: values.max_scroll,
                    viewport_rows: 4,
                }),
            ),
        ],
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
    assert_eq!(snap.panes[0].pane_id, FOCUSED, "fixture premise");
    assert!(snap.panes[0].focused, "fixture premise");
    let mut other = snap.panes[0].clone();
    other.pane_id = OTHER.into();
    other.focused = false;
    snap.panes.push(other);
    snap
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

/// Everything a fake host received, in order.
#[derive(Debug, Default)]
struct Wire {
    /// (boot, method, params) of every endpoint command.
    endpoint: Vec<(String, String, Value)>,
    api: Vec<String>,
    inputs: usize,
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    wire: Arc<Mutex<Wire>>,
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
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
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
        _target: &QualifiedTarget,
        _events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        self.wire.lock().unwrap().inputs += 1;
        Ok(())
    }
    fn resize(&self, _geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn detach(&mut self) {}
    fn is_connected(&self) -> bool {
        true
    }
}

struct FakeApi {
    wire: Arc<Mutex<Wire>>,
}

impl ApiLane for FakeApi {
    fn request(&self, method: &str, _params: Value) -> Result<Value, RuntimeError> {
        self.wire.lock().unwrap().api.push(method.to_owned());
        Ok(json!({ "type": "ok" }))
    }
}

type Reply = Result<Value, RuntimeError>;
/// OpenSSH of the window: every probe and process fails (no real host is ever reached).
struct NoSsh;

impl SshRunner for NoSsh {
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
    fn spawn(&self, _command: &OpenSshCommand) -> std::io::Result<Box<dyn SshChild>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no fake bridge",
        ))
    }
}

/// Native effects of the desktop, recorded (never the user's clipboard or browser).
#[derive(Default)]
struct Effects {
    clipboard: Mutex<Vec<String>>,
    opened: Mutex<Vec<String>>,
    clipboard_error: Mutex<Option<RuntimeError>>,
}

impl NativeEffects for Effects {
    fn write_clipboard(&self, text: &str) -> Result<(), RuntimeError> {
        if let Some(error) = self.clipboard_error.lock().unwrap().clone() {
            return Err(error);
        }
        self.clipboard.lock().unwrap().push(text.to_owned());
        Ok(())
    }
    fn open_link(&self, uri: &str) -> Result<(), RuntimeError> {
        self.opened.lock().unwrap().push(uri.to_owned());
        Ok(())
    }
}

impl Effects {
    fn clipboard(&self) -> Vec<String> {
        self.clipboard.lock().unwrap().clone()
    }
    fn opened(&self) -> Vec<String> {
        self.opened.lock().unwrap().clone()
    }
    fn none(&self) -> bool {
        self.clipboard().is_empty() && self.opened().is_empty()
    }
}

fn focus(pane: &str) -> FocusRequestDto {
    FocusRequestDto {
        pane_id: pane.into(),
    }
}

fn scroll(pane: &str, offset: u64) -> ScrollRequestDto {
    ScrollRequestDto {
        pane_id: pane.into(),
        offset_from_bottom: offset,
    }
}

fn point(row: u32, col: u16) -> TextPointDto {
    TextPointDto { row, col }
}

fn selection(pane: &str, content_revision: u64) -> SelectionRequestDto {
    SelectionRequestDto {
        pane_id: pane.into(),
        anchor: point(2, 1),
        cursor: point(3, 5),
        content_revision,
    }
}

fn link(uri: &str, col: u16, content_revision: u64) -> LinkRequestDto {
    LinkRequestDto {
        pane_id: OTHER.into(),
        uri: uri.into(),
        viewport_row: 1,
        col,
        content_revision,
    }
}

fn engine_text(pane: &str, text: &str) -> Reply {
    Ok(json!({ "type": "pane_selection", "pane_id": pane, "text": text }))
}

fn code<T: std::fmt::Debug>(result: Result<T, RuntimeError>) -> String {
    result.expect_err("action refused").code
}

// =======================================================================================
// Part A — composed window: ComposedSurface binding/epoch + TerminalActions::composed + hub
// =======================================================================================

type Admit<'a> = &'a dyn Fn() -> Result<(), RuntimeError>;

/// Endpoint lane of one fake connection that serializes requesters like the product correlator
/// (one request at a time) and runs the admission after its turn, before recording the write.
struct SerialLane {
    wire: Arc<Mutex<Wire>>,
    methods: Vec<String>,
    serial: Mutex<()>,
    /// Requesters that entered the lane (waiting or in flight).
    entered: AtomicUsize,
    gates: Mutex<BTreeMap<String, VecDeque<Gate>>>,
    replies: Mutex<BTreeMap<String, VecDeque<Reply>>>,
}

impl SerialLane {
    fn run(
        &self,
        boot_id: &str,
        method: &str,
        params: Value,
        admit: Option<Admit<'_>>,
    ) -> Result<Value, RuntimeError> {
        self.entered.fetch_add(1, Ordering::AcqRel);
        let _turn = self.serial.lock().unwrap();
        if let Some(admit) = admit {
            admit()?;
        }
        self.wire
            .lock()
            .unwrap()
            .endpoint
            .push((boot_id.to_owned(), method.to_owned(), params));
        let gate = self
            .gates
            .lock()
            .unwrap()
            .get_mut(method)
            .and_then(VecDeque::pop_front);
        if let Some(gate) = gate {
            gate.pass();
        }
        let reply = self
            .replies
            .lock()
            .unwrap()
            .get_mut(method)
            .and_then(VecDeque::pop_front);
        reply.unwrap_or_else(|| Ok(json!({ "type": "ok" })))
    }
}

impl EndpointLane for SerialLane {
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.run(boot_id, method, params, None)
    }
    fn request_admitted(
        &self,
        boot_id: &str,
        method: &str,
        params: Value,
        admit: Admit<'_>,
    ) -> Result<Value, RuntimeError> {
        self.run(boot_id, method, params, Some(admit))
    }
    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

/// A lane written before the admission existed: only `request` and `methods`.
struct UnguardedLane(Arc<Mutex<Wire>>);

impl EndpointLane for UnguardedLane {
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.0
            .lock()
            .unwrap()
            .endpoint
            .push((boot_id.to_owned(), method.to_owned(), params));
        Ok(json!({ "type": "ok" }))
    }
    fn methods(&self) -> Vec<String> {
        ALL_METHODS.iter().map(|m| (*m).to_owned()).collect()
    }
}

struct Host {
    wire: Arc<Mutex<Wire>>,
    lane: Option<Arc<SerialLane>>,
    token: u64,
    boot: &'static str,
}

impl Host {
    fn calls(&self) -> Vec<(String, Value)> {
        self.wire
            .lock()
            .unwrap()
            .endpoint
            .iter()
            .map(|(_, method, params)| (method.clone(), params.clone()))
            .collect()
    }
    fn lane(&self) -> &SerialLane {
        self.lane.as_ref().expect("serial lane")
    }
    fn hold(&self, method: &str) -> (Receiver<()>, Sender<()>) {
        let (gate, reached, go) = Gate::new();
        self.lane()
            .gates
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(gate);
        (reached, go)
    }
    fn reply(&self, method: &str, reply: Reply) {
        self.lane()
            .replies
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .push_back(reply);
    }
    /// Waits until `n` requesters entered the lane (the extra ones wait for their turn).
    fn entered(&self, n: usize) {
        let deadline = Instant::now() + WITHIN;
        while self.lane().entered.load(Ordering::Acquire) < n {
            assert!(Instant::now() < deadline, "{n} requesters entered the lane");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Give the last one time to block on the lane's turn.
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn connect_host(hub: &HostHub, endpoint: &str, values: HostValues, guarded: bool) -> Host {
    let now = Instant::now();
    let wire = Arc::new(Mutex::new(Wire::default()));
    let ticket = hub.request_connect(endpoint, now).unwrap().expect("ticket");
    let session = hub.spec(endpoint).unwrap().session;
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: endpoint.into(),
                session,
                boot: values.boot.into(),
                wire: wire.clone(),
            }),
            api: Arc::new(FakeApi { wire: wire.clone() }),
        }),
        now,
    );
    let lane = guarded.then(|| {
        Arc::new(SerialLane {
            wire: wire.clone(),
            methods: ALL_METHODS.iter().map(|m| (*m).to_owned()).collect(),
            serial: Mutex::new(()),
            entered: AtomicUsize::new(0),
            gates: Mutex::default(),
            replies: Mutex::default(),
        })
    });
    let installed: Arc<dyn EndpointLane> = match &lane {
        Some(lane) => lane.clone(),
        None => Arc::new(UnguardedLane(wire.clone())),
    };
    assert!(hub.install_endpoint_lane(endpoint, ticket.token, installed));
    hub.apply_event(
        endpoint,
        ticket.token,
        GatewayEvent::Snapshot(Box::new(snapshot(values.boot))),
        now,
    );
    hub.apply_event(
        endpoint,
        ticket.token,
        GatewayEvent::Surface(Box::new(full(values, 5))),
        now,
    );
    Host {
        wire,
        lane,
        token: ticket.token,
        boot: values.boot,
    }
}

#[derive(Default)]
struct Sink(Mutex<Vec<FrameEvent>>);

impl FrameSink for Sink {
    fn send(&self, event: FrameEvent) -> bool {
        self.0.lock().unwrap().push(event);
        true
    }
}

struct Composed {
    _dirs: Vec<tempfile::TempDir>,
    connections: ConnectionsState,
    surface: ComposedSurface,
    effects: Arc<Effects>,
    actions: TerminalActions,
    ssh: String,
    local: Host,
    remote: Host,
}

impl Composed {
    fn hub(&self) -> &HostHub {
        self.connections.hub()
    }
    fn expected(&self, endpoint: &str, pane: &str) -> SurfaceIdentityDto {
        let live = self.hub().live_identity(endpoint).unwrap();
        SurfaceIdentityDto {
            endpoint: live.endpoint,
            session: live.session,
            connection_generation: live.connection_generation,
            boot_id: live.boot_id,
            pane_id: pane.into(),
        }
    }
    fn attach(&self) {
        self.surface
            .attach(geometry(), Arc::new(Sink::default()))
            .unwrap();
    }
    /// The engine confirms `pane` focused on the SSH host (same boot, same connection): snapshot
    /// and the next full surface both carry the new focus.
    fn refocus_ssh(&self, pane: &str) {
        let mut snap = snapshot(self.remote.boot);
        for p in &mut snap.panes {
            p.focused = p.pane_id == pane;
        }
        self.hub().apply_event(
            &self.ssh,
            self.remote.token,
            GatewayEvent::Snapshot(Box::new(snap)),
            Instant::now(),
        );
        let mut frame = full(SSH_VALUES, 6);
        for p in &mut frame.panes {
            p.focused = p.pane_id == pane;
        }
        self.hub().apply_event(
            &self.ssh,
            self.remote.token,
            GatewayEvent::Surface(Box::new(frame)),
            Instant::now(),
        );
        let focused = self.hub().link(&self.ssh).unwrap().focused.map(|(p, _)| p);
        assert_eq!(focused.as_deref(), Some(pane), "premise: focus confirmed");
    }
    fn ssh_live(&self) -> bool {
        self.hub().link(&self.ssh).unwrap().surface == Some(SurfaceState::Live)
    }
}

/// Local + SSH online with the same pane ids, SSH selected and attached, actions composed the way
/// the window installs them.
fn composed(ssh_guarded: bool) -> Composed {
    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd007g-local").unwrap();
    let paths = SessionPaths::for_session(herdr_config.path(), &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let connections = ConnectionsState::with_runner(
        ConnectionsConfig {
            prefs_dir: prefs.path().to_path_buf(),
            herdr_config_dir: herdr_config.path().to_path_buf(),
            herdr_state_dir: herdr_state.path().to_path_buf(),
            local_session: Some(session.clone()),
            local_auto_start: false,
            herdr_bin: "herdr".into(),
            isolated_ssh: None,
            geometry: geometry(),
        },
        Arc::new(NoSsh),
    );
    let view = connections
        .save_profile(
            SshProfileDraft {
                id: None,
                label: "remoto".into(),
                target: "tester@127.0.0.1".into(),
                port: None,
                session: "hd007g-remote".into(),
                auth: None,
            },
            false,
        )
        .unwrap();
    let ssh = view.profiles[0].id.as_str().to_owned();
    let selection = SelectionState::new(connections.clone());
    let surface = ComposedSurface::new(
        selection,
        SurfaceConfig {
            local_config_dir: herdr_config.path().to_path_buf(),
            local_session: Some(session),
            local_auto_start: false,
            surface_trace: None,
        },
    );
    surface.select(&ssh).unwrap();
    let local = connect_host(connections.hub(), LOCAL, LOCAL_VALUES, true);
    let remote = connect_host(connections.hub(), &ssh, SSH_VALUES, ssh_guarded);
    let effects = Arc::new(Effects::default());
    let actions = TerminalActions::composed(surface.clone(), effects.clone());
    let window = Composed {
        _dirs: vec![prefs, herdr_config, herdr_state],
        connections,
        surface,
        effects,
        actions,
        ssh,
        local,
        remote,
    };
    window.attach();
    assert!(window.ssh_live(), "premise: SSH surface live");
    window
}

/// Dispatches on its own thread (as the async command does), capturing the ticket at the call.
fn spawn(
    actions: &TerminalActions,
    action: TerminalAction,
) -> std::thread::JoinHandle<Result<ActionReceipt, RuntimeError>> {
    let dispatched = actions.clone().dispatch(action);
    std::thread::spawn(move || tauri::async_runtime::block_on(dispatched))
}

fn joined(
    handle: std::thread::JoinHandle<Result<ActionReceipt, RuntimeError>>,
) -> Result<ActionReceipt, RuntimeError> {
    handle.join().expect("action thread")
}

fn scroll_action(expected: SurfaceIdentityDto, pane: &str, offset: u64) -> TerminalAction {
    TerminalAction::Scroll {
        expected,
        request: scroll(pane, offset),
    }
}

/// Wrong behaviour caught: an action admitted only at the call (before the lane turn) is written
/// after the engine already confirmed another focused pane.
#[test]
fn a_focus_waiting_behind_another_request_is_refused_when_focus_changes_before_its_turn() {
    let w = composed(true);
    let (reached, go) = w.remote.hold("pane.scroll");
    let first = spawn(
        &w.actions,
        scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 3),
    );
    reached
        .recv_timeout(WITHIN)
        .expect("first request in flight");
    let waiting = spawn(
        &w.actions,
        TerminalAction::Focus {
            expected: w.expected(&w.ssh, FOCUSED),
            request: focus(OTHER),
        },
    );
    w.remote.entered(2);
    w.refocus_ssh(OTHER);
    go.send(()).unwrap();
    assert!(joined(first).is_ok(), "the request ahead completes");
    assert_eq!(code(joined(waiting)), "focus_changed");
    assert_eq!(
        w.remote.calls(),
        vec![(
            "pane.scroll".to_owned(),
            json!({ "pane_id": OTHER, "offset_from_bottom": 3 })
        )],
        "zero second send"
    );
    assert!(w.local.calls().is_empty(), "nothing on Local");

    // A new action of the current focus is sent once.
    let receipt = joined(spawn(
        &w.actions,
        TerminalAction::Focus {
            expected: w.expected(&w.ssh, OTHER),
            request: focus(FOCUSED),
        },
    ))
    .unwrap();
    assert!(receipt.sent);
    assert_eq!(w.remote.calls().len(), 2);
    assert_eq!(w.remote.calls()[1].1, json!({ "pane_id": FOCUSED }));
}

/// Wrong behaviour caught: an action captured under one attachment/interest is written after
/// detach, hide→show or re-attach of the same endpoint (identity and focus unchanged).
#[test]
fn detach_hide_show_or_reattach_while_waiting_sends_nothing_and_a_new_binding_sends_once() {
    type Mutation = fn(&Composed);
    let mutations: [(&str, Mutation); 3] = [
        ("detach", |w| {
            w.surface.detach();
        }),
        ("hide_show", |w| {
            w.surface.set_interest(false).unwrap();
            w.surface.set_interest(true).unwrap();
            w.hub().apply_event(
                &w.ssh,
                w.remote.token,
                GatewayEvent::Snapshot(Box::new(snapshot(w.remote.boot))),
                Instant::now(),
            );
            // Show completes on a full surface of the snapshot's projection (activation rule).
            let mut frame = full(SSH_VALUES, 6);
            frame.projection_revision = snapshot(w.remote.boot).revision;
            w.hub().apply_event(
                &w.ssh,
                w.remote.token,
                GatewayEvent::Surface(Box::new(frame)),
                Instant::now(),
            );
            assert!(w.ssh_live(), "premise: surface live again after show");
        }),
        ("reattach", |w| w.attach()),
    ];
    for (name, mutate) in mutations {
        let w = composed(true);
        let (reached, go) = w.remote.hold("pane.scroll");
        let first = spawn(
            &w.actions,
            scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 3),
        );
        reached
            .recv_timeout(WITHIN)
            .expect("first request in flight");
        let waiting = spawn(
            &w.actions,
            scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 5),
        );
        w.remote.entered(2);
        mutate(&w);
        go.send(()).unwrap();
        let _ = joined(first);
        let refused = code(joined(waiting));
        assert_eq!(refused, "action_cancelled", "{name}: stale ticket refused");
        assert_eq!(w.remote.calls().len(), 1, "{name}: zero second send");
        assert!(w.local.calls().is_empty(), "{name}: nothing on Local");

        // The new binding (attached again when the mutation released it) sends once.
        if name == "detach" {
            w.attach();
        }
        let receipt = joined(spawn(
            &w.actions,
            scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 7),
        ))
        .unwrap_or_else(|e| panic!("{name}: new binding action: {e:?}"));
        assert_eq!(receipt.offset_from_bottom, Some(7));
        let calls = w.remote.calls();
        assert_eq!(calls.len(), 2, "{name}: exactly one new send");
        assert_eq!(calls[1].1["offset_from_bottom"], json!(7), "{name}");
    }
}

/// Wrong behaviour caught: a ticket read only when the blocking job starts (not at the call)
/// accepts an action whose attachment changed before it ever ran; a link opens anyway.
#[test]
fn the_ticket_is_captured_at_the_call_before_any_wait() {
    let w = composed(true);
    let link_now = w.actions.clone().dispatch(TerminalAction::OpenLink {
        expected: w.expected(&w.ssh, FOCUSED),
        request: link(SSH_VALUES.link, 1, SSH_VALUES.content_revision),
    });
    let scroll_now =
        w.actions
            .clone()
            .dispatch(scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 3));
    w.attach();
    assert_eq!(
        code(tauri::async_runtime::block_on(link_now)),
        "action_cancelled"
    );
    assert_eq!(
        code(tauri::async_runtime::block_on(scroll_now)),
        "action_cancelled"
    );
    assert!(w.effects.none(), "no link opened");
    assert!(w.remote.calls().is_empty(), "nothing sent");

    // Captured on the current binding: opens once.
    let opened =
        tauri::async_runtime::block_on(w.actions.clone().dispatch(TerminalAction::OpenLink {
            expected: w.expected(&w.ssh, FOCUSED),
            request: link(SSH_VALUES.link, 1, SSH_VALUES.content_revision),
        }));
    assert!(opened.is_ok(), "{opened:?}");
    assert_eq!(w.effects.opened(), vec![SSH_VALUES.link.to_owned()]);
}

/// Wrong behaviour caught: a selection reply that arrives after re-attach or hide is written to
/// the clipboard because selection and connection are unchanged.
#[test]
fn a_late_selection_reply_writes_no_clipboard_after_the_attachment_or_interest_changed() {
    type Mutation = fn(&Composed);
    let mutations: [(&str, Mutation); 2] = [
        ("reattach", |w| w.attach()),
        ("hide", |w| {
            w.surface.set_interest(false).unwrap();
        }),
    ];
    for (name, mutate) in mutations {
        let w = composed(true);
        w.remote.reply(
            "pane.selection.read",
            engine_text(OTHER, "texto-da-engine-ssh"),
        );
        let (reached, go) = w.remote.hold("pane.selection.read");
        let copy = spawn(
            &w.actions,
            TerminalAction::CopySelection {
                expected: w.expected(&w.ssh, FOCUSED),
                request: selection(OTHER, SSH_VALUES.content_revision),
            },
        );
        reached.recv_timeout(WITHIN).expect("selection read sent");
        mutate(&w);
        go.send(()).unwrap();
        assert_eq!(code(joined(copy)), "action_cancelled", "{name}");
        assert!(
            w.effects.clipboard().is_empty(),
            "{name}: no clipboard write"
        );
        assert_eq!(w.remote.calls().len(), 1, "{name}: sent once, never again");
    }

    // Control: unchanged attachment copies the engine text once.
    let w = composed(true);
    w.remote.reply(
        "pane.selection.read",
        engine_text(OTHER, "texto-da-engine-ssh"),
    );
    let receipt = joined(spawn(
        &w.actions,
        TerminalAction::CopySelection {
            expected: w.expected(&w.ssh, FOCUSED),
            request: selection(OTHER, SSH_VALUES.content_revision),
        },
    ))
    .unwrap();
    assert_eq!(receipt.copied_bytes, Some("texto-da-engine-ssh".len()));
    assert_eq!(
        w.effects.clipboard(),
        vec!["texto-da-engine-ssh".to_owned()]
    );
}

/// Wrong behaviour caught: the waiting action holds a hub/reader lock (frames and Local stall),
/// crosses Local and SSH `w1:p1`, or replaces the requested non-focused target with the focus.
#[test]
fn a_waiting_action_blocks_no_reader_or_other_host_and_keeps_its_requested_target() {
    let w = composed(true);
    let (reached, go) = w.remote.hold("pane.scroll");
    // The request ahead scrolls the pane that has history (`w1:p1` has none: scroll_unavailable).
    let first = spawn(
        &w.actions,
        scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 3),
    );
    reached
        .recv_timeout(WITHIN)
        .expect("first request in flight");
    let waiting = spawn(
        &w.actions,
        TerminalAction::Focus {
            expected: w.expected(&w.ssh, FOCUSED),
            request: focus(OTHER),
        },
    );
    w.remote.entered(2);

    // Reader path of the same host and the other host progress while both wait.
    let started = Instant::now();
    w.hub().apply_event(
        &w.ssh,
        w.remote.token,
        GatewayEvent::Surface(Box::new(full(SSH_VALUES, 6))),
        Instant::now(),
    );
    assert_eq!(
        w.hub().link(&w.ssh).unwrap().revision,
        Some(6),
        "SSH frame applied"
    );
    let local_live = w.hub().live_identity(LOCAL).unwrap();
    let local_scope = herdr_desktop::connections::hub::EndpointScope::Pane {
        pane_id: FOCUSED.into(),
        workspace_id: None,
    };
    w.hub()
        .run_endpoint_scoped(
            &local_live,
            &local_scope,
            "pane.scroll",
            json!({"pane_id": FOCUSED, "offset_from_bottom": 1}),
        )
        .expect("Local lane independent");
    assert!(started.elapsed() < Duration::from_secs(1), "no wait on SSH");
    // A Local action from the window while SSH is selected is refused, unsent.
    assert_eq!(
        code(joined(spawn(
            &w.actions,
            scroll_action(w.expected(LOCAL, FOCUSED), OTHER, 3)
        ))),
        "selection_changed"
    );
    assert_eq!(w.local.calls().len(), 1, "only the direct Local control");

    go.send(()).unwrap();
    assert!(joined(first).is_ok());
    let receipt = joined(waiting).expect("unchanged state admits the waiting focus");
    assert_eq!(receipt.pane_id, OTHER);
    assert_eq!(
        w.remote.calls()[1],
        ("pane.focus".to_owned(), json!({ "pane_id": OTHER })),
        "requested target, not the focused pane"
    );
    assert_eq!(w.remote.calls().len(), 2);
}

/// Wrong behaviour caught: a lane without the admission override silently sends composed actions
/// unguarded (a default that hides the missing guard).
#[test]
fn a_lane_without_the_admission_refuses_composed_actions_unsent() {
    let w = composed(false);
    let refused = joined(spawn(
        &w.actions,
        scroll_action(w.expected(&w.ssh, FOCUSED), OTHER, 3),
    ));
    assert_eq!(code(refused), "endpoint_guard_unsupported");
    assert!(w.remote.calls().is_empty(), "nothing sent");
    assert!(w.local.calls().is_empty());
}

// =======================================================================================
// Part B — product lanes (Local socket, SSH bridge) through the hub's EndpointLane trait
// =======================================================================================

#[cfg(unix)]
mod product {
    use super::*;
    use herdr_client::local::LocalGateway;
    use herdr_client::protocol::endpoint::{
        EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_SNAPSHOT_KIND, ENDPOINT_WELCOME_KIND,
    };
    use herdr_client::protocol::wire::{ClientMessage, ServerMessage};
    use herdr_client::protocol::{decode_message, read_frame, write_message, MAX_FRAME_SIZE};
    use herdr_desktop::bridge::ssh::SshConnector;
    use herdr_desktop::connections::ssh_options::{ProfileId, SshIdentity};
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    const METHODS: &[&str] = &["pane.focus", "pane.scroll"];

    fn welcome_json() -> String {
        let mut welcome: EndpointServerWelcome = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../tests/fixtures/endpoint-welcome-v1.json"
            ))
            .unwrap(),
        )
        .unwrap();
        welcome.methods = METHODS.iter().map(|m| (*m).to_owned()).collect();
        for capability in ["surface_interest", "health_check"] {
            if !welcome.capabilities.iter().any(|c| c == capability) {
                welcome.capabilities.push(capability.to_owned());
            }
        }
        serde_json::to_string(&welcome).unwrap()
    }

    /// Engine end of the connection: answers the hello, then forwards every client message.
    struct Engine {
        writer: Mutex<UnixStream>,
        inbox: Mutex<Receiver<ClientMessage>>,
    }

    impl Engine {
        fn serve(mut server: UnixStream) -> Engine {
            let hello: ClientMessage =
                decode_message(&read_frame(&mut server, MAX_FRAME_SIZE).unwrap()).unwrap();
            assert!(
                matches!(&hello, ClientMessage::EndpointControl { kind, .. } if kind == ENDPOINT_HELLO_KIND)
            );
            write_message(
                &mut server,
                &ServerMessage::EndpointControl {
                    kind: ENDPOINT_WELCOME_KIND.into(),
                    data: welcome_json(),
                },
            )
            .unwrap();
            let (tx, rx) = channel();
            let mut reader = server.try_clone().unwrap();
            std::thread::spawn(move || {
                while let Ok(frame) = read_frame(&mut reader, MAX_FRAME_SIZE) {
                    let Ok(message) = decode_message::<ClientMessage>(&frame) else {
                        return;
                    };
                    if tx.send(message).is_err() {
                        return;
                    }
                }
            });
            Engine {
                writer: Mutex::new(server),
                inbox: Mutex::new(rx),
            }
        }

        fn send(&self, message: &ServerMessage) {
            write_message(&mut *self.writer.lock().unwrap(), message).unwrap();
        }

        fn publish(&self, boot: &str, revision: u64) {
            self.send(&ServerMessage::EndpointControl {
                kind: ENDPOINT_SNAPSHOT_KIND.into(),
                data: serde_json::to_string(&snapshot(boot)).unwrap(),
            });
            let mut frame = full(SSH_VALUES, revision);
            frame.boot_id = boot.into();
            self.send(&ServerMessage::PaneSurface(frame));
        }

        /// Endpoint requests received within `window`: (id, method).
        fn requests(&self, window: Duration) -> Vec<(String, String)> {
            let inbox = self.inbox.lock().unwrap();
            let deadline = Instant::now() + window;
            let mut found = Vec::new();
            while let Ok(message) =
                inbox.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                if let ClientMessage::ClientShellEndpointRequest { request, .. } = message {
                    let value: Value = serde_json::from_str(&request).unwrap();
                    found.push((
                        value["id"].as_str().unwrap().to_owned(),
                        value["method"].as_str().unwrap().to_owned(),
                    ));
                    return found;
                }
            }
            found
        }

        fn answer(&self, boot: &str, id: &str, result: Value) {
            self.send(&ServerMessage::ClientShellEndpointResponseChunk {
                boot_id: boot.into(),
                request_id: id.into(),
                final_chunk: true,
                data: json!({ "id": id, "result": result })
                    .to_string()
                    .into_bytes(),
            });
        }
    }

    struct PipeChild {
        socket: UnixStream,
        stdin: Option<UnixStream>,
        stdout: Option<UnixStream>,
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
            Some(None)
        }
        fn kill(&mut self) {
            let _ = self.socket.shutdown(std::net::Shutdown::Both);
        }
    }

    struct PipeRunner(Mutex<Option<UnixStream>>);

    impl SshRunner for PipeRunner {
        fn output(
            &self,
            _command: &OpenSshCommand,
            _timeout: Duration,
            _stdin: Option<&[u8]>,
        ) -> std::io::Result<ProcessOutput> {
            Ok(ProcessOutput {
                status: Some(0),
                stdout: br#"{"running":true,"capabilities":{"endpoint_protocol_generation":1,"surface_interest":true,"health_check":true}}"#
                    .to_vec(),
                stderr: String::new(),
            })
        }
        fn spawn(&self, _command: &OpenSshCommand) -> std::io::Result<Box<dyn SshChild>> {
            let socket = self.0.lock().unwrap().take().expect("one bridge");
            Ok(Box::new(PipeChild {
                stdin: Some(socket.try_clone()?),
                stdout: Some(socket.try_clone()?),
                socket,
            }))
        }
    }

    fn options() -> ConnectOptions {
        ConnectOptions {
            geometry: geometry(),
            surface_active: true,
        }
    }

    fn wait_identity(gateway: &dyn RuntimeGateway) {
        let deadline = Instant::now() + WITHIN;
        while gateway.identity().is_none() {
            assert!(Instant::now() < deadline, "boot learned");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Real SSH gateway over a socket pair; returns the lane as the hub installs it.
    fn ssh(boot: &str) -> (Engine, Box<dyn RuntimeGateway>, Arc<dyn EndpointLane>) {
        let (client, server) = UnixStream::pair().unwrap();
        let accept = std::thread::spawn(move || Engine::serve(server));
        let identity = SshIdentity::new(
            ProfileId::parse("0007cccc0007cccc0007cccc0007cccc").unwrap(),
            "tester@127.0.0.1",
            None,
            "hd007g-guard",
        )
        .unwrap();
        let gateway = SshConnector::new(
            identity,
            None,
            Arc::new(PipeRunner(Mutex::new(Some(client)))),
        )
        .connect(options())
        .unwrap_or_else(|f| panic!("ssh connect: {:?}", f.error()));
        let engine = accept.join().unwrap();
        engine.publish(boot, 1);
        wait_identity(&gateway);
        let lane: Arc<dyn EndpointLane> = Arc::new(gateway.endpoint_lane().expect("ssh lane"));
        (engine, Box::new(gateway), lane)
    }

    /// Real Local gateway on a session socket of a temp config dir.
    fn local(
        boot: &str,
    ) -> (
        tempfile::TempDir,
        Engine,
        Box<dyn RuntimeGateway>,
        Arc<dyn EndpointLane>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionName::parse("hd007g-guard").unwrap();
        let paths = SessionPaths::for_session(dir.path(), &session);
        std::fs::create_dir_all(paths.client_socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&paths.client_socket).unwrap();
        let accept = std::thread::spawn(move || Engine::serve(listener.accept().unwrap().0));
        let mut gateway = LocalGateway::new(dir.path(), session);
        gateway.connect(options()).expect("local connect");
        let engine = accept.join().unwrap();
        engine.publish(boot, 1);
        wait_identity(&gateway);
        let lane: Arc<dyn EndpointLane> = Arc::new(gateway.endpoint_lane().expect("local lane"));
        (dir, engine, Box::new(gateway), lane)
    }

    /// Wrong behaviour caught: the product lane uses the trait default (refusal or unguarded
    /// send), runs the admission before the request ahead finished (so a later change is not
    /// seen), writes a refused request, or blocks its reader while a requester waits.
    fn admission_runs_after_the_request_ahead(
        name: &str,
        boot: &str,
        engine: &Engine,
        gateway: &mut dyn RuntimeGateway,
        lane: Arc<dyn EndpointLane>,
    ) {
        let events = gateway.take_events().expect("events");
        let first = {
            let lane = lane.clone();
            let boot = boot.to_owned();
            std::thread::spawn(move || {
                lane.request_admitted(
                    &boot,
                    "pane.scroll",
                    json!({"pane_id": FOCUSED}),
                    &|| Ok(()),
                )
            })
        };
        let ahead = engine.requests(WITHIN);
        assert_eq!(ahead.len(), 1, "{name}: first request written");
        let stale = Arc::new(AtomicBool::new(false));
        let admissions = Arc::new(AtomicUsize::new(0));
        let waiting = {
            let (lane, stale, admissions, boot) = (
                lane.clone(),
                stale.clone(),
                admissions.clone(),
                boot.to_owned(),
            );
            std::thread::spawn(move || {
                lane.request_admitted(&boot, "pane.focus", json!({"pane_id": OTHER}), &|| {
                    admissions.fetch_add(1, Ordering::AcqRel);
                    if stale.load(Ordering::Acquire) {
                        Err(RuntimeError::new("focus_changed", "stale in test"))
                    } else {
                        Ok(())
                    }
                })
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            admissions.load(Ordering::Acquire),
            0,
            "{name}: admission waits its turn"
        );

        // The reader still delivers frames while a requester waits.
        while events.try_recv().is_ok() {}
        engine.publish(boot, 2);
        let deadline = Instant::now() + WITHIN;
        loop {
            match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(GatewayEvent::Surface(frame)) if frame.surface_revision == 2 => break,
                Ok(_) => {}
                Err(_) => panic!("{name}: reader delivered no frame while a requester waited"),
            }
        }

        stale.store(true, Ordering::Release);
        engine.answer(boot, &ahead[0].0, json!({"type": "ok"}));
        assert!(first.join().unwrap().is_ok(), "{name}: first answered");
        assert_eq!(
            waiting.join().unwrap().map_err(|e| e.code),
            Err("focus_changed".to_owned()),
            "{name}: refused by the admission"
        );
        assert_eq!(admissions.load(Ordering::Acquire), 1, "{name}");
        assert!(
            engine.requests(Duration::from_millis(300)).is_empty(),
            "{name}: refused request never written"
        );

        // Control: an admitted request is written once and answered.
        let admitted = {
            let (lane, boot) = (lane.clone(), boot.to_owned());
            std::thread::spawn(move || {
                lane.request_admitted(&boot, "pane.focus", json!({"pane_id": OTHER}), &|| Ok(()))
            })
        };
        let sent = engine.requests(WITHIN);
        assert_eq!(sent.len(), 1, "{name}: admitted request written");
        assert_eq!(sent[0].1, "pane.focus");
        engine.answer(boot, &sent[0].0, json!({"type": "ok"}));
        assert!(
            admitted.join().unwrap().is_ok(),
            "{name}: admitted answered"
        );
        assert!(
            engine.requests(Duration::from_millis(200)).is_empty(),
            "{name}: once"
        );
    }

    #[test]
    fn local_and_ssh_product_lanes_admit_after_serialization_and_write_nothing_when_refused() {
        let (engine, mut gateway, lane) = ssh(SSH_VALUES.boot);
        admission_runs_after_the_request_ahead(
            "ssh",
            SSH_VALUES.boot,
            &engine,
            gateway.as_mut(),
            lane,
        );
        gateway.detach();
        let (_dir, engine, mut gateway, lane) = local(LOCAL_VALUES.boot);
        admission_runs_after_the_request_ahead(
            "local",
            LOCAL_VALUES.boot,
            &engine,
            gateway.as_mut(),
            lane,
        );
        gateway.detach();
    }
}
