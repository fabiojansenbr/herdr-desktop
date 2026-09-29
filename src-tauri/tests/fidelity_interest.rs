//! Spec 007 — surface interest of the composed window (hide/show of the terminal): contracts at
//! the hub + composed surface seam. No GUI, engine or default session: every host is a fake.
//!
//! Distinct values on purpose: Local boot `boot-local-4` / SSH boot `boot-ssh-9`, both expose
//! pane `w1:p1`; floors, projection and snapshot revisions differ per case.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{
    CellData, ClientPaneInputEvent, ClientShellSnapshot, FrameData, PaneSurfaceFrame,
    PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow, SurfaceRect,
};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SessionName, SurfaceGeometry,
};
use herdr_desktop::bridge::composition::{
    ComposedSurface, FrameSink, SurfaceConfig, SurfaceIdentityDto,
};
use herdr_desktop::bridge::selection::SelectionState;
use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
use herdr_desktop::connections::hub::{
    ApiLane, Connected, EndpointLane, HostHub, HostKind, HostSpec, InputBlock, VisibilityOutcome,
};
use herdr_desktop::terminal::{FrameEvent, InputDto};
use serde_json::{json, Value};

const PANE: &str = "w1:p1";
const METHOD: &str = "client_shell.surface.set";
const WITHIN: Duration = Duration::from_secs(5);

fn geometry(cols: u16) -> SurfaceGeometry {
    SurfaceGeometry {
        cols,
        rows: 2,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn full(boot: &str, projection: u64, surface: u64, cols: u16, text: &str) -> PaneSurfaceFrame {
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width: cols,
        height: 2,
    };
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: projection,
        surface_revision: surface,
        frame: FrameData {
            cells: text
                .chars()
                .chain(std::iter::repeat(' '))
                .take(usize::from(cols) * 2)
                .map(|c| CellData {
                    symbol: c.to_string(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                })
                .collect(),
            width: cols,
            height: 2,
            cursor: None,
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
            pixel_width: u32::from(cols) * 9,
            pixel_height: 36,
        }],
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

fn patch(boot: &str, projection: u64, base: u64) -> PaneSurfacePatch {
    // Valid against a committed full of `boot`/`base` (would be applied if not suppressed).
    let full = full(boot, projection, base, 12, "patched");
    PaneSurfacePatch {
        boot_id: boot.into(),
        projection_revision: projection,
        base_surface_revision: base,
        surface_revision: base + 1,
        rows: vec![PaneSurfacePatchRow {
            x: 0,
            y: 0,
            cells: full.frame.cells[..12].to_vec(),
        }],
        panes: full.panes,
        cursor: None,
    }
}

fn snapshot(boot: &str, revision: u64) -> ClientShellSnapshot {
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
    assert_eq!(snap.panes[0].pane_id, PANE, "fixture premise");
    snap
}

#[derive(Default)]
struct Wire {
    inputs: Vec<Vec<ClientPaneInputEvent>>,
    resizes: Vec<SurfaceGeometry>,
    interest: Vec<(String, Value)>,
    detached: u32,
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: String,
    wire: Arc<Mutex<Wire>>,
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
        Err(RuntimeError::new("unsupported_in_fake", "-"))
    }
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "-"))
    }
    fn endpoint_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
        Err(RuntimeError::new("unsupported_in_fake", "-"))
    }
    fn send_input(
        &self,
        _: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        self.wire.lock().unwrap().inputs.push(events);
        Ok(())
    }
    fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        self.wire.lock().unwrap().resizes.push(geometry);
        Ok(())
    }
    fn set_focus(&self, _: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn detach(&mut self) {
        self.wire.lock().unwrap().detached += 1;
    }
    fn is_connected(&self) -> bool {
        true
    }
}

struct FakeApi;
impl ApiLane for FakeApi {
    fn request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
        Ok(json!({ "type": "ok" }))
    }
}

/// Reports that the request arrived, then waits for the reply the test hands over.
type HeldReply = (Sender<()>, Receiver<Result<Value, RuntimeError>>);

