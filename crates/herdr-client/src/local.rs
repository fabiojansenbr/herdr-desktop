//! Local endpoint transport over the engine's client socket.
//!
//! Unix: filesystem socket (`GenericFilePath`); Windows: named pipe (`GenericNamespaced`),
//! both exactly as `ipc::connect_local_stream` in the engine. Only the Linux host was
//! exercised in 001; the Windows branch is compile-gated and unproven until 008.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::Stream as LocalStream;
use interprocess::TryClone as _;

use herdr_protocol::endpoint::{
    EndpointClientHello, EndpointServerWelcome, ENDPOINT_HELLO_KIND, ENDPOINT_SNAPSHOT_KIND,
    ENDPOINT_WELCOME_KIND,
};
use herdr_protocol::wire::{
    ClientClipboardImageTarget, ClientMessage, ClientPaneInputEvent, ClientShellSnapshot,
    ServerMessage, SERVER_MESSAGE_MAX_TAG,
};
use herdr_protocol::{
    decode_message, peek_tag, read_frame, write_message, FramingError, MAX_FRAME_SIZE,
};

/// Incremental length-prefixed frame reader that tolerates recv timeouts mid-frame.
struct FrameReader {
    buf: Vec<u8>,
    filled: usize,
}

enum ReadStep {
    Frame(Vec<u8>),
    Timeout,
    Closed,
    Failed(FramingError),
}

impl FrameReader {
    fn new() -> Self {
        Self {
            buf: vec![0u8; 64 * 1024],
            filled: 0,
        }
    }

    fn take_frame(&mut self) -> Result<Option<Vec<u8>>, FramingError> {
        if self.filled < 4 {
            return Ok(None);
        }
        let claimed =
            u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if claimed > MAX_FRAME_SIZE {
            return Err(FramingError::Oversized {
                claimed,
                max: MAX_FRAME_SIZE,
            });
        }
        let total = 4 + claimed;
        if self.filled < total {
            if self.buf.len() < total {
                self.buf.resize(total, 0);
            }
            return Ok(None);
        }
        let frame = self.buf[4..total].to_vec();
        self.buf.copy_within(total..self.filled, 0);
        self.filled -= total;
        Ok(Some(frame))
    }

    fn step(&mut self, stream: &mut LocalStream) -> ReadStep {
        match self.take_frame() {
            Ok(Some(frame)) => return ReadStep::Frame(frame),
            Ok(None) => {}
            Err(error) => return ReadStep::Failed(error),
        }
        if self.filled == self.buf.len() {
            self.buf.resize(self.buf.len() * 2, 0);
        }
        match stream.read(&mut self.buf[self.filled..]) {
            Ok(0) => ReadStep::Closed,
            Ok(n) => {
                self.filled += n;
                match self.take_frame() {
                    Ok(Some(frame)) => ReadStep::Frame(frame),
                    Ok(None) => ReadStep::Timeout,
                    Err(error) => ReadStep::Failed(error),
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                ReadStep::Timeout
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => ReadStep::Timeout,
            Err(error) => ReadStep::Failed(FramingError::Io(error)),
        }
    }
}

use crate::api::ApiClient;
use crate::contracts::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry, LOCAL_ENDPOINT,
};
use crate::event_queue::{event_queue, EventSender};
use crate::session::{SessionName, SessionPaths};

/// Handshake read budget for a local socket (engine uses 5 s).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded event queue shared with the SSH bridge; the reader never blocks on it.
pub use crate::event_queue::EventStream as GatewayEvents;
pub use crate::event_queue::PRESENTATION_QUEUE_BUDGET;
/// While the held control backlog is full the reader stops reading the socket (server
/// backpressure) but stays interruptible.
const BACKLOG_POLL: Duration = Duration::from_millis(10);
const ENDPOINT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Largest aggregated endpoint reply accepted for one request.
pub const ENDPOINT_RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
/// Process-wide sequence: request ids never repeat, across requests and connections.
static ENDPOINT_REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);
/// The reader wakes at this cadence to observe the stop flag without losing partial frames.
const READER_POLL: Duration = Duration::from_millis(250);

