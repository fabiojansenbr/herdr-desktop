//! Spec 007 — connection-confirmed tab focus. The engine applies `tab.focus` to the SOURCE
//! connection only (pinned engine `client_shell_tab_focus_changes_only_the_source_connection`):
//! the JSON API `tab.list[].focused` is the server's global view and need not change. The
//! composed window must therefore project the active tab from the client-shell snapshot of the
//! same endpoint/boot/connection generation whose revision matches the committed full surface,
//! never from the API flag, never optimistically and never by polling.
//!
//! Seam: hub + selection + composed surface (the window's one observer) + hosted agents, every
//! host a fake. No GUI, engine, default session or second gateway.
//!
//! Distinct values on purpose: the API reports `w1:t1` globally focused, this connection's
//! snapshot focuses `w1:t2`, another client/host focuses `w1:t3`; Local (`boot-local-5`) and SSH
//! (`boot-ssh-8`, rebooted `boot-ssh-9`) expose the same tab and pane ids; snapshot revisions
//! 7/8/9 differ from each other and from a stale full's projection revision.

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, SurfaceRect,
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
const SSH: &str = "ssh-tab";
const WITHIN: Duration = Duration::from_secs(5);
/// Time given to a wrongly delivered event to show up before asserting it did not.
const QUIET: Duration = Duration::from_millis(150);

// =======================================================================================
// Fixtures
// =======================================================================================

