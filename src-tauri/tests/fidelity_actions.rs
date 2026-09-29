//! Spec 007 — terminal actions of the composed window (focus, scroll, copy selection, open link)
//! over the one `ConnectionsState` hub (AC-007-02/04 seam; no GUI, no engine, no default session,
//! no real clipboard/browser: native effects are injected and recorded).
//!
//! Fixture values are distinct on purpose: both hosts expose panes `w1:p1` (focused) and `w1:p2`
//! (visible, not focused); boots `boot-local-7` / `boot-ssh-3`, sessions `hd007a-local` /
//! `hd007a-remote`, `w1:p2` content revisions 11 / 40, scrollback limits 60 / 120, links
//! `https://local.example/a` / `https://remote.example/b`, engine selection texts differ from the
//! screen text.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::pin;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow, PaneSurfaceScrollMetrics, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SessionPaths, SurfaceGeometry,
};
use herdr_desktop::bridge::composition::SurfaceIdentityDto;
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshRunner};
use herdr_desktop::bridge::terminal_actions::{
    ActionReceipt, NativeEffects, TerminalAction, TerminalActions, MAX_SELECTION_TEXT_BYTES,
};
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{ApiLane, Connected, EndpointLane, HostHub};
use herdr_desktop::connections::profiles::SshProfileDraft;
use herdr_desktop::connections::ssh_options::OpenSshCommand;
use herdr_desktop::terminal::{
    FocusRequestDto, LinkRequestDto, ScrollRequestDto, SelectionRequestDto, TextPointDto,
};
use serde_json::{json, Value};

// =======================================================================================
// Fixtures
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

/// Endpoint lane of one fake connection: announced methods, scripted replies per method (default
/// `{"type":"ok"}`) and an optional gate held inside the next request of one method.
#[derive(Default)]
struct LaneScript {
    methods: Vec<String>,
    replies: BTreeMap<String, VecDeque<Reply>>,
    gates: BTreeMap<String, VecDeque<Gate>>,
}

struct FakeLane {
    wire: Arc<Mutex<Wire>>,
    script: Arc<Mutex<LaneScript>>,
}

impl EndpointLane for FakeLane {
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.wire
            .lock()
            .unwrap()
            .endpoint
            .push((boot_id.to_owned(), method.to_owned(), params));
        let (reply, gate) = {
            let mut script = self.script.lock().unwrap();
            let reply = script.replies.get_mut(method).and_then(VecDeque::pop_front);
            let gate = script.gates.get_mut(method).and_then(VecDeque::pop_front);
            (reply, gate)
        };
        if let Some(gate) = gate {
            gate.pass();
        }
        reply.unwrap_or_else(|| Ok(json!({ "type": "ok" })))
    }
    fn methods(&self) -> Vec<String> {
        self.script.lock().unwrap().methods.clone()
    }
}

struct Host {
    wire: Arc<Mutex<Wire>>,
    script: Arc<Mutex<LaneScript>>,
    token: u64,
}

impl Host {
    fn calls(&self) -> Vec<(String, String, Value)> {
        self.wire.lock().unwrap().endpoint.clone()
    }
    fn untouched(&self) -> bool {
        let wire = self.wire.lock().unwrap();
        wire.endpoint.is_empty() && wire.api.is_empty() && wire.inputs == 0
    }
    fn reply(&self, method: &str, reply: Reply) {
        self.script
            .lock()
            .unwrap()
            .replies
            .entry(method.into())
            .or_default()
            .push_back(reply);
    }
    fn hold(&self, method: &str) -> (Receiver<()>, Sender<()>) {
        let (gate, reached, go) = Gate::new();
        self.script
            .lock()
            .unwrap()
            .gates
            .entry(method.into())
            .or_default()
            .push_back(gate);
        (reached, go)
    }
}

