//! Spec 007 — composed window, terminal fidelity and scale (seam: renderer/bridge contracts of
//! the composed runtime plus one native E2E of the composed window).
//!
//! AC-007-01: invalid base revision, reboot or a presentation queue overflow larger than 8 MiB
//!            discard only the invalid presentation, request one full surface and block input
//!            until reconciled; valid input already accepted is neither dropped nor replayed.
//! AC-007-02: fidelity corpus and accessibility (contracts here; native keyboard/IME in the E2E).
//! AC-007-03: measured by `just bench-render-scale` (evidence only); the editor-module probe of
//!            the composed window runs in the E2E.
//! AC-007-04: Local and SSH projects with the same pane id `w1:p1`: every action is routed by the
//!            qualified target of the selected host and reaches it once; remote endpoint requests
//!            are correlated by unique ids; a server without the optional API bridge disables
//!            only the dependent actions.
//!
//! Fixture values are distinct on purpose: boots `boot-local-7` / `boot-ssh-3` / `boot-ssh-4`,
//! surface revisions 5 / 9, texts `antes` / `depois` / `durante`.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::endpoint::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_WELCOME_KIND,
};
use herdr_client::protocol::wire::{
    CellData, ClientMessage, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData,
    PaneSurfaceFrame, PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow, ServerMessage,
    SurfaceRect,
};
use herdr_client::protocol::{decode_message, read_frame, write_message, MAX_FRAME_SIZE};
use herdr_client::{
    ApplyOutcome, ConnectOptions, FrameStore, GatewayEvent, LiveIdentity, LocalGateway, Negotiated,
    QualifiedTarget, RuntimeError, RuntimeGateway, SessionName, SessionPaths, StaleReason,
    SurfaceGeometry, SurfaceState,
};
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshConnector, SshGateway, SshRunner};
use herdr_desktop::connections::hub::{
    ApiLane, Connected, EndpointLane, HostHub, HostKind, HostNotice, HostSpec, HubObserver,
    NoticeOrigin,
};
use herdr_desktop::connections::ssh_options::{OpenSshCommand, ProfileId, SshIdentity};
use serde_json::{json, Value};

// =======================================================================================
// Fixtures
// =======================================================================================

const LOCAL: &str = "local";
const SSH_ID: &str = "0007aaaa0007aaaa0007aaaa0007aaaa";
const PANE: &str = "w1:p1";

