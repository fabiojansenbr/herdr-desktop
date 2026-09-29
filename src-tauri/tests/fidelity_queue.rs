//! Spec 007 — AC-007-01 on the SSH visual lane: the event queue shared with the local gateway.
//!
//! The ssh process is replaced by a socket pair (no sshd, engine or display). The consumer is
//! the `RuntimeGateway::take_events` receiver the hub uses; it stays alive and paused while the
//! remote engine bursts. Contracts:
//! - a burst that ends above the 8 MiB byte budget (fewer frames than slots) signals one
//!   overflow without needing a later frame, and recovery reconciles with one full surface;
//! - a burst of more small frames than the 512 slots, followed by control events, never stops
//!   the reader: an endpoint reply is still correlated while paused, controls are delivered in
//!   order after the notice, and `detach` completes with the receiver alive;
//! - input accepted before the overflow reaches the engine exactly once and is never replayed.
//!
//! Fixture values are distinct on purpose: boot `boot-ssh-8`, texts `antes` / `depois` /
//! `durante` / `apos`, control messages `erro-1..3`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::protocol::endpoint::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_SNAPSHOT_KIND,
    ENDPOINT_WELCOME_KIND,
};
use herdr_client::protocol::wire::{
    CellData, ClientMessage, ClientPaneInputEvent, ClientShellSnapshot, CursorState, FrameData,
    PaneSurfaceFrame, PaneSurfacePane, ServerMessage, SurfaceRect,
};
use herdr_client::protocol::{
    decode_message, encode_message, read_frame, write_message, MAX_FRAME_SIZE,
};
use herdr_client::{
    ConnectOptions, FrameStore, GatewayEvent, QualifiedTarget, RuntimeGateway, StaleReason,
    SurfaceGeometry,
};
use herdr_desktop::bridge::ssh::{ProcessOutput, SshChild, SshConnector, SshGateway, SshRunner};
use herdr_desktop::connections::ssh_options::{OpenSshCommand, ProfileId, SshIdentity};
use serde_json::{json, Value};

const SSH_ID: &str = "0007bbbb0007bbbb0007bbbb0007bbbb";
const PANE: &str = "w1:p1";
const BOOT: &str = "boot-ssh-8";
const BUDGET: usize = 8 * 1024 * 1024;
const SLOTS: usize = 512;

// --- fixtures ------------------------------------------------------------------------------

