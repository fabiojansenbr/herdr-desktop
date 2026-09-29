//! Remote files over SFTP (spec 006): read-only explorer, text snapshots and diffs per SSH host.
//!
//! - Transport: `ssh … -T [-p port] -s -- <target> sftp` built by the 003 seam
//!   (`connections::ssh_options::build_sftp_subsystem`), so host keys, non-interactive
//!   authentication and test isolation are exactly those of the Herdr lanes. The codec is
//!   `openssh-sftp-client` over those pipes, behind the guards in [`guard`] (inbound frames
//!   limited to 5..=1048576 bytes before the codec) and [`channel`] (explicit read offsets, DATA
//!   no larger than requested, sanitized classification).
//! - Identity: every operation carries a [`RemoteFileTarget`] (endpoint, session, connection
//!   generation, boot) validated against what the connection hub reports ([`RemoteLinks`])
//!   **before any I/O**. A host that is not online is refused: cached content is never live
//!   state. Channels are keyed by that identity; a renewed connection closes the old channel and
//!   its paging cursors, and a reload runs on a new channel. Nothing falls back to the local
//!   filesystem; URIs of another provider or host are refused.
//! - Roots: only roots authorized by the desktop for the endpoint are served. Paths are POSIX
//!   strings (independent of the client OS); `..` and symlinks are resolved by the server and the
//!   answer must stay inside a root.
//! - Operations: an IPC envelope with an operation id, a total deadline of 10000 ms and explicit
//!   cancellation. Expiring or cancelling ends only that host's SFTP channel (terminals, engine
//!   and other hosts are untouched) and never replaces what the WebView already shows. Each host
//!   has its own queue of at most 32 pending operations; the 33rd is refused without affecting
//!   the others. Nothing is retried or replayed automatically.
//! - Read-only MVP: [`FileCapabilities::write`] is false, [`FileProvider::write`] keeps the
//!   default refusal and no save command exists.
//! - SFTP availability (TASK-006-05): a working SSH terminal does not imply that the host's sshd
//!   offers the `sftp` subsystem. A missing subsystem is reported as `sftp_unavailable` on the
//!   file resource only; the Herdr lanes of the same host keep working.
//!
//! Tauri commands (`COMMANDS`) are registered by the window composition (spec 007); until then
//! they are compiled and exercised by `src-tauri/tests/files_remote.rs`.

pub mod channel;
pub mod errors;
pub mod guard;
pub mod posix;
pub mod text;
pub mod transport;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use herdr_client::{
    FileCapabilities, FileEntry, FileProvider, FileStat, FileUri, LiveIdentity, QualifiedTarget,
    RuntimeError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use crate::connections::commands::ConnectionsState;
use crate::connections::hub::HostKind;
use crate::connections::ssh_options::{
    build_sftp_subsystem, IsolatedSshConfig, OpenSshCommand, SshIdentity,
};
use crate::connections::state::LinkPhase;

pub use self::channel::ChannelInfo;
use self::channel::{Channel, ChannelKey, OpError};
pub use self::guard::{MAX_FRAME_BYTES, MIN_FRAME_BYTES};
pub use self::transport::SftpTransport;
pub use crate::files::local::{FilePage, TextSnapshot, MAX_PAGE_ENTRIES, MAX_TEXT_BYTES};

/// Provider id used in every [`FileUri`] this module serves (`host` = endpoint profile id).
pub const PROVIDER_ID: &str = "sftp";
/// Total deadline of one file operation, including opening the channel.
pub const OPERATION_DEADLINE: Duration = Duration::from_millis(10_000);
/// Pending operations per host channel; the next one is refused.
pub const MAX_PENDING_OPERATIONS: usize = 32;
/// Longest operation id accepted from the WebView.
pub const MAX_OPERATION_ID: usize = 64;

// ---------------------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------------------

/// Connection an operation is addressed to (the qualified target without a pane).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFileTarget {
    pub endpoint: String,
    pub session: String,
    pub connection_generation: u64,
    pub boot_id: String,
}

/// Connection (and channel) that produced a page or snapshot. The WebView compares it with the
/// host's live identity to mark cached content as stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteConnectionStamp {
    pub endpoint: String,
    pub session: String,
    pub connection_generation: u64,
    pub boot_id: String,
    pub channel: u64,
}