fn geometry() -> SurfaceGeometry {
    SurfaceGeometry {
        cols: 12,
        rows: 2,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn pane(width: u16, height: u16, focused: bool, id: &str, x: u16) -> PaneSurfacePane {
    let rect = SurfaceRect {
        x,
        y: 0,
        width,
        height,
    };
    PaneSurfacePane {
        pane_id: id.into(),
        content_revision: 1,
        rect,
        inner_rect: rect,
        scrollbar_rect: None,
        scroll: None,
        focused,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: u32::from(width) * 9,
        pixel_height: u32::from(height) * 18,
    }
}

fn cells(text: &str, width: u16, height: u16) -> Vec<CellData> {
    text.chars()
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
        .collect()
}

fn full(boot: &str, revision: u64, text: &str) -> PaneSurfaceFrame {
    let (width, height) = (12, 2);
    PaneSurfaceFrame {
        boot_id: boot.into(),
        projection_revision: 1,
        surface_revision: revision,
        frame: FrameData {
            cells: cells(text, width, height),
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
        panes: vec![pane(width, height, true, PANE, 0)],
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

fn patch(boot: &str, base: u64, revision: u64, row_text: &str) -> PaneSurfacePatch {
    PaneSurfacePatch {
        boot_id: boot.into(),
        projection_revision: 1,
        base_surface_revision: base,
        surface_revision: revision,
        rows: vec![PaneSurfacePatchRow {
            x: 0,
            y: 1,
            cells: cells(row_text, 12, 1),
        }],
        panes: vec![pane(12, 2, true, PANE, 0)],
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
    assert_eq!(snap.panes[0].pane_id, PANE, "fixture premise");
    snap
}

fn text(s: &str) -> Vec<ClientPaneInputEvent> {
    vec![ClientPaneInputEvent::TextCommit(s.into())]
}

fn row_text(store: &FrameStore, y: usize) -> String {
    store.text_rows()[y].trim_end().to_owned()
}

// =======================================================================================
// Fakes: gateway with a mutable boot, recording API and endpoint lanes
// =======================================================================================

#[derive(Debug, Default)]
struct Wire {
    inputs: Vec<(String, Vec<ClientPaneInputEvent>)>,
    resizes: Vec<SurfaceGeometry>,
    endpoint: Vec<(String, Value)>,
    /// Boot each endpoint request was issued for (parallel to `endpoint`).
    endpoint_boots: Vec<String>,
    api: Vec<(String, Value)>,
    detached: u32,
}

struct FakeGateway {
    endpoint: String,
    session: String,
    boot: Arc<Mutex<String>>,
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
            boot_id: self.boot.lock().unwrap().clone(),
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
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        target.validate(&self.identity().unwrap())?;
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
    fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn detach(&mut self) {
        self.wire.lock().unwrap().detached += 1;
    }
    fn is_connected(&self) -> bool {
        true
    }
}

/// Records API calls on the host's wire; scripted replies by method. An optional gate blocks the
/// next request (after it was recorded) until released with its reply.
struct FakeApi {
    wire: Arc<Mutex<Wire>>,
    replies: Mutex<Vec<(String, Result<Value, RuntimeError>)>>,
    gate: Mutex<VecDeque<Receiver<Result<Value, RuntimeError>>>>,
}

impl ApiLane for FakeApi {
    fn request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        self.wire
            .lock()
            .unwrap()
            .api
            .push((method.to_owned(), params.clone()));
        let gate = self.gate.lock().unwrap().pop_front();
        if let Some(rx) = gate {
            return rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        let replies = self.replies.lock().unwrap();
        replies
            .iter()
            .find(|(m, _)| m == method)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| Ok(json!({ "type": "ok" })))
    }
}

/// Endpoint lane that records requests; an optional gate blocks the next request until released.
struct FakeLane {
    wire: Arc<Mutex<Wire>>,
    methods: Vec<String>,
    gate: Mutex<VecDeque<Receiver<Result<Value, RuntimeError>>>>,
}

impl EndpointLane for FakeLane {
    fn request(&self, boot_id: &str, method: &str, params: Value) -> Result<Value, RuntimeError> {
        {
            let mut wire = self.wire.lock().unwrap();
            wire.endpoint.push((method.to_owned(), params));
            wire.endpoint_boots.push(boot_id.to_owned());
        }
        let gate = self.gate.lock().unwrap().pop_front();
        match gate {
            Some(rx) => rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            None => Ok(json!({ "type": "ok" })),
        }
    }
    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

struct Host {
    wire: Arc<Mutex<Wire>>,
    boot: Arc<Mutex<String>>,
    token: u64,
    lane: Arc<FakeLane>,
    api: Arc<FakeApi>,
}

fn spec(endpoint: &str, visible: bool) -> HostSpec {
    let local = endpoint == LOCAL;
    HostSpec {
        endpoint: endpoint.into(),
        label: if local { "Este computador" } else { "dev-box" }.into(),
        kind: if local {
            HostKind::Local
        } else {
            HostKind::Ssh
        },
        session: if local { "hd007-local" } else { "hd007-remote" }.into(),
        target: (!local).then(|| "tester@127.0.0.1".into()),
        visible,
    }
}

fn announced() -> Vec<String> {
    [
        "pane.focus",
        "pane.split",
        "layout.set_split_ratio",
        "tab.create",
        "tab.focus",
        "workspace.focus",
        "pane.scroll",
    ]
    .iter()
    .map(|m| (*m).to_owned())
    .collect()
}

/// Connects `endpoint` with fakes, installs its endpoint lane and feeds snapshot + full frame.
fn connect_host(hub: &HostHub, endpoint: &str, boot: &str, revision: u64, screen: &str) -> Host {
    connect_host_api(hub, endpoint, boot, revision, screen, Vec::new())
}

/// [`connect_host`] with scripted JSON API replies by method.
fn connect_host_api(
    hub: &HostHub,
    endpoint: &str,
    boot: &str,
    revision: u64,
    screen: &str,
    replies: Vec<(String, Result<Value, RuntimeError>)>,
) -> Host {
    let now = Instant::now();
    let wire = Arc::new(Mutex::new(Wire::default()));
    let boot_cell = Arc::new(Mutex::new(boot.to_owned()));
    let ticket = hub
        .request_connect(endpoint, now)
        .unwrap()
        .expect("a connect ticket");
    let session = spec(endpoint, true).session;
    let api = Arc::new(FakeApi {
        wire: wire.clone(),
        replies: Mutex::new(replies),
        gate: Mutex::new(VecDeque::new()),
    });
    hub.finish_connect(
        &ticket,
        Ok(Connected {
            gateway: Box::new(FakeGateway {
                endpoint: endpoint.into(),
                session,
                boot: boot_cell.clone(),
                wire: wire.clone(),
            }),
            api: api.clone(),
        }),
        now,
    );
    let lane = Arc::new(FakeLane {
        wire: wire.clone(),
        methods: announced(),
        gate: Mutex::new(VecDeque::new()),
    });
    assert!(hub.install_endpoint_lane(endpoint, ticket.token, lane.clone()));
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
        boot: boot_cell,
        token: ticket.token,
        lane,
        api,
    }
}

// =======================================================================================
// AC-007-01 — reconciliation of invalid presentation
// =======================================================================================

/// Would catch: a rejected patch mutating cells, a stale episode issuing one full-surface
/// request per rejected patch (request storm), or a patch resurrecting a stale surface.
#[test]
fn invalid_base_revision_discards_only_the_patch_and_requests_one_full_surface() {
    let mut store = FrameStore::new();
    store.apply_full(full("boot-local-7", 5, "antes")).unwrap();
    assert_eq!(
        store.apply_patch(patch("boot-local-7", 5, 6, "linha seis")),
        ApplyOutcome::Applied
    );
    assert!(store.input_allowed());
    assert!(
        !store.take_recovery_request(),
        "live surface needs no recovery"
    );

    // Base 4 does not chain on revision 6: the whole patch is discarded.
    let outcome = store.apply_patch(patch("boot-local-7", 4, 5, "INVALIDA"));
    assert_eq!(outcome, ApplyOutcome::Rejected(StaleReason::RevisionGap));
    assert_eq!(row_text(&store, 0), "antes", "committed cells untouched");
    assert_eq!(
        row_text(&store, 1),
        "linha seis",
        "committed cells untouched"
    );
    assert_eq!(store.revision(), Some(6));
    assert!(!store.input_allowed(), "input blocked until reconciled");

    assert!(store.take_recovery_request(), "one full surface requested");
    // Further patches of the same episode (even one that would chain) change nothing and do
    // not ask again.
    for (base, rev) in [(6, 7), (7, 8), (2, 3)] {
        assert!(matches!(
            store.apply_patch(patch("boot-local-7", base, rev, "DEPOIS-INV")),
            ApplyOutcome::Rejected(_)
        ));
        assert!(!store.take_recovery_request(), "no request storm");
    }
    assert_eq!(row_text(&store, 1), "linha seis");

    // The full surface reconciles; the next chained patch applies again.
    store.apply_full(full("boot-local-7", 9, "depois")).unwrap();
    assert!(store.input_allowed());
    assert!(!store.take_recovery_request());
    assert_eq!(
        store.apply_patch(patch("boot-local-7", 9, 10, "ok")),
        ApplyOutcome::Applied
    );
    assert_eq!(row_text(&store, 0), "depois");
    assert_eq!(row_text(&store, 1), "ok");

    // A new episode may request again, exactly once.
    store.mark_stale(StaleReason::QueueOverflow);
    assert!(store.take_recovery_request());
    assert!(!store.take_recovery_request());
    // An overflow inside an episode re-arms the request: the full surface itself may have
    // been among the dropped frames.
    store.mark_stale(StaleReason::QueueOverflow);
    assert!(store.take_recovery_request());
}

/// Would catch: a patch from a rebooted server applied over the old surface, the boot change of
/// a full surface going unnoticed, or a target of the previous boot still accepted.
#[test]
fn reboot_invalidates_patches_and_targets_of_the_previous_boot() {
    let mut store = FrameStore::new();
    assert_eq!(store.apply_full(full("boot-ssh-3", 5, "antes")), Ok(false));
    let old = LiveIdentity {
        endpoint: SSH_ID.into(),
        session: "hd007-remote".into(),
        connection_generation: 2,
        boot_id: "boot-ssh-3".into(),
    };
    let target = QualifiedTarget::new(&old, Some("w1".into()), PANE);

    let outcome = store.apply_patch(patch("boot-ssh-4", 5, 6, "novo boot"));
    assert_eq!(outcome, ApplyOutcome::Rejected(StaleReason::BootChanged));
    assert_eq!(row_text(&store, 1), "", "patch of the new boot not applied");
    assert!(store.take_recovery_request());
    assert!(!store.input_allowed());

    assert_eq!(
        store.apply_full(full("boot-ssh-4", 1, "novo")),
        Ok(true),
        "boot change reported so targets are re-qualified"
    );
    let rebooted = LiveIdentity {
        boot_id: "boot-ssh-4".into(),
        ..old.clone()
    };
    let refused = target.validate(&rebooted).unwrap_err();
    assert_eq!(refused.code, "target_boot_stale");
    assert_eq!(refused.endpoint.as_deref(), Some(SSH_ID));
    assert!(QualifiedTarget::new(&rebooted, None, PANE)
        .validate(&rebooted)
        .is_ok());
}

/// Would catch: the hub never asking for a full surface after a rejected patch (surface stale
/// forever), asking once per rejected patch, input accepted while stale, or input accepted
/// before the episode being re-sent after reconciliation.
#[test]
fn hub_recovers_with_one_resize_blocks_input_and_never_replays_accepted_input() {
    let hub = HostHub::new();
    hub.add_host(spec(SSH_ID, true)).unwrap();
    let host = connect_host(&hub, SSH_ID, "boot-ssh-3", 5, "antes");
    hub.resize(SSH_ID, geometry()).unwrap();
    assert_eq!(host.wire.lock().unwrap().resizes, vec![geometry()]);

    let target = hub.input_gate(SSH_ID, PANE).expect("live input");
    hub.send_input(&target, text("antes")).unwrap();

    let now = Instant::now();
    for (base, rev) in [(3, 4), (6, 7), (7, 8)] {
        hub.apply_event(
            SSH_ID,
            host.token,
            GatewayEvent::Patch(Box::new(patch("boot-ssh-3", base, rev, "INVALIDA"))),
            now,
        );
    }
    let wire = host.wire.lock().unwrap();
    assert_eq!(
        wire.resizes,
        vec![geometry(), geometry()],
        "exactly one full-surface request (same geometry) for the episode"
    );
    drop(wire);
    let blocked = hub.send_input(&target, text("durante")).unwrap_err();
    assert_eq!(blocked.code, "input_blocked");
    let snap = serde_json::to_value(hub.snapshot(now)).unwrap();
    assert_eq!(
        snap["hosts"][0]["surface"],
        json!({ "state": "stale", "reason": "revision_gap" })
    );

    hub.apply_event(
        SSH_ID,
        host.token,
        GatewayEvent::Surface(Box::new(full("boot-ssh-3", 9, "depois"))),
        now,
    );
    let target = hub.input_gate(SSH_ID, PANE).expect("reconciled");
    hub.send_input(&target, text("depois")).unwrap();
    let inputs: Vec<Vec<ClientPaneInputEvent>> = host
        .wire
        .lock()
        .unwrap()
        .inputs
        .iter()
        .map(|(_, e)| e.clone())
        .collect();
    assert_eq!(
        inputs,
        vec![text("antes"), text("depois")],
        "accepted input sent once, refused input never queued or replayed"
    );

    // Overflow re-arms the request even inside an episode.
    hub.apply_event(
        SSH_ID,
        host.token,
        GatewayEvent::QueueOverflow { dropped_frames: 3 },
        now,
    );
    hub.apply_event(
        SSH_ID,
        host.token,
        GatewayEvent::QueueOverflow { dropped_frames: 1 },
        now,
    );
    assert_eq!(host.wire.lock().unwrap().resizes.len(), 4);
    assert_eq!(*host.boot.lock().unwrap(), "boot-ssh-3");
}

// --- fake engine over the local socket ---------------------------------------------------

struct EngineLog {
    inputs: Vec<(String, Vec<ClientPaneInputEvent>)>,
    resizes: usize,
    endpoint_requests: Vec<(String, String)>,
}

fn welcome_json(methods: &[&str]) -> String {
    let mut welcome: EndpointServerWelcome = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-welcome-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    welcome.methods = methods.iter().map(|m| (*m).to_owned()).collect();
    // The SSH connector requires these capabilities; the local gateway ignores them.
    for capability in ["surface_interest", "health_check"] {
        if !welcome.capabilities.iter().any(|c| c == capability) {
            welcome.capabilities.push(capability.to_owned());
        }
    }
    serde_json::to_string(&welcome).unwrap()
}

fn accept_hello(stream: &mut std::os::unix::net::UnixStream, methods: &[&str]) {
    let hello: ClientMessage =
        decode_message(&read_frame(stream, MAX_FRAME_SIZE).unwrap()).unwrap();
    let ClientMessage::EndpointControl { kind, data } = hello else {
        panic!("expected hello, got {hello:?}");
    };
    assert_eq!(kind, ENDPOINT_HELLO_KIND);
    let hello: EndpointClientHello = serde_json::from_str(&data).unwrap();
    assert_eq!(hello.generation, 1);
    write_message(
        stream,
        &ServerMessage::EndpointControl {
            kind: ENDPOINT_WELCOME_KIND.into(),
            data: welcome_json(methods),
        },
    )
    .unwrap();
}

/// A full frame of roughly `bytes` encoded bytes (wide surface of one-byte symbols).
fn big_full(boot: &str, revision: u64, bytes: usize) -> PaneSurfaceFrame {
    let width: u16 = 400;
    let height = (bytes / (usize::from(width) * 12)).max(1) as u16;
    let mut frame = full(boot, revision, "");
    frame.frame.width = width;
    frame.frame.height = height;
    frame.frame.cells = cells(&"x".repeat(usize::from(width)), width, height);
    frame.panes = vec![pane(width, height, true, PANE, 0)];
    frame
}

/// Would catch: an overflow notice that only arrives if another frame fits later (a burst
/// that ends leaves the surface silently wrong), frames beyond the 8 MiB budget being queued,
/// recovery that re-sends input, or more than one full-surface request for the episode.
#[cfg(unix)]
#[test]
fn queue_overflow_beyond_8_mib_is_reported_recovers_once_and_keeps_accepted_input() {
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd007-overflow").unwrap();
    let paths = SessionPaths::for_session(dir.path(), &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let listener = UnixListener::bind(&paths.client_socket).unwrap();
    let log = Arc::new(Mutex::new(EngineLog {
        inputs: Vec::new(),
        resizes: 0,
        endpoint_requests: Vec::new(),
    }));
    let (burst_done_tx, burst_done_rx) = sync_channel::<usize>(1);
    let (input_seen_tx, input_seen_rx) = sync_channel::<()>(4);
    let server_log = log.clone();
    let frame_bytes = 1_500_000usize;
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        accept_hello(&mut stream, &["pane.focus"]);
        write_message(
            &mut stream,
            &ServerMessage::EndpointControl {
                kind: herdr_client::protocol::endpoint::ENDPOINT_SNAPSHOT_KIND.into(),
                data: serde_json::to_string(&snapshot("boot-local-7")).unwrap(),
            },
        )
        .unwrap();
        write_message(
            &mut stream,
            &ServerMessage::PaneSurface(full("boot-local-7", 1, "antes")),
        )
        .unwrap();
        let mut reader = stream.try_clone().unwrap();
        let reader_log = server_log.clone();
        let reader_thread = std::thread::spawn(move || {
            while let Ok(frame) = read_frame(&mut reader, MAX_FRAME_SIZE) {
                match decode_message::<ClientMessage>(&frame).unwrap() {
                    ClientMessage::ClientShellPaneInput { pane_id, events } => {
                        reader_log.lock().unwrap().inputs.push((pane_id, events));
                        let _ = input_seen_tx.try_send(());
                    }
                    ClientMessage::ClientShellResize { .. } => {
                        reader_log.lock().unwrap().resizes += 1;
                    }
                    ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                        reader_log
                            .lock()
                            .unwrap()
                            .endpoint_requests
                            .push((boot_id, request));
                    }
                    _ => {}
                }
            }
        });
        // Wait for the accepted input, then burst ~12 MiB of full frames while the consumer
        // is paused; the burst then ends (no later frame will "carry" an overflow notice).
        input_seen_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("input before the burst");
        let mut sent = 0usize;
        let mut revision = 2;
        while sent < 12 * 1024 * 1024 {
            let frame = big_full("boot-local-7", revision, frame_bytes);
            let encoded =
                herdr_client::protocol::encode_message(&ServerMessage::PaneSurface(frame.clone()))
                    .unwrap();
            sent += encoded.len();
            revision += 1;
            write_message(&mut stream, &ServerMessage::PaneSurface(frame)).unwrap();
        }
        burst_done_tx.send(sent).unwrap();
        // After the full-surface request: one fresh full frame reconciles.
        let deadline = Instant::now() + Duration::from_secs(10);
        while server_log.lock().unwrap().resizes < 2 {
            assert!(Instant::now() < deadline, "no full-surface request");
            std::thread::sleep(Duration::from_millis(10));
        }
        write_message(
            &mut stream,
            &ServerMessage::PaneSurface(full("boot-local-7", 100, "depois")),
        )
        .unwrap();
        (stream, reader_thread)
    });

    let hub = HostHub::new();
    hub.add_host(HostSpec {
        endpoint: LOCAL.into(),
        label: "Este computador".into(),
        kind: HostKind::Local,
        session: "hd007-overflow".into(),
        target: None,
        visible: true,
    })
    .unwrap();
    let mut gateway = LocalGateway::new(dir.path(), session.clone());
    gateway
        .connect(ConnectOptions {
            geometry: geometry(),
            surface_active: true,
        })
        .unwrap();
    let now = Instant::now();
    let ticket = hub.request_connect(LOCAL, now).unwrap().unwrap();
    let events = hub
        .finish_connect(
            &ticket,
            Ok(Connected {
                api: Arc::new(gateway.api().clone()),
                gateway: Box::new(gateway),
            }),
            now,
        )
        .expect("event stream");
    hub.resize(LOCAL, geometry()).unwrap();

    // Pump until live, send input once, then pause the consumer during the burst.
    let pump_until = |pred: &dyn Fn() -> bool, what: &str| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !pred() {
            assert!(Instant::now() < deadline, "timeout: {what}");
            if let Ok(event) = events.recv_timeout(Duration::from_millis(50)) {
                hub.apply_event(LOCAL, ticket.token, event, Instant::now());
            }
        }
    };
    pump_until(&|| hub.input_gate(LOCAL, PANE).is_ok(), "live surface");
    let target = hub.input_gate(LOCAL, PANE).unwrap();
    hub.send_input(&target, text("antes")).unwrap();
    let sent = burst_done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("burst sent");
    assert!(sent > 8 * 1024 * 1024, "premise: burst larger than 8 MiB");
    std::thread::sleep(Duration::from_millis(300));

    let mut overflow_seen = None;
    let mut fulls_after = 0usize;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "no reconciliation");
        let Ok(event) = events.recv_timeout(Duration::from_millis(50)) else {
            if overflow_seen.is_some() && hub.input_gate(LOCAL, PANE).is_ok() {
                break;
            }
            continue;
        };
        match &event {
            GatewayEvent::QueueOverflow { dropped_frames } => {
                overflow_seen = Some(*dropped_frames);
            }
            GatewayEvent::Surface(frame) if overflow_seen.is_some() => {
                fulls_after += usize::from(frame.surface_revision == 100);
            }
            _ => {}
        }
        let was_overflow = matches!(event, GatewayEvent::QueueOverflow { .. });
        hub.apply_event(LOCAL, ticket.token, event, Instant::now());
        if was_overflow {
            let blocked = hub.send_input(&target, text("durante")).unwrap_err();
            assert_eq!(blocked.code, "input_blocked");
        }
    }
    let dropped = overflow_seen.expect("overflow notice delivered after the burst ended");
    assert!(dropped > 0);
    assert_eq!(fulls_after, 1, "reconciled by the requested full surface");
    let target = hub.input_gate(LOCAL, PANE).unwrap();
    hub.send_input(&target, text("depois")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while log.lock().unwrap().inputs.len() < 2 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let engine = log.lock().unwrap();
    let texts: Vec<Vec<ClientPaneInputEvent>> =
        engine.inputs.iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(texts, vec![text("antes"), text("depois")]);
    assert_eq!(
        engine.resizes, 2,
        "initial geometry plus exactly one full-surface request"
    );
    drop(engine);
    hub.cancel(LOCAL).unwrap();
    let (stream, reader) = server.join().unwrap();
    drop(stream);
    let _ = reader.join();
}

// --- scripted fake engine: the test drives every server write and reads every client message --

/// One accepted local connection of a fake engine. Every client message is forwarded to
/// `inbox`; writes are explicit, so late, foreign or oversized replies are scripted per test.
#[cfg(unix)]
struct Engine {
    writer: Arc<Mutex<std::os::unix::net::UnixStream>>,
    inbox: Mutex<Receiver<ClientMessage>>,
    _dir: tempfile::TempDir,
}

#[cfg(unix)]
impl Engine {
    /// Binds a private session socket, connects a real `LocalGateway`, answers the hello and
    /// publishes `boot` plus one full surface at revision 1 (`antes`).
    fn start(session: &str, boot: &str, methods: &[&'static str]) -> (Engine, LocalGateway) {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let session = SessionName::parse(session).unwrap();
        let paths = SessionPaths::for_session(dir.path(), &session);
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let listener = UnixListener::bind(&paths.client_socket).unwrap();
        let methods: Vec<&'static str> = methods.to_vec();
        let accept = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            accept_hello(&mut stream, &methods);
            stream
        });
        let mut gateway = LocalGateway::new(dir.path(), session);
        gateway
            .connect(ConnectOptions {
                geometry: geometry(),
                surface_active: true,
            })
            .unwrap();
        let stream = accept.join().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut reader = stream.try_clone().unwrap();
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
        let engine = Engine {
            writer: Arc::new(Mutex::new(stream)),
            inbox: Mutex::new(rx),
            _dir: dir,
        };
        engine.send(&ServerMessage::EndpointControl {
            kind: herdr_client::protocol::endpoint::ENDPOINT_SNAPSHOT_KIND.into(),
            data: serde_json::to_string(&snapshot(boot)).unwrap(),
        });
        engine.send(&ServerMessage::PaneSurface(full(boot, 1, "antes")));
        (engine, gateway)
    }

    fn send(&self, message: &ServerMessage) {
        write_message(&mut *self.writer.lock().unwrap(), message).unwrap();
    }

    /// Next endpoint request as (boot, id, method).
    fn endpoint_request(&self) -> (String, String, String) {
        let (boot, request) = self.expect("endpoint request", |m| match m {
            ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                Some((boot_id, request))
            }
            _ => None,
        });
        let (id, method) = request_id_method(&request);
        (boot, id, method)
    }

    fn chunk(&self, boot: &str, id: &str, final_chunk: bool, data: &[u8]) {
        self.send(&ServerMessage::ClientShellEndpointResponseChunk {
            boot_id: boot.into(),
            request_id: id.into(),
            final_chunk,
            data: data.to_vec(),
        });
    }

    /// Final single-chunk success reply.
    fn reply(&self, boot: &str, id: &str, result: Value) {
        let body = json!({ "id": id, "result": result }).to_string();
        self.chunk(boot, id, true, body.as_bytes());
    }

    /// The engine drops the connection (both directions).
    fn hang_up(&self) {
        let _ = self
            .writer
            .lock()
            .unwrap()
            .shutdown(std::net::Shutdown::Both);
    }

    /// True once the client closed its side (the engine's reader saw EOF) within `window`.
    fn client_closed_within(&self, window: Duration) -> bool {
        let inbox = self.inbox.lock().unwrap();
        let deadline = Instant::now() + window;
        loop {
            match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return true,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return false,
            }
        }
    }

    /// Next client message matching `pick`, skipping the others.
    fn expect<T>(&self, what: &str, pick: impl Fn(ClientMessage) -> Option<T>) -> T {
        let inbox = self.inbox.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = inbox
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("engine never received {what}"));
            if let Some(found) = pick(message) {
                return found;
            }
        }
    }

    /// Client messages received within `window`, for "nothing else arrived" assertions.
    fn quiet(&self, window: Duration) -> Vec<ClientMessage> {
        let inbox = self.inbox.lock().unwrap();
        let mut seen = Vec::new();
        let deadline = Instant::now() + window;
        while let Ok(message) =
            inbox.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            seen.push(message);
        }
        seen
    }

    /// Writes `count` small full surfaces (revisions `first..`) on a separate thread; the
    /// returned channel yields the encoded byte total once every frame reached the socket.
    fn burst(&self, boot: &'static str, first: u64, count: u64) -> Receiver<usize> {
        let writer = self.writer.clone();
        let (done_tx, done_rx) = sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = 0usize;
            for revision in first..first + count {
                let message = ServerMessage::PaneSurface(full(boot, revision, "rajada"));
                bytes += herdr_client::protocol::encode_message(&message)
                    .unwrap()
                    .len();
                if write_message(&mut *writer.lock().unwrap(), &message).is_err() {
                    return;
                }
            }
            let _ = done_tx.send(bytes);
        });
        done_rx
    }
}