fn geometry() -> SurfaceGeometry {
    SurfaceGeometry {
        cols: 12,
        rows: 2,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn pane(width: u16, height: u16) -> PaneSurfacePane {
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    PaneSurfacePane {
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

fn full(revision: u64, text: &str) -> PaneSurfaceFrame {
    let (width, height) = (12, 2);
    PaneSurfaceFrame {
        boot_id: BOOT.into(),
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
        panes: vec![pane(width, height)],
        splits: vec![],
        popup: None,
        graphics: Default::default(),
    }
}

/// A full frame of roughly `bytes` encoded bytes (wide surface of one-byte symbols).
fn big_full(revision: u64, bytes: usize) -> PaneSurfaceFrame {
    let width: u16 = 400;
    let height = (bytes / (usize::from(width) * 12)).max(1) as u16;
    let mut frame = full(revision, "");
    frame.frame.width = width;
    frame.frame.height = height;
    frame.frame.cells = cells(&"x".repeat(usize::from(width)), width, height);
    frame.panes = vec![pane(width, height)];
    frame
}

fn snapshot() -> ClientShellSnapshot {
    let mut snap: ClientShellSnapshot = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/endpoint-snapshot-v1.json"
        ))
        .unwrap(),
    )
    .unwrap();
    snap.boot_id = BOOT.into();
    assert_eq!(snap.panes[0].pane_id, PANE, "fixture premise");
    snap
}

fn row_text(store: &FrameStore, y: usize) -> String {
    store.text_rows()[y].trim_end().to_owned()
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
    for capability in ["surface_interest", "health_check"] {
        if !welcome.capabilities.iter().any(|c| c == capability) {
            welcome.capabilities.push(capability.to_owned());
        }
    }
    serde_json::to_string(&welcome).unwrap()
}

// --- ssh process stand-in -------------------------------------------------------------------

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
    /// Like a real ssh process that does not exit by itself: waits the whole budget, so a
    /// detach gives the writer its grace period before the kill.
    fn wait_exit(&mut self, timeout: Duration) -> Option<Option<i32>> {
        if !self.killed {
            std::thread::sleep(timeout);
        }
        self.killed.then_some(None)
    }
    fn kill(&mut self) {
        self.killed = true;
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

/// Probe answers a compatible running server; the bridge is the client end of a socket pair.
struct PipeRunner(Mutex<Option<std::os::unix::net::UnixStream>>);

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

/// Remote engine end of the socket pair: every client message goes to `inbox`.
struct Engine {
    writer: Arc<Mutex<std::os::unix::net::UnixStream>>,
    inbox: Mutex<Receiver<ClientMessage>>,
}

impl Engine {
    /// Connects a real `SshGateway` through `SshConnector`, publishes the boot and one full
    /// surface at revision 1 (`antes`).
    fn ssh(methods: &[&'static str]) -> (Engine, SshGateway) {
        let (client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        let methods: Vec<&'static str> = methods.to_vec();
        let accept = std::thread::spawn(move || {
            let hello: ClientMessage =
                decode_message(&read_frame(&mut server, MAX_FRAME_SIZE).unwrap()).unwrap();
            let ClientMessage::EndpointControl { kind, data } = hello else {
                panic!("expected hello, got {hello:?}");
            };
            assert_eq!(kind, ENDPOINT_HELLO_KIND);
            let hello: EndpointClientHello = serde_json::from_str(&data).unwrap();
            assert_eq!(hello.generation, 1);
            write_message(
                &mut server,
                &ServerMessage::EndpointControl {
                    kind: ENDPOINT_WELCOME_KIND.into(),
                    data: welcome_json(&methods),
                },
            )
            .unwrap();
            server
        });
        let identity = SshIdentity::new(
            ProfileId::parse(SSH_ID).unwrap(),
            "tester@127.0.0.1",
            None,
            "hd007-queue",
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
        };
        engine.send(&ServerMessage::EndpointControl {
            kind: ENDPOINT_SNAPSHOT_KIND.into(),
            data: serde_json::to_string(&snapshot()).unwrap(),
        });
        engine.send(&ServerMessage::PaneSurface(full(1, "antes")));
        let deadline = Instant::now() + Duration::from_secs(5);
        while gateway.identity().is_none() {
            assert!(Instant::now() < deadline, "boot learned from the snapshot");
            std::thread::sleep(Duration::from_millis(10));
        }
        (engine, gateway)
    }

    fn send(&self, message: &ServerMessage) {
        write_message(&mut *self.writer.lock().unwrap(), message).unwrap();
    }

    /// Writes `frames` on a separate thread; the returned channel yields the encoded byte
    /// total once every frame reached the socket (the client reader kept reading).
    fn burst(&self, frames: Vec<ServerMessage>) -> Receiver<usize> {
        let writer = self.writer.clone();
        let (done_tx, done_rx) = sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = 0usize;
            for message in frames {
                bytes += encode_message(&message).unwrap().len();
                if write_message(&mut *writer.lock().unwrap(), &message).is_err() {
                    return;
                }
            }
            let _ = done_tx.send(bytes);
        });
        done_rx
    }

    /// Next client message matching `pick`; the others are appended to `seen`.
    fn expect<T>(
        &self,
        what: &str,
        seen: &mut Vec<ClientMessage>,
        pick: impl Fn(&ClientMessage) -> Option<T>,
    ) -> T {
        let inbox = self.inbox.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = inbox
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("engine never received {what}"));
            if let Some(found) = pick(&message) {
                seen.push(message);
                return found;
            }
            seen.push(message);
        }
    }

    /// Client messages received within `window`.
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

    /// Answers the next endpoint request of `method` for the boot, returning its id.
    fn answer(&self, seen: &mut Vec<ClientMessage>, method: &str, result: Value) -> String {
        let (boot, request) = self.expect("endpoint request", seen, |m| match m {
            ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                Some((boot_id.clone(), request.clone()))
            }
            _ => None,
        });
        let value: Value = serde_json::from_str(&request).unwrap();
        let id = value["id"].as_str().unwrap().to_owned();
        assert_eq!(
            (boot.as_str(), value["method"].as_str()),
            (BOOT, Some(method))
        );
        self.send(&ServerMessage::ClientShellEndpointResponseChunk {
            boot_id: boot,
            request_id: id.clone(),
            final_chunk: true,
            data: json!({ "id": id, "result": result })
                .to_string()
                .into_bytes(),
        });
        id
    }
}