/// Endpoint lane: records `client_shell.surface.set`, answers with the next queued floor or,
/// when `held` is set, waits for the test to hand over the reply.
struct FakeLane {
    wire: Arc<Mutex<Wire>>,
    announced: bool,
    floors: Mutex<Vec<u64>>,
    held: Mutex<Option<HeldReply>>,
}

impl EndpointLane for FakeLane {
    fn request(&self, boot: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.wire
            .lock()
            .unwrap()
            .interest
            .push((format!("{boot}:{method}"), params.clone()));
        if let Some((reached, reply)) = self.held.lock().unwrap().take() {
            reached.send(()).unwrap();
            return reply.recv_timeout(WITHIN).expect("reply released");
        }
        let floor = self.floors.lock().unwrap().pop().unwrap_or(0);
        Ok(
            json!({"type": "client_shell_surface_set", "active": params["active"], "projection_revision": floor}),
        )
    }
    fn methods(&self) -> Vec<String> {
        let mut methods = vec!["pane.focus".to_owned()];
        if self.announced {
            methods.push(METHOD.to_owned());
        }
        methods
    }
}

struct Host {
    wire: Arc<Mutex<Wire>>,
    lane: Arc<FakeLane>,
    token: u64,
    endpoint: String,
    boot: String,
}

const BOTH: &[&str] = &["surface_interest", "presentation_effects_fence"];

/// Connects `endpoint` with fakes, installs lane + capabilities, feeds snapshot `snap_rev` and a
/// full frame with projection `snap_rev`.
fn connect(hub: &HostHub, endpoint: &str, boot: &str, caps: &[&str], method: bool) -> Host {
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
            api: Arc::new(FakeApi),
        }),
        now,
    );
    let lane = Arc::new(FakeLane {
        wire: wire.clone(),
        announced: method,
        floors: Mutex::new(Vec::new()),
        held: Mutex::new(None),
    });
    assert!(hub.install_endpoint_lane(endpoint, ticket.token, lane.clone()));
    assert!(hub.install_capabilities(
        endpoint,
        ticket.token,
        caps.iter().map(|c| (*c).to_owned()).collect()
    ));
    let host = Host {
        wire,
        lane,
        token: ticket.token,
        endpoint: endpoint.into(),
        boot: boot.into(),
    };
    host.event(hub, GatewayEvent::Snapshot(Box::new(snapshot(boot, 3))));
    host.event(
        hub,
        GatewayEvent::Surface(Box::new(full(boot, 3, 10, 12, "shown"))),
    );
    host
}

impl Host {
    fn event(&self, hub: &HostHub, event: GatewayEvent) {
        hub.apply_event(&self.endpoint, self.token, event, Instant::now());
    }
    fn open(&self, hub: &HostHub) -> bool {
        hub.input_gate(&self.endpoint, PANE).is_ok()
    }
    fn interest_calls(&self) -> Vec<(String, Value)> {
        self.wire.lock().unwrap().interest.clone()
    }
}

fn hub_with(endpoints: &[(&str, HostKind, &str)]) -> HostHub {
    let hub = HostHub::new();
    for (endpoint, kind, session) in endpoints {
        hub.add_host(HostSpec {
            endpoint: (*endpoint).into(),
            label: (*endpoint).into(),
            kind: *kind,
            session: (*session).into(),
            target: None,
            visible: true,
        })
        .unwrap();
    }
    for (endpoint, _, _) in endpoints {
        hub.resize(endpoint, geometry(12)).unwrap();
    }
    hub
}

fn local_hub() -> HostHub {
    hub_with(&[("local", HostKind::Local, "hd007i-local")])
}

// =======================================================================================
// Hub contracts
// =======================================================================================

