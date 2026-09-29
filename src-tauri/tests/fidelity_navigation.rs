//! Spec 007 — navigation checkpoint: the hosted agents of the composed window follow the
//! topology/focus/geometry CONFIRMED by the selected connection (project open, split, focus,
//! resize) without an incidental action, and never keep a confirmation of a connection that
//! ended. Seam: hub + selection + composed surface (the window's one observer) + hosted agents,
//! every host a fake. No GUI, engine, default session, `pane.read` or second gateway.
//!
//! Distinct values on purpose: Local boots `boot-local-5` / `boot-local-6`, SSH boot
//! `boot-ssh-8`; both hosts expose the same pane id `w1:p1`; the project opens `w2:p1` at
//! 120x40 over an initial 80x24 single pane.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow, PaneSurfaceSplit,
    PaneSurfaceSplitDirection, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SurfaceGeometry,
};
use herdr_desktop::bridge::agent_commands::AgentsState;
use herdr_desktop::bridge::composition::{ComposedSurface, SurfaceConfig};
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{
    ApiLane, Connected, EndpointLane, HostHub, HostKind, HostSpec,
};
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};

const LOCAL: &str = "local";
const SSH: &str = "ssh-nav";
const WITHIN: Duration = Duration::from_secs(5);
/// Time given to a wrongly delivered event to show up before asserting it did not.
const QUIET: Duration = Duration::from_millis(150);

// =======================================================================================
// Fixtures
// =======================================================================================

/// One pane of a fixture layout: id, rect (x, y, w, h), focused.
type PaneSpec<'a> = (&'a str, (u16, u16, u16, u16), bool);

fn rect((x, y, width, height): (u16, u16, u16, u16)) -> SurfaceRect {
    SurfaceRect {
        x,
        y,
        width,
        height,
    }
}

/// Full surface of `width`x`height` with `panes`; a vertical split when there are two panes.
fn layout(
    boot: &str,
    surface: u64,
    (width, height): (u16, u16),
    panes: &[PaneSpec<'_>],
    text: &str,
) -> PaneSurfaceFrame {
    let splits = if panes.len() == 2 {
        let pos = panes[1].1 .0.saturating_sub(1);
        vec![PaneSurfaceSplit {
            direction: PaneSurfaceSplitDirection::Horizontal,
            pos,
            area: rect((0, 0, width, height)),
            hit_rect: rect((pos, 0, 1, height)),
            path: vec![],
        }]
    } else {
        vec![]
    };
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: surface,
        frame: FrameData {
            cells: text
                .chars()
                .chain(std::iter::repeat(' '))
                .take(usize::from(width) * usize::from(height))
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
            cursor: None,
            hyperlinks: vec![],
            graphics: vec![],
        },
        panes: panes
            .iter()
            .map(|(pane, area, focused)| PaneSurfacePane {
                pane_id: (*pane).into(),
                content_revision: 1,
                rect: rect(*area),
                inner_rect: rect(*area),
                scrollbar_rect: None,
                scroll: None,
                focused: *focused,
                mouse_reporting: false,
                sgr_pixel_mouse: false,
                alternate_screen_active: false,
                pixel_width: u32::from(area.2) * 9,
                pixel_height: u32::from(area.3) * 18,
            })
            .collect(),
        splits,
        popup: None,
        graphics: Default::default(),
    }
}

const INITIAL: (u16, u16) = (80, 24);
const OPENED: (u16, u16) = (120, 40);

fn initial(boot: &str, surface: u64, text: &str) -> PaneSurfaceFrame {
    layout(
        boot,
        surface,
        INITIAL,
        &[("w1:p1", (0, 0, 80, 24), true)],
        text,
    )
}

/// What the project open leaves on the server: workspace 2 focused at the window's geometry.
fn project_opened(boot: &str, surface: u64, text: &str) -> PaneSurfaceFrame {
    layout(
        boot,
        surface,
        OPENED,
        &[("w2:p1", (0, 0, 120, 40), true)],
        text,
    )
}