fn target(gateway: &SshGateway) -> QualifiedTarget {
    let live = gateway.identity().expect("live identity");
    QualifiedTarget {
        endpoint: live.endpoint,
        session: live.session,
        connection_generation: live.connection_generation,
        boot_id: live.boot_id,
        workspace_id: None,
        pane_id: PANE.into(),
    }
}

fn text(s: &str) -> Vec<ClientPaneInputEvent> {
    vec![ClientPaneInputEvent::TextCommit(s.into())]
}

/// Texts of every pane input the engine received.
fn inputs(messages: &[ClientMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            ClientMessage::ClientShellPaneInput { pane_id, events } => {
                assert_eq!(pane_id, PANE);
                Some(
                    events
                        .iter()
                        .map(|e| match e {
                            ClientPaneInputEvent::TextCommit(t) => t.clone(),
                            other => format!("{other:?}"),
                        })
                        .collect::<String>(),
                )
            }
            _ => None,
        })
        .collect()
}

fn resizes(messages: &[ClientMessage]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m, ClientMessage::ClientShellResize { .. }))
        .count()
}

/// Applies the initial surface (revision 1).
fn initial_surface(events: &Receiver<GatewayEvent>) -> FrameStore {
    let mut store = FrameStore::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while store.revision() != Some(1) {
        assert!(Instant::now() < deadline, "initial surface");
        if let Ok(GatewayEvent::Surface(frame)) = events.recv_timeout(Duration::from_millis(50)) {
            store.apply_full(*frame).unwrap();
        }
    }
    assert!(store.input_allowed());
    store
}

/// Sends the full-surface request of the episode, answers it with `revision`/`depois` and
/// waits until the store reconciles. Returns the client messages the engine saw meanwhile.
fn reconcile(
    engine: &Engine,
    events: &Receiver<GatewayEvent>,
    store: &mut FrameStore,
    revision: u64,
) -> Vec<ClientMessage> {
    let mut seen = Vec::new();
    engine.expect("full-surface request", &mut seen, |m| {
        matches!(m, ClientMessage::ClientShellResize { .. }).then_some(())
    });
    engine.send(&ServerMessage::PaneSurface(full(revision, "depois")));
    let deadline = Instant::now() + Duration::from_secs(5);
    while store.revision() != Some(revision) {
        assert!(Instant::now() < deadline, "reconciling full surface");
        if let Ok(GatewayEvent::Surface(frame)) = events.recv_timeout(Duration::from_millis(50)) {
            store.apply_full(*frame).unwrap();
        }
    }
    assert!(
        store.input_allowed(),
        "input unblocked after reconciliation"
    );
    assert_eq!(row_text(store, 0), "depois");
    seen
}

// --- contracts ------------------------------------------------------------------------------