/// Would catch: interest changes not deduplicated (floor advanced by every call) or sent to a
/// connection that did not announce the optimization.
#[test]
fn hide_and_show_send_one_surface_set_per_change_and_are_deduplicated() {
    let hub = local_hub();
    let host = connect(&hub, "local", "boot-local-4", BOTH, true);
    assert!(
        host.open(&hub),
        "positive control: shown surface accepts input"
    );

    // Already shown: nothing to send.
    let shown = hub.set_surface_interest("local", true).unwrap();
    assert!(!shown.sent);
    assert!(host.interest_calls().is_empty());

    let hidden = hub.set_surface_interest("local", false).unwrap();
    assert!(hidden.sent && hidden.supported && !hidden.active);
    let again = hub.set_surface_interest("local", false).unwrap();
    assert!(!again.sent, "hidden twice sends once");
    assert_eq!(
        host.interest_calls(),
        vec![(format!("boot-local-4:{METHOD}"), json!({"active": false}))]
    );
    assert_eq!(
        hub.input_gate("local", PANE).err(),
        Some(InputBlock::SurfaceHidden),
        "hidden surface refuses input as hidden, not stale"
    );
    assert_eq!(host.wire.lock().unwrap().detached, 0, "host kept");
}

/// Would catch: fanout/serialization of Full/Patch or recovery resizes while hidden, input
/// accepted while hidden, or the runtime (snapshot/agents) cut off by hiding.
#[test]
fn hidden_host_keeps_runtime_metadata_but_applies_no_frames_and_accepts_no_input() {
    let hub = local_hub();
    let host = connect(&hub, "local", "boot-local-4", BOTH, true);
    hub.set_surface_interest("local", false).unwrap();
    let resizes = host.wire.lock().unwrap().resizes.len();

    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 4, 11, 12, "hidden"))),
    );
    host.event(
        &hub,
        GatewayEvent::Patch(Box::new(patch("boot-local-4", 3, 10))),
    );
    host.event(&hub, GatewayEvent::QueueOverflow { dropped_frames: 3 });
    host.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 4))),
    );

    let link = hub.link("local").unwrap();
    assert_eq!(
        link.identity.as_ref().map(|i| i.boot_id.as_str()),
        Some("boot-local-4")
    );
    assert!(link.focused.is_some(), "snapshot metadata still tracked");
    let identity = link.identity.unwrap();
    assert!(
        hub.surface_for(&identity).is_none(),
        "no surface while hidden"
    );
    assert_eq!(
        host.wire.lock().unwrap().resizes.len(),
        resizes,
        "no recovery"
    );
    assert!(!host.open(&hub));
    let dto = hub.snapshot(Instant::now());
    let pane = &dto.hosts[0].panes[0];
    assert_eq!(pane.pane_id, PANE, "panes/agents still listed");
    assert!(!pane.input_enabled);
}

fn show_in_background(
    hub: &Arc<HostHub>,
    host: &Host,
) -> (
    Sender<Result<Value, RuntimeError>>,
    std::thread::JoinHandle<()>,
) {
    let (reached_tx, reached_rx) = channel();
    let (reply_tx, reply_rx) = channel();
    *host.lane.held.lock().unwrap() = Some((reached_tx, reply_rx));
    let hub2 = hub.clone();
    let endpoint = host.endpoint.clone();
    let join = std::thread::spawn(move || {
        let _ = hub2.set_surface_interest(&endpoint, true);
    });
    reached_rx
        .recv_timeout(WITHIN)
        .expect("activation request sent");
    (reply_tx, join)
}

fn ack(floor: u64) -> Result<Value, RuntimeError> {
    Ok(json!({"type": "client_shell_surface_set", "active": true, "projection_revision": floor}))
}

