//! Contracts shared by all feature fronts (docs/CONTRATOS.md).

use std::fmt;
use std::sync::mpsc::Receiver;

use serde::{Deserialize, Serialize};

use herdr_protocol::endpoint::EndpointServerWelcome;
use herdr_protocol::wire::{
    ClientHostThemeUpdate, ClientPaneInputEvent, ClientShellSnapshot, ClientSurfaceSize,
    PaneSurfaceFrame, PaneSurfacePatch,
};

/// Profile id of the local endpoint. Remote profiles use their catalog ids.
pub const LOCAL_ENDPOINT: &str = "local";

// ---------------------------------------------------------------------------
// RuntimeError
// ---------------------------------------------------------------------------

/// Error surfaced to the presentation layer. Carries only a stable code, a human
/// message we compose ourselves, retryability and the endpoint id. It never carries
/// credentials, environment values, socket paths or internal stack traces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl RuntimeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            endpoint: None,
        }
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Maps an I/O error to a stable code using only its kind (no OS message, no path).
    pub fn from_io_kind(kind: std::io::ErrorKind, context: &'static str) -> Self {
        use std::io::ErrorKind as K;
        let (code, retryable) = match kind {
            K::NotFound | K::ConnectionRefused => ("server_unavailable", true),
            K::PermissionDenied => ("permission_denied", false),
            K::TimedOut | K::WouldBlock => ("timeout", true),
            K::ConnectionReset | K::BrokenPipe | K::ConnectionAborted | K::UnexpectedEof => {
                ("connection_lost", true)
            }
            K::InvalidData => ("protocol_error", false),
            _ => ("io_error", false),
        };
        let mut error = Self::new(code, format!("{context}: {}", kind_label(kind)));
        error.retryable = retryable;
        error
    }
}

fn kind_label(kind: std::io::ErrorKind) -> &'static str {
    use std::io::ErrorKind as K;
    match kind {
        K::NotFound => "resource not found",
        K::ConnectionRefused => "connection refused",
        K::PermissionDenied => "permission denied",
        K::TimedOut | K::WouldBlock => "timed out",
        K::ConnectionReset | K::BrokenPipe | K::ConnectionAborted | K::UnexpectedEof => {
            "connection closed"
        }
        K::InvalidData => "invalid data",
        _ => "I/O error",
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RuntimeError {}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Identity of one live connection as observed from the handshake and the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveIdentity {
    pub endpoint: String,
    pub session: String,
    pub connection_generation: u64,
    pub boot_id: String,
}

/// Fully qualified action target: endpoint/profile, session, connection generation,
/// boot and the current workspace/pane ids. Every runtime action validates it against
/// the live identity; a mismatch invalidates the target and never falls back to Local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualifiedTarget {
    pub endpoint: String,
    pub session: String,
    pub connection_generation: u64,
    pub boot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub pane_id: String,
}