/// Connects `endpoint` on the hub with fakes (session of its spec), installs its endpoint lane
/// announcing `methods` and feeds snapshot + one full frame.
fn connect_host(hub: &HostHub, endpoint: &str, values: HostValues, methods: &[&str]) -> Host {
    let now = Instant::now();
    let wire = Arc::new(Mutex::new(Wire::default()));
    let script = Arc::new(Mutex::new(LaneScript {
        methods: methods.iter().map(|m| (*m).to_owned()).collect(),
        ..LaneScript::default()
    }));
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
                boot: values.boot.into(),
                wire: wire.clone(),
            }),
            api: Arc::new(FakeApi { wire: wire.clone() }),
        }),
        now,
    );
    assert!(hub.install_endpoint_lane(
        endpoint,
        ticket.token,
        Arc::new(FakeLane {
            wire: wire.clone(),
            script: script.clone(),
        })
    ));
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
        script,
        token: ticket.token,
    }
}

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

struct Window {
    _dirs: Vec<tempfile::TempDir>,
    connections: ConnectionsState,
    selection: SelectionState,
    ssh: String,
    effects: Arc<Effects>,
    actions: TerminalActions,
}

impl Window {
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
    fn phase(&self, endpoint: &str) -> String {
        let host = self
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
}

/// Local session `hd007a-local` plus one saved SSH profile (`hd007a-remote`), nothing connected.
fn window() -> Window {
    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd007a-local").unwrap();
    let paths = SessionPaths::for_session(herdr_config.path(), &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
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
        Arc::new(NoSsh),
    );
    let view = connections
        .save_profile(
            SshProfileDraft {
                id: None,
                label: "remoto".into(),
                target: "tester@127.0.0.1".into(),
                port: None,
                session: "hd007a-remote".into(),
                auth: None,
            },
            false,
        )
        .unwrap();
    let ssh = view.profiles[0].id.as_str().to_owned();
    let selection = SelectionState::new(connections.clone());
    let effects = Arc::new(Effects::default());
    let actions = TerminalActions::new(selection.clone(), effects.clone());
    Window {
        _dirs: vec![prefs, herdr_config, herdr_state],
        connections,
        selection,
        ssh,
        effects,
        actions,
    }
}

/// SSH selected; Local and SSH both online with the same pane ids.
fn both_online(methods: &[&str]) -> (Window, Host, Host) {
    let w = window();
    w.selection.select(&w.ssh).unwrap();
    let local = connect_host(w.hub(), LOCAL, LOCAL_VALUES, ALL_METHODS);
    let ssh = connect_host(w.hub(), &w.ssh.clone(), SSH_VALUES, methods);
    assert!(w.hub().live_identity(LOCAL).is_ok(), "Local online premise");
    (w, local, ssh)
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
// Contracts
// =======================================================================================

/// Would catch: an action routed to Local (same pane ids) while SSH is selected, a focus/scroll
/// target replaced by the focused pane (`w1:p1`) instead of the requested visible pane, an offset
/// not clamped to the SSH scrollback (120, not Local's 60), a focus sent for an already focused
/// pane, a second send, or any native effect for these actions.
#[test]
fn focus_and_scroll_reach_only_the_selected_ssh_host_once_with_the_requested_pane() {
    let (w, local, ssh) = both_online(ALL_METHODS);
    let expected = w.expected(&w.ssh, FOCUSED);

    let receipt: ActionReceipt = w.actions.focus(&expected, &focus(OTHER)).unwrap();
    assert_eq!(receipt.pane_id, OTHER);
    assert!(receipt.sent);
    assert_eq!(
        ssh.calls(),
        [(
            "boot-ssh-3".to_owned(),
            "pane.focus".to_owned(),
            json!({ "pane_id": OTHER })
        )]
    );

    let already = w.actions.focus(&expected, &focus(FOCUSED)).unwrap();
    assert!(
        !already.sent,
        "the confirmed focused pane needs no pane.focus"
    );
    assert_eq!(ssh.calls().len(), 1);

    let scrolled = w.actions.scroll(&expected, &scroll(OTHER, 500)).unwrap();
    assert!(scrolled.sent);
    assert_eq!(scrolled.offset_from_bottom, Some(120));
    assert_eq!(
        ssh.calls()[1..],
        [(
            "boot-ssh-3".to_owned(),
            "pane.scroll".to_owned(),
            json!({ "pane_id": OTHER, "offset_from_bottom": 120 })
        )]
    );
    assert_eq!(
        code(w.actions.scroll(&expected, &scroll(FOCUSED, 3))),
        "scroll_unavailable",
        "a pane without scrollback is refused, not substituted"
    );
    assert_eq!(ssh.calls().len(), 2);

    // Local's identity (same pane ids) while SSH is selected: nothing anywhere.
    let local_expected = w.expected(LOCAL, FOCUSED);
    assert_eq!(
        code(w.actions.focus(&local_expected, &focus(OTHER))),
        "selection_changed"
    );
    assert_eq!(
        code(w.actions.scroll(&local_expected, &scroll(OTHER, 5))),
        "selection_changed"
    );
    assert!(local.untouched(), "nothing reached Local");
    assert_eq!(ssh.calls().len(), 2);
    assert!(w.effects.none());
}

/// Would catch: an action accepted with another host's boot, a wrong session or connection
/// generation, a focus that moved since the action was captured, an unknown pane, a stale
/// surface, or an identity from before a reconnect of the same boot — each must send nothing
/// and produce no native effect.
#[test]
fn stale_foreign_or_refocused_identities_send_nothing_and_cause_no_effect() {
    let (w, local, ssh) = both_online(ALL_METHODS);
    let good = w.expected(&w.ssh, FOCUSED);
    let ssh_link = link(SSH_VALUES.link, 1, SSH_VALUES.content_revision);
    let ssh_selection = selection(OTHER, SSH_VALUES.content_revision);
    ssh.reply("pane.selection.read", engine_text(OTHER, "nunca"));

    let mut foreign_boot = good.clone();
    foreign_boot.boot_id = LOCAL_VALUES.boot.into();
    let mut foreign_session = good.clone();
    foreign_session.session = "hd007a-local".into();
    let mut newer_generation = good.clone();
    newer_generation.connection_generation += 1;
    let mut refocused = good.clone();
    refocused.pane_id = OTHER.into();
    let cases = [
        (foreign_boot, "target_boot_stale"),
        (foreign_session, "target_session_mismatch"),
        (newer_generation, "target_generation_stale"),
        (refocused, "focus_changed"),
    ];
    for (expected, want) in cases {
        assert_eq!(code(w.actions.focus(&expected, &focus(OTHER))), want);
        assert_eq!(code(w.actions.scroll(&expected, &scroll(OTHER, 9))), want);
        assert_eq!(
            code(w.actions.copy_selection(&expected, &ssh_selection)),
            want
        );
        assert_eq!(code(w.actions.open_link(&expected, &ssh_link)), want);
    }
    assert_eq!(
        code(w.actions.focus(&good, &focus("w9:p9"))),
        "pane_not_found"
    );
    assert!(ssh.untouched() && local.untouched());
    assert!(w.effects.none());

    // Stale surface (patch on a wrong base): every action is refused before any effect.
    w.hub().apply_event(
        &w.ssh,
        ssh.token,
        GatewayEvent::Patch(Box::new(PaneSurfacePatch {
            boot_id: SSH_VALUES.boot.into(),
            projection_revision: 1,
            base_surface_revision: 99,
            surface_revision: 100,
            rows: vec![PaneSurfacePatchRow {
                x: 0,
                y: 0,
                cells: full(SSH_VALUES, 1).frame.cells[..24].to_vec(),
            }],
            panes: vec![],
            cursor: None,
        })),
        Instant::now(),
    );
    assert_eq!(code(w.actions.focus(&good, &focus(OTHER))), "surface_stale");
    assert_eq!(
        code(w.actions.copy_selection(&good, &ssh_selection)),
        "surface_stale"
    );
    assert_eq!(code(w.actions.open_link(&good, &ssh_link)), "surface_stale");

    // Reconnect of the same endpoint and boot: the old identity is refused on the new connection.
    w.hub().cancel(&w.ssh).unwrap();
    let renewed = connect_host(w.hub(), &w.ssh.clone(), SSH_VALUES, ALL_METHODS);
    assert_eq!(
        code(w.actions.focus(&good, &focus(OTHER))),
        "target_generation_stale"
    );
    assert_eq!(
        code(w.actions.open_link(&good, &ssh_link)),
        "target_generation_stale"
    );
    assert!(ssh.untouched() && renewed.untouched() && local.untouched());
    assert!(w.effects.none());
}

/// Would catch: clipboard text taken from anything but the engine reply (screen text, the
/// request), a reply of another pane or of an unexpected shape accepted, a selection read on
/// Local, a stale content revision or out-of-bounds selection sent anyway, an oversized text
/// written, or the native clipboard written more than once.
#[test]
fn copy_writes_only_the_engine_text_of_the_selected_host_once() {
    let (w, local, ssh) = both_online(ALL_METHODS);
    let expected = w.expected(&w.ssh, FOCUSED);
    let request = selection(OTHER, SSH_VALUES.content_revision);

    ssh.reply(
        "pane.selection.read",
        engine_text(OTHER, "texto do motor ssh\nlinha 2"),
    );
    let receipt = w.actions.copy_selection(&expected, &request).unwrap();
    assert!(receipt.sent);
    assert_eq!(receipt.copied_bytes, Some(26));
    assert_eq!(w.effects.clipboard(), ["texto do motor ssh\nlinha 2"]);
    assert_eq!(
        ssh.calls(),
        [(
            "boot-ssh-3".to_owned(),
            "pane.selection.read".to_owned(),
            json!({
                "pane_id": OTHER,
                "anchor": { "row": 2, "col": 1 },
                "cursor": { "row": 3, "col": 5 },
                "content_revision": 40,
            })
        )]
    );

    // Replies that are not this pane's selection text: sent once each, never written.
    let bad_replies = [
        (
            engine_text(FOCUSED, "outro pane"),
            "selection_reply_mismatch",
        ),
        (Ok(json!({ "type": "ok" })), "selection_reply_invalid"),
        (
            Ok(json!({ "type": "pane_selection", "pane_id": OTHER })),
            "selection_reply_invalid",
        ),
        (
            engine_text(OTHER, &"x".repeat(MAX_SELECTION_TEXT_BYTES + 1)),
            "selection_too_large",
        ),
        (
            Err(RuntimeError::new("stale_content", "pane content changed")),
            "stale_content",
        ),
    ];
    for (i, (reply, want)) in bad_replies.into_iter().enumerate() {
        ssh.reply("pane.selection.read", reply);
        assert_eq!(code(w.actions.copy_selection(&expected, &request)), want);
        assert_eq!(ssh.calls().len(), 2 + i, "one send per copy, no retry");
    }
    let sent = ssh.calls().len();

    // Refused before sending: Local's content revision, a selection outside the pane.
    assert_eq!(
        code(
            w.actions
                .copy_selection(&expected, &selection(OTHER, LOCAL_VALUES.content_revision))
        ),
        "stale_content"
    );
    let mut outside = request.clone();
    outside.cursor = point(3, 12);
    assert_eq!(
        code(w.actions.copy_selection(&expected, &outside)),
        "invalid_selection"
    );
    let mut beyond_history = request.clone();
    beyond_history.anchor = point(124, 0);
    assert_eq!(
        code(w.actions.copy_selection(&expected, &beyond_history)),
        "invalid_selection"
    );
    assert_eq!(ssh.calls().len(), sent);
    assert_eq!(
        w.effects.clipboard().len(),
        1,
        "exactly one clipboard write"
    );
    assert!(w.effects.opened().is_empty());
    assert!(local.untouched());
}

/// Would catch: a link opened without checking the current cell, content revision and URI, a
/// non-web scheme opened, the other host's link accepted, the link opened twice or through the
/// remote host (any endpoint/API call), instead of once on the local desktop.
#[test]
fn link_opens_once_on_the_desktop_only_when_cell_revision_and_uri_match() {
    let (w, local, ssh) = both_online(ALL_METHODS);
    let expected = w.expected(&w.ssh, FOCUSED);

    let refusals = [
        (
            link(LOCAL_VALUES.link, 1, SSH_VALUES.content_revision),
            "stale_link",
        ),
        (
            link(SSH_VALUES.link, 2, SSH_VALUES.content_revision),
            "stale_link",
        ),
        (
            link(UNSAFE_LINK, 3, SSH_VALUES.content_revision),
            "unsafe_link",
        ),
        (
            link(SSH_VALUES.link, 1, LOCAL_VALUES.content_revision),
            "stale_content",
        ),
        (
            link(SSH_VALUES.link, 12, SSH_VALUES.content_revision),
            "invalid_link",
        ),
    ];
    for (request, want) in refusals {
        assert_eq!(code(w.actions.open_link(&expected, &request)), want);
    }
    assert!(w.effects.none());

    let receipt = w
        .actions
        .open_link(
            &expected,
            &link(SSH_VALUES.link, 1, SSH_VALUES.content_revision),
        )
        .unwrap();
    assert!(!receipt.sent, "nothing is sent to the host");
    assert_eq!(w.effects.opened(), [SSH_VALUES.link]);
    assert!(w.effects.clipboard().is_empty());
    assert!(ssh.untouched() && local.untouched());
}

/// Would catch: a server without `pane.selection.read` disabling focus/scroll/links too, or copy
/// still sending (or writing) when the method is not announced.
#[test]
fn a_missing_selection_read_denies_only_copy() {
    let (w, local, ssh) = both_online(&["pane.focus", "pane.scroll"]);
    let expected = w.expected(&w.ssh, FOCUSED);
    assert_eq!(
        code(
            w.actions
                .copy_selection(&expected, &selection(OTHER, SSH_VALUES.content_revision))
        ),
        "unsupported_method"
    );
    assert!(ssh.untouched());
    assert!(w.effects.none());

    w.actions.focus(&expected, &focus(OTHER)).unwrap();
    w.actions.scroll(&expected, &scroll(OTHER, 4)).unwrap();
    w.actions
        .open_link(
            &expected,
            &link(SSH_VALUES.link, 1, SSH_VALUES.content_revision),
        )
        .unwrap();
    let methods: Vec<String> = ssh.calls().into_iter().map(|(_, m, _)| m).collect();
    assert_eq!(methods, ["pane.focus", "pane.scroll"]);
    assert_eq!(w.effects.opened(), [SSH_VALUES.link]);
    assert!(local.untouched());
}

/// Would catch: a slow copy writing the clipboard after a newer copy already did (older text
/// overwriting newer), a reply that arrives after the host selection changed reaching the
/// clipboard, or any late/unknown result being re-sent.
#[test]
fn late_copy_results_never_write_after_a_newer_copy_or_a_selection_change() {
    let (w, local, ssh) = both_online(ALL_METHODS);
    let expected = w.expected(&w.ssh, FOCUSED);
    let request = selection(OTHER, SSH_VALUES.content_revision);

    let (reached, go) = ssh.hold("pane.selection.read");
    ssh.reply("pane.selection.read", engine_text(OTHER, "antigo"));
    ssh.reply("pane.selection.read", engine_text(OTHER, "novo"));
    let older = {
        let actions = w.actions.clone();
        let expected = expected.clone();
        let request = request.clone();
        std::thread::spawn(move || actions.copy_selection(&expected, &request))
    };
    reached.recv_timeout(WITHIN).expect("older copy in flight");
    let newer = w.actions.copy_selection(&expected, &request).unwrap();
    assert_eq!(newer.copied_bytes, Some(4));
    go.send(()).unwrap();
    assert_eq!(
        code(older.join().unwrap()),
        "copy_superseded",
        "the older reply is not written"
    );
    assert_eq!(w.effects.clipboard(), ["novo"]);
    assert_eq!(ssh.calls().len(), 2);

    // A reply that arrives after Local was selected is never written (and never re-sent).
    let (reached, go) = ssh.hold("pane.selection.read");
    ssh.reply("pane.selection.read", engine_text(OTHER, "tardio"));
    let late = {
        let actions = w.actions.clone();
        let expected = expected.clone();
        std::thread::spawn(move || actions.copy_selection(&expected, &request))
    };
    reached.recv_timeout(WITHIN).expect("copy in flight");
    w.selection.select(LOCAL).unwrap();
    go.send(()).unwrap();
    let error = late.join().unwrap().expect_err("late reply refused");
    assert!(
        ["selection_changed", "result_unknown"].contains(&error.code.as_str()),
        "{error:?}"
    );
    assert_eq!(w.effects.clipboard(), ["novo"]);
    assert_eq!(ssh.calls().len(), 3);
    let deadline = Instant::now() + WITHIN;
    while w.phase(LOCAL) == "connecting" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(local.untouched(), "nothing reached Local");
    assert!(w.effects.opened().is_empty());
}

/// Would catch: an unknown result (timeout) or an engine refusal retried, reported as success, or
/// followed by a clipboard write; a failing native clipboard reported as copied.
#[test]
fn endpoint_and_native_failures_are_returned_once_without_replay() {
    let (w, local, ssh) = both_online(ALL_METHODS);
    let expected = w.expected(&w.ssh, FOCUSED);
    let request = selection(OTHER, SSH_VALUES.content_revision);

    ssh.reply(
        "pane.selection.read",
        Err(RuntimeError::new("timeout", "the endpoint did not answer in time").retryable()),
    );
    assert_eq!(
        code(w.actions.copy_selection(&expected, &request)),
        "result_unknown"
    );
    assert_eq!(ssh.calls().len(), 1);

    ssh.reply(
        "pane.focus",
        Err(RuntimeError::new("pane_not_found", "pane not found: w1:p2")),
    );
    assert_eq!(
        code(w.actions.focus(&expected, &focus(OTHER))),
        "pane_not_found"
    );
    assert_eq!(ssh.calls().len(), 2);

    *w.effects.clipboard_error.lock().unwrap() = Some(RuntimeError::new(
        "clipboard_failed",
        "sem área de transferência",
    ));
    ssh.reply("pane.selection.read", engine_text(OTHER, "texto"));
    assert_eq!(
        code(w.actions.copy_selection(&expected, &request)),
        "clipboard_failed"
    );
    assert_eq!(ssh.calls().len(), 3);
    assert!(w.effects.none());
    assert!(local.untouched());
}

struct Flag(Mutex<bool>);

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        *self.0.lock().unwrap() = true;
    }
}