/// Would catch: reopening on the first Full (without ack), on an ack alone, on a Full below the
/// floor, on a snapshot/surface revision mismatch or on stale geometry.
#[test]
fn show_reopens_only_after_ack_snapshot_and_coherent_full_ack_last() {
    let hub = Arc::new(local_hub());
    let host = connect(&hub, "local", "boot-local-4", BOTH, true);
    hub.set_surface_interest("local", false).unwrap();
    let (reply, join) = show_in_background(&hub, &host);
    assert_eq!(
        host.wire.lock().unwrap().resizes.last(),
        Some(&geometry(12)),
        "current geometry applied before active:true"
    );
    // Full and snapshot with revision 7 arrive before the ack with floor 7.
    host.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 7))),
    );
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 7, 20, 12, "back"))),
    );
    assert!(!host.open(&hub), "no ack yet");
    reply.send(ack(7)).unwrap();
    join.join().unwrap();
    assert!(host.open(&hub), "ack + snapshot 7 + full 7 reopen");
}

#[test]
fn show_rejects_old_full_wrong_geometry_and_snapshot_mismatch_ack_first() {
    let hub = Arc::new(local_hub());
    let host = connect(&hub, "local", "boot-local-4", BOTH, true);
    hub.set_surface_interest("local", false).unwrap();
    let (reply, join) = show_in_background(&hub, &host);
    reply.send(ack(9)).unwrap();
    join.join().unwrap();
    assert!(!host.open(&hub), "ack alone does not reopen");
    // Snapshot and full agree with each other but are below the acknowledged floor.
    host.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 8))),
    );
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 8, 29, 12, "below"))),
    );
    assert!(!host.open(&hub), "coherent pair below the floor");

    host.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 9))),
    );
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 8, 30, 12, "old"))),
    );
    assert!(!host.open(&hub), "full below floor");
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-other", 9, 31, 12, "boot"))),
    );
    assert!(!host.open(&hub), "full of another boot");
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 9, 32, 30, "size"))),
    );
    assert!(!host.open(&hub), "full with stale geometry");
    host.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 10))),
    );
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 9, 33, 12, "behind"))),
    );
    assert!(!host.open(&hub), "snapshot revision ahead of the full");
    host.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 10, 34, 12, "ok"))),
    );
    assert!(host.open(&hub), "coherent full reopens");
    let identity = hub.link("local").unwrap().identity.unwrap();
    assert_eq!(hub.surface_for(&identity).unwrap().surface_revision, 34);
}

/// Would catch: a late acknowledgement of an older cycle or connection reopening the surface.
#[test]
fn late_ack_after_reconnect_does_not_reopen_and_hide_show_hide_ends_hidden() {
    let hub = Arc::new(local_hub());
    let host = connect(&hub, "local", "boot-local-4", BOTH, true);
    hub.set_surface_interest("local", false).unwrap();
    let (reply, join) = show_in_background(&hub, &host);
    // The connection is replaced while the ack is in flight.
    host.event(
        &hub,
        GatewayEvent::Disconnected(RuntimeError::new("connection_lost", "gone")),
    );
    reply.send(ack(5)).unwrap();
    join.join().unwrap();
    let next = connect(&hub, "local", "boot-local-5", BOTH, true);
    assert!(next.open(&hub), "new connection shows its own full");
    hub.set_surface_interest("local", false).unwrap();
    assert!(!next.open(&hub));

    // hide → show (ack held) → hide queued behind it: ends hidden, ack cannot reopen.
    let (reply, join) = show_in_background(&hub, &next);
    let hub2 = hub.clone();
    let hider = std::thread::spawn(move || hub2.set_surface_interest("local", false).unwrap());
    next.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-5", 6))),
    );
    reply.send(ack(6)).unwrap();
    join.join().unwrap();
    assert!(hider.join().unwrap().sent);
    next.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-5", 6, 40, 12, "late"))),
    );
    assert!(!next.open(&hub), "hidden after hide-show-hide");
    let calls: Vec<Value> = next.interest_calls().into_iter().map(|(_, v)| v).collect();
    assert_eq!(
        calls,
        vec![
            json!({"active": false}),
            json!({"active": true}),
            json!({"active": false})
        ],
        "ordered on the wire"
    );
}