/// Parses the JSON body of an endpoint request into (id, method).
fn request_id_method(request: &str) -> (String, String) {
    let value: Value = serde_json::from_str(request).unwrap();
    (
        value["id"].as_str().unwrap().to_owned(),
        value["method"].as_str().unwrap().to_owned(),
    )
}

type Recv = Box<dyn Fn(Duration) -> Result<GatewayEvent, std::sync::mpsc::RecvTimeoutError>>;

/// More small frames than the 512 event slots, far below the 8 MiB byte budget, while the
/// consumer is paused (alive, not receiving). Would catch: the reader blocking on a full slot
/// queue (endpoint replies stop being read, `detach` hangs joining it), frames lost without a
/// notice, a notice that never reaches the resumed consumer, frames delivered after the notice
/// out of order, or more than one full-surface request for the episode.
#[cfg(unix)]
fn saturated_slots_keep_reader_live(adapter: bool) {
    const BURST: u64 = 1000;
    const SLOTS: usize = 512;
    let name = if adapter {
        "hd007-sat-adapter"
    } else {
        "hd007-sat-typed"
    };
    let (engine, mut gateway) = Engine::start(name, "boot-local-7", &["pane.focus"]);
    let events: Recv = if adapter {
        let rx = gateway.take_events().unwrap();
        Box::new(move |t| rx.recv_timeout(t))
    } else {
        let stream = gateway.take_event_stream().unwrap();
        Box::new(move |t| stream.recv_timeout(t))
    };
    let mut store = FrameStore::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while store.revision() != Some(1) {
        assert!(Instant::now() < deadline, "initial surface");
        if let Ok(GatewayEvent::Surface(frame)) = events(Duration::from_millis(50)) {
            store.apply_full(*frame).unwrap();
        }
    }

    // Consumer paused: the burst must still be fully read off the socket.
    let bytes = engine
        .burst("boot-local-7", 2, BURST)
        .recv_timeout(Duration::from_secs(10))
        .expect("reader kept reading the socket with the slot queue full");
    assert!(bytes < 8 * 1024 * 1024, "premise: below the byte budget");
    assert!(BURST as usize > SLOTS, "premise: more frames than slots");
    std::thread::sleep(Duration::from_millis(300));

    // Still paused: an endpoint reply is read and correlated.
    let started = Instant::now();
    let reply = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (boot, request) = engine.expect("endpoint request", |m| match m {
                ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                    Some((boot_id, request))
                }
                _ => None,
            });
            let (id, method) = request_id_method(&request);
            assert_eq!(
                (boot.as_str(), method.as_str()),
                ("boot-local-7", "pane.focus")
            );
            engine.send(&ServerMessage::ClientShellEndpointResponseChunk {
                boot_id: boot,
                request_id: id.clone(),
                final_chunk: true,
                data: json!({"id": id, "result": {"focado": "w1:p1"}})
                    .to_string()
                    .into_bytes(),
            });
        });
        gateway.endpoint_request("pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(reply.unwrap(), json!({"focado": "w1:p1"}));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "reply read while paused"
    );

    // Resume: accepted frames in order, then exactly one notice covering every dropped frame.
    let mut accepted = Vec::new();
    let mut notices = Vec::new();
    let mut after_notice = 0usize;
    loop {
        match events(Duration::from_millis(500)) {
            Ok(GatewayEvent::Surface(frame)) => {
                if notices.is_empty() {
                    accepted.push(frame.surface_revision);
                    store.apply_full(*frame).unwrap();
                } else {
                    after_notice += 1;
                }
            }
            Ok(GatewayEvent::QueueOverflow { dropped_frames }) => {
                notices.push(dropped_frames);
                store.mark_stale(StaleReason::QueueOverflow);
                if store.take_recovery_request() {
                    gateway.resize(geometry()).unwrap();
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert_eq!(notices.len(), 1, "one notice for the episode: {notices:?}");
    assert!(!accepted.is_empty() && accepted.len() <= SLOTS);
    assert_eq!(
        accepted,
        (2..2 + accepted.len() as u64).collect::<Vec<_>>(),
        "accepted frames arrive in order without gaps"
    );
    assert_eq!(
        accepted.len() + notices[0],
        BURST as usize,
        "every frame is either delivered or counted as dropped"
    );
    assert_eq!(after_notice, 0, "no stale burst frame after the notice");
    assert!(!store.input_allowed(), "input blocked until reconciled");

    // One full-surface request reconciles.
    engine.expect("full-surface request", |m| {
        matches!(m, ClientMessage::ClientShellResize { .. }).then_some(())
    });
    engine.send(&ServerMessage::PaneSurface(full(
        "boot-local-7",
        2000,
        "depois",
    )));
    let deadline = Instant::now() + Duration::from_secs(5);
    while store.revision() != Some(2000) {
        assert!(Instant::now() < deadline, "reconciling full surface");
        if let Ok(GatewayEvent::Surface(frame)) = events(Duration::from_millis(50)) {
            store.apply_full(*frame).unwrap();
        }
    }
    assert!(store.input_allowed());
    assert_eq!(row_text(&store, 0), "depois");
    let extra = engine.quiet(Duration::from_millis(300));
    assert!(
        !extra
            .iter()
            .any(|m| matches!(m, ClientMessage::ClientShellResize { .. })),
        "exactly one full-surface request"
    );

    // Saturate again and detach with the consumer alive and paused (receiver not dropped).
    engine
        .burst("boot-local-7", 3000, BURST)
        .recv_timeout(Duration::from_secs(10))
        .expect("second burst read");
    std::thread::sleep(Duration::from_millis(300));
    let (detached_tx, detached_rx) = sync_channel(1);
    std::thread::spawn(move || {
        gateway.detach();
        let _ = detached_tx.send(gateway);
    });
    let gateway = detached_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("detach returns while the consumer is paused");
    assert!(!gateway.is_connected());
    engine.expect("detach", |m| {
        matches!(m, ClientMessage::Detach).then_some(())
    });
    // The paused consumer can still drain what was queued, then sees the stream end.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "stream ends after detach");
        match events(Duration::from_millis(100)) {
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(unix)]
#[test]
fn saturated_512_slot_typed_stream_keeps_reader_live_resyncs_once_and_detaches_while_paused() {
    saturated_slots_keep_reader_live(false);
}

#[cfg(unix)]
#[test]
fn saturated_512_slot_adapter_stream_keeps_reader_live_resyncs_once_and_detaches_while_paused() {
    saturated_slots_keep_reader_live(true);
}

// =======================================================================================
// Observer: frames reach the composed surface outside the host lock
// =======================================================================================

#[derive(Default)]
struct Recorder {
    notices: Mutex<Vec<(String, String)>>,
}

impl HubObserver for Recorder {
    fn notice(&self, origin: &NoticeOrigin, notice: &HostNotice) {
        let endpoint = origin.endpoint.as_str();
        let label = match notice {
            HostNotice::Connected { .. } => "connected".to_owned(),
            HostNotice::Snapshot => "snapshot".to_owned(),
            HostNotice::Full(frame) => format!("full:{}", frame.surface_revision),
            HostNotice::Patch(patch) => format!("patch:{}", patch.surface_revision),
            HostNotice::Stale(reason) => format!("stale:{reason:?}"),
            HostNotice::Lost(error) => format!("lost:{}", error.code),
            HostNotice::Failed(error) => format!("failed:{}", error.code),
            HostNotice::Invalidated(error) => format!("invalidated:{}", error.code),
        };
        self.notices
            .lock()
            .unwrap()
            .push((endpoint.to_owned(), label));
    }
}

/// Would catch: frames of a hidden host pushed to the renderer, rejected patches forwarded,
/// or observers invoked with the host lock held (a re-entrant hub call would deadlock).
#[test]
fn observer_receives_only_applied_frames_of_visible_hosts_without_the_host_lock() {
    struct Reentrant {
        hub: Mutex<Option<Arc<HostHub>>>,
        inner: Recorder,
    }
    impl HubObserver for Reentrant {
        fn notice(&self, origin: &NoticeOrigin, notice: &HostNotice) {
            let endpoint = origin.endpoint.as_str();
            if let Some(hub) = self.hub.lock().unwrap().as_ref() {
                // Re-entering the hub for the same host must not deadlock.
                let _ = hub.snapshot(Instant::now());
                let _ = hub.surface(endpoint);
            }
            self.inner.notice(origin, notice);
        }
    }
    let hub = Arc::new(HostHub::new());
    let observer = Arc::new(Reentrant {
        hub: Mutex::new(None),
        inner: Recorder::default(),
    });
    hub.set_observer(observer.clone());
    *observer.hub.lock().unwrap() = Some(hub.clone());
    hub.add_host(spec(LOCAL, true)).unwrap();
    hub.add_host(spec(SSH_ID, false)).unwrap();
    let local = connect_host(&hub, LOCAL, "boot-local-7", 5, "local$");
    let ssh = connect_host(&hub, SSH_ID, "boot-ssh-3", 5, "remote$");
    let now = Instant::now();
    hub.apply_event(
        LOCAL,
        local.token,
        GatewayEvent::Patch(Box::new(patch("boot-local-7", 5, 6, "ok"))),
        now,
    );
    hub.apply_event(
        LOCAL,
        local.token,
        GatewayEvent::Patch(Box::new(patch("boot-local-7", 1, 2, "ruim"))),
        now,
    );
    hub.apply_event(
        SSH_ID,
        ssh.token,
        GatewayEvent::Patch(Box::new(patch("boot-ssh-3", 5, 6, "oculto"))),
        now,
    );
    let notices = observer.inner.notices.lock().unwrap().clone();
    let local_labels: Vec<&str> = notices
        .iter()
        .filter(|(e, _)| e == LOCAL)
        .map(|(_, l)| l.as_str())
        .collect();
    assert_eq!(
        local_labels,
        [
            "connected",
            "snapshot",
            "full:5",
            "patch:6",
            "stale:RevisionGap"
        ]
    );
    assert!(
        !notices
            .iter()
            .any(|(e, l)| e == SSH_ID && (l.starts_with("full") || l.starts_with("patch"))),
        "hidden host frames never reach the renderer: {notices:?}"
    );
    let (frame, state) = hub.surface(LOCAL).unwrap();
    assert_eq!(frame.surface_revision, 6);
    assert_eq!(state, SurfaceState::Stale(StaleReason::RevisionGap));
    assert!(
        hub.surface(SSH_ID).is_none(),
        "hidden host keeps no surface"
    );
    let _ = (ssh.boot, ssh.lane, local.lane);
}

// =======================================================================================
// AC-007-04 — endpoint commands correlated per request and boot, routed by qualified target
// =======================================================================================

#[cfg(unix)]
fn live_lane(gateway: &LocalGateway) -> herdr_client::local::LocalEndpointLane {
    let deadline = Instant::now() + Duration::from_secs(5);
    while gateway.identity().is_none() {
        assert!(Instant::now() < deadline, "boot learned from the snapshot");
        std::thread::sleep(Duration::from_millis(10));
    }
    gateway
        .endpoint_lane()
        .expect("a connected gateway has an endpoint lane")
}

#[cfg(unix)]
fn sent_endpoint_requests(messages: &[ClientMessage]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, ClientMessage::ClientShellEndpointRequest { .. }))
        .count()
}

/// Would catch: one request id per connection (the late reply of a timed-out request would
/// complete the next request with the wrong result), a foreign id accepted, or a timed-out
/// request leaving the lane wedged as in flight.
#[cfg(unix)]
#[test]
fn local_endpoint_ids_are_unique_and_late_or_foreign_replies_never_complete_another_request() {
    let (engine, gateway) = Engine::start(
        "hd007-ep-ids",
        "boot-local-7",
        &["pane.focus", "pane.split"],
    );
    let lane = live_lane(&gateway);
    let (timed_out, (boot_a, id_a, method_a)) = std::thread::scope(|scope| {
        let engine_side = scope.spawn(|| engine.endpoint_request());
        let err = lane
            .clone()
            .with_timeout(Duration::from_millis(400))
            .request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
            .unwrap_err();
        (err, engine_side.join().unwrap())
    });
    assert_eq!(timed_out.code, "timeout");
    assert_eq!(
        (boot_a.as_str(), method_a.as_str()),
        ("boot-local-7", "pane.focus")
    );

    let (reply_b, id_b) = std::thread::scope(|scope| {
        let engine_side = scope.spawn(|| {
            let (boot, id_b, method) = engine.endpoint_request();
            assert_eq!(
                (boot.as_str(), method.as_str()),
                ("boot-local-7", "pane.split")
            );
            engine.reply("boot-local-7", &id_a, json!({"de": "A-tardia"}));
            engine.reply(
                "boot-local-7",
                "desktop-endpoint:alheio",
                json!({"de": "alheia"}),
            );
            engine.reply("boot-local-7", &id_b, json!({"de": "B"}));
            id_b
        });
        let reply = lane.clone().with_timeout(Duration::from_secs(5)).request(
            "boot-local-7",
            "pane.split",
            json!({"pane_id": PANE}),
        );
        (reply, engine_side.join().unwrap())
    });
    assert_eq!(reply_b.unwrap(), json!({"de": "B"}));
    assert_ne!(id_a, id_b, "every request has its own id");

    let (reply_c, id_c) = std::thread::scope(|scope| {
        let engine_side = scope.spawn(|| {
            let (_, id_c, _) = engine.endpoint_request();
            engine.reply("boot-local-7", &id_c, json!({"de": "C"}));
            id_c
        });
        let reply = lane.clone().with_timeout(Duration::from_secs(5)).request(
            "boot-local-7",
            "pane.focus",
            json!({"pane_id": PANE}),
        );
        (reply, engine_side.join().unwrap())
    });
    assert_eq!(reply_c.unwrap(), json!({"de": "C"}));
    assert!(
        id_c != id_a && id_c != id_b,
        "same method, new id: {id_a} {id_b} {id_c}"
    );
}

/// Would catch: a reply of another boot accepted as the result, a request for a boot the
/// connection no longer has (or an unannounced method) sent anyway, or a reboot announced
/// mid-request waiting for the timeout instead of failing at once.
#[cfg(unix)]
#[test]
fn local_endpoint_never_mixes_boots_and_a_reboot_fails_the_pending_request_at_once() {
    let (engine, gateway) = Engine::start("hd007-ep-boot", "boot-local-7", &["pane.focus"]);
    let lane = live_lane(&gateway).with_timeout(Duration::from_secs(8));

    let stale = lane
        .request("boot-local-6", "pane.focus", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(stale.code, "target_boot_stale");
    let unsupported = lane
        .request("boot-local-7", "agent.start", json!({}))
        .unwrap_err();
    assert_eq!(unsupported.code, "unsupported_method");
    assert_eq!(
        sent_endpoint_requests(&engine.quiet(Duration::from_millis(300))),
        0,
        "refused requests never reach the engine"
    );

    let other_boot = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (boot, id, _) = engine.endpoint_request();
            assert_eq!(boot, "boot-local-7");
            engine.reply("boot-local-8", &id, json!({"de": "outro-boot"}));
        });
        lane.request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(other_boot.unwrap_err().code, "endpoint_boot_changed");

    let started = Instant::now();
    let rebooted = std::thread::scope(|scope| {
        scope.spawn(|| {
            engine.endpoint_request();
            engine.send(&ServerMessage::EndpointControl {
                kind: herdr_client::protocol::endpoint::ENDPOINT_SNAPSHOT_KIND.into(),
                data: serde_json::to_string(&snapshot("boot-local-9")).unwrap(),
            });
        });
        lane.request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(rebooted.unwrap_err().code, "endpoint_boot_changed");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "reboot fails the pending request without waiting for the timeout"
    );

    let old = lane
        .request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(old.code, "target_boot_stale");
    let served = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (boot, id, _) = engine.endpoint_request();
            assert_eq!(boot, "boot-local-9");
            engine.reply("boot-local-9", &id, json!({"de": "novo-boot"}));
        });
        lane.request("boot-local-9", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(served.unwrap(), json!({"de": "novo-boot"}));
}

/// Would catch: chunks not concatenated in order, unbounded aggregation of a reply, or a
/// too-large reply leaving the request in flight (its remaining chunks would then block or
/// complete the next request).
#[cfg(unix)]
#[test]
fn local_endpoint_reply_is_aggregated_in_order_and_bounded() {
    let (engine, gateway) = Engine::start("hd007-ep-limit", "boot-local-7", &["pane.focus"]);
    let lane = live_lane(&gateway)
        .with_timeout(Duration::from_secs(5))
        .with_response_limit(256);

    let chunked = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (_, id, _) = engine.endpoint_request();
            let body = json!({"id": id, "result": {"texto": "abc-def-ghi"}}).to_string();
            let bytes = body.as_bytes();
            engine.chunk("boot-local-7", &id, false, &bytes[..10]);
            engine.chunk("boot-local-7", &id, false, &bytes[10..25]);
            engine.chunk("boot-local-7", &id, true, &bytes[25..]);
        });
        lane.request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(chunked.unwrap(), json!({"texto": "abc-def-ghi"}));

    let (done_tx, done_rx) = sync_channel::<()>(1);
    let started = Instant::now();
    let too_large = std::thread::scope(|scope| {
        let engine = &engine;
        scope.spawn(move || {
            let (_, id, _) = engine.endpoint_request();
            let body = json!({"id": id, "result": {"texto": "x".repeat(400)}}).to_string();
            let bytes = body.as_bytes();
            engine.chunk("boot-local-7", &id, false, &bytes[..200]);
            engine.chunk("boot-local-7", &id, false, &bytes[200..300]);
            // The rest arrives only after the requester already gave up.
            done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            engine.chunk("boot-local-7", &id, true, &bytes[300..]);
        });
        let result = lane.request("boot-local-7", "pane.focus", json!({"pane_id": PANE}));
        done_tx.send(()).unwrap();
        result
    });
    assert_eq!(too_large.unwrap_err().code, "response_too_large");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "failed when the limit was crossed"
    );

    let next = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (_, id, _) = engine.endpoint_request();
            engine.reply("boot-local-7", &id, json!({"de": "depois"}));
        });
        lane.request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(next.unwrap(), json!({"de": "depois"}));
}