fn poll_once<F: Future>(future: std::pin::Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

/// Would catch: the IPC entry point running the host wait on the calling (GUI) thread — the
/// future would only return after the held selection read — or delivering its result twice.
#[test]
fn dispatched_actions_wait_off_the_calling_thread_and_deliver_once() {
    let (w, _local, ssh) = both_online(ALL_METHODS);
    let expected = w.expected(&w.ssh, FOCUSED);
    let (reached, go) = ssh.hold("pane.selection.read");
    ssh.reply("pane.selection.read", engine_text(OTHER, "fora da GUI"));

    let flag = Arc::new(Flag(Mutex::new(false)));
    let waker = Waker::from(flag.clone());
    let mut future = pin!(w.actions.clone().dispatch(TerminalAction::CopySelection {
        expected,
        request: selection(OTHER, SSH_VALUES.content_revision),
    }));
    assert!(poll_once(future.as_mut(), &waker).is_pending());
    reached.recv_timeout(WITHIN).expect("read runs elsewhere");
    assert!(
        poll_once(future.as_mut(), &waker).is_pending(),
        "still waiting on the host, calling thread free"
    );
    assert!(w.effects.clipboard().is_empty());
    go.send(()).unwrap();
    let deadline = Instant::now() + WITHIN;
    let result = loop {
        if let Poll::Ready(result) = poll_once(future.as_mut(), &waker) {
            break result;
        }
        assert!(Instant::now() < deadline, "result delivered");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(result.unwrap().copied_bytes, Some(11));
    assert_eq!(w.effects.clipboard(), ["fora da GUI"]);
    assert_eq!(ssh.calls().len(), 1);
}