impl QualifiedTarget {
    pub fn new(
        identity: &LiveIdentity,
        workspace_id: Option<String>,
        pane_id: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: identity.endpoint.clone(),
            session: identity.session.clone(),
            connection_generation: identity.connection_generation,
            boot_id: identity.boot_id.clone(),
            workspace_id,
            pane_id: pane_id.into(),
        }
    }

    /// Validates endpoint, session, generation and boot. Returns the first mismatch.
    pub fn validate(&self, live: &LiveIdentity) -> Result<(), RuntimeError> {
        if self.endpoint != live.endpoint {
            return Err(RuntimeError::new(
                "target_endpoint_mismatch",
                "the target belongs to another endpoint; the action was cancelled",
            )
            .with_endpoint(self.endpoint.clone()));
        }
        if self.session != live.session {
            return Err(RuntimeError::new(
                "target_session_mismatch",
                "the target belongs to another session; the action was cancelled",
            )
            .with_endpoint(self.endpoint.clone()));
        }
        if self.connection_generation != live.connection_generation {
            return Err(RuntimeError::new(
                "target_generation_stale",
                "the connection was renewed; select the target again",
            )
            .with_endpoint(self.endpoint.clone()));
        }
        if self.boot_id != live.boot_id {
            return Err(RuntimeError::new(
                "target_boot_stale",
                "the server restarted; select the target again",
            )
            .with_endpoint(self.endpoint.clone()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RuntimeGateway
// ---------------------------------------------------------------------------

/// Geometry negotiated for the pane surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGeometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

impl SurfaceGeometry {
    pub fn surface_size(&self) -> ClientSurfaceSize {
        ClientSurfaceSize {
            cols: self.cols,
            rows: self.rows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOptions {
    pub geometry: SurfaceGeometry,
    pub surface_active: bool,
}

/// Result of a successful generation-1 negotiation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    pub identity: LiveIdentity,
    pub generation: u32,
    pub server_version: String,
    pub methods: Vec<String>,
    pub capabilities: Vec<String>,
}

impl Negotiated {
    pub fn from_welcome(identity: LiveIdentity, welcome: &EndpointServerWelcome) -> Self {
        Self {
            identity,
            generation: welcome.generation,
            server_version: welcome.server_version.clone(),
            methods: welcome.methods.clone(),
            capabilities: welcome.capabilities.clone(),
        }
    }

    pub fn supports_method(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }
}

/// Events emitted by a gateway's reader. Render events are droppable under queue
/// pressure; control events are not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayEvent {
    Snapshot(Box<ClientShellSnapshot>),
    Surface(Box<PaneSurfaceFrame>),
    Patch(Box<PaneSurfacePatch>),
    EndpointResponse {
        boot_id: String,
        request_id: String,
        body: Vec<u8>,
    },
    ShellError(String),
    Shutdown(Option<String>),
    KeyboardReportAll(bool),
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    /// Frame carried a ServerMessage tag this client does not know; it was skipped.
    Unsupported {
        tag: u32,
    },
    /// Presentation queue exceeded its byte budget; deltas were dropped and a full
    /// surface must be recovered. Input is never dropped by this path.
    QueueOverflow {
        dropped_frames: usize,
    },
    Disconnected(RuntimeError),
}

/// Lifecycle + API request + input + resize. Local and remote adapters implement it.
pub trait RuntimeGateway: Send {
    /// Endpoint profile id ("local" or a catalog id).
    fn endpoint(&self) -> &str;
    /// Live identity once a snapshot established the boot id.
    fn identity(&self) -> Option<LiveIdentity>;
    /// Negotiates endpoint generation 1 and starts the reader. Recoverable failures
    /// (server absent) return `RuntimeError { retryable: true }` and leave the gateway idle.
    fn connect(&mut self, options: ConnectOptions) -> Result<Negotiated, RuntimeError>;
    /// Takes the event receiver (once per connection).
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>>;
    /// JSON API request over the session's API socket (runtime actions).
    fn api_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError>;
    /// Endpoint-lane request (subset of the API announced in the welcome).
    fn endpoint_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError>;
    /// Semantic input to a validated pane target.
    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError>;
    /// One clipboard image (already encoded: png/jpg/gif/webp/bmp) to a validated pane target,
    /// exactly as the TUI bridges local images (`ClientMessage::ClipboardImage`). A gateway that
    /// cannot carry images refuses instead of dropping or re-encoding them.
    fn send_clipboard_image(
        &self,
        target: &QualifiedTarget,
        extension: &str,
        data: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let _ = (target, extension, data);
        Err(RuntimeError::new(
            "clipboard_image_unsupported",
            "this connection does not accept clipboard images; nothing was sent",
        )
        .with_endpoint(self.endpoint().to_owned()))
    }
    fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError>;
    fn set_focus(&self, focused: bool) -> Result<(), RuntimeError>;
    /// Host colors used by the engine for default cells and OSC color queries.
    fn set_host_theme(&self, _updates: &[ClientHostThemeUpdate]) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Detaches without stopping the engine or its processes.
    fn detach(&mut self);
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// FileProvider
// ---------------------------------------------------------------------------

/// URI qualified by provider and host; remote paths are opaque to the local OS.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileUri {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub path: String,
}

impl FileUri {
    pub fn local(path: impl Into<String>) -> Self {
        Self {
            provider: "local".into(),
            host: None,
            path: path.into(),
        }
    }

    pub fn remote(
        provider: impl Into<String>,
        host: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            host: Some(host.into()),
            path: path.into(),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }
}

impl fmt::Display for FileUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Some(host) => write!(f, "{}://{}{}", self.provider, host, self.path),
            None => write!(f, "{}://{}", self.provider, self.path),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileCapabilities {
    pub list: bool,
    pub read: bool,
    pub stat: bool,
    /// Optional. The initial remote provider never announces write.
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub uri: FileUri,
    pub name: String,
    pub kind: FileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    pub uri: FileUri,
    pub kind: FileKind,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<u64>,
    pub read_only: bool,
}

/// list/read/stat with optional write. Implementations are per provider/host.
pub trait FileProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn capabilities(&self) -> FileCapabilities;
    fn list(&self, dir: &FileUri) -> Result<Vec<FileEntry>, RuntimeError>;
    fn read(&self, file: &FileUri, max_bytes: u64) -> Result<Vec<u8>, RuntimeError>;
    fn stat(&self, uri: &FileUri) -> Result<FileStat, RuntimeError>;
    /// Default rejects: providers must opt in and announce `capabilities().write`.
    fn write(&self, file: &FileUri, _contents: &[u8]) -> Result<(), RuntimeError> {
        Err(RuntimeError::new(
            "write_unsupported",
            format!("the {} provider is read-only", self.provider_id()),
        )
        .with_endpoint(file.host.clone().unwrap_or_else(|| LOCAL_ENDPOINT.into())))
    }
}

// ---------------------------------------------------------------------------
// ProjectRef / RuntimeBinding
// ---------------------------------------------------------------------------

/// Durable local project identity. Workspace/pane ids are *not* stored here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRef {
    /// UUID v4, generated locally, stable across reboots.
    pub id: String,
    pub label: String,
    pub endpoint_profile_id: String,
    pub session_name: String,
    /// Root path as seen by the endpoint (opaque for remote endpoints).
    pub root: String,
}

impl ProjectRef {
    pub fn new(
        label: impl Into<String>,
        endpoint_profile_id: impl Into<String>,
        session_name: impl Into<String>,
        root: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.into(),
            endpoint_profile_id: endpoint_profile_id.into(),
            session_name: session_name.into(),
            root: root.into(),
        }
    }
}

/// Ephemeral link between a project and a running workspace. Discard on generation/boot change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeBinding {
    pub project_id: String,
    pub connection_generation: u64,
    pub boot_id: String,
    pub workspace_id: String,
}

impl RuntimeBinding {
    pub fn is_valid_for(&self, live: &LiveIdentity) -> bool {
        self.connection_generation == live.connection_generation && self.boot_id == live.boot_id
    }
}