fn content_patch(boot: &str, base: u64, frame: &PaneSurfaceFrame) -> PaneSurfacePatch {
    PaneSurfacePatch {
        boot_id: boot.into(),
        projection_revision: frame.projection_revision,
        base_surface_revision: base,
        surface_revision: base + 1,
        rows: vec![PaneSurfacePatchRow {
            x: 0,
            y: 0,
            cells: frame.frame.cells[..usize::from(frame.frame.width)].to_vec(),
        }],
        panes: frame.panes.clone(),
        cursor: None,
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
    snap
}

#[derive(Default)]
struct Wire {
    api: Vec<String>,
    endpoint: Vec<String>,
    inputs: usize,
    /// Pauses the n-th (1-based) API request of that method: reports "entered" and waits for
    /// "resume" (R1 attach race).
    pause: Option<(&'static str, usize, mpsc::Sender<()>, mpsc::Receiver<()>)>,
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
    fn connect(&mut self, _: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(not_in_fake())
    }
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn endpoint_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn send_input(
        &self,
        _: &QualifiedTarget,
        _: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        self.wire.lock().unwrap().inputs += 1;
        Ok(())
    }
    fn resize(&self, _: SurfaceGeometry) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_focus(&self, _: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn detach(&mut self) {}
    fn is_connected(&self) -> bool {
        true
    }
}

struct FakeApi(Arc<Mutex<Wire>>);
impl ApiLane for FakeApi {
    fn request(&self, method: &str, _: Value) -> Result<Value, RuntimeError> {
        let pause = {
            let mut wire = self.0.lock().unwrap();
            wire.api.push(method.to_owned());
            let seen = wire.api.iter().filter(|m| *m == method).count();
            if wire
                .pause
                .as_ref()
                .is_some_and(|(paused, nth, ..)| *paused == method && *nth == seen)
            {
                wire.pause.take()
            } else {
                None
            }
        };
        if let Some((_, _, entered, resume)) = pause {
            entered.send(()).unwrap();
            resume.recv().unwrap();
        }
        Ok(json!({ "type": "ok" }))
    }
}

struct FakeLane(Arc<Mutex<Wire>>);
impl EndpointLane for FakeLane {
    fn request(&self, _: &str, method: &str, _: Value) -> Result<Value, RuntimeError> {
        self.0.lock().unwrap().endpoint.push(method.to_owned());
        Ok(json!({ "type": "ok" }))
    }
    fn methods(&self) -> Vec<String> {
        ["pane.focus", "pane.split", "tab.create", "tab.focus"]
            .iter()
            .map(|m| (*m).to_owned())
            .collect()
    }
}

struct Host {
    endpoint: String,
    token: u64,
    wire: Arc<Mutex<Wire>>,
}

impl Host {
    fn event(&self, hub: &HostHub, event: GatewayEvent) {
        hub.apply_event(&self.endpoint, self.token, event, Instant::now());
    }
    fn full(&self, hub: &HostHub, frame: PaneSurfaceFrame) {
        self.event(hub, GatewayEvent::Surface(Box::new(frame)));
    }
    /// Calls the agents made on this host (API + endpoint + input).
    fn calls(&self) -> (usize, usize, usize) {
        let wire = self.wire.lock().unwrap();
        (wire.api.len(), wire.endpoint.len(), wire.inputs)
    }
}

/// Connects `endpoint` with fakes, feeds its snapshot and `first` as its first full surface.
fn connect(hub: &HostHub, endpoint: &str, boot: &str, first: PaneSurfaceFrame) -> Host {
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
                boot: boot.into(),
                wire: wire.clone(),
            }),
            api: Arc::new(FakeApi(wire.clone())),
        }),
        now,
    );
    assert!(hub.install_endpoint_lane(endpoint, ticket.token, Arc::new(FakeLane(wire.clone()))));
    let host = Host {
        endpoint: endpoint.into(),
        token: ticket.token,
        wire,
    };
    host.event(hub, GatewayEvent::Snapshot(Box::new(snapshot(boot))));
    host.full(hub, first);
    host
}