/// One page (≤ 128 entries, opaque cursor) of a remote directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFilePage {
    #[serde(flatten)]
    pub page: FilePage,
    pub connection: RemoteConnectionStamp,
}

/// Immutable text read from a remote file (same semantics as the local [`TextSnapshot`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTextSnapshot {
    #[serde(flatten)]
    pub snapshot: TextSnapshot,
    pub connection: RemoteConnectionStamp,
}

/// What the WebView may know about one SSH host for files: no target string, key or option.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteHostDto {
    pub endpoint: String,
    pub label: String,
    pub session: String,
    pub phase: LinkPhase,
    pub phase_label: String,
    pub online: bool,
    pub connection_generation: Option<u64>,
    pub boot_id: Option<String>,
    pub roots: Vec<String>,
    pub provider: String,
    pub capabilities: FileCapabilities,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteHostsView {
    pub revision: u64,
    pub hosts: Vec<RemoteHostDto>,
}

// ---------------------------------------------------------------------------------------
// Seams: connection links and transports
// ---------------------------------------------------------------------------------------

/// One SSH host as reported by the connection layer (spec 003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHostLink {
    pub ssh: SshIdentity,
    pub label: String,
    pub phase: LinkPhase,
    /// Present while the connection has a negotiated identity.
    pub live: Option<LiveIdentity>,
}

impl RemoteHostLink {
    fn online(&self) -> Option<&LiveIdentity> {
        match self.phase {
            LinkPhase::Online => self.live.as_ref(),
            _ => None,
        }
    }
}

/// Source of host identity (the connection hub in the app; fakes in tests).
pub trait RemoteLinks: Send + Sync {
    fn link(&self, endpoint: &str) -> Result<RemoteHostLink, RuntimeError>;
    fn links(&self) -> Vec<RemoteHostLink>;
    fn revision(&self) -> u64;
    /// Blocks until the revision differs from `since` or `timeout` elapses.
    fn wait_changed(&self, since: u64, timeout: Duration) -> u64;
}

impl RemoteLinks for ConnectionsState {
    fn link(&self, endpoint: &str) -> Result<RemoteHostLink, RuntimeError> {
        self.links()
            .into_iter()
            .find(|l| l.ssh.endpoint_id() == endpoint)
            .ok_or_else(|| errors::host_unknown(endpoint))
    }

    fn links(&self) -> Vec<RemoteHostLink> {
        let view = self.view();
        view.profiles
            .iter()
            .filter_map(|profile| {
                let host = view
                    .hub
                    .hosts
                    .iter()
                    .find(|h| h.endpoint == profile.id.as_str() && h.kind == HostKind::Ssh)?;
                let live = match (host.generation, host.boot_id.as_ref()) {
                    (Some(generation), Some(boot)) => Some(LiveIdentity {
                        endpoint: host.endpoint.clone(),
                        session: host.session.clone(),
                        connection_generation: generation,
                        boot_id: boot.clone(),
                    }),
                    _ => None,
                };
                Some(RemoteHostLink {
                    ssh: profile.identity(),
                    label: profile.label.clone(),
                    phase: host.phase,
                    live,
                })
            })
            .collect()
    }

    fn revision(&self) -> u64 {
        self.hub().revision()
    }

    fn wait_changed(&self, since: u64, timeout: Duration) -> u64 {
        self.hub().wait_changed(since, timeout)
    }
}

/// Opens the byte transport of a new channel. Called inside the provider's runtime.
pub trait SftpConnector: Send + Sync {
    fn open(&self, link: &RemoteHostLink) -> Result<SftpTransport, RuntimeError>;
}

/// Production connector: OpenSSH subprocess with the 003 options (and test isolation, set by
/// the process that builds the state, never by the WebView).
#[derive(Debug, Clone, Default)]
pub struct OpenSshSftpConnector {
    isolated: Option<IsolatedSshConfig>,
}

impl OpenSshSftpConnector {
    pub fn new(isolated: Option<IsolatedSshConfig>) -> Self {
        Self { isolated }
    }

    /// The exact OpenSSH invocation of a channel.
    pub fn command(&self, identity: &SshIdentity) -> OpenSshCommand {
        build_sftp_subsystem(identity, self.isolated.as_ref())
    }
}