/// Would catch: disconnecting/marking attention on an old server, sending the method anyway, or
/// the fallback leaking into another host.
#[test]
fn server_without_capability_or_method_keeps_connection_and_other_host() {
    for (caps, method) in [(&["surface_interest"][..], true), (BOTH, false)] {
        let hub = hub_with(&[
            ("local", HostKind::Local, "hd007i-local"),
            ("ssh-1", HostKind::Ssh, "hd007i-remote"),
        ]);
        let old = connect(&hub, "local", "boot-local-4", caps, method);
        let other = connect(&hub, "ssh-1", "boot-ssh-9", BOTH, true);
        let hidden = hub.set_surface_interest("local", false).unwrap();
        assert!(!hidden.supported && !hidden.sent);
        assert!(old.interest_calls().is_empty(), "method never sent");
        assert!(!old.open(&hub), "renderer/fanout suspended locally");
        assert!(other.open(&hub), "other host untouched");
        assert_eq!(old.wire.lock().unwrap().detached, 0);

        // Fallback show: geometry re-sent, next full with current geometry reopens.
        let resizes = old.wire.lock().unwrap().resizes.len();
        let shown = hub.set_surface_interest("local", true).unwrap();
        assert!(!shown.sent && !shown.supported);
        assert_eq!(old.wire.lock().unwrap().resizes.len(), resizes + 1);
        assert!(!old.open(&hub));
        old.event(
            &hub,
            GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 5))),
        );
        old.event(
            &hub,
            GatewayEvent::Surface(Box::new(full("boot-local-4", 5, 50, 12, "fallback"))),
        );
        assert!(old.open(&hub));
        assert!(old.interest_calls().is_empty());
        let _ = &other.boot;
    }
}

/// Would catch: an acknowledgement error dropping the host or being retried.
#[test]
fn activation_error_is_returned_once_and_keeps_host() {
    let hub = Arc::new(local_hub());
    let host = connect(&hub, "local", "boot-local-4", BOTH, true);
    hub.set_surface_interest("local", false).unwrap();
    let (reply, join) = show_in_background(&hub, &host);
    reply
        .send(Err(RuntimeError::new("timeout", "no reply")))
        .unwrap();
    join.join().unwrap();
    assert_eq!(host.interest_calls().len(), 2, "no retry");
    assert!(!host.open(&hub));
    assert_eq!(host.wire.lock().unwrap().detached, 0);
    assert!(hub.link("local").unwrap().identity.is_some());
}

// =======================================================================================
// Host switch (spec 035)
// =======================================================================================

/// Hides `leaving` and shows `entering`, asserting each side toggles its lease in place: the
/// interest of the leaving host is released before the entering one is activated.
fn switch_visibility(hub: &HostHub, leaving: &str, entering: &str) {
    let hidden = hub.set_visible(leaving, false, Instant::now()).unwrap();
    assert!(
        matches!(&hidden, VisibilityOutcome::Interest(o) if o.sent && !o.active),
        "leaving host not released in place: {hidden:?}"
    );
    let shown = hub.set_visible(entering, true, Instant::now()).unwrap();
    assert!(
        matches!(&shown, VisibilityOutcome::Interest(o) if o.sent && o.active),
        "entering host not activated in place: {shown:?}"
    );
}