/// Records every agents event sent to one channel.
#[derive(Clone)]
struct Recorder {
    messages: Arc<Mutex<Vec<Value>>>,
    arrived: Arc<Condvar>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            messages: Arc::default(),
            arrived: Arc::new(Condvar::new()),
        }
    }
    fn channel<T>(&self) -> Channel<T> {
        let messages = self.messages.clone();
        let arrived = self.arrived.clone();
        Channel::new(move |body| {
            let value = match body {
                InvokeResponseBody::Json(text) => serde_json::from_str(&text).unwrap(),
                InvokeResponseBody::Raw(bytes) => json!({ "raw": bytes }),
            };
            messages.lock().unwrap().push(value);
            arrived.notify_all();
            Ok(())
        })
    }
    fn of(&self, kind: &str) -> Vec<Value> {
        self.messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m["type"] == kind)
            .cloned()
            .collect()
    }
    /// Waits until at least `count` events of `kind` arrived; returns them.
    fn wait_of(&self, kind: &str, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + WITHIN;
        let mut messages = self.messages.lock().unwrap();
        loop {
            let found: Vec<Value> = messages
                .iter()
                .filter(|m| m["type"] == kind)
                .cloned()
                .collect();
            if found.len() >= count {
                return found;
            }
            let left = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("waiting for {count} {kind}: {:?}", *messages));
            messages = self.arrived.wait_timeout(messages, left).unwrap().0;
        }
    }
    /// Waits until a `disconnected` state event arrived (other state events may come first).
    fn wait_disconnected(&self) {
        let deadline = Instant::now() + WITHIN;
        while self.disconnected().is_empty() {
            assert!(
                Instant::now() < deadline,
                "no disconnected state: {:?}",
                self.messages.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn disconnected(&self) -> Vec<Value> {
        self.of("state")
            .into_iter()
            .filter(|m| m["state"] == "disconnected")
            .collect()
    }
}

/// The window's services over one hub: Local `hd007n-local` and an SSH-kind host `ssh-nav`
/// (session `hd007n-remote`), nothing connected, both able to expose `w1:p1`.
struct Window {
    _dirs: Vec<tempfile::TempDir>,
    connections: ConnectionsState,
    surface: ComposedSurface,
    agents: AgentsState,
}

impl Window {
    fn hub(&self) -> &HostHub {
        self.connections.hub()
    }
}

fn window() -> Window {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let session = SessionName::parse("hd007n-local").unwrap();
    let connections = ConnectionsState::new(ConnectionsConfig {
        prefs_dir: dirs[0].path().into(),
        herdr_config_dir: dirs[1].path().into(),
        herdr_state_dir: dirs[2].path().into(),
        local_session: Some(session.clone()),
        local_auto_start: false,
        herdr_bin: "herdr".into(),
        isolated_ssh: None,
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
    });
    connections
        .hub()
        .add_host(HostSpec {
            endpoint: SSH.into(),
            label: "remoto".into(),
            kind: HostKind::Ssh,
            session: "hd007n-remote".into(),
            target: None,
            visible: false,
        })
        .unwrap();
    // Same composition as `DesktopServices::new`: one selection, the surface installs the hub's
    // one observer, agents are hosted on the selected host.
    let selection = SelectionState::new(connections.clone());
    let surface = ComposedSurface::new(
        selection.clone(),
        SurfaceConfig {
            local_config_dir: dirs[1].path().into(),
            local_session: Some(session),
            local_auto_start: false,
            surface_trace: None,
        },
    );
    let agents = AgentsState::hosted(selection.agents_host());
    Window {
        _dirs: dirs,
        connections,
        surface,
        agents,
    }
}

fn focused(event: &Value) -> &str {
    event["topology"]["focused_pane_id"].as_str().unwrap_or("")
}

fn size(event: &Value) -> (u64, u64) {
    (
        event["topology"]["width"].as_u64().unwrap(),
        event["topology"]["height"].as_u64().unwrap(),
    )
}

// =======================================================================================
// Contracts
// =======================================================================================

/// Would catch: the defect observed in the native window — hosted agents keeping the topology
/// captured on attach (w1:p1, 80x24) after the server confirmed the project's w2:p1 at 120x40,
/// until some unrelated action happened to resync; or the resync needing API/endpoint traffic.
#[test]
fn hosted_agents_receive_the_confirmed_project_focus_and_geometry_without_an_action() {
    let w = window();
    w.surface.select(LOCAL).unwrap();
    let host = connect(
        w.hub(),
        LOCAL,
        "boot-local-5",
        initial("boot-local-5", 10, "shell"),
    );
    let events = Recorder::new();
    let overview = w.agents.attach_hosted(Some(events.channel())).unwrap();
    let attached = overview.topology.expect("topology captured on attach");
    assert_eq!(attached.focused_pane_id.as_deref(), Some("w1:p1"));
    assert_eq!((attached.width, attached.height), INITIAL);
    let calls = host.calls();

    // Project open: the server focuses workspace 2 and answers the window's geometry.
    host.full(w.hub(), project_opened("boot-local-5", 11, "project"));
    let topology = events.wait_of("topology", 1);
    assert_eq!(focused(&topology[0]), "w2:p1");
    assert_eq!(size(&topology[0]), (120, 40));

    // Split + focus confirmed by the server: pane w2:p2 focused on the right.
    host.full(
        w.hub(),
        layout(
            "boot-local-5",
            12,
            OPENED,
            &[
                ("w2:p1", (0, 0, 60, 40), false),
                ("w2:p2", (61, 0, 59, 40), true),
            ],
            "split",
        ),
    );
    let topology = events.wait_of("topology", 2);
    assert_eq!(focused(&topology[1]), "w2:p2");
    assert_eq!(
        topology[1]["topology"]["panes"].as_array().unwrap().len(),
        2
    );

    // Window resize confirmed by the server: same panes, new geometry.
    host.full(
        w.hub(),
        layout(
            "boot-local-5",
            13,
            (100, 30),
            &[
                ("w2:p1", (0, 0, 50, 30), false),
                ("w2:p2", (51, 0, 49, 30), true),
            ],
            "resized",
        ),
    );
    let topology = events.wait_of("topology", 3);
    assert_eq!(size(&topology[2]), (100, 30));

    assert_eq!(
        host.calls(),
        calls,
        "no API/endpoint/input call to follow the server"
    );
}

/// Would catch: topology re-emitted for every committed frame (content-only fulls, patches,
/// snapshots), i.e. panel updates proportional to terminal output instead of layout changes.
#[test]
fn content_only_frames_and_patches_do_not_repeat_the_topology() {
    let w = window();
    w.surface.select(LOCAL).unwrap();
    let host = connect(
        w.hub(),
        LOCAL,
        "boot-local-5",
        initial("boot-local-5", 10, "shell"),
    );
    let events = Recorder::new();
    w.agents.attach_hosted(Some(events.channel())).unwrap();

    // Positive control: one layout change is published once.
    host.full(w.hub(), project_opened("boot-local-5", 20, "a"));
    events.wait_of("topology", 1);

    for (revision, text) in [(21, "b"), (22, "cc"), (23, "ddd")] {
        host.full(w.hub(), project_opened("boot-local-5", revision, text));
    }
    let latest = project_opened("boot-local-5", 23, "ddd");
    host.event(
        w.hub(),
        GatewayEvent::Patch(Box::new(content_patch("boot-local-5", 23, &latest))),
    );
    host.event(
        w.hub(),
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-5"))),
    );
    std::thread::sleep(QUIET);
    assert_eq!(
        events.of("topology").len(),
        1,
        "content changes publish nothing"
    );

    // Same shape as the attach-time capture is not re-published either.
    host.full(w.hub(), initial("boot-local-5", 24, "back"));
    let topology = events.wait_of("topology", 2);
    assert_eq!(focused(&topology[1]), "w1:p1");
    host.full(w.hub(), initial("boot-local-5", 25, "again"));
    std::thread::sleep(QUIET);
    assert_eq!(events.of("topology").len(), 2);
}

/// Would catch: agents attached to Local still confirming focus after Local's connection was
/// dropped by the selection change, or the SSH host's frames (same pane id `w1:p1`) reaching the
/// agents attached to Local; and the SSH agents not following SSH once attached there.
#[test]
fn another_hosts_frames_never_reach_agents_attached_elsewhere_and_selection_change_ends_confirmation(
) {
    let w = window();
    w.surface.select(LOCAL).unwrap();
    let local = connect(
        w.hub(),
        LOCAL,
        "boot-local-5",
        initial("boot-local-5", 10, "local"),
    );
    let local_events = Recorder::new();
    w.agents
        .attach_hosted(Some(local_events.channel()))
        .unwrap();

    w.surface.select(SSH).unwrap();
    local_events.wait_disconnected();
    assert_eq!(
        local_events.disconnected().len(),
        1,
        "Local's confirmation ends once"
    );
    let ssh = connect(
        w.hub(),
        SSH,
        "boot-ssh-8",
        layout(
            "boot-ssh-8",
            40,
            OPENED,
            &[
                ("w1:p1", (0, 0, 60, 40), false),
                ("w1:p2", (61, 0, 59, 40), true),
            ],
            "remote",
        ),
    );
    ssh.full(
        w.hub(),
        layout(
            "boot-ssh-8",
            41,
            OPENED,
            &[
                ("w1:p1", (0, 0, 60, 40), true),
                ("w1:p2", (61, 0, 59, 40), false),
            ],
            "remote focus",
        ),
    );
    std::thread::sleep(QUIET);
    assert!(
        local_events.of("topology").is_empty(),
        "no SSH topology on Local agents"
    );
    assert_eq!(local.calls().1, 0, "nothing sent to Local");

    let ssh_events = Recorder::new();
    let overview = w.agents.attach_hosted(Some(ssh_events.channel())).unwrap();
    assert_eq!(overview.identity.unwrap().boot_id, "boot-ssh-8");
    assert_eq!(
        overview.topology.unwrap().focused_pane_id.as_deref(),
        Some("w1:p1")
    );
    ssh.full(
        w.hub(),
        layout(
            "boot-ssh-8",
            42,
            OPENED,
            &[
                ("w1:p1", (0, 0, 60, 40), false),
                ("w1:p2", (61, 0, 59, 40), true),
            ],
            "remote refocus",
        ),
    );
    let topology = ssh_events.wait_of("topology", 1);
    assert_eq!(focused(&topology[0]), "w1:p2");
    std::thread::sleep(QUIET);
    assert!(local_events.of("topology").is_empty());
}

/// Would catch: a lost connection leaving the panel with its old confirmed focus, or frames of
/// the reconnected server (new boot, different focus) updating agents attached to the old one
/// instead of requiring a new attach.
#[test]
fn a_lost_connection_is_unavailable_and_the_reconnection_needs_a_new_attach() {
    let w = window();
    w.surface.select(LOCAL).unwrap();
    let first = connect(
        w.hub(),
        LOCAL,
        "boot-local-5",
        initial("boot-local-5", 10, "old"),
    );
    let events = Recorder::new();
    w.agents.attach_hosted(Some(events.channel())).unwrap();

    first.event(
        w.hub(),
        GatewayEvent::Disconnected(RuntimeError::new("connection_lost", "gone")),
    );
    events.wait_disconnected();
    assert_eq!(events.disconnected().len(), 1);
    // Late frame of the dropped token: ignored by the hub, never a topology.
    first.full(w.hub(), project_opened("boot-local-5", 11, "late"));

    let second = connect(
        w.hub(),
        LOCAL,
        "boot-local-6",
        project_opened("boot-local-6", 3, "rebooted"),
    );
    second.full(
        w.hub(),
        layout(
            "boot-local-6",
            4,
            OPENED,
            &[
                ("w2:p1", (0, 0, 60, 40), false),
                ("w2:p2", (61, 0, 59, 40), true),
            ],
            "rebooted split",
        ),
    );
    std::thread::sleep(QUIET);
    assert!(
        events.of("topology").is_empty(),
        "old attach follows no new connection"
    );
    assert_eq!(events.disconnected().len(), 1, "told once");

    let again = Recorder::new();
    let overview = w.agents.attach_hosted(Some(again.channel())).unwrap();
    assert_eq!(overview.identity.unwrap().boot_id, "boot-local-6");
    assert_eq!(
        overview.topology.unwrap().focused_pane_id.as_deref(),
        Some("w2:p2")
    );
}

/// R1 — would catch: `attach_hosted` returning a live overview (and starting its watcher) for a
/// connection that ended while its discovery/tab refresh was on the wire: the feed already told the
/// channel `disconnected`, so the late overview re-confirmed w1:p1 on a dead connection. The feed
/// lock is not held during that I/O (the ended signal is delivered while the request is paused).
#[test]
fn a_connection_lost_while_the_attach_is_on_the_wire_fails_the_attach_instead_of_confirming_it() {
    let w = window();
    w.surface.select(LOCAL).unwrap();
    let host = connect(
        w.hub(),
        LOCAL,
        "boot-local-5",
        initial("boot-local-5", 10, "shell"),
    );
    let (entered_tx, entered) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    // The 2nd `tab.list` is the attach's tab refresh after discovery: its failure is tolerated, so
    // nothing on the wire fails the attach once the connection ended there.
    host.wire.lock().unwrap().pause = Some(("tab.list", 2, entered_tx, resume_rx));
    let events = Recorder::new();

    let attached = std::thread::scope(|scope| {
        let attaching = scope.spawn(|| w.agents.attach_hosted(Some(events.channel())));
        entered
            .recv_timeout(WITHIN)
            .expect("the attach reached the wire");
        host.event(
            w.hub(),
            GatewayEvent::Disconnected(RuntimeError::new("link_dropped", "gone")),
        );
        // Delivered while discovery is still paused: the reader never waits on the attach.
        events.wait_disconnected();
        resume.send(()).unwrap();
        attaching.join().unwrap()
    });

    let error = attached.expect_err("an ended connection is never confirmed by its attach");
    assert_eq!(error.code, "connection_lost");
    assert_eq!(events.disconnected().len(), 1, "told once");
    assert!(events.of("topology").is_empty());
    let overview = w.agents.overview(false);
    assert!(
        overview.identity.is_none(),
        "no confirmed identity: {overview:?}"
    );
    assert!(
        overview.topology.is_none(),
        "no confirmed topology: {overview:?}"
    );

    // Only a new explicit attach to the new connection confirms again.
    let second = connect(
        w.hub(),
        LOCAL,
        "boot-local-6",
        initial("boot-local-6", 3, "rebooted"),
    );
    let again = Recorder::new();
    let overview = w.agents.attach_hosted(Some(again.channel())).unwrap();
    assert_eq!(overview.identity.unwrap().boot_id, "boot-local-6");
    second.full(w.hub(), project_opened("boot-local-6", 4, "project"));
    assert_eq!(focused(&again.wait_of("topology", 1)[0]), "w2:p1");
}