impl SftpConnector for OpenSshSftpConnector {
    fn open(&self, link: &RemoteHostLink) -> Result<SftpTransport, RuntimeError> {
        let built = self.command(&link.ssh);
        let mut command = tokio::process::Command::new(&built.program);
        command.args(&built.args);
        SftpTransport::spawn(command).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                crate::connections::failure::classify_spawn_error(link.ssh.endpoint_id(), &error)
                    .error()
                    .clone()
            }
            _ => errors::connection_lost(link.ssh.endpoint_id()),
        })
    }
}

// ---------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------

pub struct RemoteFilesConfig {
    pub links: Arc<dyn RemoteLinks>,
    pub connector: Arc<dyn SftpConnector>,
    /// Authorized project roots per endpoint (absolute POSIX paths on that host).
    pub roots: BTreeMap<String, Vec<String>>,
    pub deadline: Duration,
}

#[derive(Default)]
struct Cancel {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.flag.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

struct Slot {
    permits: Arc<Semaphore>,
    channel: tokio::sync::Mutex<Option<Arc<Channel>>>,
    current: Mutex<Option<Arc<Channel>>>,
}

impl Slot {
    fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(MAX_PENDING_OPERATIONS)),
            channel: tokio::sync::Mutex::new(None),
            current: Mutex::new(None),
        }
    }

    fn current(&self) -> Option<Arc<Channel>> {
        self.current.lock().expect("slot").clone()
    }

    fn forget(&self, id: u64) {
        let mut current = self.current.lock().expect("slot");
        if current.as_ref().is_some_and(|c| c.id == id) {
            *current = None;
        }
    }

    /// Closes a channel of another identity (renewed connection) or a dead one.
    async fn invalidate_stale(&self, key: &ChannelKey) {
        let mut guard = self.channel.lock().await;
        let stale = guard.as_ref().is_some_and(|c| c.key != *key || c.is_dead());
        if stale {
            if let Some(old) = guard.take() {
                self.forget(old.id);
                old.close_with(errors::channel_renewed(&key.endpoint)).await;
            }
        }
    }

    async fn channel_for(
        &self,
        inner: &Inner,
        key: &ChannelKey,
        link: &RemoteHostLink,
    ) -> Result<Arc<Channel>, RuntimeError> {
        let mut guard = self.channel.lock().await;
        if let Some(channel) = guard.as_ref() {
            if channel.key == *key && !channel.is_dead() {
                return Ok(channel.clone());
            }
        }
        if let Some(old) = guard.take() {
            self.forget(old.id);
            old.close_with(errors::channel_renewed(&key.endpoint)).await;
        }
        let transport = inner.connector.open(link)?;
        let id = inner.next_channel.fetch_add(1, Ordering::SeqCst);
        let channel = Channel::open(id, key.clone(), transport).await?;
        *guard = Some(channel.clone());
        *self.current.lock().expect("slot") = Some(channel.clone());
        Ok(channel)
    }

    /// Ends the channel an interrupted operation was using (only this host's channel).
    async fn abort(&self, reason: RuntimeError) {
        let channel = self.current();
        if let Some(channel) = channel {
            self.forget(channel.id);
            channel.close_with(reason).await;
        }
    }
}

struct Inner {
    links: Arc<dyn RemoteLinks>,
    connector: Arc<dyn SftpConnector>,
    roots: Mutex<BTreeMap<String, Vec<String>>>,
    deadline: Duration,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    ops: Mutex<HashMap<String, Arc<Cancel>>>,
    next_channel: AtomicU64,
}

enum Request {
    List {
        uri: FileUri,
        cursor: Option<String>,
    },
    Read {
        uri: FileUri,
    },
    ReadBytes {
        uri: FileUri,
        max: u64,
    },
    Stat {
        uri: FileUri,
    },
}

enum Response {
    Page(RemoteFilePage),
    Text(RemoteTextSnapshot),
    Bytes(Vec<u8>),
    Stat(FileStat),
}

struct OpRegistration {
    inner: Arc<Inner>,
    id: String,
}

impl Drop for OpRegistration {
    fn drop(&mut self) {
        self.inner.ops.lock().expect("ops").remove(&self.id);
    }
}