pub fn connect_local_stream(path: &Path) -> io::Result<LocalStream> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::{prelude::*, GenericFilePath};
        let name = path.to_fs_name::<GenericFilePath>()?;
        LocalStream::connect(name)
    }
    #[cfg(windows)]
    {
        use interprocess::local_socket::{prelude::*, GenericNamespaced};
        let name = path.to_string_lossy().to_string();
        let name = name.to_ns_name::<GenericNamespaced>()?;
        LocalStream::connect(name)
    }
}

pub(crate) fn set_stream_timeouts(stream: &LocalStream, timeout: Duration) -> io::Result<()> {
    for result in [
        stream.set_send_timeout(Some(timeout)),
        stream.set_recv_timeout(Some(timeout)),
    ] {
        match result {
            Ok(()) => {}
            #[cfg(windows)]
            Err(err) if err.kind() == io::ErrorKind::Unsupported => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// True when something accepts a connection at `path` (a stale socket file returns false).
pub fn probe_socket(path: &Path) -> bool {
    #[cfg(unix)]
    if !path.exists() {
        return false;
    }
    connect_local_stream(path).is_ok()
}

struct Connection {
    /// Shared with endpoint lanes; `detach` takes the stream so a kept lane cannot hold the
    /// connection open.
    writer: Arc<Mutex<Option<LocalStream>>>,
    reader_handle: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
}

/// State shared between the reader thread and the gateway.
struct Shared {
    endpoint: Arc<EndpointCorrelator>,
}

struct PendingRequest {
    request_id: String,
    /// Boot the request was issued for; a reply or snapshot of another boot fails it.
    boot_id: String,
    body: Vec<u8>,
    limit: usize,
    reply: SyncSender<Result<Vec<u8>, RuntimeError>>,
}

#[derive(Default)]
struct CorrelatorState {
    boot_id: Option<String>,
    pending: Option<PendingRequest>,
    /// Set once the connection is gone; later requests are refused before writing.
    closed: bool,
}

/// Correlation of endpoint command requests and response chunks for one connection: one
/// request in flight, a unique id per request, replies matched by id and boot, bounded
/// aggregation, and the pending request failed at once when the boot changes or the
/// connection ends. Transport-independent: the SSH bridge reuses it.
pub struct EndpointCorrelator {
    endpoint: String,
    generation: u64,
    state: Mutex<CorrelatorState>,
    /// Serializes requesters: at most one request in flight per connection.
    serial: Mutex<()>,
}

impl EndpointCorrelator {
    pub fn new(endpoint: impl Into<String>, generation: u64) -> Self {
        Self {
            endpoint: endpoint.into(),
            generation,
            state: Mutex::new(CorrelatorState::default()),
            serial: Mutex::new(()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, CorrelatorState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn fail(&self, pending: PendingRequest, code: &str, message: &str, retryable: bool) {
        let mut error = RuntimeError::new(code, message).with_endpoint(self.endpoint.clone());
        if retryable {
            error = error.retryable();
        }
        let _ = pending.reply.try_send(Err(error));
    }

    /// Boot currently served by the connection.
    pub fn boot_id(&self) -> Option<String> {
        self.state().boot_id.clone()
    }

    /// A snapshot announced `boot_id`. A pending request of another boot fails at once: its
    /// result is unknown.
    pub fn observe_boot(&self, boot_id: &str) {
        let mut state = self.state();
        state.boot_id = Some(boot_id.to_owned());
        if state.pending.as_ref().is_some_and(|p| p.boot_id != boot_id) {
            let pending = state.pending.take().expect("checked");
            drop(state);
            self.fail(
                pending,
                "endpoint_boot_changed",
                "the server restarted during the request; the result is unknown",
                false,
            );
        }
    }

    /// Learns the boot from a surface only when no snapshot announced one yet.
    pub fn observe_boot_if_unknown(&self, boot_id: &str) {
        let mut state = self.state();
        if state.boot_id.is_none() {
            state.boot_id = Some(boot_id.to_owned());
        }
    }

    /// One response chunk. Chunks of other ids (late replies of finished requests, foreign
    /// ids) are ignored. Returns the complete body when this chunk completed the request.
    pub fn chunk(
        &self,
        boot_id: &str,
        request_id: &str,
        final_chunk: bool,
        data: &[u8],
    ) -> Option<Vec<u8>> {
        let mut state = self.state();
        let pending = state.pending.as_mut()?;
        if pending.request_id != request_id {
            return None;
        }
        if pending.boot_id != boot_id {
            let pending = state.pending.take().expect("checked");
            drop(state);
            self.fail(
                pending,
                "endpoint_boot_changed",
                "the response came from another server boot; the result is unknown",
                false,
            );
            return None;
        }
        if pending.body.len().saturating_add(data.len()) > pending.limit {
            let pending = state.pending.take().expect("checked");
            drop(state);
            self.fail(
                pending,
                "response_too_large",
                "the endpoint response exceeded the limit; the result is unknown",
                false,
            );
            return None;
        }
        pending.body.extend_from_slice(data);
        if !final_chunk {
            return None;
        }
        let done = state.pending.take().expect("checked");
        drop(state);
        let _ = done.reply.try_send(Ok(done.body.clone()));
        Some(done.body)
    }

    /// The connection ended: the pending request fails now and later requests are refused.
    pub fn close(&self) {
        let pending = {
            let mut state = self.state();
            state.closed = true;
            state.pending.take()
        };
        if let Some(pending) = pending {
            self.fail(
                pending,
                "connection_lost",
                "the connection was lost during the request; the result is unknown",
                true,
            );
        }
    }

    /// Issues one request for `boot_id` through `send` and waits for its correlated reply.
    /// Nothing is written when the connection is closed or serves another boot.
    pub fn request(
        &self,
        boot_id: &str,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
        limit: usize,
        send: impl FnOnce(ClientMessage) -> Result<(), RuntimeError>,
    ) -> Result<serde_json::Value, RuntimeError> {
        self.request_guarded(boot_id, method, params, timeout, limit, || Ok(()), send)
    }

    /// [`Self::request`] with an admission: `admit` runs once this requester holds the lane's
    /// turn (after any request ahead of it was answered or failed), with no lock of the
    /// correlator's state held and no I/O of its own. When it refuses, its error is returned and
    /// nothing is written. It is not atomic with the write: after it accepts, the state lock
    /// (closed/boot/pending) and `send` (writer) are still taken before the bytes go out.
    #[allow(clippy::too_many_arguments)]
    pub fn request_guarded(
        &self,
        boot_id: &str,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
        limit: usize,
        admit: impl FnOnce() -> Result<(), RuntimeError>,
        send: impl FnOnce(ClientMessage) -> Result<(), RuntimeError>,
    ) -> Result<serde_json::Value, RuntimeError> {
        let _serial = self.serial.lock().unwrap_or_else(|p| p.into_inner());
        admit()?;
        let endpoint = self.endpoint.clone();
        let request_id = format!(
            "desktop-endpoint:{}:{}",
            self.generation,
            ENDPOINT_REQUEST_SEQ.fetch_add(1, Ordering::AcqRel)
        );
        let (reply_tx, reply_rx) = sync_channel(1);
        {
            let mut state = self.state();
            if state.closed {
                return Err(RuntimeError::new(
                    "not_connected",
                    "disconnected from the Herdr server",
                )
                .retryable()
                .with_endpoint(endpoint));
            }
            match state.boot_id.as_deref() {
                None => {
                    return Err(RuntimeError::new(
                        "boot_unknown",
                        "the server identity is not established yet",
                    )
                    .retryable()
                    .with_endpoint(endpoint))
                }
                Some(live) if live != boot_id => {
                    return Err(RuntimeError::new(
                        "target_boot_stale",
                        "the server restarted; select the target again",
                    )
                    .with_endpoint(endpoint))
                }
                Some(_) => {}
            }
            if state.pending.is_some() {
                return Err(RuntimeError::new(
                    "request_in_flight",
                    "a request is already in flight on this endpoint",
                )
                .retryable()
                .with_endpoint(endpoint));
            }
            state.pending = Some(PendingRequest {
                request_id: request_id.clone(),
                boot_id: boot_id.to_owned(),
                body: Vec::new(),
                limit,
                reply: reply_tx,
            });
        }
        let clear = || {
            let mut state = self.state();
            if state
                .pending
                .as_ref()
                .is_some_and(|p| p.request_id == request_id)
            {
                state.pending = None;
            }
        };
        let request =
            serde_json::json!({ "id": request_id, "method": method, "params": params }).to_string();
        if let Err(error) = send(ClientMessage::ClientShellEndpointRequest {
            boot_id: boot_id.to_owned(),
            request,
        }) {
            clear();
            return Err(error);
        }
        let body = match reply_rx.recv_timeout(timeout) {
            Ok(Ok(body)) => body,
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                clear();
                return Err(
                    RuntimeError::new("timeout", "the endpoint did not answer in time")
                        .retryable()
                        .with_endpoint(endpoint),
                );
            }
        };
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
            RuntimeError::new("protocol_error", "invalid endpoint response")
                .with_endpoint(endpoint.clone())
        })?;
        if let Some(error) = value.get("error") {
            let code = error
                .get("code")
                .and_then(|c| c.as_str())
                .unwrap_or("endpoint_error");
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("endpoint error");
            return Err(RuntimeError::new(code, message).with_endpoint(endpoint));
        }
        Ok(value
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}

/// Endpoint command lane of one local connection, cloneable out of the gateway so a caller
/// can wait for a reply without holding any lock of the gateway's owner.
#[derive(Clone)]
pub struct LocalEndpointLane {
    writer: Arc<Mutex<Option<LocalStream>>>,
    correlator: Arc<EndpointCorrelator>,
    methods: Arc<Vec<String>>,
    timeout: Duration,
    limit: usize,
}

impl LocalEndpointLane {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_response_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Methods announced by the server's welcome.
    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    /// One announced command for `boot_id`, correlated with its reply.
    pub fn request(
        &self,
        boot_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        self.request_admitted(boot_id, method, params, &|| Ok(()))
    }

    /// [`Self::request`] whose `admit` runs once this requester holds the lane's turn (after the
    /// request ahead of it finished) and before the write; a refusal writes nothing. After an
    /// accepted admission the correlator state and the writer are still acquired before the
    /// bytes go out: facts admitted there (focus, attachment) may change in that residual window.
    pub fn request_admitted(
        &self,
        boot_id: &str,
        method: &str,
        params: serde_json::Value,
        admit: &dyn Fn() -> Result<(), RuntimeError>,
    ) -> Result<serde_json::Value, RuntimeError> {
        if !self.methods.iter().any(|m| m == method) {
            return Err(RuntimeError::new(
                "unsupported_method",
                format!("the {method} method is not available on this machine"),
            )
            .with_endpoint(LOCAL_ENDPOINT));
        }
        self.correlator.request_guarded(
            boot_id,
            method,
            params,
            self.timeout,
            self.limit,
            admit,
            |message| write_to(&self.writer, &message),
        )
    }
}

fn write_to(
    writer: &Mutex<Option<LocalStream>>,
    message: &ClientMessage,
) -> Result<(), RuntimeError> {
    let mut writer = writer.lock().map_err(|_| {
        RuntimeError::new(
            "gateway_poisoned",
            "the gateway's internal state is invalid",
        )
    })?;
    let Some(stream) = writer.as_mut() else {
        return Err(
            RuntimeError::new("not_connected", "disconnected from the Herdr server")
                .retryable()
                .with_endpoint(LOCAL_ENDPOINT),
        );
    };
    write_message(stream, message)
        .map_err(|error| framing_error(error, "could not send to the server"))
}

/// Local gateway for one named session.
pub struct LocalGateway {
    session: SessionName,
    paths: SessionPaths,
    api: ApiClient,
    generation: AtomicU64,
    geometry: Mutex<Option<SurfaceGeometry>>,
    connection: Option<Connection>,
    events: Option<GatewayEvents>,
    negotiated: Option<Negotiated>,
}

impl LocalGateway {
    pub fn new(config_dir: &Path, session: SessionName) -> Self {
        let paths = SessionPaths::for_session(config_dir, &session);
        let api = ApiClient::new(&paths.api_socket, LOCAL_ENDPOINT);
        Self {
            session,
            paths,
            api,
            generation: AtomicU64::new(0),
            geometry: Mutex::new(None),
            connection: None,
            events: None,
            negotiated: None,
        }
    }

    pub fn session(&self) -> &SessionName {
        &self.session
    }

    pub fn paths(&self) -> &SessionPaths {
        &self.paths
    }

    pub fn client_socket(&self) -> &PathBuf {
        &self.paths.client_socket
    }

    pub fn api(&self) -> &ApiClient {
        &self.api
    }

    pub fn negotiated(&self) -> Option<&Negotiated> {
        self.negotiated.as_ref()
    }

    pub fn connection_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Takes the typed event stream (once per connection).
    pub fn take_event_stream(&mut self) -> Option<GatewayEvents> {
        self.events.take()
    }

    fn send(&self, message: &ClientMessage) -> Result<(), RuntimeError> {
        let Some(connection) = self.connection.as_ref() else {
            return Err(
                RuntimeError::new("not_connected", "disconnected from the Herdr server")
                    .retryable()
                    .with_endpoint(LOCAL_ENDPOINT),
            );
        };
        write_to(&connection.writer, message)
    }

    fn current_boot_id(&self) -> Option<String> {
        self.connection
            .as_ref()
            .and_then(|c| c.shared.endpoint.boot_id())
    }

    /// Endpoint command lane of the current connection.
    pub fn endpoint_lane(&self) -> Option<LocalEndpointLane> {
        let connection = self.connection.as_ref()?;
        let negotiated = self.negotiated.as_ref()?;
        Some(LocalEndpointLane {
            writer: connection.writer.clone(),
            correlator: connection.shared.endpoint.clone(),
            methods: Arc::new(negotiated.methods.clone()),
            timeout: ENDPOINT_REQUEST_TIMEOUT,
            limit: ENDPOINT_RESPONSE_LIMIT,
        })
    }
}

fn framing_error(error: FramingError, context: &'static str) -> RuntimeError {
    match error {
        FramingError::Io(io) => {
            RuntimeError::from_io_kind(io.kind(), context).with_endpoint(LOCAL_ENDPOINT)
        }
        FramingError::UnexpectedEof => {
            RuntimeError::from_io_kind(io::ErrorKind::UnexpectedEof, context)
                .with_endpoint(LOCAL_ENDPOINT)
        }
        FramingError::Oversized { .. } => RuntimeError::new("frame_oversized", context)
            .retryable()
            .with_endpoint(LOCAL_ENDPOINT),
        FramingError::Bincode(_) => {
            RuntimeError::new("protocol_error", context).with_endpoint(LOCAL_ENDPOINT)
        }
    }
}

impl RuntimeGateway for LocalGateway {
    fn endpoint(&self) -> &str {
        LOCAL_ENDPOINT
    }

    fn identity(&self) -> Option<LiveIdentity> {
        let boot_id = self.current_boot_id()?;
        Some(LiveIdentity {
            endpoint: LOCAL_ENDPOINT.into(),
            session: self.session.as_str().to_owned(),
            connection_generation: self.connection_generation(),
            boot_id,
        })
    }

    fn connect(&mut self, options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        self.detach();
        let mut stream = connect_local_stream(&self.paths.client_socket).map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "the Herdr server is unavailable")
                .with_endpoint(LOCAL_ENDPOINT)
        })?;
        stream.set_nonblocking(false).map_err(|e| {
            RuntimeError::from_io_kind(e.kind(), "local socket").with_endpoint(LOCAL_ENDPOINT)
        })?;
        set_stream_timeouts(&stream, HANDSHAKE_TIMEOUT).map_err(|e| {
            RuntimeError::from_io_kind(e.kind(), "local socket").with_endpoint(LOCAL_ENDPOINT)
        })?;

        let geometry = options.geometry;
        let mut hello = EndpointClientHello::generation_one(
            geometry.surface_size(),
            geometry.cell_width_px,
            geometry.cell_height_px,
        );
        hello.surface_active = options.surface_active;
        let hello_json = serde_json::to_string(&hello).map_err(|_| {
            RuntimeError::new("serialization_error", "could not serialize the hello")
        })?;
        write_message(
            &mut stream,
            &ClientMessage::EndpointControl {
                kind: ENDPOINT_HELLO_KIND.into(),
                data: hello_json,
            },
        )
        .map_err(|e| framing_error(e, "handshake failed"))?;

        let welcome_frame = read_frame(&mut stream, MAX_FRAME_SIZE)
            .map_err(|e| framing_error(e, "handshake failed"))?;
        let welcome_message: ServerMessage =
            decode_message(&welcome_frame).map_err(|e| framing_error(e, "invalid welcome"))?;
        let ServerMessage::EndpointControl { kind, data } = welcome_message else {
            return Err(RuntimeError::new(
                "endpoint_unsupported",
                "the server does not speak the stable endpoint protocol; update Herdr",
            )
            .with_endpoint(LOCAL_ENDPOINT));
        };
        if kind != ENDPOINT_WELCOME_KIND {
            return Err(
                RuntimeError::new("protocol_error", "expected endpoint.welcome.v1")
                    .with_endpoint(LOCAL_ENDPOINT),
            );
        }
        let welcome: EndpointServerWelcome = serde_json::from_str(&data).map_err(|_| {
            RuntimeError::new("protocol_error", "invalid endpoint welcome")
                .with_endpoint(LOCAL_ENDPOINT)
        })?;
        if let Some(error) = welcome.error.as_ref() {
            return Err(RuntimeError::new(
                format!("handshake_{}", error.code),
                error.message.clone(),
            )
            .with_endpoint(LOCAL_ENDPOINT));
        }
        if !welcome.is_generation_one_core() {
            return Err(RuntimeError::new(
                "handshake_no_common_core",
                "the server does not offer the generation 1 core; update Herdr",
            )
            .with_endpoint(LOCAL_ENDPOINT));
        }
        stream.set_recv_timeout(None).map_err(|e| {
            RuntimeError::from_io_kind(e.kind(), "local socket").with_endpoint(LOCAL_ENDPOINT)
        })?;

        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let reader_stream = stream.try_clone().map_err(|e| {
            RuntimeError::from_io_kind(e.kind(), "local socket").with_endpoint(LOCAL_ENDPOINT)
        })?;
        let shared = Arc::new(Shared {
            endpoint: Arc::new(EndpointCorrelator::new(LOCAL_ENDPOINT, generation)),
        });
        let (tx, events) = event_queue();
        let stop = Arc::new(AtomicBool::new(false));
        let reader_handle = {
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("herdr-desktop-endpoint-reader".into())
                .spawn(move || reader_loop(reader_stream, tx, shared, stop))
                .map_err(|e| RuntimeError::from_io_kind(e.kind(), "reader thread"))?
        };
        *self.geometry.lock().expect("geometry lock") = Some(geometry);
        self.connection = Some(Connection {
            writer: Arc::new(Mutex::new(Some(stream))),
            reader_handle: Some(reader_handle),
            stop,
            shared: shared.clone(),
        });
        self.events = Some(events);
        let identity = LiveIdentity {
            endpoint: LOCAL_ENDPOINT.into(),
            session: self.session.as_str().to_owned(),
            connection_generation: generation,
            boot_id: String::new(),
        };
        let negotiated = Negotiated::from_welcome(identity, &welcome);
        self.negotiated = Some(negotiated.clone());
        Ok(negotiated)
    }

    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        // Typed stream preferred; adapt to a plain receiver for trait consumers.
        self.events.take()?.into_receiver()
    }

    fn api_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        self.api.request(method, params)
    }

    fn endpoint_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        let Some(lane) = self.endpoint_lane() else {
            return Err(
                RuntimeError::new("not_connected", "disconnected from the Herdr server")
                    .retryable(),
            );
        };
        if !lane.methods().iter().any(|m| m == method) {
            return lane.request("", method, params);
        }
        let boot_id = self.current_boot_id().ok_or_else(|| {
            RuntimeError::new("boot_unknown", "the server identity is not established yet")
                .retryable()
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
        })?;
        target.validate(&live)?;
        if events.is_empty() {
            return Ok(());
        }
        self.send(&ClientMessage::ClientShellPaneInput {
            pane_id: target.pane_id.clone(),
            events,
        })
    }

    fn send_clipboard_image(
        &self,
        target: &QualifiedTarget,
        extension: &str,
        data: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let live = self.identity().ok_or_else(|| {
            RuntimeError::new("boot_unknown", "the server identity is not established yet")
                .retryable()
        })?;
        target.validate(&live)?;
        if data.is_empty() {
            return Ok(());
        }
        self.send(&ClientMessage::ClipboardImage {
            target: ClientClipboardImageTarget::Pane(target.pane_id.clone()),
            extension: extension.to_owned(),
            data,
        })
    }

    fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        self.send(&ClientMessage::ClientShellResize {
            cell_width_px: geometry.cell_width_px,
            cell_height_px: geometry.cell_height_px,
            surface_size: geometry.surface_size(),
            pixel_mouse: false,
        })?;
        *self.geometry.lock().expect("geometry lock") = Some(geometry);
        Ok(())
    }

    fn set_focus(&self, focused: bool) -> Result<(), RuntimeError> {
        self.send(&ClientMessage::ClientShellFocus { focused })
    }

    fn set_host_theme(
        &self,
        updates: &[herdr_protocol::wire::ClientHostThemeUpdate],
    ) -> Result<(), RuntimeError> {
        for update in updates {
            self.send(&ClientMessage::ClientShellHostTheme {
                update: update.clone(),
            })?;
        }
        Ok(())
    }

    fn detach(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            connection.stop.store(true, Ordering::Release);
            connection.shared.endpoint.close();
            if let Ok(mut writer) = connection.writer.lock() {
                if let Some(mut stream) = writer.take() {
                    let _ = write_message(&mut stream, &ClientMessage::Detach);
                    let _ = stream.flush();
                }
            }
            // Dropping the stream closes the socket even if a lane is still held; the reader
            // thread then observes EOF or the stop flag.
            drop(connection.writer);
            self.events = None;
            if let Some(handle) = connection.reader_handle.take() {
                let _ = handle.join();
            }
        }
        self.events = None;
        self.negotiated = None;
    }

    fn is_connected(&self) -> bool {
        self.connection.is_some()
    }
}