/// Spec 035 (AC-035-01) — two switches between two online hosts, exactly like the TUI: the hub
/// sends one surface interest per host per switch on the live connections and closes/renews
/// nothing. Would catch: the selection path dropping the bridge and renegotiating `surface_active`
/// (the measured 3.5 s `servidor`/`ponte`/`handshake` per switch), a new generation, or a switch
/// that leaves the entered host without a committed surface.
#[test]
fn switching_between_online_hosts_toggles_interest_without_reconnecting() {
    let hub = hub_with(&[
        ("local", HostKind::Local, "hd007i-local"),
        ("ssh-1", HostKind::Ssh, "hd007i-remote"),
    ]);
    // Only the selected host keeps a surface: the other is hidden while offline, as a real
    // selection left it.
    assert!(matches!(
        hub.set_visible("ssh-1", false, Instant::now()).unwrap(),
        VisibilityOutcome::Interest(_)
    ));
    let first = connect(&hub, "local", "boot-local-4", BOTH, true);
    let second = connect(&hub, "ssh-1", "boot-ssh-9", BOTH, true);
    let generations = (
        hub.link("local").unwrap().generation,
        hub.link("ssh-1").unwrap().generation,
    );
    let started = Instant::now();

    switch_visibility(&hub, "local", "ssh-1");
    second.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-ssh-9", 3))),
    );
    second.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-ssh-9", 3, 11, 12, "second"))),
    );
    assert!(
        second.open(&hub),
        "the entered host presents its own surface without reconnecting"
    );

    switch_visibility(&hub, "ssh-1", "local");
    first.event(
        &hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 3))),
    );
    first.event(
        &hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 3, 12, 12, "first"))),
    );
    assert!(first.open(&hub), "the first host presents again");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the two switches went through a reconnect path"
    );
    assert_eq!(
        (
            hub.link("local").unwrap().generation,
            hub.link("ssh-1").unwrap().generation,
        ),
        generations,
        "no connection was replaced"
    );
    assert_eq!(first.wire.lock().unwrap().detached, 0, "bridge kept");
    assert_eq!(second.wire.lock().unwrap().detached, 0, "bridge kept");
    for endpoint in ["local", "ssh-1"] {
        let link = hub.link(endpoint).unwrap();
        assert_eq!(
            link.phase,
            herdr_desktop::connections::state::LinkPhase::Online,
            "{endpoint} left Online"
        );
        assert!(link.identity.is_some(), "{endpoint} keeps its identity");
    }
    assert_eq!(
        first.interest_calls(),
        vec![
            (format!("boot-local-4:{METHOD}"), json!({"active": false})),
            (format!("boot-local-4:{METHOD}"), json!({"active": true})),
        ],
        "one interest message per switch, in order"
    );
    assert_eq!(
        second.interest_calls(),
        vec![
            (format!("boot-ssh-9:{METHOD}"), json!({"active": true})),
            (format!("boot-ssh-9:{METHOD}"), json!({"active": false})),
        ],
        "one interest message per switch, in order"
    );
}

/// Spec 035 (AC-035-01) — a connection that did not announce `surface_interest` keeps the old
/// behavior: the visibility change drops it and returns the renegotiation ticket, and no
/// `client_shell.surface.set` is ever sent to it.
#[test]
fn switching_a_host_without_surface_interest_keeps_the_old_renegotiation() {
    let hub = hub_with(&[("local", HostKind::Local, "hd007i-local")]);
    let host = connect(&hub, "local", "boot-local-4", &[], false);
    let outcome = hub.set_visible("local", false, Instant::now()).unwrap();
    let VisibilityOutcome::Renegotiate(ticket) = outcome else {
        panic!("a host without the optimization must renegotiate: {outcome:?}");
    };
    assert!(!ticket.surface_active);
    assert_eq!(
        host.wire.lock().unwrap().detached,
        1,
        "old connection dropped"
    );
    assert!(!host.open(&hub));
    assert!(host.interest_calls().is_empty(), "method never sent");
    assert_eq!(
        hub.link("local").unwrap().phase,
        herdr_desktop::connections::state::LinkPhase::Reconnecting
    );
}

// =======================================================================================
// Composed surface
// =======================================================================================

#[derive(Default)]
struct Sink(Mutex<Vec<String>>);
impl FrameSink for Sink {
    fn send(&self, event: FrameEvent) -> bool {
        let kind = serde_json::to_value(&event).unwrap()["type"]
            .as_str()
            .unwrap_or("?")
            .to_owned();
        self.0.lock().unwrap().push(kind);
        true
    }
}

impl Sink {
    fn count(&self, kind: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|k| *k == kind).count()
    }
}