fn valid_op_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_OPERATION_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

impl Inner {
    fn slot(&self, endpoint: &str) -> Arc<Slot> {
        self.slots
            .lock()
            .expect("slots")
            .entry(endpoint.to_owned())
            .or_insert_with(|| Arc::new(Slot::new()))
            .clone()
    }

    fn roots_of(&self, endpoint: &str) -> Vec<String> {
        self.roots
            .lock()
            .expect("roots")
            .get(endpoint)
            .cloned()
            .unwrap_or_default()
    }
}

async fn execute(
    inner: Arc<Inner>,
    op_id: String,
    target: RemoteFileTarget,
    request: Request,
) -> Result<Response, RuntimeError> {
    let endpoint = target.endpoint.clone();
    if !valid_op_id(&op_id) {
        return Err(errors::operation_id_invalid(&endpoint));
    }
    let cancel = Arc::new(Cancel::default());
    let _registration = {
        let mut ops = inner.ops.lock().expect("ops");
        if ops.contains_key(&op_id) {
            return Err(errors::operation_duplicate(&endpoint));
        }
        ops.insert(op_id.clone(), cancel.clone());
        OpRegistration {
            inner: inner.clone(),
            id: op_id.clone(),
        }
    };

    // Identity before any I/O.
    let link = inner
        .links
        .link(&endpoint)
        .map_err(|_| errors::host_unknown(&endpoint))?;
    let Some(live) = link.online().cloned() else {
        return Err(errors::host_unavailable(&endpoint));
    };
    QualifiedTarget {
        endpoint: target.endpoint.clone(),
        session: target.session.clone(),
        connection_generation: target.connection_generation,
        boot_id: target.boot_id.clone(),
        workspace_id: None,
        pane_id: String::new(),
    }
    .validate(&live)?;
    let uri = match &request {
        Request::List { uri, .. }
        | Request::Read { uri }
        | Request::ReadBytes { uri, .. }
        | Request::Stat { uri } => uri,
    };
    if uri.provider != PROVIDER_ID {
        return Err(errors::uri_invalid(&endpoint));
    }
    if uri.host.as_deref() != Some(endpoint.as_str()) {
        return Err(errors::host_mismatch(&endpoint));
    }
    if !posix::is_supported(&uri.path) {
        return Err(errors::path_unsupported(&endpoint));
    }
    let roots = inner.roots_of(&endpoint);
    if roots.is_empty() {
        return Err(errors::root_not_authorized(&endpoint));
    }
    if let Request::List {
        cursor: Some(cursor),
        ..
    } = &request
    {
        if channel::cursor_channel(cursor).is_none() {
            return Err(errors::invalid_cursor(&endpoint));
        }
    }

    let slot = inner.slot(&endpoint);
    let _permit: OwnedSemaphorePermit = slot
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| errors::queue_full(&endpoint, MAX_PENDING_OPERATIONS))?;
    let key = ChannelKey {
        endpoint: live.endpoint.clone(),
        session: live.session.clone(),
        generation: live.connection_generation,
        boot_id: live.boot_id.clone(),
    };
    let stamp = |channel: &Channel| RemoteConnectionStamp {
        endpoint: key.endpoint.clone(),
        session: key.session.clone(),
        connection_generation: key.generation,
        boot_id: key.boot_id.clone(),
        channel: channel.id,
    };

    let work = async {
        slot.invalidate_stale(&key).await;
        if let Request::List {
            cursor: Some(cursor),
            ..
        } = &request
        {
            let live_channel = slot.current().map(|c| c.id);
            if channel::cursor_channel(cursor) != live_channel {
                return Err(errors::cursor_stale(&endpoint));
            }
        }
        let channel = slot.channel_for(&inner, &key, &link).await?;
        let result = match &request {
            Request::List { uri, cursor } => {
                let listed = match cursor {
                    Some(token) => channel.list_next(uri, token).await,
                    None => channel.list_first(uri, &roots).await,
                };
                listed.map(|(entries, next_cursor)| {
                    Response::Page(RemoteFilePage {
                        page: FilePage {
                            uri: uri.clone(),
                            entries,
                            next_cursor,
                        },
                        connection: stamp(&channel),
                    })
                })
            }
            Request::Read { uri } => match channel.read_bytes(uri, &roots, MAX_TEXT_BYTES).await {
                Ok(read) => match text::decode(&read.bytes) {
                    Some(decoded) => Ok(Response::Text(RemoteTextSnapshot {
                        snapshot: TextSnapshot {
                            id: uuid::Uuid::new_v4().to_string(),
                            uri: uri.clone(),
                            content: decoded.content,
                            bom: decoded.bom,
                            eol: decoded.eol,
                            size: read.bytes.len() as u64,
                            modified_unix_ms: read
                                .metadata
                                .modified()
                                .map(|t| t.as_duration().as_millis() as u64),
                        },
                        connection: stamp(&channel),
                    })),
                    None => Err(OpError::Resource(errors::binary_unsupported(&endpoint))),
                },
                Err(error) => Err(error),
            },
            Request::ReadBytes { uri, max } => channel
                .read_bytes(uri, &roots, (*max).min(MAX_TEXT_BYTES))
                .await
                .map(|read| Response::Bytes(read.bytes)),
            Request::Stat { uri } => channel.stat(uri, &roots).await.map(Response::Stat),
        };
        match result {
            Ok(response) => Ok(response),
            Err(OpError::Resource(error)) => Err(error),
            Err(OpError::Protocol) => {
                slot.forget(channel.id);
                Err(channel.close_with(errors::protocol(&endpoint)).await)
            }
            Err(OpError::Sftp(error)) => match channel::resource_error(&error, &endpoint) {
                Some(resource) => Err(resource),
                None => {
                    slot.forget(channel.id);
                    Err(channel.fail(error).await)
                }
            },
        }
    };

    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(errors::cancelled(&endpoint)),
        _ = tokio::time::sleep(inner.deadline) => Err(errors::timeout(&endpoint, inner.deadline)),
        result = work => return result,
    };
    // Cancelled or expired: end this host's channel only; nothing is replayed.
    let reason = match &outcome {
        Err(error) => error.clone(),
        Ok(_) => unreachable!("only interruptions reach this point"),
    };
    slot.abort(reason).await;
    outcome
}