/// Would catch: the SSH queue bounded only by slots (frames beyond 8 MiB queued), an overflow
/// notice that only arrives when a later frame is read (a burst that ends leaves the surface
/// silently wrong), more than one full-surface request, or input accepted before the burst
/// dropped or replayed by the recovery.
#[test]
fn ssh_burst_ending_beyond_8_mib_signals_one_resync_without_a_later_frame_and_keeps_input_once() {
    let (engine, mut gateway) = Engine::ssh(&["pane.focus"]);
    let events = gateway.take_events().expect("event receiver");
    let mut store = initial_surface(&events);

    // Input accepted before the burst.
    gateway
        .send_input(&target(&gateway), text("durante"))
        .unwrap();
    let mut seen = Vec::new();
    engine.expect("accepted input", &mut seen, |m| {
        matches!(m, ClientMessage::ClientShellPaneInput { .. }).then_some(())
    });

    // Consumer paused (receiver alive): ~12 MiB of full frames, then the burst ends.
    let frame_bytes = 1_500_000usize;
    let mut frames = Vec::new();
    let mut planned = 0usize;
    while planned < 12 * 1024 * 1024 {
        let message = ServerMessage::PaneSurface(big_full(2 + frames.len() as u64, frame_bytes));
        planned += encode_message(&message).unwrap().len();
        frames.push(message);
    }
    let count = frames.len();
    let sent = engine
        .burst(frames)
        .recv_timeout(Duration::from_secs(20))
        .expect("reader kept reading the socket while paused");
    assert!(sent >= 12 * 1024 * 1024, "premise: burst beyond 8 MiB");
    assert!(count < SLOTS, "premise: fewer frames than slots");
    std::thread::sleep(Duration::from_millis(500));

    // Resume. No frame follows the burst: the notice must already be queued.
    let mut accepted = Vec::new();
    let mut accepted_bytes = 0usize;
    let mut notices = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_millis(700)) {
            Ok(GatewayEvent::Surface(frame)) => {
                accepted_bytes += encode_message(&ServerMessage::PaneSurface((*frame).clone()))
                    .unwrap()
                    .len();
                accepted.push(frame.surface_revision);
                if notices.is_empty() {
                    store.apply_full(*frame).unwrap();
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
    assert_eq!(notices.len(), 1, "one overflow notice: {notices:?}");
    assert!(
        accepted_bytes <= BUDGET,
        "queued frames bounded by 8 MiB: {accepted_bytes} bytes in {accepted:?}"
    );
    assert!(!accepted.is_empty());
    assert_eq!(
        accepted,
        (2..2 + accepted.len() as u64).collect::<Vec<_>>(),
        "accepted frames in order without gaps"
    );
    assert_eq!(
        accepted.len() + notices[0],
        count,
        "every frame delivered or counted"
    );
    assert!(!store.input_allowed(), "input blocked until reconciled");

    seen.extend(reconcile(&engine, &events, &mut store, 100));
    gateway.send_input(&target(&gateway), text("apos")).unwrap();
    engine.expect("input after reconciliation", &mut seen, |m| {
        (inputs(std::slice::from_ref(m)) == ["apos"]).then_some(())
    });
    seen.extend(engine.quiet(Duration::from_millis(400)));
    assert_eq!(resizes(&seen), 1, "exactly one full-surface request");
    assert_eq!(
        inputs(&seen),
        ["durante", "apos"],
        "accepted input reaches the engine once, never replayed"
    );
}

/// Would catch: the SSH reader blocking on a full queue for a control event (endpoint replies
/// stop being read while the consumer is paused), control events lost or reordered around the
/// notice, a notice that waits for another frame, stale frames after the notice, more than one
/// full-surface request, input replayed, or `detach` not completing while the receiver is
/// alive and paused.
#[test]
fn ssh_saturated_512_slots_with_controls_keeps_reader_live_resyncs_once_and_detaches_while_paused()
{
    const BURST: u64 = 1000;
    let (engine, mut gateway) = Engine::ssh(&["pane.focus"]);
    let events = gateway.take_events().expect("event receiver");
    let mut store = initial_surface(&events);
    gateway
        .send_input(&target(&gateway), text("durante"))
        .unwrap();
    let mut seen = Vec::new();
    engine.expect("accepted input", &mut seen, |m| {
        matches!(m, ClientMessage::ClientShellPaneInput { .. }).then_some(())
    });

    let burst_then_controls = |first: u64| {
        let mut messages: Vec<ServerMessage> = (first..first + BURST)
            .map(|revision| ServerMessage::PaneSurface(full(revision, "rajada")))
            .collect();
        messages.extend((1..=3).map(|n| ServerMessage::ClientShellError {
            message: format!("erro-{n}"),
        }));
        messages
    };

    // Consumer paused: more small frames than slots, far below the byte budget, then controls.
    let bytes = engine
        .burst(burst_then_controls(2))
        .recv_timeout(Duration::from_secs(10))
        .expect("reader kept reading the socket with the slots full");
    assert!(bytes < BUDGET, "premise: below the byte budget");
    assert!(BURST as usize > SLOTS, "premise: more frames than slots");
    std::thread::sleep(Duration::from_millis(300));

    // Still paused: an endpoint reply is read and correlated.
    let lane = gateway.endpoint_lane().expect("connected lane");
    let started = Instant::now();
    let reply = std::thread::scope(|scope| {
        let engine_side = scope.spawn(|| {
            let mut local = Vec::new();
            engine.answer(&mut local, "pane.focus", json!({"focado": PANE}));
            local
        });
        let reply = lane.with_timeout(Duration::from_secs(4)).request(
            BOOT,
            "pane.focus",
            json!({"pane_id": PANE}),
        );
        seen.extend(engine_side.join().unwrap());
        reply
    });
    assert_eq!(
        reply.unwrap(),
        json!({"focado": PANE}),
        "reply read while paused"
    );
    assert!(started.elapsed() < Duration::from_secs(3));

    // Resume: frames in order, one notice for the episode, then the controls in order.
    let mut accepted = Vec::new();
    let mut notices = Vec::new();
    let mut controls = Vec::new();
    let mut frames_after_notice = 0usize;
    loop {
        match events.recv_timeout(Duration::from_millis(700)) {
            Ok(GatewayEvent::Surface(frame)) => {
                if notices.is_empty() {
                    accepted.push(frame.surface_revision);
                    store.apply_full(*frame).unwrap();
                } else {
                    frames_after_notice += 1;
                }
            }
            Ok(GatewayEvent::QueueOverflow { dropped_frames }) => {
                notices.push(dropped_frames);
                store.mark_stale(StaleReason::QueueOverflow);
                if store.take_recovery_request() {
                    gateway.resize(geometry()).unwrap();
                }
            }
            Ok(GatewayEvent::ShellError(message)) => {
                assert_eq!(
                    notices.len(),
                    1,
                    "controls after the notice, not lost before"
                );
                controls.push(message);
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
        "accepted frames in order without gaps"
    );
    assert_eq!(accepted.len() + notices[0], BURST as usize);
    assert_eq!(
        frames_after_notice, 0,
        "no stale burst frame after the notice"
    );
    assert_eq!(controls, ["erro-1", "erro-2", "erro-3"]);
    assert!(!store.input_allowed(), "input blocked until reconciled");

    seen.extend(reconcile(&engine, &events, &mut store, 2000));
    seen.extend(engine.quiet(Duration::from_millis(400)));
    assert_eq!(resizes(&seen), 1, "exactly one full-surface request");

    // Saturate again and detach with the receiver alive and paused.
    engine
        .burst(burst_then_controls(3000))
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
    engine.expect("detach", &mut seen, |m| {
        matches!(m, ClientMessage::Detach).then_some(())
    });
    seen.extend(engine.quiet(Duration::from_millis(300)));
    assert_eq!(
        inputs(&seen),
        ["durante"],
        "accepted input reaches the engine once, never replayed"
    );
    // The paused consumer drains what was queued (bounded), then sees the stream end.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut drained = 0usize;
    loop {
        assert!(Instant::now() < deadline, "stream ends after detach");
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(_) => drained += 1,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    assert!(
        drained <= SLOTS + 2 * 64 + 1,
        "drained a bounded queue: {drained}"
    );
    drop(gateway);
}