/// Full surface of one or two panes (`w1:p1`, optionally `w1:p2`) at 80x24.
fn full(boot: &str, projection: u64, surface: u64, two_panes: bool) -> PaneSurfaceFrame {
    let (width, height) = (80u16, 24u16);
    let mut panes = vec![("w1:p1", (0u16, 0u16, 80u16, 24u16), !two_panes)];
    if two_panes {
        panes[0].1 = (0, 0, 40, 24);
        panes.push(("w1:p2", (41, 0, 39, 24), true));
    }
    let rect = |(x, y, width, height): (u16, u16, u16, u16)| SurfaceRect {
        x,
        y,
        width,
        height,
    };
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: projection,
        surface_revision: surface,
        frame: FrameData {
            cells: (0..usize::from(width) * usize::from(height))
                .map(|_| CellData {
                    symbol: " ".into(),
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
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

/// Client-shell snapshot of `boot` at `revision` whose connection focuses `tab`.
fn snapshot(boot: &str, revision: u64, tab: &str) -> ClientShellSnapshot {
    let mut snap: ClientShellSnapshot = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-snapshot-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    snap.boot_id = boot.into();
    snap.revision = revision;
    snap.focused_tab_id = Some(tab.into());
    snap
}

#[derive(Default)]
struct Wire {
    api: Vec<String>,
    endpoint: Vec<String>,
    /// Tab the JSON API reports as focused (the server's global view).
    global_tab: String,
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
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
        let mut wire = self.0.lock().unwrap();
        wire.api.push(method.to_owned());
        if method != "tab.list" {
            return Ok(json!({ "type": "ok" }));
        }
        let tabs: Vec<Value> = ["w1:t1", "w1:t2", "w1:t3"]
            .iter()
            .map(|tab| {
                json!({
                    "tab_id": tab,
                    "workspace_id": "w1",
                    "label": &tab[4..],
                    "focused": *tab == wire.global_tab,
                })
            })
            .collect();
        Ok(json!({ "type": "tab_list", "tabs": tabs }))
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
    fn snapshot(&self, hub: &HostHub, snap: ClientShellSnapshot) {
        self.event(hub, GatewayEvent::Snapshot(Box::new(snap)));
    }
    fn full(&self, hub: &HostHub, frame: PaneSurfaceFrame) {
        self.event(hub, GatewayEvent::Surface(Box::new(frame)));
    }
    /// API and endpoint calls made on this host.
    fn calls(&self) -> (usize, usize) {
        let wire = self.wire.lock().unwrap();
        (wire.api.len(), wire.endpoint.len())
    }
}

/// Connects `endpoint` with fakes (API global focus `global_tab`), then feeds `snap` and `first`.
fn connect(
    hub: &HostHub,
    endpoint: &str,
    boot: &str,
    global_tab: &str,
    snap: ClientShellSnapshot,
    first: PaneSurfaceFrame,
) -> Host {
    let now = Instant::now();
    let wire = Arc::new(Mutex::new(Wire {
        global_tab: global_tab.into(),
        ..Wire::default()
    }));
    let ticket = hub.request_connect(endpoint, now).unwrap().expect("ticket");
    let session = hub.spec(endpoint).unwrap().session;
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: endpoint.into(),
                session,
                boot: boot.into(),
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
    host.snapshot(hub, snap);
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
    fn wait_disconnected(&self) {
        let deadline = Instant::now() + WITHIN;
        while !self
            .of("state")
            .iter()
            .any(|m| m["state"] == "disconnected")
        {
            assert!(Instant::now() < deadline, "no disconnected state");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The window's services over one hub: Local `hd007t-local` and SSH-kind `ssh-tab`
/// (session `hd007t-remote`), nothing connected.
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
    let session = SessionName::parse("hd007t-local").unwrap();
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
            session: "hd007t-remote".into(),
            target: None,
            visible: false,
        })
        .unwrap();
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

fn focus_of(event: &Value) -> (&str, u64) {
    (
        event["focus"]["tab_id"].as_str().unwrap_or(""),
        event["focus"]["revision"].as_u64().unwrap_or(0),
    )
}

/// SSH selected and connected: API says `w1:t1`, this connection's snapshot rev 7 says `w1:t2`.
fn attached_ssh(w: &Window) -> (Host, Recorder) {
    w.surface.select(SSH).unwrap();
    let ssh = connect(
        w.hub(),
        SSH,
        "boot-ssh-8",
        "w1:t1",
        snapshot("boot-ssh-8", 7, "w1:t2"),
        full("boot-ssh-8", 7, 30, false),
    );
    let events = Recorder::new();
    let overview = w.agents.attach_hosted(Some(events.channel())).unwrap();
    let focus = overview.tab_focus.expect("tab focus confirmed on attach");
    let identity = overview.identity.expect("identity");
    assert_eq!(focus.tab_id.as_deref(), Some("w1:t2"));
    assert_eq!(focus.revision, 7);
    assert_eq!(
        (
            focus.endpoint.as_str(),
            focus.boot_id.as_str(),
            focus.connection_generation
        ),
        (
            identity.endpoint.as_str(),
            identity.boot_id.as_str(),
            identity.connection_generation
        )
    );
    (ssh, events)
}

// =======================================================================================
// Contracts
// =======================================================================================

/// Would catch: the r2 native defect — the SSH panel pressing the tab from the global JSON API
/// flag (`w1:t1`) while its own connection shows `w1:t2`; a client focus change that keeps the
/// layout shape (same panes) never reaching the panel; following the connection by polling
/// `tab.list`; a late API refresh overriding the confirmed client focus.
#[test]
fn hosted_tab_focus_is_the_source_connection_snapshot_not_the_global_api_flag() {
    let w = window();
    let (ssh, events) = attached_ssh(&w);
    let api_tabs = w.agents.overview(false).tabs;
    assert!(
        api_tabs.iter().any(|t| t.tab_id == "w1:t1" && t.focused),
        "API list itself is kept as reported (global view): {api_tabs:?}"
    );
    let calls = ssh.calls();

    // This connection focuses w1:t1 now; same panes, so no topology change.
    ssh.snapshot(w.hub(), snapshot("boot-ssh-8", 8, "w1:t1"));
    ssh.full(w.hub(), full("boot-ssh-8", 8, 31, false));
    let focus = events.wait_of("tab_focus", 1);
    assert_eq!(focus_of(&focus[0]), ("w1:t1", 8));
    assert_eq!(focus[0]["focus"]["endpoint"], SSH);
    assert_eq!(focus[0]["focus"]["boot_id"], "boot-ssh-8");
    std::thread::sleep(QUIET);
    assert!(events.of("topology").is_empty(), "shape unchanged");
    assert_eq!(events.of("tab_focus").len(), 1, "published once");
    assert_eq!(
        ssh.calls(),
        calls,
        "no API/endpoint traffic to follow the connection"
    );

    // Content-only full of the same revision: nothing new.
    ssh.full(w.hub(), full("boot-ssh-8", 8, 32, false));
    std::thread::sleep(QUIET);
    assert_eq!(events.of("tab_focus").len(), 1);

    // Another client focuses w1:t3 globally; a late API refresh reports it.
    ssh.wire.lock().unwrap().global_tab = "w1:t3".into();
    let tabs = w
        .agents
        .with_core(|gateway, core| core.refresh_tabs(gateway))
        .unwrap();
    assert!(tabs.iter().any(|t| t.tab_id == "w1:t3" && t.focused));
    let confirmed = w.agents.overview(false).tab_focus.expect("still confirmed");
    assert_eq!(
        (confirmed.tab_id.as_deref(), confirmed.revision),
        (Some("w1:t1"), 8),
        "late API tab.list must not override the connection's focus"
    );
}

/// Would catch: projecting a snapshot whose revision differs from the committed full (focus of a
/// newer or older projection shown with another surface), or the hub answering for a revision
/// it never committed.
#[test]
fn a_snapshot_of_another_revision_is_never_projected_with_the_committed_full() {
    let w = window();
    let (ssh, events) = attached_ssh(&w);
    let identity = w.hub().live_identity(SSH).unwrap();

    ssh.snapshot(w.hub(), snapshot("boot-ssh-8", 9, "w1:t3"));
    // Stale full: projection 8 was never the revision of the stored snapshot (9).
    ssh.full(w.hub(), full("boot-ssh-8", 8, 31, true));
    events.wait_of("topology", 1);
    std::thread::sleep(QUIET);
    assert!(
        events.of("tab_focus").is_empty(),
        "revision 9 focus with projection 8"
    );
    assert_eq!(w.hub().confirmed_tab_focus(&identity, 8), None);
    assert_eq!(
        w.hub().confirmed_tab_focus(&identity, 9),
        None,
        "not committed yet"
    );
    let kept = w.agents.overview(false).tab_focus.unwrap();
    assert_eq!((kept.tab_id.as_deref(), kept.revision), (Some("w1:t2"), 7));

    ssh.full(w.hub(), full("boot-ssh-8", 9, 32, true));
    let focus = events.wait_of("tab_focus", 1);
    assert_eq!(focus_of(&focus[0]), ("w1:t3", 9));
    assert_eq!(
        w.hub().confirmed_tab_focus(&identity, 9),
        Some(Some("w1:t3".to_owned()))
    );

    // Resync pending (frames lost): the committed surface no longer confirms anything.
    ssh.event(w.hub(), GatewayEvent::QueueOverflow { dropped_frames: 3 });
    assert_eq!(w.hub().confirmed_tab_focus(&identity, 9), None);
}

/// Would catch: tab focus of another host with the same tab/pane ids (or of a hidden host)
/// reaching agents attached elsewhere; the hub confirming a focus for a foreign boot or
/// connection generation.
#[test]
fn same_tab_ids_on_another_host_or_connection_never_confirm_the_attached_focus() {
    let w = window();
    w.surface.select(LOCAL).unwrap();
    let local = connect(
        w.hub(),
        LOCAL,
        "boot-local-5",
        "w1:t1",
        snapshot("boot-local-5", 7, "w1:t3"),
        full("boot-local-5", 7, 10, false),
    );
    let local_identity = w.hub().live_identity(LOCAL).unwrap();
    assert_eq!(
        w.hub().confirmed_tab_focus(&local_identity, 7),
        Some(Some("w1:t3".to_owned()))
    );
    let local_events = Recorder::new();
    let overview = w
        .agents
        .attach_hosted(Some(local_events.channel()))
        .unwrap();
    assert_eq!(overview.tab_focus.unwrap().tab_id.as_deref(), Some("w1:t3"));

    let (ssh, ssh_events) = attached_ssh(&w);
    local_events.wait_disconnected();
    // Local hidden now: no surface, so nothing is confirmed for it.
    assert_eq!(w.hub().confirmed_tab_focus(&local_identity, 7), None);
    local.snapshot(w.hub(), snapshot("boot-local-5", 8, "w1:t1"));
    local.full(w.hub(), full("boot-local-5", 8, 11, false));

    let ssh_identity = w.hub().live_identity(SSH).unwrap();
    let foreign_boot = LiveIdentity {
        boot_id: "boot-local-5".into(),
        ..ssh_identity.clone()
    };
    let foreign_generation = LiveIdentity {
        connection_generation: ssh_identity.connection_generation + 1,
        ..ssh_identity.clone()
    };
    assert_eq!(w.hub().confirmed_tab_focus(&foreign_boot, 7), None);
    assert_eq!(w.hub().confirmed_tab_focus(&foreign_generation, 7), None);

    ssh.snapshot(w.hub(), snapshot("boot-ssh-8", 8, "w1:t1"));
    ssh.full(w.hub(), full("boot-ssh-8", 8, 31, false));
    ssh_events.wait_of("tab_focus", 1);
    std::thread::sleep(QUIET);
    assert!(
        local_events.of("tab_focus").is_empty(),
        "no SSH focus on Local agents"
    );
    assert_eq!(
        ssh_events.of("tab_focus").len(),
        1,
        "no Local focus on SSH agents"
    );
}

/// Would catch: a confirmed tab focus surviving the loss of its connection, or the rebooted
/// server's focus (same ids, new boot) reaching agents attached to the old boot.
#[test]
fn loss_or_reboot_clears_the_confirmed_tab_focus_until_a_new_attach() {
    let w = window();
    let (ssh, events) = attached_ssh(&w);
    let old = w.hub().live_identity(SSH).unwrap();

    ssh.event(
        w.hub(),
        GatewayEvent::Disconnected(RuntimeError::new("connection_lost", "gone")),
    );
    events.wait_disconnected();
    assert_eq!(
        w.agents.overview(false).tab_focus,
        None,
        "cleared with the connection"
    );
    assert_eq!(w.hub().confirmed_tab_focus(&old, 7), None);

    let rebooted = connect(
        w.hub(),
        SSH,
        "boot-ssh-9",
        "w1:t1",
        snapshot("boot-ssh-9", 7, "w1:t3"),
        full("boot-ssh-9", 7, 1, false),
    );
    rebooted.snapshot(w.hub(), snapshot("boot-ssh-9", 8, "w1:t1"));
    rebooted.full(w.hub(), full("boot-ssh-9", 8, 2, false));
    std::thread::sleep(QUIET);
    assert!(
        events.of("tab_focus").is_empty(),
        "old attach follows no new boot"
    );
    assert_eq!(w.hub().confirmed_tab_focus(&old, 8), None);

    let again = Recorder::new();
    let overview = w.agents.attach_hosted(Some(again.channel())).unwrap();
    let focus = overview.tab_focus.unwrap();
    assert_eq!(
        (
            focus.boot_id.as_str(),
            focus.tab_id.as_deref(),
            focus.revision
        ),
        ("boot-ssh-9", Some("w1:t1"), 8)
    );
}

/// Spec 027 AC-027-01: the reconciliation the window asks for after a forwarded lifecycle event
/// reads the engine's `tab.list` exactly once and returns it; a plain overview reads nothing, and
/// a disconnected host produces no read at all.
///
/// Would catch: a forwarded event that never reaches the engine list, a reconciliation that polls
/// more than once per request, or an overview that reads the API on every call.
#[test]
fn a_requested_reconciliation_reads_tab_list_once_and_a_plain_overview_reads_nothing() {
    let w = window();
    let (ssh, _events) = attached_ssh(&w);
    let before = ssh.calls();
    let overview = w.agents.overview(true);
    assert_eq!(
        ssh.calls(),
        (before.0 + 1, before.1),
        "exactly one tab.list for the reconciliation"
    );
    assert!(
        overview.tabs.iter().any(|t| t.tab_id == "w1:t2"),
        "the reconciled list is returned: {:?}",
        overview.tabs
    );
    let quiet = ssh.calls();
    let _ = w.agents.overview(false);
    assert_eq!(ssh.calls(), quiet, "a plain overview reads nothing");

    // A disconnected host is never read (target selection still holds the other host).
    w.surface.select(LOCAL).unwrap();
    let quiet = ssh.calls();
    let _ = w.agents.overview(true);
    assert_eq!(ssh.calls(), quiet, "no read on a host that is not attached");
}