impl Drop for LocalGateway {
    fn drop(&mut self) {
        self.detach();
    }
}

impl LocalGateway {
    /// Current geometry, for resync requests.
    pub fn geometry(&self) -> Option<SurfaceGeometry> {
        *self.geometry.lock().expect("geometry lock")
    }
}

fn reader_loop(
    mut stream: LocalStream,
    events: EventSender,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) {
    let _ = stream.set_recv_timeout(Some(READER_POLL));
    // Whatever ends the reader fails the pending endpoint request at once.
    struct CloseOnExit(Arc<EndpointCorrelator>);
    impl Drop for CloseOnExit {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = CloseOnExit(shared.endpoint.clone());
    // Terminal event, delivered after everything held unless the gateway is detaching.
    let finish = |error: RuntimeError| {
        if !stop.load(Ordering::Acquire) {
            events.control(GatewayEvent::Disconnected(error));
        }
    };
    let mut reader = FrameReader::new();
    loop {
        if stop.load(Ordering::Acquire) || !events.flush() {
            return;
        }
        if events.backlogged() {
            std::thread::sleep(BACKLOG_POLL);
            continue;
        }
        let frame = match reader.step(&mut stream) {
            ReadStep::Frame(frame) => frame,
            ReadStep::Timeout => continue,
            ReadStep::Closed => {
                finish(framing_error(
                    FramingError::UnexpectedEof,
                    "the connection to the server was closed",
                ));
                return;
            }
            ReadStep::Failed(error) => {
                finish(framing_error(
                    error,
                    "the connection to the server was closed",
                ));
                return;
            }
        };
        let frame_len = frame.len();
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
                finish(RuntimeError::new(
                    "protocol_error",
                    "invalid server message",
                ));
                return;
            }
        };
        let event = match message {
            ServerMessage::EndpointControl { kind, data } => {
                if kind == ENDPOINT_SNAPSHOT_KIND {
                    match serde_json::from_str::<ClientShellSnapshot>(&data) {
                        Ok(snapshot) => {
                            shared.endpoint.observe_boot(&snapshot.boot_id);
                            GatewayEvent::Snapshot(Box::new(snapshot))
                        }
                        Err(_) => GatewayEvent::ShellError("invalid endpoint snapshot".into()),
                    }
                } else {
                    // Unknown named controls are optional and ignored.
                    continue;
                }
            }
            ServerMessage::ClientShellSnapshot(snapshot) => {
                shared.endpoint.observe_boot(&snapshot.boot_id);
                GatewayEvent::Snapshot(snapshot)
            }
            ServerMessage::PaneSurface(surface) => {
                shared.endpoint.observe_boot_if_unknown(&surface.boot_id);
                GatewayEvent::Surface(Box::new(surface))
            }
            ServerMessage::PaneSurfacePatch(patch) => GatewayEvent::Patch(Box::new(patch)),
            ServerMessage::ClientShellEndpointResponseChunk {
                boot_id,
                request_id,
                final_chunk,
                data,
            } => {
                if let Some(body) = shared
                    .endpoint
                    .chunk(&boot_id, &request_id, final_chunk, &data)
                {
                    // Informational copy; the requester already has the reply, so it is
                    // dropped rather than waiting for a slot.
                    if !events.offer(GatewayEvent::EndpointResponse {
                        boot_id,
                        request_id,
                        body,
                    }) {
                        return;
                    }
                }
                continue;
            }
            ServerMessage::ClientShellError { message } => GatewayEvent::ShellError(message),
            ServerMessage::ServerShutdown { reason } => GatewayEvent::Shutdown(reason),
            ServerMessage::ClientShellKeyboardReportAll { enabled } => {
                GatewayEvent::KeyboardReportAll(enabled)
            }
            ServerMessage::MouseCapture {
                enabled,
                sgr_pixels,
            } => GatewayEvent::MouseCapture {
                enabled,
                sgr_pixels,
            },
            // Presentation-only or direct-terminal lanes the desktop does not render yet.
            ServerMessage::Welcome { .. }
            | ServerMessage::Terminal(_)
            | ServerMessage::Graphics { .. }
            | ServerMessage::Notify { .. }
            | ServerMessage::Clipboard { .. }
            | ServerMessage::WindowTitle { .. }
            | ServerMessage::ReloadSoundConfig
            | ServerMessage::TerminalBell { .. }
            | ServerMessage::GraphicsFile { .. }
            | ServerMessage::GraphicsTransmissionRetired { .. }
            | ServerMessage::SemanticNotification(_)
            | ServerMessage::DirectTerminalKeyboardProtocol { .. } => continue,
        };
        // Frames never overtake held events and never wait for a slot.
        let delivered = if matches!(event, GatewayEvent::Surface(_) | GatewayEvent::Patch(_)) {
            events.frame(frame_len, event)
        } else {
            events.control(event)
        };
        if !delivered {
            return;
        }
    }
}