/// Would catch: Full/Patch reaching the channel or input dispatched while hidden/activating.
#[test]
fn composed_surface_sends_no_frames_or_input_while_hidden_and_resumes_after_coherent_full() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let connections = ConnectionsState::new(ConnectionsConfig {
        prefs_dir: dirs[0].path().into(),
        herdr_config_dir: dirs[1].path().into(),
        herdr_state_dir: dirs[2].path().into(),
        local_session: Some(SessionName::parse("hd007i-local").unwrap()),
        local_auto_start: false,
        herdr_bin: "herdr".into(),
        isolated_ssh: None,
        geometry: geometry(12),
    });
    let surface = ComposedSurface::new(
        SelectionState::new(connections.clone()),
        SurfaceConfig {
            local_config_dir: dirs[1].path().into(),
            local_session: Some(SessionName::parse("hd007i-local").unwrap()),
            local_auto_start: false,
            surface_trace: None,
        },
    );
    surface.select("local").unwrap();
    let hub = connections.hub();
    let host = connect(hub, "local", "boot-local-4", BOTH, true);
    let sink = Arc::new(Sink::default());
    surface.attach(geometry(12), sink.clone()).unwrap();
    host.event(
        hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 3, 11, 12, "control"))),
    );
    let fulls = sink.count("full");
    assert!(fulls >= 1, "positive control reaches the channel");

    surface.set_interest(false).unwrap();
    host.event(
        hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 3, 12, 12, "hidden"))),
    );
    host.event(
        hub,
        GatewayEvent::Patch(Box::new(patch("boot-local-4", 3, 11))),
    );
    assert_eq!(sink.count("full"), fulls);
    assert_eq!(sink.count("patch"), 0);
    let expected = SurfaceIdentityDto {
        endpoint: "local".into(),
        session: "hd007i-local".into(),
        connection_generation: hub.link("local").unwrap().generation,
        boot_id: "boot-local-4".into(),
        pane_id: PANE.into(),
    };
    let text: Vec<InputDto> =
        serde_json::from_value(json!([{"kind": "text", "text": "x"}])).unwrap();
    assert!(surface.input(&expected, &text).is_err());
    assert!(host.wire.lock().unwrap().inputs.is_empty(), "nothing sent");

    host.lane.floors.lock().unwrap().push(8);
    surface.set_interest(true).unwrap();
    assert!(surface.input(&expected, &text).is_err(), "activating");
    host.event(
        hub,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-local-4", 8))),
    );
    host.event(
        hub,
        GatewayEvent::Surface(Box::new(full("boot-local-4", 8, 13, 12, "resumed"))),
    );
    assert_eq!(sink.count("full"), fulls + 1, "one coherent full");
    surface.input(&expected, &text).unwrap();
    assert_eq!(
        host.wire.lock().unwrap().inputs.len(),
        1,
        "sent once, no replay"
    );
}

/// Would catch: a connection opened while the window is hidden presenting frames, or not being
/// told once (with its own boot) that the window is not interested.
#[test]
fn new_connection_while_hidden_stays_closed_and_is_told_once() {
    let hub = local_hub();
    let first = connect(&hub, "local", "boot-local-4", BOTH, true);
    hub.set_surface_interest("local", false).unwrap();
    first.event(
        &hub,
        GatewayEvent::Disconnected(RuntimeError::new("connection_lost", "gone")),
    );
    let next = connect(&hub, "local", "boot-local-5", BOTH, true);
    assert_eq!(
        hub.input_gate("local", PANE).err(),
        Some(InputBlock::SurfaceHidden)
    );
    let told = hub.restore_surface_interest("local", next.token).unwrap();
    assert!(told.is_some_and(|o| o.sent && !o.active));
    assert!(hub
        .restore_surface_interest("local", first.token)
        .unwrap()
        .is_none());
    assert_eq!(
        next.interest_calls(),
        vec![(format!("boot-local-5:{METHOD}"), json!({"active": false}))]
    );
    let identity = hub.link("local").unwrap().identity.unwrap();
    assert!(hub.surface_for(&identity).is_none());
}