/// Root authorization of a [`RemoteFilesState`], without access to files or channels.
#[derive(Clone)]
pub struct RemoteRootAuthorizer {
    inner: Arc<Inner>,
}

impl RemoteRootAuthorizer {
    /// Authorizes one more project root for `endpoint` (absolute POSIX path on that host);
    /// repeating a root is a no-op.
    pub fn authorize_root(&self, endpoint: &str, root: &str) -> Result<(), RuntimeError> {
        if !posix::is_supported(root) {
            return Err(errors::path_unsupported(endpoint));
        }
        let mut roots = self.inner.roots.lock().expect("roots");
        let list = roots.entry(endpoint.to_owned()).or_default();
        if !list.iter().any(|r| r == root) {
            list.push(root.to_owned());
        }
        Ok(())
    }
}

/// Managed state of the remote files feature.
pub struct RemoteFilesState {
    inner: Arc<Inner>,
    handle: tokio::runtime::Handle,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for RemoteFilesState {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl RemoteFilesState {
    pub fn new(config: RemoteFilesConfig) -> Result<Self, RuntimeError> {
        for (endpoint, roots) in &config.roots {
            if roots.iter().any(|root| !posix::is_supported(root)) {
                return Err(errors::path_unsupported(endpoint));
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("herdr-desktop-sftp")
            .enable_all()
            .build()
            .map_err(|_| errors::runtime_failed())?;
        Ok(Self {
            handle: runtime.handle().clone(),
            runtime: Some(runtime),
            inner: Arc::new(Inner {
                links: config.links,
                connector: config.connector,
                roots: Mutex::new(config.roots),
                deadline: config.deadline,
                slots: Mutex::new(HashMap::new()),
                ops: Mutex::new(HashMap::new()),
                next_channel: AtomicU64::new(1),
            }),
        })
    }

    /// Authorizes one more project root for `endpoint` (absolute POSIX path on that host).
    pub fn authorize_root(&self, endpoint: &str, root: &str) -> Result<(), RuntimeError> {
        self.root_authorizer().authorize_root(endpoint, root)
    }

    /// Shareable handle that only authorizes project roots of this state (for the project
    /// service of the window). It shares the root table, not the transport or its runtime.
    pub fn root_authorizer(&self) -> RemoteRootAuthorizer {
        RemoteRootAuthorizer {
            inner: self.inner.clone(),
        }
    }

    async fn run(
        &self,
        op_id: &str,
        target: &RemoteFileTarget,
        request: Request,
    ) -> Result<Response, RuntimeError> {
        let endpoint = target.endpoint.clone();
        self.handle
            .spawn(execute(
                self.inner.clone(),
                op_id.to_owned(),
                target.clone(),
                request,
            ))
            .await
            .map_err(|_| errors::connection_lost(&endpoint))?
    }

    pub async fn list(
        &self,
        op_id: &str,
        target: &RemoteFileTarget,
        uri: &FileUri,
        cursor: Option<&str>,
    ) -> Result<RemoteFilePage, RuntimeError> {
        let request = Request::List {
            uri: uri.clone(),
            cursor: cursor.map(str::to_owned),
        };
        match self.run(op_id, target, request).await? {
            Response::Page(page) => Ok(page),
            _ => Err(errors::protocol(&target.endpoint)),
        }
    }

    pub async fn read(
        &self,
        op_id: &str,
        target: &RemoteFileTarget,
        uri: &FileUri,
    ) -> Result<RemoteTextSnapshot, RuntimeError> {
        match self
            .run(op_id, target, Request::Read { uri: uri.clone() })
            .await?
        {
            Response::Text(snapshot) => Ok(snapshot),
            _ => Err(errors::protocol(&target.endpoint)),
        }
    }

    pub async fn stat(
        &self,
        op_id: &str,
        target: &RemoteFileTarget,
        uri: &FileUri,
    ) -> Result<FileStat, RuntimeError> {
        match self
            .run(op_id, target, Request::Stat { uri: uri.clone() })
            .await?
        {
            Response::Stat(stat) => Ok(stat),
            _ => Err(errors::protocol(&target.endpoint)),
        }
    }

    /// Cancels a pending operation. `false` when no such operation is pending.
    pub fn cancel(&self, op_id: &str) -> bool {
        match self.inner.ops.lock().expect("ops").get(op_id) {
            Some(cancel) => {
                cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Operations holding a queue place for `endpoint`.
    pub fn pending(&self, endpoint: &str) -> usize {
        let slot = self.inner.slot(endpoint);
        MAX_PENDING_OPERATIONS - slot.permits.available_permits()
    }

    /// Current channel of `endpoint`, if one is open.
    pub fn channel(&self, endpoint: &str) -> Option<ChannelInfo> {
        self.inner.slot(endpoint).current().map(|c| c.info())
    }

    /// Hosts for the explorer. Presentation only: never opens a channel.
    pub fn hosts(&self) -> RemoteHostsView {
        let revision = self.inner.links.revision();
        let hosts = self
            .inner
            .links
            .links()
            .into_iter()
            .map(|link| {
                let endpoint = link.ssh.endpoint_id().to_owned();
                let live = link.online().cloned();
                RemoteHostDto {
                    roots: self.inner.roots_of(&endpoint),
                    label: link.label.clone(),
                    session: link.ssh.session.as_str().to_owned(),
                    phase: link.phase,
                    phase_label: link.phase.label().to_owned(),
                    online: live.is_some(),
                    connection_generation: link.live.as_ref().map(|l| l.connection_generation),
                    boot_id: link.live.as_ref().map(|l| l.boot_id.clone()),
                    provider: PROVIDER_ID.to_owned(),
                    capabilities: remote_capabilities(),
                    endpoint,
                }
            })
            .collect();
        RemoteHostsView { revision, hosts }
    }

    /// Waits (off the async executor) for a host change, then returns the hosts.
    pub async fn watch(&self, revision: u64) -> RemoteHostsView {
        let links = self.inner.links.clone();
        let _ = self
            .handle
            .spawn_blocking(move || links.wait_changed(revision, Duration::from_secs(2)))
            .await;
        self.hosts()
    }

    /// Synchronous [`FileProvider`] bound to one connection (not for use inside async code).
    pub fn provider(&self, target: RemoteFileTarget) -> BoundRemoteProvider {
        BoundRemoteProvider {
            inner: self.inner.clone(),
            handle: self.handle.clone(),
            target,
        }
    }
}

fn remote_capabilities() -> FileCapabilities {
    FileCapabilities {
        list: true,
        read: true,
        stat: true,
        write: false,
    }
}

/// [`FileProvider`] contract of the remote provider for one qualified connection.
pub struct BoundRemoteProvider {
    inner: Arc<Inner>,
    handle: tokio::runtime::Handle,
    target: RemoteFileTarget,
}

impl BoundRemoteProvider {
    fn run(&self, request: Request) -> Result<Response, RuntimeError> {
        let op_id = format!("bound-{}", uuid::Uuid::new_v4().simple());
        let task = self.handle.spawn(execute(
            self.inner.clone(),
            op_id,
            self.target.clone(),
            request,
        ));
        self.handle
            .block_on(task)
            .map_err(|_| errors::connection_lost(&self.target.endpoint))?
    }
}

impl FileProvider for BoundRemoteProvider {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn capabilities(&self) -> FileCapabilities {
        remote_capabilities()
    }

    /// Full listing, page after page (never truncated).
    fn list(&self, dir: &FileUri) -> Result<Vec<FileEntry>, RuntimeError> {
        let mut entries = Vec::new();
        let mut cursor = None;
        loop {
            match self.run(Request::List {
                uri: dir.clone(),
                cursor: cursor.take(),
            })? {
                Response::Page(page) => {
                    entries.extend(page.page.entries);
                    match page.page.next_cursor {
                        Some(next) => cursor = Some(next),
                        None => return Ok(entries),
                    }
                }
                _ => return Err(errors::protocol(&self.target.endpoint)),
            }
        }
    }

    fn read(&self, file: &FileUri, max_bytes: u64) -> Result<Vec<u8>, RuntimeError> {
        match self.run(Request::ReadBytes {
            uri: file.clone(),
            max: max_bytes,
        })? {
            Response::Bytes(bytes) => Ok(bytes),
            _ => Err(errors::protocol(&self.target.endpoint)),
        }
    }

    fn stat(&self, uri: &FileUri) -> Result<FileStat, RuntimeError> {
        match self.run(Request::Stat { uri: uri.clone() })? {
            Response::Stat(stat) => Ok(stat),
            _ => Err(errors::protocol(&self.target.endpoint)),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------------------

/// Commands this module exposes to the WebView (registered by the window composition, 007).
/// None writes: the remote MVP is read-only.
pub const COMMANDS: &[&str] = &[
    "remote_files_hosts",
    "remote_files_watch",
    "remote_files_list",
    "remote_files_read",
    "remote_files_stat",
    "remote_files_cancel",
];

#[tauri::command]
pub fn remote_files_hosts(state: tauri::State<'_, RemoteFilesState>) -> RemoteHostsView {
    state.hosts()
}

#[tauri::command]
pub async fn remote_files_watch(
    state: tauri::State<'_, RemoteFilesState>,
    revision: u64,
) -> Result<RemoteHostsView, RuntimeError> {
    Ok(state.watch(revision).await)
}

#[tauri::command]
pub async fn remote_files_list(
    state: tauri::State<'_, RemoteFilesState>,
    op_id: String,
    target: RemoteFileTarget,
    uri: FileUri,
    cursor: Option<String>,
) -> Result<RemoteFilePage, RuntimeError> {
    state.list(&op_id, &target, &uri, cursor.as_deref()).await
}

#[tauri::command]
pub async fn remote_files_read(
    state: tauri::State<'_, RemoteFilesState>,
    op_id: String,
    target: RemoteFileTarget,
    uri: FileUri,
) -> Result<RemoteTextSnapshot, RuntimeError> {
    state.read(&op_id, &target, &uri).await
}

#[tauri::command]
pub async fn remote_files_stat(
    state: tauri::State<'_, RemoteFilesState>,
    op_id: String,
    target: RemoteFileTarget,
    uri: FileUri,
) -> Result<FileStat, RuntimeError> {
    state.stat(&op_id, &target, &uri).await
}

#[tauri::command]
pub fn remote_files_cancel(state: tauri::State<'_, RemoteFilesState>, op_id: String) -> bool {
    state.cancel(&op_id)
}