/// Would catch: a pending request surviving the loss of its connection until the timeout, a
/// request after the loss written anyway, or a lane kept by the hub holding a detached
/// connection open.
#[cfg(unix)]
#[test]
fn local_endpoint_pending_request_fails_on_disconnect_and_a_kept_lane_does_not_hold_the_socket() {
    let (engine, gateway) = Engine::start("hd007-ep-lost", "boot-local-7", &["pane.focus"]);
    let lane = live_lane(&gateway).with_timeout(Duration::from_secs(10));
    let started = Instant::now();
    let lost = std::thread::scope(|scope| {
        scope.spawn(|| {
            engine.endpoint_request();
            engine.hang_up();
        });
        lane.request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(lost.unwrap_err().code, "connection_lost");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "failed at the loss, not the timeout"
    );
    let after = lane
        .request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(
        after.code, "not_connected",
        "nothing is written after the loss"
    );

    let (engine2, mut gateway2) = Engine::start("hd007-ep-detach", "boot-local-7", &["pane.focus"]);
    let kept = live_lane(&gateway2);
    gateway2.detach();
    assert!(
        engine2.client_closed_within(Duration::from_secs(3)),
        "detached connection closed although a lane is still held"
    );
    let err = kept
        .request("boot-local-7", "pane.focus", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(err.code, "not_connected");
}

// --- SSH visual lane over a socket pair standing in for the ssh process ---------------------

#[cfg(unix)]
struct PipeChild {
    socket: std::os::unix::net::UnixStream,
    stdin: Option<std::os::unix::net::UnixStream>,
    stdout: Option<std::os::unix::net::UnixStream>,
    killed: bool,
}

#[cfg(unix)]
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

/// Probe answers a compatible running server; the bridge is the client end of a socket pair.
#[cfg(unix)]
struct PipeRunner(Mutex<Option<std::os::unix::net::UnixStream>>);

#[cfg(unix)]
impl SshRunner for PipeRunner {
    fn output(
        &self,
        _command: &OpenSshCommand,
        _timeout: Duration,
        _stdin: Option<&[u8]>,
    ) -> std::io::Result<ProcessOutput> {
        Ok(ProcessOutput {
            status: Some(0),
            stdout: br#"{"running":true,"capabilities":{"endpoint_protocol_generation":1,"surface_interest":true,"health_check":true}}"#.to_vec(),
            stderr: String::new(),
        })
    }
    fn spawn(&self, _command: &OpenSshCommand) -> std::io::Result<Box<dyn SshChild>> {
        let socket = self.0.lock().unwrap().take().expect("one bridge per test");
        Ok(Box::new(PipeChild {
            stdin: Some(socket.try_clone()?),
            stdout: Some(socket.try_clone()?),
            socket,
            killed: false,
        }))
    }
}

#[cfg(unix)]
impl Engine {
    /// Remote engine reached through `SshConnector` (probe + bridge), publishing `boot`.
    fn ssh(boot: &str, methods: &[&'static str]) -> (Engine, SshGateway) {
        let (client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        let methods: Vec<&'static str> = methods.to_vec();
        let accept = std::thread::spawn(move || {
            accept_hello(&mut server, &methods);
            server
        });
        let identity = SshIdentity::new(
            ProfileId::parse(SSH_ID).unwrap(),
            "tester@127.0.0.1",
            None,
            "hd007-remote",
        )
        .unwrap();
        let connector = SshConnector::new(
            identity,
            None,
            Arc::new(PipeRunner(Mutex::new(Some(client)))),
        );
        let gateway = connector
            .connect(ConnectOptions {
                geometry: geometry(),
                surface_active: true,
            })
            .unwrap_or_else(|failure| panic!("ssh connect: {:?}", failure.error()));
        let stream = accept.join().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut reader = stream.try_clone().unwrap();
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
        let engine = Engine {
            writer: Arc::new(Mutex::new(stream)),
            inbox: Mutex::new(rx),
            _dir: tempfile::tempdir().unwrap(),
        };
        engine.send(&ServerMessage::EndpointControl {
            kind: herdr_client::protocol::endpoint::ENDPOINT_SNAPSHOT_KIND.into(),
            data: serde_json::to_string(&snapshot(boot)).unwrap(),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while gateway.identity().is_none() {
            assert!(
                Instant::now() < deadline,
                "ssh boot learned from the snapshot"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        (engine, gateway)
    }
}

/// Would catch: the SSH bridge still refusing announced endpoint commands, one id reused per
/// connection (a late reply completing the next request), replies of another boot or beyond
/// the size limit accepted, requests for a stale boot or unannounced method sent, or a pending
/// request surviving the loss of the ssh process until its timeout.
#[cfg(unix)]
#[test]
fn ssh_endpoint_commands_are_correlated_by_id_and_boot_bounded_and_fail_at_once_on_loss() {
    let (engine, gateway) = Engine::ssh("boot-ssh-3", &["pane.focus", "pane.split"]);

    // Through the gateway contract, with the connection's current boot.
    let via_gateway = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (boot, id, method) = engine.endpoint_request();
            assert_eq!(
                (boot.as_str(), method.as_str()),
                ("boot-ssh-3", "pane.split")
            );
            engine.reply("boot-ssh-3", &id, json!({"pane_id": "w1:p2"}));
        });
        gateway.endpoint_request("pane.split", json!({"pane_id": PANE, "direction": "right"}))
    });
    assert_eq!(via_gateway.unwrap(), json!({"pane_id": "w1:p2"}));

    let lane = gateway.endpoint_lane().expect("connected ssh lane");
    assert_eq!(lane.methods(), ["pane.focus", "pane.split"]);
    let (timed_out, (_, id_a, _)) = std::thread::scope(|scope| {
        let engine_side = scope.spawn(|| engine.endpoint_request());
        let err = lane
            .clone()
            .with_timeout(Duration::from_millis(400))
            .request("boot-ssh-3", "pane.focus", json!({"pane_id": PANE}))
            .unwrap_err();
        (err, engine_side.join().unwrap())
    });
    assert_eq!(timed_out.code, "timeout");
    assert_eq!(timed_out.endpoint.as_deref(), Some(SSH_ID));
    let (reply_b, id_b) = std::thread::scope(|scope| {
        let engine_side = scope.spawn(|| {
            let (_, id_b, _) = engine.endpoint_request();
            engine.reply("boot-ssh-3", &id_a, json!({"de": "A-tardia"}));
            engine.reply(
                "boot-ssh-3",
                "desktop-endpoint:alheio",
                json!({"de": "alheia"}),
            );
            engine.reply("boot-ssh-3", &id_b, json!({"de": "B"}));
            id_b
        });
        let reply = lane.request("boot-ssh-3", "pane.focus", json!({"pane_id": PANE}));
        (reply, engine_side.join().unwrap())
    });
    assert_eq!(reply_b.unwrap(), json!({"de": "B"}));
    assert_ne!(id_a, id_b);

    assert_eq!(
        lane.request("boot-ssh-2", "pane.focus", json!({}))
            .unwrap_err()
            .code,
        "target_boot_stale"
    );
    assert_eq!(
        lane.request("boot-ssh-3", "agent.start", json!({}))
            .unwrap_err()
            .code,
        "unsupported_method"
    );
    assert_eq!(
        sent_endpoint_requests(&engine.quiet(Duration::from_millis(300))),
        0,
        "refused requests never reach the remote engine"
    );

    let other_boot = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (_, id, _) = engine.endpoint_request();
            engine.reply("boot-local-7", &id, json!({"de": "boot-local"}));
        });
        lane.request("boot-ssh-3", "pane.focus", json!({"pane_id": PANE}))
    });
    assert_eq!(other_boot.unwrap_err().code, "endpoint_boot_changed");

    let too_large = std::thread::scope(|scope| {
        scope.spawn(|| {
            let (_, id, _) = engine.endpoint_request();
            engine.chunk("boot-ssh-3", &id, false, &[b'x'; 100]);
        });
        lane.clone().with_response_limit(64).request(
            "boot-ssh-3",
            "pane.focus",
            json!({"pane_id": PANE}),
        )
    });
    assert_eq!(too_large.unwrap_err().code, "response_too_large");

    let started = Instant::now();
    let lost = std::thread::scope(|scope| {
        scope.spawn(|| {
            engine.endpoint_request();
            engine.hang_up();
        });
        lane.clone().with_timeout(Duration::from_secs(10)).request(
            "boot-ssh-3",
            "pane.focus",
            json!({"pane_id": PANE}),
        )
    });
    assert_eq!(lost.unwrap_err().code, "connection_lost");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "failed at the loss, not the timeout"
    );
    assert_eq!(
        lane.request("boot-ssh-3", "pane.focus", json!({}))
            .unwrap_err()
            .code,
        "not_connected"
    );
}