// Keep `Read`/`Write` imported for the interprocess stream trait bounds on all platforms.
#[allow(dead_code)]
fn _assert_stream_traits<S: Read + Write>(_s: &S) {}

#[cfg(all(test, unix))]
mod host_theme_tests {
    use super::*;
    use herdr_protocol::wire::{
        ClientHostAppearance, ClientHostColor as Color, ClientHostDefaultColorKind as Kind,
        ClientHostThemeUpdate as Update,
    };

    #[test]
    fn local_host_theme_sends_four_ordered_messages_and_nothing_else() {
        let (client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let stream = LocalStream::from(interprocess::os::unix::uds_local_socket::Stream::from(
            client,
        ));
        let dir = tempfile::tempdir().unwrap();
        let mut gateway =
            LocalGateway::new(dir.path(), SessionName::parse("spec066-fake").unwrap());
        gateway.connection = Some(Connection {
            writer: Arc::new(Mutex::new(Some(stream))),
            reader_handle: None,
            stop: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Shared {
                endpoint: Arc::new(EndpointCorrelator::new("local", 1)),
            }),
        });
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
        gateway.set_host_theme(&updates).unwrap();
        for update in updates {
            let frame = read_frame(&mut server, MAX_FRAME_SIZE).unwrap();
            let message: ClientMessage = decode_message(&frame).unwrap();
            assert_eq!(message, ClientMessage::ClientShellHostTheme { update });
        }
        let error = server.read(&mut [0]).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
    }
}