/// Would catch: a lane kept by the hub still writing to (or waiting on) a detached SSH
/// connection.
#[cfg(unix)]
#[test]
fn ssh_endpoint_lane_kept_after_detach_refuses_without_sending() {
    let (engine, mut gateway) = Engine::ssh("boot-ssh-4", &["pane.focus"]);
    let kept = gateway.endpoint_lane().expect("connected ssh lane");
    gateway.detach();
    let started = Instant::now();
    let err = kept
        .request("boot-ssh-4", "pane.focus", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(err.code, "not_connected");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(gateway.endpoint_lane().is_none(), "no lane after detach");
    let seen = engine.quiet(Duration::from_millis(500));
    assert_eq!(sent_endpoint_requests(&seen), 0, "{seen:?}");
}

fn target_for(
    live: &QualifiedTarget,
    endpoint: &str,
    session: &str,
    boot: &str,
) -> QualifiedTarget {
    QualifiedTarget {
        endpoint: endpoint.into(),
        session: session.into(),
        boot_id: boot.into(),
        ..live.clone()
    }
}

/// Would catch: an endpoint action routed by pane id alone (Local has the same `w1:p1`), the
/// lane called with a boot other than the target's, a stale boot/session/generation, unknown
/// pane or unannounced method still sent, or refusals recorded as sent actions.
#[test]
fn hub_run_endpoint_routes_by_qualified_target_once_and_refuses_invalid_targets_before_sending() {
    let hub = HostHub::new();
    hub.add_host(spec(LOCAL, false)).unwrap();
    hub.add_host(spec(SSH_ID, true)).unwrap();
    let local = connect_host(&hub, LOCAL, "boot-local-7", 5, "local$");
    let ssh = connect_host(&hub, SSH_ID, "boot-ssh-3", 9, "remote$");
    let target = hub.input_gate(SSH_ID, PANE).expect("ssh target");
    assert_eq!(target.boot_id, "boot-ssh-3");

    let params = json!({"pane_id": PANE, "direction": "right"});
    let reply = hub
        .run_endpoint(&target, "pane.split", params.clone())
        .unwrap();
    assert_eq!(reply, json!({"type": "ok"}));
    {
        let wire = ssh.wire.lock().unwrap();
        assert_eq!(wire.endpoint, vec![("pane.split".to_owned(), params)]);
        assert_eq!(wire.endpoint_boots, vec!["boot-ssh-3".to_owned()]);
    }
    assert!(
        local.wire.lock().unwrap().endpoint.is_empty(),
        "nothing on Local"
    );

    let mut unknown_host = target.clone();
    unknown_host.endpoint = "0007bbbb0007bbbb0007bbbb0007bbbb".into();
    let mut stale_generation = target.clone();
    stale_generation.connection_generation += 1;
    let mut foreign_pane = target.clone();
    foreign_pane.pane_id = "w9:p9".into();
    let mut foreign_workspace = target.clone();
    foreign_workspace.workspace_id = Some("w9".into());
    let cases = [
        (unknown_host, "pane.focus", "endpoint_unknown"),
        (
            target_for(&target, SSH_ID, "hd007-remote", "boot-local-7"),
            "pane.focus",
            "target_boot_stale",
        ),
        (
            target_for(&target, SSH_ID, "hd007-local", "boot-ssh-3"),
            "pane.focus",
            "target_session_mismatch",
        ),
        (stale_generation, "pane.focus", "target_generation_stale"),
        (foreign_pane, "pane.focus", "pane_not_in_snapshot"),
        (foreign_workspace, "pane.focus", "pane_not_in_snapshot"),
        (target.clone(), "agent.start", "unsupported_method"),
    ];
    for (bad, method, code) in cases {
        let err = hub.run_endpoint(&bad, method, json!({})).unwrap_err();
        assert_eq!(err.code, code, "{bad:?} {method}");
    }
    assert_eq!(
        ssh.wire.lock().unwrap().endpoint.len(),
        1,
        "refusals send nothing"
    );

    let local_target = target_for(&target, LOCAL, "hd007-local", "boot-local-7");
    hub.run_endpoint(&local_target, "pane.focus", json!({"pane_id": PANE}))
        .unwrap();
    assert_eq!(
        local.wire.lock().unwrap().endpoint_boots,
        vec!["boot-local-7".to_owned()]
    );
    assert_eq!(
        ssh.wire.lock().unwrap().endpoint.len(),
        1,
        "Local action not on SSH"
    );

    // Reboot of the SSH server: the previous target is refused before sending.
    *ssh.boot.lock().unwrap() = "boot-ssh-4".into();
    hub.apply_event(
        SSH_ID,
        ssh.token,
        GatewayEvent::Snapshot(Box::new(snapshot("boot-ssh-4"))),
        Instant::now(),
    );
    let err = hub
        .run_endpoint(&target, "pane.focus", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(err.code, "target_boot_stale");
    assert_eq!(ssh.wire.lock().unwrap().endpoint.len(), 1);

    let snap = serde_json::to_value(hub.snapshot(Instant::now())).unwrap();
    let actions = |i: usize| snap["hosts"][i]["actions"].as_array().unwrap().clone();
    assert_eq!(actions(0).len(), 1);
    assert_eq!(actions(1).len(), 1, "only the sent action is in the ledger");
    assert_eq!(actions(1)[0]["method"], "pane.split");
    assert_eq!(actions(1)[0]["outcome"], "succeeded");
}

/// Would catch: the host lock held while waiting for the endpoint reply (input and snapshots
/// of the same host, the other host's action and cancel would stall), an unknown result
/// (timeout, boot change, oversize, lost transport) retried or reported as failed, or a
/// server error reported as unknown.
#[test]
fn hub_run_endpoint_waits_outside_the_host_lock_and_never_repeats_an_unknown_result() {
    let hub = Arc::new(HostHub::new());
    hub.add_host(spec(LOCAL, false)).unwrap();
    hub.add_host(spec(SSH_ID, true)).unwrap();
    let local = connect_host(&hub, LOCAL, "boot-local-7", 5, "local$");
    let ssh = connect_host(&hub, SSH_ID, "boot-ssh-3", 9, "remote$");
    let target = hub.input_gate(SSH_ID, PANE).expect("ssh target");
    let local_target = target_for(&target, LOCAL, "hd007-local", "boot-local-7");

    let wait_calls = |n: usize| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while ssh.wire.lock().unwrap().endpoint.len() < n {
            assert!(Instant::now() < deadline, "lane called");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let cases = [
        ("timeout", "result_unknown", "unknown"),
        ("endpoint_boot_changed", "result_unknown", "unknown"),
        ("response_too_large", "result_unknown", "unknown"),
        ("connection_lost", "result_unknown", "unknown"),
        ("pane_not_found", "pane_not_found", "failed"),
    ];
    for (round, (lane_code, returned, outcome)) in cases.into_iter().enumerate() {
        let (release, gate) = sync_channel(1);
        ssh.lane.gate.lock().unwrap().push_back(gate);
        let pending = {
            let (hub, target) = (hub.clone(), target.clone());
            std::thread::spawn(move || {
                hub.run_endpoint(&target, "pane.focus", json!({"pane_id": PANE}))
            })
        };
        wait_calls(round + 1);

        let started = Instant::now();
        hub.send_input(&target, text("durante")).unwrap();
        let snap = serde_json::to_value(hub.snapshot(Instant::now())).unwrap();
        assert_eq!(snap["hosts"][1]["actions"][0]["outcome"], "pending");
        hub.run_endpoint(
            &local_target,
            "workspace.focus",
            json!({"workspace_id": "w1"}),
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "same-host input, snapshot and other host proceed while the reply is awaited"
        );

        release
            .send(Err(RuntimeError::new(lane_code, "falha simulada")))
            .unwrap();
        let err = pending.join().unwrap().unwrap_err();
        assert_eq!(err.code, returned, "{lane_code}");
        assert_eq!(err.endpoint.as_deref(), Some(SSH_ID));
        let snap = serde_json::to_value(hub.snapshot(Instant::now())).unwrap();
        assert_eq!(
            snap["hosts"][1]["actions"][0]["outcome"], outcome,
            "{lane_code}"
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            ssh.wire.lock().unwrap().endpoint.len(),
            round + 1,
            "never re-sent after {lane_code}"
        );
    }

    // Disconnect while the reply is awaited: cancel does not wait for it, the action becomes
    // unknown and is not repeated; the offline host refuses new actions before sending.
    let calls = cases.len();
    let (release, gate) = sync_channel(1);
    ssh.lane.gate.lock().unwrap().push_back(gate);
    let pending = {
        let (hub, target) = (hub.clone(), target.clone());
        std::thread::spawn(move || {
            hub.run_endpoint(&target, "pane.split", json!({"pane_id": PANE}))
        })
    };
    wait_calls(calls + 1);
    let started = Instant::now();
    hub.cancel(SSH_ID).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel not blocked by the request"
    );
    release
        .send(Err(RuntimeError::new("connection_lost", "perdida")))
        .unwrap();
    assert_eq!(pending.join().unwrap().unwrap_err().code, "result_unknown");
    let snap = serde_json::to_value(hub.snapshot(Instant::now())).unwrap();
    assert_eq!(snap["hosts"][1]["actions"][0]["method"], "pane.split");
    assert_eq!(snap["hosts"][1]["actions"][0]["outcome"], "unknown");
    let offline = hub
        .run_endpoint(&target, "pane.split", json!({"pane_id": PANE}))
        .unwrap_err();
    assert_eq!(offline.code, "host_offline");
    let wire = ssh.wire.lock().unwrap();
    assert_eq!(
        wire.endpoint.len(),
        calls + 1,
        "nothing repeated or sent offline"
    );
    let durante = wire
        .inputs
        .iter()
        .filter(|(_, e)| *e == text("durante"))
        .count();
    assert_eq!(durante, calls, "each accepted input delivered once");
    assert_eq!(local.wire.lock().unwrap().endpoint.len(), calls);
}

// Keep imports used by later sections compiling in every configuration.
#[allow(dead_code)]
fn _unused(_: &mut dyn Read, _: &mut dyn Write) {}

// =======================================================================================
// Bloco 04 — composition: the connector of ConnectionsState installs the endpoint lane
// =======================================================================================

#[cfg(unix)]
fn engine_on(stream: std::os::unix::net::UnixStream) -> Engine {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut reader = stream.try_clone().unwrap();
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
        writer: Arc::new(Mutex::new(stream)),
        inbox: Mutex::new(rx),
        _dir: tempfile::tempdir().unwrap(),
    }
}

#[cfg(unix)]
fn publish(engine: &Engine, boot: &str, screen: &str) {
    engine.send(&ServerMessage::EndpointControl {
        kind: herdr_client::protocol::endpoint::ENDPOINT_SNAPSHOT_KIND.into(),
        data: serde_json::to_string(&snapshot(boot)).unwrap(),
    });
    engine.send(&ServerMessage::PaneSurface(full(boot, 1, screen)));
}

#[cfg(unix)]
fn gate_within(hub: &HostHub, endpoint: &str) -> QualifiedTarget {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match hub.input_gate(endpoint, PANE) {
            Ok(target) => return target,
            Err(block) => assert!(
                Instant::now() < deadline,
                "{endpoint} never became actionable: {block:?}"
            ),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Would catch: the connector of the composed window moving the gateway into `Connected`
/// without installing its endpoint lane (every endpoint action `endpoint_unavailable` in the app),
/// a lane of one host installed on the other (Local and SSH both expose `w1:p1`), or the lane of
/// a connection whose reply boot is not the one of the target.
#[cfg(unix)]
#[test]
fn connections_state_connector_installs_each_hosts_own_endpoint_lane_after_connecting() {
    use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
    use herdr_desktop::connections::profiles::SshProfileDraft;
    use std::os::unix::net::{UnixListener, UnixStream};

    let prefs = tempfile::tempdir().unwrap();
    let herdr_config = tempfile::tempdir().unwrap();
    let herdr_state = tempfile::tempdir().unwrap();
    let session = SessionName::parse("hd007-local").unwrap();
    let paths = SessionPaths::for_session(herdr_config.path(), &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let listener = UnixListener::bind(&paths.client_socket).unwrap();
    let local_accept = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        accept_hello(&mut stream, &["pane.split", "pane.focus"]);
        stream
    });
    let (client, mut server) = UnixStream::pair().unwrap();
    let ssh_accept = std::thread::spawn(move || {
        accept_hello(&mut server, &["pane.split", "pane.focus"]);
        server
    });

    let state = ConnectionsState::with_runner(
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
        Arc::new(PipeRunner(Mutex::new(Some(client)))),
    );
    state.connect(LOCAL).unwrap();
    let view = state
        .save_profile(
            SshProfileDraft {
                id: None,
                label: "remoto".into(),
                target: "tester@127.0.0.1".into(),
                port: None,
                session: "hd007-remote".into(),
                auth: None,
            },
            true,
        )
        .unwrap();
    let ssh_id = view.profiles[0].id.as_str().to_owned();

    let local = engine_on(local_accept.join().unwrap());
    let ssh = engine_on(ssh_accept.join().unwrap());
    publish(&local, "boot-local-7", "local$");
    publish(&ssh, "boot-ssh-3", "remote$");
    let hub = state.hub();
    let local_target = gate_within(hub, LOCAL);
    let ssh_target = gate_within(hub, &ssh_id);
    assert_eq!(local_target.pane_id, ssh_target.pane_id, "fixture premise");
    assert_eq!(ssh_target.boot_id, "boot-ssh-3");

    for (engine, other, target, boot, direction) in [
        (&ssh, &local, &ssh_target, "boot-ssh-3", "right"),
        (&local, &ssh, &local_target, "boot-local-7", "down"),
    ] {
        let params = json!({"pane_id": PANE, "direction": direction});
        let call = {
            let state = state.clone();
            let target = target.clone();
            std::thread::spawn(move || state.hub().run_endpoint(&target, "pane.split", params))
        };
        let (sent_boot, id, method) = engine.endpoint_request();
        assert_eq!((sent_boot.as_str(), method.as_str()), (boot, "pane.split"));
        engine.reply(boot, &id, json!({"type": "ok", "direction": direction}));
        let reply = call
            .join()
            .unwrap()
            .expect("endpoint lane installed by the connector");
        assert_eq!(reply["direction"], direction);
        assert_eq!(
            sent_endpoint_requests(&other.quiet(Duration::from_millis(150))),
            0,
            "the other host received nothing"
        );
    }
    state.detach_all();
}

// =======================================================================================
// Bloco 04b — agents hosted by the selected host of the composed window (fake host)
// =======================================================================================

mod hosted_agents {
    use super::*;
    use herdr_desktop::bridge::agent_commands::{AgentsHost, AgentsState, EventStream};

    #[derive(Clone, Copy, PartialEq)]
    enum Api {
        Ok,
        RemoteApiUnsupported,
        Unreachable,
    }

    struct Remote {
        boot: &'static str,
        agent: &'static str,
        kind: &'static str,
        api: Api,
        surface: Option<PaneSurfaceFrame>,
        stream: Option<std::os::unix::net::UnixStream>,
    }

    pub(super) struct Host {
        selected: Mutex<String>,
        hosts: Mutex<std::collections::BTreeMap<String, Remote>>,
        log: Arc<Mutex<Vec<(String, String)>>>,
    }

    struct Gateway {
        endpoint: String,
        boot: String,
        api: Api,
        agent: &'static str,
        kind: &'static str,
        log: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl RuntimeGateway for Gateway {
        fn endpoint(&self) -> &str {
            &self.endpoint
        }
        fn identity(&self) -> Option<LiveIdentity> {
            Some(LiveIdentity {
                endpoint: self.endpoint.clone(),
                session: format!("session-{}", self.endpoint),
                connection_generation: 4,
                boot_id: self.boot.clone(),
            })
        }
        fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
            Err(not_in_fake())
        }
        fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
            None
        }
        fn api_request(&self, method: &str, _params: Value) -> Result<Value, RuntimeError> {
            self.log
                .lock()
                .unwrap()
                .push((self.endpoint.clone(), format!("api:{method}")));
            match self.api {
                Api::RemoteApiUnsupported => {
                    return Err(RuntimeError::new("remote_api_unsupported", "sem bridge")
                        .with_endpoint(&self.endpoint))
                }
                Api::Unreachable => {
                    return Err(RuntimeError::new("ssh_unreachable", "host inacessível")
                        .retryable()
                        .with_endpoint(&self.endpoint))
                }
                Api::Ok => {}
            }
            let agent = json!({"pane_id": PANE, "workspace_id": "w1", "tab_id": "w1:t1",
                "name": self.agent, "agent": self.kind, "agent_status": "idle"});
            match method {
                "server.agent_manifests" => Ok(json!({"manifests": [{"agent": self.kind}]})),
                "agent.list" => Ok(json!({"agents": [agent]})),
                "tab.list" => Ok(json!({"tabs": [{"tab_id": "w1:t1", "workspace_id": "w1",
                    "label": self.agent, "focused": true}]})),
                _ => Err(RuntimeError::new(
                    "invalid_request",
                    "missing field `pane_id`",
                )),
            }
        }
        fn endpoint_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
            self.log
                .lock()
                .unwrap()
                .push((self.endpoint.clone(), format!("endpoint:{method}:{params}")));
            Ok(json!({"type": "ok"}))
        }
        fn send_input(
            &self,
            _target: &QualifiedTarget,
            _events: Vec<ClientPaneInputEvent>,
        ) -> Result<(), RuntimeError> {
            Err(not_in_fake())
        }
        fn resize(&self, _geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn detach(&mut self) {
            panic!("the hub owns hosted connections; agents never detach them");
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    impl AgentsHost for Host {
        fn endpoint(&self) -> String {
            self.selected.lock().unwrap().clone()
        }
        fn gateway(&self) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
            let endpoint = self.endpoint();
            let hosts = self.hosts.lock().unwrap();
            let remote = &hosts[&endpoint];
            Ok(Box::new(Gateway {
                endpoint: endpoint.clone(),
                boot: remote.boot.into(),
                api: remote.api,
                agent: remote.agent,
                kind: remote.kind,
                log: self.log.clone(),
            }))
        }
        fn endpoint_methods(&self) -> Result<Vec<String>, RuntimeError> {
            Ok(vec!["pane.split".into(), "pane.focus".into()])
        }
        fn surface(&self) -> Option<PaneSurfaceFrame> {
            self.hosts.lock().unwrap()[&self.endpoint()].surface.clone()
        }
        fn event_stream(
            &self,
            attached: &LiveIdentity,
        ) -> Result<Box<dyn EventStream>, RuntimeError> {
            let endpoint = attached.endpoint.clone();
            let stream = self
                .hosts
                .lock()
                .unwrap()
                .get_mut(&endpoint)
                .unwrap()
                .stream
                .take()
                .ok_or_else(|| RuntimeError::new("events_unavailable", "sem stream no fake"))?;
            Ok(Box::new(stream))
        }
    }

    fn remote(boot: &'static str, agent: &'static str, kind: &'static str) -> Remote {
        Remote {
            boot,
            agent,
            kind,
            api: Api::Ok,
            surface: Some(full(boot, 1, agent)),
            stream: None,
        }
    }

    pub(super) fn host() -> Arc<Host> {
        let mut hosts = std::collections::BTreeMap::new();
        hosts.insert(
            LOCAL.to_owned(),
            remote("boot-local-7", "agente-local", "claude"),
        );
        hosts.insert(
            SSH_ID.to_owned(),
            remote("boot-ssh-3", "agente-remoto", "pi"),
        );
        Arc::new(Host {
            selected: Mutex::new(SSH_ID.into()),
            hosts: Mutex::new(hosts),
            log: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn calls(host: &Host, endpoint: &str) -> Vec<String> {
        host.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| e == endpoint)
            .map(|(_, c)| c.clone())
            .collect()
    }

    fn target(endpoint: &str, boot: &str, pane: &str) -> QualifiedTarget {
        QualifiedTarget::new(
            &LiveIdentity {
                endpoint: endpoint.into(),
                session: format!("session-{endpoint}"),
                connection_generation: 4,
                boot_id: boot.into(),
            },
            None,
            pane,
        )
    }

    /// Would catch: agents still bound to a LocalGateway (discovery or actions on Local while SSH
    /// is selected), a Local target with the same `w1:p1` accepted on the SSH gateway, the core
    /// acting on a topology older than the host's committed surface, or a changed selection
    /// silently retargeting an action to the newly selected host instead of refusing it.
    #[test]
    fn hosted_agents_follow_the_selected_host_resync_its_surface_and_never_fall_back_to_local() {
        let host = host();
        let state = AgentsState::hosted(host.clone());
        let overview = state
            .attach_hosted(None)
            .expect("attach to the selected SSH host");
        assert_eq!(overview.identity.unwrap().boot_id, "boot-ssh-3");
        assert_eq!(overview.agents[0].name.as_deref(), Some("agente-remoto"));
        assert_eq!(overview.kinds, vec!["pi".to_owned()]);
        assert!(
            calls(&host, LOCAL).is_empty(),
            "discovery never reached Local"
        );

        let ssh = target(SSH_ID, "boot-ssh-3", PANE);
        state
            .with_core(|gateway, core| core.split(gateway, &ssh, "right"))
            .unwrap();
        let local = target(LOCAL, "boot-local-7", PANE);
        let refused = state
            .with_core(|gateway, core| core.split(gateway, &local, "down"))
            .unwrap_err();
        assert_eq!(refused.code, "target_endpoint_mismatch", "{refused:?}");
        let endpoint_calls: Vec<String> = calls(&host, SSH_ID)
            .into_iter()
            .filter(|c| c.starts_with("endpoint:"))
            .collect();
        assert_eq!(endpoint_calls.len(), 1, "{endpoint_calls:?}");
        assert!(endpoint_calls[0].contains("\"target_pane_id\":\"w1:p1\""));
        assert!(endpoint_calls[0].contains("\"direction\":\"right\""));
        assert!(calls(&host, LOCAL).is_empty(), "no action on Local");

        // The host committed a new surface with a second pane: actions see it without re-attach.
        {
            let mut frame = full("boot-ssh-3", 2, "remote$");
            frame.panes = vec![pane(6, 2, true, PANE, 0), pane(6, 2, false, "w1:p2", 6)];
            host.hosts.lock().unwrap().get_mut(SSH_ID).unwrap().surface = Some(frame);
        }
        state
            .with_core(|gateway, core| {
                core.split(gateway, &target(SSH_ID, "boot-ssh-3", "w1:p2"), "down")
            })
            .expect("second pane known from the host's committed surface");

        // Selection moved to Local: the attached SSH core refuses; nothing reaches either host.
        *host.selected.lock().unwrap() = LOCAL.into();
        let before = host.log.lock().unwrap().len();
        let changed = state
            .with_core(|gateway, core| core.split(gateway, &ssh, "right"))
            .unwrap_err();
        assert_eq!(changed.code, "selection_changed", "{changed:?}");
        let changed_local = state
            .with_core(|gateway, core| core.split(gateway, &local, "right"))
            .unwrap_err();
        assert_eq!(changed_local.code, "selection_changed", "{changed_local:?}");
        assert_eq!(host.log.lock().unwrap().len(), before, "nothing sent");

        let overview = state.attach_hosted(None).expect("re-attach to Local");
        assert_eq!(overview.identity.unwrap().boot_id, "boot-local-7");
        assert_eq!(overview.agents[0].name.as_deref(), Some("agente-local"));
    }

    /// Would catch: a server without `remote-api-bridge` failing the whole agents attach (or
    /// disabling split/focus with it), one ssh probe per API method after the bridge was already
    /// reported missing, an API action still attempted, or a transport failure during discovery
    /// mistaken for a missing optional API.
    #[test]
    fn hosted_agents_without_the_remote_api_disable_only_api_actions_and_keep_endpoint_actions() {
        let host = host();
        host.hosts.lock().unwrap().get_mut(SSH_ID).unwrap().api = Api::RemoteApiUnsupported;
        let state = AgentsState::hosted(host.clone());
        let overview = state
            .attach_hosted(None)
            .expect("attach without the API bridge");
        let caps = overview.capabilities.unwrap();
        assert!(!caps.list_agents && !caps.start_agent && !caps.send_prompt);
        assert!(caps.split && caps.focus && caps.open_attention);
        assert_eq!(calls(&host, SSH_ID), vec!["api:agent.list".to_owned()]);
        assert!(overview.error.is_none());

        let ssh = target(SSH_ID, "boot-ssh-3", PANE);
        let start = state
            .with_core(|gateway, core| core.start_agent(gateway, &ssh, "pi", "agente"))
            .unwrap_err();
        assert_eq!(start.code, "unsupported_method");
        state
            .with_core(|gateway, core| core.split(gateway, &ssh, "right"))
            .expect("endpoint action preserved");
        assert_eq!(calls(&host, SSH_ID).len(), 2, "{:?}", calls(&host, SSH_ID));
        assert!(calls(&host, LOCAL).is_empty());

        host.hosts.lock().unwrap().get_mut(SSH_ID).unwrap().api = Api::Unreachable;
        let error = state.attach_hosted(None).unwrap_err();
        assert_eq!(error.code, "ssh_unreachable");
        assert!(calls(&host, LOCAL).is_empty());
    }

    /// Would catch: the hosted watcher still opening the local API socket, a subscription that
    /// does not follow the selected host's panes, or an event refreshing agents through Local.
    #[cfg(unix)]
    #[test]
    fn hosted_agent_events_use_the_hosts_stream_and_refresh_through_the_selected_gateway() {
        use std::io::BufRead;
        let host = host();
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        host.hosts.lock().unwrap().get_mut(SSH_ID).unwrap().stream = Some(client);
        let state = AgentsState::hosted(host.clone());
        state.attach_hosted(None).unwrap();

        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = std::io::BufReader::new(server.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).expect("subscription request");
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "events.subscribe");
        assert!(request["params"]["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["type"] == "pane.agent_status_changed" && s["pane_id"] == PANE));

        let lists = |host: &Host| {
            calls(host, SSH_ID)
                .iter()
                .filter(|c| *c == "api:agent.list")
                .count()
        };
        let before = lists(&host);
        (&server)
            .write_all(b"{\"event\":\"pane.agent_status_changed\",\"data\":{}}\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while lists(&host) == before {
            assert!(Instant::now() < deadline, "event never refreshed agents");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(calls(&host, LOCAL).is_empty());
        state.detach();
    }
}

// =======================================================================================
// Bloco 05 — SelectionState: gateway views of the shared hub, selected AgentsHost and the
// durable `events.subscribe` streams (Local API socket, SSH `remote-api-bridge`)
// =======================================================================================

#[cfg(unix)]
mod composed_selection {
    use super::*;
    use herdr_desktop::bridge::agent_commands::AgentsState;
    use herdr_desktop::bridge::selection::SelectionState;
    use herdr_desktop::connections::commands::{ConnectionsConfig, ConnectionsState};
    use herdr_desktop::connections::profiles::SshProfileDraft;
    use herdr_desktop::connections::ssh_options::{build_ssh, RemoteHerdrCommand};
    use std::io::BufRead;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// OpenSSH of the composed window: probes fail (a renegotiation never succeeds in the fake),
    /// every spawned process takes the next socket and records its command; kills are counted.
    #[derive(Default)]
    struct BridgeRunner {
        commands: Mutex<Vec<OpenSshCommand>>,
        sockets: Mutex<VecDeque<UnixStream>>,
        kills: Arc<AtomicUsize>,
    }

    struct BridgeChild {
        socket: UnixStream,
        kills: Arc<AtomicUsize>,
        killed: bool,
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
            self.killed.then_some(None)
        }
        fn kill(&mut self) {
            if !self.killed {
                self.killed = true;
                self.kills.fetch_add(1, Ordering::SeqCst);
                let _ = self.socket.shutdown(std::net::Shutdown::Both);
            }
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
            self.commands.lock().unwrap().push(command.clone());
            let socket = self.sockets.lock().unwrap().pop_front().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no fake bridge")
            })?;
            Ok(Box::new(BridgeChild {
                socket,
                kills: self.kills.clone(),
                killed: false,
            }))
        }
    }

    struct Composed {
        _dirs: Vec<tempfile::TempDir>,
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
        fn host_json(&self, endpoint: &str) -> Value {
            let snap = serde_json::to_value(self.hub().snapshot(Instant::now())).unwrap();
            snap["hosts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["endpoint"] == endpoint)
                .cloned()
                .expect("host in snapshot")
        }
        fn actions(&self, endpoint: &str) -> Vec<(String, String)> {
            self.host_json(endpoint)["actions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| {
                    (
                        a["method"].as_str().unwrap().to_owned(),
                        a["outcome"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        }
    }

    /// Local session `hd007-local` and one saved SSH profile (session `hd007-remote`), nothing
    /// connected, sharing one `ConnectionsState` with the selection.
    fn composed() -> Composed {
        let prefs = tempfile::tempdir().unwrap();
        let herdr_config = tempfile::tempdir().unwrap();
        let herdr_state = tempfile::tempdir().unwrap();
        let session = SessionName::parse("hd007-local").unwrap();
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
                    session: "hd007-remote".into(),
                    auth: None,
                },
                false,
            )
            .unwrap();
        let ssh = view.profiles[0].id.as_str().to_owned();
        let selection = SelectionState::new(connections.clone());
        Composed {
            _dirs: vec![prefs, herdr_config, herdr_state],
            connections,
            selection,
            runner,
            ssh,
            api_socket: paths.api_socket,
        }
    }

    fn api_unsupported() -> Vec<(String, Result<Value, RuntimeError>)> {
        [
            "agent.list",
            "agent.get",
            "agent.start",
            "agent.prompt",
            "server.agent_manifests",
            "tab.list",
        ]
        .iter()
        .map(|m| {
            (
                (*m).to_owned(),
                Err(RuntimeError::new(
                    "remote_api_unsupported",
                    "sem remote-api-bridge",
                )),
            )
        })
        .collect()
    }

    fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Reads one line from an event stream, riding its periodic read timeouts; `None` on end.
    fn read_event_line(stream: &mut dyn Read, within: Duration) -> Option<String> {
        let deadline = Instant::now() + within;
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            assert!(Instant::now() < deadline, "no event line or end in time");
            match stream.read(&mut byte) {
                Ok(0) => return None,
                Ok(_) if byte[0] == b'\n' => return Some(String::from_utf8(line).unwrap()),
                Ok(_) => line.push(byte[0]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("event stream failed: {error}"),
            }
        }
    }

    /// Would catch: a view that routes by pane id alone (Local has the same `w1:p1`), resolves
    /// the target lazily from whatever connection is live now (a reconnect or reboot silently
    /// retargeting), invents the active pane when the method's real target field is missing, sends
    /// before checking the snapshot, falls back to Local for an unknown endpoint, or lets the
    /// adapter connect/detach the hub's own connection.
    #[test]
    fn selection_view_sends_once_to_its_captured_host_and_refuses_stale_or_implicit_targets() {
        let c = composed();
        let ssh = c.ssh.as_str();
        c.selection.select(ssh).unwrap();
        assert_eq!(c.selection.selected().as_deref(), Some(ssh));
        let hub = c.hub();
        let local = connect_host(hub, LOCAL, "boot-local-7", 5, "local$");
        let remote = connect_host(hub, ssh, "boot-ssh-3", 9, "remote$");
        assert!(
            hub.surface(LOCAL).is_none(),
            "only the selected host keeps a surface"
        );
        assert!(hub.surface(ssh).is_some());

        let view = c.selection.selected_gateway().unwrap();
        assert_eq!(view.endpoint(), ssh);
        let identity = view.identity().unwrap();
        assert_eq!(identity.session, "hd007-remote");
        assert_eq!(identity.boot_id, "boot-ssh-3");
        let sent = [
            (
                "pane.split",
                json!({"target_pane_id": PANE, "direction": "right", "focus": false}),
            ),
            ("pane.focus", json!({"pane_id": PANE})),
            ("tab.focus", json!({"tab_id": "w1:t1"})),
            ("tab.create", json!({"workspace_id": "w1", "focus": true})),
        ];
        for (method, params) in &sent {
            view.endpoint_request(method, params.clone())
                .unwrap_or_else(|e| panic!("{method}: {e}"));
        }
        let refused = [
            (
                "pane.focus",
                json!({"pane_id": "w9:p9"}),
                "pane_not_in_snapshot",
            ),
            (
                "tab.focus",
                json!({"tab_id": "w9:t9"}),
                "tab_not_in_snapshot",
            ),
            (
                "tab.create",
                json!({"workspace_id": "w9", "focus": true}),
                "workspace_not_in_snapshot",
            ),
            (
                "pane.split",
                json!({"direction": "right"}),
                "target_required",
            ),
            (
                "pane.split",
                json!({"target_pane_id": PANE, "pane_id": "w9:p9", "direction": "down"}),
                "target_ambiguous",
            ),
        ];
        for (method, params, code) in refused {
            let error = view.endpoint_request(method, params).unwrap_err();
            assert_eq!(error.code, code, "{method}");
            assert_eq!(error.endpoint.as_deref(), Some(ssh));
        }
        {
            let wire = remote.wire.lock().unwrap();
            let got: Vec<(String, Value)> = wire.endpoint.clone();
            let want: Vec<(String, Value)> = sent
                .iter()
                .map(|(m, p)| ((*m).to_owned(), p.clone()))
                .collect();
            assert_eq!(got, want, "each explicit action once, params untouched");
            assert!(wire.endpoint_boots.iter().all(|b| b == "boot-ssh-3"));
        }
        assert_eq!(
            c.actions(ssh).len(),
            sent.len(),
            "refusals are not recorded"
        );
        assert!(local.wire.lock().unwrap().endpoint.is_empty());

        // Reconnect of the same endpoint (same boot, new generation): the captured view refuses.
        let stale = c.selection.gateway_for(ssh).unwrap();
        hub.cancel(ssh).unwrap();
        let again = connect_host(hub, ssh, "boot-ssh-3", 11, "remote2$");
        let target = hub.input_gate(ssh, PANE).unwrap();
        let focus = stale
            .endpoint_request("pane.focus", json!({"pane_id": PANE}))
            .unwrap_err();
        assert_eq!(focus.code, "target_generation_stale");
        let prompt = stale
            .api_request("agent.prompt", json!({"target": PANE, "text": "x"}))
            .unwrap_err();
        assert_eq!(prompt.code, "target_generation_stale");
        let input = stale.send_input(&target, text("velho")).unwrap_err();
        assert_eq!(input.code, "target_generation_stale");
        {
            let wire = again.wire.lock().unwrap();
            assert!(wire.endpoint.is_empty() && wire.api.is_empty() && wire.inputs.is_empty());
        }
        let mut fresh = c.selection.gateway_for(ssh).unwrap();
        fresh.send_input(&target, text("novo")).unwrap();
        assert_eq!(again.wire.lock().unwrap().inputs.len(), 1);

        // Reboot seen on the same connection: refused before sending.
        *again.boot.lock().unwrap() = "boot-ssh-4".into();
        let rebooted = fresh
            .endpoint_request("pane.focus", json!({"pane_id": PANE}))
            .unwrap_err();
        assert_eq!(rebooted.code, "target_boot_stale");
        assert!(again.wire.lock().unwrap().endpoint.is_empty());

        // The hub owns the connection: the view neither connects nor detaches it.
        let owned = fresh
            .connect(ConnectOptions {
                geometry: geometry(),
                surface_active: true,
            })
            .unwrap_err();
        assert_eq!(owned.code, "hub_owned");
        assert!(fresh.take_events().is_none());
        fresh.detach();
        assert_eq!(again.wire.lock().unwrap().detached, 0);
        assert!(c.selection.gateway_for(ssh).is_ok(), "hub connection kept");

        // Unknown endpoint and offline host: errors, never Local.
        let unknown = "0007ffff0007ffff0007ffff0007ffff";
        assert_eq!(
            c.selection.gateway_for(unknown).err().unwrap().code,
            "endpoint_unknown"
        );
        assert_eq!(
            c.selection.select(unknown).unwrap_err().code,
            "endpoint_unknown"
        );
        assert_eq!(c.selection.selected().as_deref(), Some(ssh));
        hub.cancel(LOCAL).unwrap();
        assert_eq!(
            c.selection.gateway_for(LOCAL).err().unwrap().code,
            "host_offline"
        );
        assert!(local.wire.lock().unwrap().api.is_empty());
    }

    /// Would catch: reads (and capability probes) polluting the action ledger or the host's
    /// action error, a consequential API call left out of the ledger, a lost/timed-out
    /// consequential call reported as a plain failure (inviting a retry) or re-sent.
    #[test]
    fn selection_view_api_reads_skip_the_ledger_and_consequential_calls_become_unknown_once() {
        let c = composed();
        let ssh = c.ssh.as_str();
        c.selection.select(ssh).unwrap();
        let hub = c.hub();
        let local = connect_host(hub, LOCAL, "boot-local-7", 5, "local$");
        let remote = connect_host_api(
            hub,
            ssh,
            "boot-ssh-3",
            9,
            "remote$",
            vec![
                (
                    "agent.get".into(),
                    Err(RuntimeError::new("connection_lost", "caiu").retryable()),
                ),
                (
                    "agent.start".into(),
                    Err(RuntimeError::new("timeout", "sem resposta").retryable()),
                ),
            ],
        );
        let view = c.selection.selected_gateway().unwrap();
        view.api_request("agent.list", json!({})).unwrap();
        view.api_request("tab.list", json!({})).unwrap();
        view.api_request("server.agent_manifests", json!({}))
            .unwrap();
        view.api_request("agent.prompt", json!({})).unwrap();
        let read = view
            .api_request("agent.get", json!({"target": PANE}))
            .unwrap_err();
        assert_eq!(read.code, "connection_lost");
        assert_eq!(read.endpoint.as_deref(), Some(ssh));
        assert!(c.actions(ssh).is_empty(), "{:?}", c.actions(ssh));
        assert!(c.host_json(ssh)["action_error"].is_null());

        view.api_request("agent.prompt", json!({"target": PANE, "text": "olá"}))
            .unwrap();
        let start = view
            .api_request(
                "agent.start",
                json!({"name": "a", "kind": "pi", "pane_id": PANE}),
            )
            .unwrap_err();
        assert_eq!(start.code, "result_unknown");
        assert_eq!(
            c.actions(ssh),
            vec![
                ("agent.start".to_owned(), "unknown".to_owned()),
                ("agent.prompt".to_owned(), "succeeded".to_owned()),
            ]
        );
        std::thread::sleep(Duration::from_millis(100));
        let starts = remote
            .wire
            .lock()
            .unwrap()
            .api
            .iter()
            .filter(|(m, _)| m == "agent.start")
            .count();
        assert_eq!(starts, 1, "never re-sent");
        assert!(local.wire.lock().unwrap().api.is_empty());
    }

    /// Would catch: the selected AgentsHost pairing the gateway of one connection with the
    /// surface of another (a reconnect/reboot or a selection change between the two calls), the
    /// hosted agents discovering or acting on Local, a Local target accepted on SSH, discovery
    /// probes recorded as actions, or detaching the agents detaching the hub's connection.
    #[test]
    fn selected_agents_host_pairs_surface_with_its_gateway_and_hosted_agents_stay_on_ssh() {
        let c = composed();
        let ssh = c.ssh.as_str();
        c.selection.select(ssh).unwrap();
        let hub = c.hub();
        let local = connect_host(hub, LOCAL, "boot-local-7", 5, "local$");
        let remote = connect_host(hub, ssh, "boot-ssh-3", 9, "remote$");
        let host = c.selection.agents_host();
        assert_eq!(host.endpoint(), ssh);
        assert_eq!(host.endpoint_methods().unwrap(), announced());
        assert!(
            host.surface().is_none(),
            "no surface before a gateway of this connection was issued"
        );
        assert_eq!(host.gateway().unwrap().endpoint(), ssh);
        assert_eq!(host.surface().unwrap().boot_id, "boot-ssh-3");

        let agents = AgentsState::hosted(host.clone());
        let overview = agents.attach_hosted(None).unwrap();
        assert_eq!(overview.identity.unwrap().endpoint, ssh);
        let target = hub.input_gate(ssh, PANE).unwrap();
        agents
            .with_core(|gateway, core| core.split(gateway, &target, "right"))
            .unwrap();
        let local_target = target_for(&target, LOCAL, "hd007-local", "boot-local-7");
        let wrong = agents
            .with_core(|gateway, core| core.split(gateway, &local_target, "down"))
            .unwrap_err();
        assert_eq!(wrong.code, "target_endpoint_mismatch");
        {
            let wire = remote.wire.lock().unwrap();
            assert_eq!(wire.endpoint.len(), 1);
            assert_eq!(wire.endpoint[0].0, "pane.split");
            assert_eq!(wire.endpoint[0].1["target_pane_id"], PANE);
            assert!(!wire.api.is_empty(), "discovery went to the SSH host");
        }
        {
            let wire = local.wire.lock().unwrap();
            assert!(wire.api.is_empty() && wire.endpoint.is_empty());
        }
        assert_eq!(
            c.actions(ssh),
            vec![
                ("pane.split".to_owned(), "succeeded".to_owned()),
                // Spec 013: the discovery of this host also reads `pane.list` (the working
                // directory of each pane, shown in the frames); read-only and on this host only.
                ("pane.list".to_owned(), "succeeded".to_owned()),
            ]
        );
        agents.detach();
        assert_eq!(remote.wire.lock().unwrap().detached, 0);
        assert!(hub.input_gate(ssh, PANE).is_ok(), "hub connection kept");

        // Reconnect to a rebooted server between gateway() and surface().
        let _issued = host.gateway().unwrap();
        hub.cancel(ssh).unwrap();
        let _rebooted = connect_host(hub, ssh, "boot-ssh-5", 3, "novo$");
        assert!(
            host.surface().is_none(),
            "a surface of another connection is not paired with the issued gateway"
        );
        assert_eq!(
            host.gateway().unwrap().identity().unwrap().boot_id,
            "boot-ssh-5"
        );
        assert_eq!(host.surface().unwrap().boot_id, "boot-ssh-5");

        // Selection change between gateway() and surface(): never another host's frame.
        let _issued = host.gateway().unwrap();
        c.selection.select(LOCAL).unwrap();
        assert_eq!(host.endpoint(), LOCAL);
        assert!(host.surface().is_none());
        assert!(
            hub.surface(ssh).is_none(),
            "the unselected host is metadata-only"
        );
        assert!(local.wire.lock().unwrap().api.is_empty());
    }

    /// Would catch: a server without `remote-api-bridge` failing the attach, disabling endpoint
    /// actions or terminal input with the API, a `remote-api-bridge` process started for the
    /// event stream anyway, or Local touched instead.
    #[test]
    fn selection_without_remote_api_keeps_endpoint_actions_terminal_and_opens_no_bridge() {
        let c = composed();
        let ssh = c.ssh.as_str();
        c.selection.select(ssh).unwrap();
        let hub = c.hub();
        let local = connect_host(hub, LOCAL, "boot-local-7", 5, "local$");
        let remote = connect_host_api(hub, ssh, "boot-ssh-3", 9, "remote$", api_unsupported());
        let host = c.selection.agents_host();
        let agents = AgentsState::hosted(host.clone());
        let overview = agents.attach_hosted(None).expect("attach without the API");
        let caps = overview.capabilities.unwrap();
        assert!(!caps.list_agents && !caps.start_agent && !caps.send_prompt);
        assert!(caps.split && caps.focus);
        let target = hub.input_gate(ssh, PANE).unwrap();
        agents
            .with_core(|gateway, core| core.split(gateway, &target, "down"))
            .unwrap();
        hub.send_input(&target, text("terminal segue")).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            c.runner.commands.lock().unwrap().is_empty(),
            "no remote-api-bridge process"
        );
        assert_eq!(
            host.event_stream(
                &c.hub()
                    .live_identity(&c.selection.selected().unwrap())
                    .unwrap()
            )
            .err()
            .unwrap()
            .code,
            "remote_api_unsupported"
        );
        {
            let wire = remote.wire.lock().unwrap();
            assert_eq!(wire.endpoint.len(), 1);
            assert_eq!(wire.inputs.len(), 1);
        }
        assert!(c.actions(ssh).iter().all(|(m, _)| m == "pane.split"));
        let wire = local.wire.lock().unwrap();
        assert!(wire.api.is_empty() && wire.endpoint.is_empty() && wire.inputs.is_empty());
        drop(wire);
        agents.detach();
    }

    /// Would catch: a Local stream that is not the session's API socket or blocks without a read
    /// timeout; an SSH stream that is not the profile's `remote-api-bridge` command, buffers
    /// without bound while the watcher is idle, keeps the process after being dropped; a stream
    /// of a previous connection or of a deselected host that keeps delivering events; or the
    /// hosted watcher retaining its bridge process after detach or reselection.
    #[test]
    fn selection_event_streams_are_the_hosts_own_bounded_fenced_and_cancelled() {
        let c = composed();
        let ssh = c.ssh.as_str();
        let hub = c.hub();
        c.selection.select(LOCAL).unwrap();
        let _local = connect_host(hub, LOCAL, "boot-local-7", 5, "local$");
        let host = c.selection.agents_host();

        // Local: the session's API socket, periodic read timeout, fenced by connection.
        let listener = UnixListener::bind(&c.api_socket).unwrap();
        let mut stream = host
            .event_stream(
                &c.hub()
                    .live_identity(&c.selection.selected().unwrap())
                    .unwrap(),
            )
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        stream
            .write_all(b"{\"id\":\"s1\",\"method\":\"events.subscribe\"}\n")
            .unwrap();
        let mut reader = std::io::BufReader::new(server.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("events.subscribe"));
        (&server)
            .write_all(b"{\"event\":\"tab.created\"}\n")
            .unwrap();
        assert_eq!(
            read_event_line(&mut *stream, Duration::from_secs(2)).as_deref(),
            Some("{\"event\":\"tab.created\"}")
        );
        let started = Instant::now();
        let idle = stream.read(&mut [0u8; 16]).unwrap_err();
        assert!(matches!(
            idle.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
        assert!(started.elapsed() < Duration::from_secs(1), "read timeout");
        hub.cancel(LOCAL).unwrap();
        let _local2 = connect_host(hub, LOCAL, "boot-local-8", 1, "local2$");
        (&server)
            .write_all(b"{\"event\":\"tab.closed\"}\n")
            .unwrap();
        assert_eq!(
            read_event_line(&mut *stream, Duration::from_secs(2)),
            None,
            "a stream of the previous connection ends instead of delivering"
        );
        drop(stream);

        // SSH: the profile's remote-api-bridge, bounded while idle, killed on drop.
        c.selection.select(ssh).unwrap();
        let _remote = connect_host(hub, ssh, "boot-ssh-3", 9, "remote$");
        let (client, server) = UnixStream::pair().unwrap();
        c.runner.sockets.lock().unwrap().push_back(client);
        let mut stream = host
            .event_stream(
                &c.hub()
                    .live_identity(&c.selection.selected().unwrap())
                    .unwrap(),
            )
            .unwrap();
        let identity = SshIdentity::new(
            ProfileId::parse(ssh).unwrap(),
            "tester@127.0.0.1",
            None,
            "hd007-remote",
        )
        .unwrap();
        assert_eq!(
            *c.runner.commands.lock().unwrap(),
            vec![build_ssh(&identity, None, RemoteHerdrCommand::ApiBridge)]
        );
        stream
            .write_all(b"{\"id\":\"s2\",\"method\":\"events.subscribe\"}\n")
            .unwrap();
        let mut reader = std::io::BufReader::new(server.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("\"s2\""));
        (&server)
            .write_all(b"{\"event\":\"pane.created\"}\n")
            .unwrap();
        assert_eq!(
            read_event_line(&mut *stream, Duration::from_secs(2)).as_deref(),
            Some("{\"event\":\"pane.created\"}")
        );
        let started = Instant::now();
        let idle = stream.read(&mut [0u8; 16]).unwrap_err();
        assert!(matches!(
            idle.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
        assert!(started.elapsed() < Duration::from_secs(1), "read timeout");

        let written = Arc::new(AtomicUsize::new(0));
        let flood = {
            let (mut server, written) = (server.try_clone().unwrap(), written.clone());
            std::thread::spawn(move || {
                let chunk = vec![b'x'; 64 * 1024];
                while written.load(Ordering::SeqCst) < 64 * 1024 * 1024 {
                    if server.write_all(&chunk).is_err() {
                        return;
                    }
                    written.fetch_add(chunk.len(), Ordering::SeqCst);
                }
            })
        };
        std::thread::sleep(Duration::from_millis(500));
        let settled = written.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            written.load(Ordering::SeqCst),
            settled,
            "nothing more is read while the watcher is idle"
        );
        assert!(settled < 4 * 1024 * 1024, "bounded buffer: {settled} bytes");
        assert_eq!(c.runner.kills.load(Ordering::SeqCst), 0);
        drop(stream);
        assert_eq!(c.runner.kills.load(Ordering::SeqCst), 1, "killed on drop");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !flood.is_finished() {
            assert!(Instant::now() < deadline, "bridge output released");
            std::thread::sleep(Duration::from_millis(10));
        }

        // Hosted watcher: its bridge process ends on detach and on reselection.
        let agents = AgentsState::hosted(host.clone());
        for round in 0..2 {
            let (client, server) = UnixStream::pair().unwrap();
            c.runner.sockets.lock().unwrap().push_back(client);
            let kills = c.runner.kills.load(Ordering::SeqCst);
            agents.attach_hosted(None).unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(server.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("events.subscribe"), "watcher subscribed");
            if round == 0 {
                agents.detach();
            } else {
                c.selection.select(LOCAL).unwrap();
            }
            eventually("watcher bridge process killed", || {
                c.runner.kills.load(Ordering::SeqCst) == kills + 1
            });
            if round == 0 {
                continue;
            }
            agents.detach();
        }
    }

    /// Would catch: a read whose reply arrives after the server rebooted or the host reconnected
    /// still handed to the caller (an old result refreshing the core of the new connection), or
    /// an action whose success arrives after a reboot, reconnect or loss reported as success (and
    /// recorded as succeeded) instead of unknown, or any of them re-sent.
    #[test]
    fn late_replies_after_reboot_reconnect_or_loss_are_refused_or_unknown_and_sent_once() {
        let c = composed();
        let ssh = c.ssh.as_str();
        c.selection.select(ssh).unwrap();
        let hub = c.hub();
        let calls = |host: &Host, method: &str| {
            let wire = host.wire.lock().unwrap();
            wire.api.iter().filter(|(m, _)| m == method).count()
                + wire.endpoint.iter().filter(|(m, _)| m == method).count()
        };
        let wait_call = |host: &Host, method: &str| {
            eventually("request reached the lane", || calls(host, method) == 1)
        };
        let reply = Ok(json!({"type": "ok", "agents": []}));
        #[derive(Clone, Copy, PartialEq)]
        enum Change {
            None,
            Reboot,
            Reconnect,
            Loss,
        }
        let cases = [
            ("agent.list", json!({}), Change::None, Ok(()), None),
            (
                "agent.list",
                json!({}),
                Change::Reboot,
                Err("target_boot_stale"),
                None,
            ),
            (
                "agent.get",
                json!({"target": PANE}),
                Change::Reconnect,
                Err("target_generation_stale"),
                None,
            ),
            (
                "agent.prompt",
                json!({"target": PANE, "text": "a"}),
                Change::None,
                Ok(()),
                Some("succeeded"),
            ),
            (
                "agent.prompt",
                json!({"target": PANE, "text": "b"}),
                Change::Reboot,
                Err("result_unknown"),
                Some("unknown"),
            ),
            (
                "pane.focus",
                json!({"pane_id": PANE}),
                Change::None,
                Ok(()),
                Some("succeeded"),
            ),
            (
                "pane.focus",
                json!({"pane_id": PANE}),
                Change::Reboot,
                Err("result_unknown"),
                Some("unknown"),
            ),
            (
                "pane.split",
                json!({"target_pane_id": PANE, "direction": "right"}),
                Change::Reconnect,
                Err("result_unknown"),
                Some("unknown"),
            ),
            (
                "pane.split",
                json!({"target_pane_id": PANE, "direction": "down"}),
                Change::Loss,
                Err("result_unknown"),
                Some("unknown"),
            ),
        ];
        for (round, (method, params, change, expected, outcome)) in cases.into_iter().enumerate() {
            let boot = format!("boot-ssh-{round}");
            let _ = hub.cancel(ssh);
            let host = connect_host(hub, ssh, &boot, 1, "remote$");
            let view = c.selection.gateway_for(ssh).unwrap();
            let (release, gate) = sync_channel(1);
            let endpoint = method.starts_with("pane.");
            if endpoint {
                host.lane.gate.lock().unwrap().push_back(gate);
            } else {
                host.api.gate.lock().unwrap().push_back(gate);
            }
            let pending = std::thread::spawn(move || {
                if endpoint {
                    view.endpoint_request(method, params)
                } else {
                    view.api_request(method, params)
                }
            });
            wait_call(&host, method);
            match change {
                Change::None => {}
                Change::Reboot => *host.boot.lock().unwrap() = format!("{boot}-rebooted"),
                Change::Reconnect => {
                    hub.cancel(ssh).unwrap();
                    let _next = connect_host(hub, ssh, &boot, 2, "remote2$");
                }
                Change::Loss => hub.cancel(ssh).unwrap(),
            }
            release.send(reply.clone()).unwrap();
            let got = pending.join().unwrap();
            match expected {
                Ok(()) => assert!(got.is_ok(), "{method} round {round}: {got:?}"),
                Err(code) => assert_eq!(
                    got.as_ref().err().map(|e| e.code.as_str()),
                    Some(code),
                    "{method} round {round}: {got:?}"
                ),
            }
            let actions = c.actions(ssh);
            match outcome {
                None => assert!(
                    actions.iter().all(|(_, o)| o != "pending")
                        && !actions.iter().any(|(m, _)| m == method),
                    "reads are not recorded: {actions:?}"
                ),
                Some(outcome) => assert_eq!(
                    actions.first(),
                    Some(&(method.to_owned(), outcome.to_owned())),
                    "round {round}"
                ),
            }
            std::thread::sleep(Duration::from_millis(30));
            assert_eq!(calls(&host, method), 1, "{method} round {round} sent once");
        }
    }
}
