//! Tauri commands of the connections feature and the background supervisor that executes
//! connection attempts and drains gateway events. Registering these commands in the window
//! belongs to spec 007; until then they run in the feature harness
//! (`src-tauri/tests/connections.rs`).
//!
//! The WebView sees DTOs only: no sockets, credentials, environment, OpenSSH options or remote
//! commands. Profiles carry a validated target and session; the test-only isolated SSH
//! configuration is injected by the process that builds [`ConnectionsState`].

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use herdr_client::bootstrap::ensure_session_running;
use herdr_client::protocol::wire::{ClientKeyCode, ClientPaneInputEvent};
use herdr_client::{
    ConnectOptions, LocalGateway, QualifiedTarget, RuntimeError, RuntimeGateway, SessionName,
    SurfaceGeometry, LOCAL_ENDPOINT,
};
use serde::Serialize;
use serde_json::Value;

use super::failure::{AttentionReason, ConnectFailure};
use super::hub::{
    ConnectTicket, Connected, EndpointLane, HostHub, HostKind, HostSpec, HubSnapshot,
    VisibilityOutcome,
};
use super::profiles::{
    tui_catalog_path, ImportReport, ProfileStore, SshProfile, SshProfileDraft, SSH_AGENT_AUTH,
};
use super::remote_binary::RemoteBinary;
use super::ssh_options::{IsolatedSshConfig, OpenSshCommand, RemoteHerdrCommand};
use crate::bridge::ssh::{OpenSshRunner, SshConnector, SshRunner};

use crate::theme;

/// Commands this module exposes to the WebView.
pub const COMMANDS: &[&str] = &[
    "connections_list",
    "connections_watch",
    "connection_profile_save",
    "connection_profiles_import",
    "connection_connect",
    "connection_cancel",
    "connection_disconnect",
    "connection_reconnect",
    "connection_remove",
    "connection_send_text",
    "connection_workspaces",
    "connections_set_connect_on_open",
];

const SUPERVISOR_TICK: Duration = Duration::from_millis(100);
const WATCH_TIMEOUT: Duration = Duration::from_secs(2);
const WORKSPACES_TIMEOUT: Duration = Duration::from_secs(15);

/// Literal text plus an optional Enter. Nothing else can be synthesized through this path.
pub fn text_events(text: &str, submit: bool) -> Vec<ClientPaneInputEvent> {
    let mut events = Vec::new();
    if !text.is_empty() {
        events.push(ClientPaneInputEvent::TextCommit(text.to_owned()));
    }
    if submit {
        events.push(ClientPaneInputEvent::key_press(ClientKeyCode::Enter, 0));
    }
    events
}

#[derive(Debug, Clone)]
pub struct ConnectionsConfig {
    /// Desktop preferences directory (holds `connections.json`).
    pub prefs_dir: PathBuf,
    pub herdr_config_dir: PathBuf,
    pub herdr_state_dir: PathBuf,
    /// Local session shown as "This computer" (the sidebar names it with `t()`); `None` = no local host.
    pub local_session: Option<SessionName>,
    /// Zero-config: an absent Local session is started detached and waited for before the
    /// connection attempt (AC-016-01). False for an explicitly named session (previous
    /// behavior: only `session_start` starts it).
    pub local_auto_start: bool,
    /// Engine binary the automatic start uses (`herdr` by default).
    pub herdr_bin: PathBuf,
    /// Test-only isolated SSH configuration (never set by the WebView).
    pub isolated_ssh: Option<IsolatedSshConfig>,
    pub geometry: SurfaceGeometry,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionsView {
    pub hub: HubSnapshot,
    pub profiles: Vec<SshProfile>,
    pub store_error: Option<RuntimeError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportView {
    pub report: ImportReport,
    pub view: ConnectionsView,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDto {
    pub workspace_id: String,
    pub label: String,
}

#[cfg(debug_assertions)]
fn trace_workspaces(hub: &HostHub, endpoint: &str, started: Instant, result: &str) {
    let host = hub
        .spec(endpoint)
        .ok()
        .and_then(|spec| spec.target)
        .unwrap_or_else(|| endpoint.to_owned());
    eprintln!(
        "[conn] {} workspaces {}ms {}",
        host,
        started.elapsed().as_millis(),
        result
    );
}

#[cfg(not(debug_assertions))]
fn trace_workspaces(_hub: &HostHub, _endpoint: &str, _started: Instant, _result: &str) {}

/// Every decision point between the probes and the bridge of one attempt (AC-031-02).
#[cfg(debug_assertions)]
fn trace_attempt(target: &str, step: &str, started: Instant, result: &str) {
    eprintln!(
        "[conn] {} {} {}ms {}",
        target,
        step,
        started.elapsed().as_millis(),
        result
    );
}

#[cfg(not(debug_assertions))]
fn trace_attempt(_target: &str, _step: &str, _started: Instant, _result: &str) {}

/// Refuses an explicit `ssh-agent` choice when the desktop process has no agent socket. The
/// check happens before the profile store is written and before any OpenSSH process starts.
fn ensure_ssh_agent(
    auth: Option<&str>,
    target: &str,
    endpoint: Option<&str>,
) -> Result<(), RuntimeError> {
    let wanted_agent = auth == Some(SSH_AGENT_AUTH);
    let available = std::env::var_os("SSH_AUTH_SOCK").is_some_and(|value| !value.is_empty());
    if !wanted_agent || available {
        return Ok(());
    }
    trace_attempt(
        target,
        "autenticação",
        Instant::now(),
        "ssh-agent is unavailable in this session",
    );
    let mut error = RuntimeError::new(
        "ssh_agent_unavailable",
        "ssh-agent is unavailable in this session",
    );
    if let Some(endpoint) = endpoint {
        error = error.with_endpoint(endpoint);
    }
    Err(error)
}

/// Where the `events.subscribe` stream of one host is opened (`crate::bridge::events`): the
/// session's API socket, or one long-lived `remote-api-bridge` process of the SSH profile.
#[derive(Clone)]
pub enum EventSource {
    Local(herdr_client::api::ApiClient),
    Ssh {
        command: OpenSshCommand,
        runner: Arc<dyn SshRunner>,
    },
}

/// A connection plus the endpoint command lane of the same connection, taken before the
/// gateway is moved into [`Connected`] and installed right after `finish_connect`.
struct Attached {
    connected: Connected,
    lane: Option<Arc<dyn EndpointLane>>,
    /// Capabilities of the negotiated welcome, captured before the gateway type is erased.
    capabilities: Vec<String>,
    /// Server version of the same welcome (spec 010 status bar).
    server_version: String,
    /// Remote binary chosen by discovery (spec 029); `None` for Local.
    remote_binary: Option<RemoteBinary>,
    latency_ms: u64,
}

trait HostConnector: Send + Sync {
    /// `options` carries the hub's current geometry (default when unknown) and the surface mode
    /// of the ticket: a host that is not selected connects with `surface_active=false`.
    fn connect(&self, options: ConnectOptions) -> Result<Attached, ConnectFailure>;
    /// Source of the host's `events.subscribe` stream (same socket/profile as its API lane).
    fn event_source(
        &self,
        isolated: Option<&IsolatedSshConfig>,
        runner: &Arc<dyn SshRunner>,
    ) -> EventSource;
}

struct LocalConnector {
    config_dir: PathBuf,
    session: SessionName,
    /// Starts an absent session detached before connecting (zero-config only).
    auto_start: bool,
    herdr_bin: PathBuf,
}

impl LocalConnector {
    /// The TUI's own bootstrap: start the absent session detached and wait for its client
    /// socket (AC-016-01). The literals of AC-016-02 (`herdr não encontrado no PATH — instale
    /// o Herdr`, `o servidor não respondeu em 10 s`) come straight from `herdr-client`.
    fn prepare_session(&self) -> Result<(), RuntimeError> {
        if !self.auto_start {
            return Ok(());
        }
        let cwd = std::env::current_dir().ok();
        ensure_session_running(
            &self.herdr_bin,
            &self.config_dir,
            &self.session,
            cwd.as_deref(),
            herdr_client::bootstrap::SESSION_START_TIMEOUT,
        )
    }
}

impl HostConnector for LocalConnector {
    fn connect(&self, options: ConnectOptions) -> Result<Attached, ConnectFailure> {
        if let Err(error) = self.prepare_session() {
            // The start failed before any socket existed: retryable, literal cause, and the
            // retry button (and the backoff) repeat this same bootstrap.
            return Err(ConnectFailure::Transient(
                error.retryable().with_endpoint(LOCAL_ENDPOINT),
            ));
        }
        let handshake_start = Instant::now();
        let mut gateway = LocalGateway::new(&self.config_dir, self.session.clone());
        match gateway.connect(options) {
            Ok(negotiated) => {
                let latency_ms = handshake_start.elapsed().as_millis() as u64;
                let capabilities = negotiated.capabilities.clone();
                let server_version = negotiated.server_version.clone();
                let api = Arc::new(gateway.api().clone());
                let lane = gateway
                    .endpoint_lane()
                    .map(|lane| Arc::new(lane) as Arc<dyn EndpointLane>);
                Ok(Attached {
                    connected: Connected {
                        gateway: Box::new(gateway),
                        api,
                    },
                    lane,
                    capabilities,
                    server_version,
                    remote_binary: None,
                    latency_ms,
                })
            }
            Err(error)
                if error.code.starts_with("handshake") || error.code == "endpoint_unsupported" =>
            {
                Err(ConnectFailure::attention(
                    LOCAL_ENDPOINT,
                    AttentionReason::ServerIncompatible,
                ))
            }
            Err(error) => Err(ConnectFailure::Transient(
                error.retryable().with_endpoint(LOCAL_ENDPOINT),
            )),
        }
    }

    fn event_source(
        &self,
        _isolated: Option<&IsolatedSshConfig>,
        _runner: &Arc<dyn SshRunner>,
    ) -> EventSource {
        // The API client of an unconnected gateway: same session socket, nothing opened.
        EventSource::Local(
            LocalGateway::new(&self.config_dir, self.session.clone())
                .api()
                .clone(),
        )
    }
}

struct SshHostConnector {
    connector: SshConnector,
}

impl HostConnector for SshHostConnector {
    fn connect(&self, options: ConnectOptions) -> Result<Attached, ConnectFailure> {
        let gateway = self.connector.connect(options)?;
        let latency_ms = gateway.handshake_latency_ms();
        let api = Arc::new(gateway.api_lane());
        let capabilities = gateway.negotiated().capabilities.clone();
        let server_version = gateway.negotiated().server_version.clone();
        let remote_binary = Some(gateway.remote_binary().clone());
        let lane = gateway
            .endpoint_lane()
            .map(|lane| Arc::new(lane) as Arc<dyn EndpointLane>);
        Ok(Attached {
            connected: Connected {
                gateway: Box::new(gateway),
                api,
            },
            lane,
            capabilities,
            server_version,
            remote_binary,
            latency_ms,
        })
    }

    fn event_source(
        &self,
        _isolated: Option<&IsolatedSshConfig>,
        runner: &Arc<dyn SshRunner>,
    ) -> EventSource {
        EventSource::Ssh {
            // Same lane and same discovered binary as the API client of this connection.
            command: self.connector.command(RemoteHerdrCommand::ApiBridge),
            runner: runner.clone(),
        }
    }
}

struct Shared {
    hub: HostHub,
    config: ConnectionsConfig,
    runner: Arc<dyn SshRunner>,
    store: Mutex<Result<ProfileStore, RuntimeError>>,
    connectors: Mutex<BTreeMap<String, Arc<dyn HostConnector>>>,
    supervisor: AtomicBool,
}

/// Managed state of the connections feature.
#[derive(Clone)]
pub struct ConnectionsState {
    shared: Arc<Shared>,
}

fn ssh_spec(profile: &SshProfile) -> HostSpec {
    HostSpec {
        endpoint: profile.id.as_str().to_owned(),
        label: profile.label.clone(),
        kind: HostKind::Ssh,
        session: profile.session.as_str().to_owned(),
        target: Some(match profile.port {
            Some(port) => format!("{} (port {port})", profile.target.as_str()),
            None => profile.target.as_str().to_owned(),
        }),
        visible: true,
    }
}

impl ConnectionsState {
    /// Registers the local host and the saved SSH profiles; connects nothing.
    pub fn new(config: ConnectionsConfig) -> Self {
        Self::with_runner(config, Arc::new(OpenSshRunner))
    }

    /// Same as [`ConnectionsState::new`] with an explicit OpenSSH runner (test seam of the
    /// composed connector; the window always uses [`OpenSshRunner`]).
    pub fn with_runner(config: ConnectionsConfig, runner: Arc<dyn SshRunner>) -> Self {
        let engine_dirs = [
            config.herdr_config_dir.as_path(),
            config.herdr_state_dir.as_path(),
        ];
        let store = ProfileStore::open(&config.prefs_dir, &engine_dirs);
        let shared = Arc::new(Shared {
            hub: HostHub::new(),
            runner,
            store: Mutex::new(store),
            connectors: Mutex::new(BTreeMap::new()),
            supervisor: AtomicBool::new(false),
            config,
        });
        let state = Self { shared };
        if let Some(session) = state.shared.config.local_session.clone() {
            let _ = state.shared.hub.add_host(HostSpec {
                endpoint: LOCAL_ENDPOINT.into(),
                label: "This computer".into(),
                kind: HostKind::Local,
                session: session.as_str().to_owned(),
                target: None,
                visible: true,
            });
            state.shared.connectors.lock().expect("connectors").insert(
                LOCAL_ENDPOINT.into(),
                Arc::new(LocalConnector {
                    config_dir: state.shared.config.herdr_config_dir.clone(),
                    session,
                    auto_start: state.shared.config.local_auto_start,
                    herdr_bin: state.shared.config.herdr_bin.clone(),
                }),
            );
        }
        let profiles = state
            .shared
            .store
            .lock()
            .expect("store")
            .as_ref()
            .map(|s| s.profiles().to_vec())
            .unwrap_or_default();
        for profile in &profiles {
            state.register(profile);
        }
        state
    }

    pub fn hub(&self) -> &HostHub {
        &self.shared.hub
    }

    fn register(&self, profile: &SshProfile) {
        let spec = ssh_spec(profile);
        if self.shared.hub.add_host(spec.clone()).is_err() {
            let _ = self.shared.hub.update_host(spec);
        }
        self.shared.connectors.lock().expect("connectors").insert(
            profile.id.as_str().to_owned(),
            Arc::new(SshHostConnector {
                connector: SshConnector::new(
                    profile.identity(),
                    self.shared.config.isolated_ssh.clone(),
                    self.shared.runner.clone(),
                ),
            }),
        );
    }

    pub fn view(&self) -> ConnectionsView {
        let store = self.shared.store.lock().expect("store");
        ConnectionsView {
            hub: self.shared.hub.snapshot(Instant::now()),
            profiles: store
                .as_ref()
                .map(|s| s.profiles().to_vec())
                .unwrap_or_default(),
            store_error: store.as_ref().err().cloned(),
        }
    }

    fn ensure_supervisor(&self) {
        if self.shared.supervisor.swap(true, Ordering::AcqRel) {
            return;
        }
        let state = self.clone();
        let _ = std::thread::Builder::new()
            .name("herdr-desktop-connections".into())
            .spawn(move || loop {
                std::thread::sleep(SUPERVISOR_TICK);
                for ticket in state.shared.hub.due_connects(Instant::now()) {
                    state.run_attempt(ticket);
                }
            });
    }

    fn run_attempt(&self, ticket: ConnectTicket) {
        let state = self.clone();
        let _ = std::thread::Builder::new()
            .name("herdr-desktop-connect".into())
            .spawn(move || {
                // Decision point: the connectors map of the hub (never silent, bound by the
                // lock itself, which is only held for a map lookup).
                let lock_started = Instant::now();
                let connector = state
                    .shared
                    .connectors
                    .lock()
                    .expect("connectors")
                    .get(&ticket.endpoint)
                    .cloned();
                let target = state
                    .shared
                    .hub
                    .spec(&ticket.endpoint)
                    .ok()
                    .and_then(|spec| spec.target)
                    .unwrap_or_else(|| ticket.endpoint.clone());
                trace_attempt(
                    &target,
                    "lock do hub",
                    lock_started,
                    if connector.is_some() {
                        "ok"
                    } else {
                        "sem conector"
                    },
                );
                // Decision point: geometry of this attempt. The hub keeps the geometry the
                // WebView reported; without one the configured default is used, and a host that
                // is not selected still connects (surface_active=false).
                let geometry_started = Instant::now();
                let geometry = state
                    .shared
                    .hub
                    .geometry(&ticket.endpoint)
                    .unwrap_or(state.shared.config.geometry);
                trace_attempt(
                    &target,
                    "aguardando geometria",
                    geometry_started,
                    &format!(
                        "{}x{} surface={}",
                        geometry.cols, geometry.rows, ticket.surface_active
                    ),
                );
                let options = ConnectOptions {
                    geometry,
                    surface_active: ticket.surface_active,
                };
                let result = match connector {
                    Some(connector) => connector.connect(options),
                    None => Err(ConnectFailure::transient(
                        &ticket.endpoint,
                        "endpoint_unknown",
                        "the host has no connector",
                    )),
                };
                let (result, lane, negotiated) = match result {
                    Ok(attached) => {
                        let updates =
                            theme::host_theme_updates(theme::omarchy_theme_dir().as_deref());
                        if let Err(error) = attached.connected.gateway.set_host_theme(&updates) {
                            eprintln!("[conn] {} host theme: {error}", ticket.endpoint);
                        }
                        (
                            Ok(attached.connected),
                            attached.lane,
                            Some((
                                attached.capabilities,
                                attached.server_version,
                                attached.remote_binary,
                                attached.latency_ms,
                            )),
                        )
                    }
                    Err(failure) => (Err(failure), None, None),
                };
                let events = state
                    .shared
                    .hub
                    .finish_connect(&ticket, result, Instant::now());
                // Refused by the hub unless this very attempt is the installed connection.
                if let Some(lane) = lane {
                    state
                        .shared
                        .hub
                        .install_endpoint_lane(&ticket.endpoint, ticket.token, lane);
                }
                if let Some((capabilities, server_version, remote_binary, latency_ms)) = negotiated
                {
                    let hub = &state.shared.hub;
                    hub.install_server_version(&ticket.endpoint, ticket.token, server_version);
                    hub.install_latency(&ticket.endpoint, ticket.token, latency_ms);
                    if let Some(binary) = remote_binary {
                        hub.install_remote_binary(&ticket.endpoint, ticket.token, binary);
                    }
                    if hub.install_capabilities(&ticket.endpoint, ticket.token, capabilities) {
                        // A hidden window re-sends its interest to the new connection once; a
                        // failure is not retried and never drops the connection.
                        let _ = hub.restore_surface_interest(&ticket.endpoint, ticket.token);
                    }
                }
                if let Some(renegotiate) = state.shared.hub.settle_visibility(&ticket) {
                    // Visibility changed during the attempt: never keep the old surface mode.
                    state.run_attempt(renegotiate);
                    return;
                }
                let Some(events) = events else {
                    return;
                };
                while let Ok(event) = events.recv() {
                    state.shared.hub.apply_event(
                        &ticket.endpoint,
                        ticket.token,
                        event,
                        Instant::now(),
                    );
                }
            });
    }

    pub fn connect(&self, endpoint: &str) -> Result<ConnectionsView, RuntimeError> {
        let profile = self.ssh_profile(endpoint).ok();
        if let Some(profile) = &profile {
            self.ensure_auth(profile)?;
        }
        self.ensure_supervisor();
        if let Some(ticket) = self.shared.hub.request_connect(endpoint, Instant::now())? {
            self.run_attempt(ticket);
        }
        if let Some(profile) = &profile {
            self.remember_resume(profile.id.as_str(), true);
        }
        Ok(self.view())
    }

    /// Records, on the saved profile, whether this host is to be dialed again at the next start
    /// (spec 058, AC-058-01). A preference that cannot be written never fails the action the user
    /// asked for: the connection itself already happened, and the store error is the one
    /// [`ConnectionsView::store_error`] already reports.
    fn remember_resume(&self, id: &str, resume: bool) {
        let mut store = self.shared.store.lock().expect("store");
        if let Ok(store) = store.as_mut() {
            let _ = store.set_resume_on_open(id, resume);
        }
    }

    /// Saved SSH profiles to dial when the app opens: the ones that were connected when it was
    /// last used and whose "Conectar ao abrir" is on (spec 058, AC-058-02).
    fn resumable(&self) -> Vec<String> {
        let store = self.shared.store.lock().expect("store");
        store
            .as_ref()
            .map(|store| {
                store
                    .profiles()
                    .iter()
                    .filter(|profile| profile.connect_on_open && profile.resume_on_open)
                    .map(|profile| profile.id.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Dials those hosts on a thread of its own (spec 058, AC-058-02): the window is already
    /// installed and opens while the attempts run, and each one goes through exactly the same
    /// [`ConnectionsState::connect`] the Conectar button uses — so an unknown or changed host key
    /// is never accepted on its own and a failure lands in the state that path already produces
    /// (offline or "Needs attention"), with no dialog and no retry beyond the health monitor's.
    /// `None` when the thread could not be spawned; nothing is resumed and the window still opens.
    pub fn resume_connected_hosts(&self) -> Option<std::thread::JoinHandle<()>> {
        let state = self.clone();
        std::thread::Builder::new()
            .name("herdr-desktop-resume".into())
            .spawn(move || {
                for endpoint in state.resumable() {
                    // A host that cannot be dialed keeps its own error; the next one still runs.
                    let _ = state.connect(&endpoint);
                }
            })
            .ok()
    }

    /// "Conectar ao abrir" of the host menu (spec 058, AC-058-03). Only a saved SSH profile has
    /// the preference; Local is refused like the other profile actions.
    pub fn set_connect_on_open(
        &self,
        endpoint: &str,
        enabled: bool,
    ) -> Result<ConnectionsView, RuntimeError> {
        let profile = self.ssh_profile(endpoint)?;
        {
            let mut store = self.shared.store.lock().expect("store");
            let store = store.as_mut().map_err(|error| error.clone())?;
            store.set_connect_on_open(profile.id.as_str(), enabled)?;
        }
        Ok(self.view())
    }

    /// An explicit `ssh-agent` choice without a session agent must fail before any OpenSSH
    /// process runs (AC-031-02). `SSH_AUTH_SOCK` is read from the desktop process, never from
    /// the WebView; an empty value counts as absent.
    fn ensure_auth(&self, profile: &SshProfile) -> Result<(), RuntimeError> {
        ensure_ssh_agent(
            profile.auth.as_deref(),
            profile.target.as_str(),
            Some(profile.id.as_str()),
        )
    }

    /// Shows (surface) or hides (metadata-only) one host. A live connection that announced
    /// `surface_interest` toggles its lease in place — no bridge is closed, exactly like the TUI
    /// (spec 035) — and the trace reads `superfície <ms> ativa|inativa`. Any other host keeps the
    /// old renegotiation path, with its motive in the trace.
    pub fn set_visible(&self, endpoint: &str, visible: bool) -> Result<(), RuntimeError> {
        let target = self
            .shared
            .hub
            .spec(endpoint)
            .ok()
            .and_then(|spec| spec.target)
            .unwrap_or_else(|| endpoint.to_owned());
        let started = Instant::now();
        match self
            .shared
            .hub
            .set_visible(endpoint, visible, Instant::now())?
        {
            VisibilityOutcome::Unchanged => {}
            VisibilityOutcome::Interest(outcome) if outcome.sent => trace_attempt(
                &target,
                "superfície",
                started,
                if visible { "ativa" } else { "inativa" },
            ),
            VisibilityOutcome::Interest(outcome) if !outcome.supported => trace_attempt(
                &target,
                "superfície",
                started,
                "registrada (host sem conexão)",
            ),
            VisibilityOutcome::Interest(_) => trace_attempt(
                &target,
                "superfície",
                started,
                "sem envio (interesse já vigente)",
            ),
            VisibilityOutcome::Renegotiate(ticket) => {
                trace_attempt(
                    &target,
                    "superfície",
                    started,
                    "renegociando (sem surface_interest)",
                );
                self.run_attempt(ticket);
            }
        }
        Ok(())
    }

    /// Source of the `events.subscribe` stream of a registered host.
    pub fn event_source(&self, endpoint: &str) -> Result<EventSource, RuntimeError> {
        let connector = self
            .shared
            .connectors
            .lock()
            .expect("connectors")
            .get(endpoint)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new("endpoint_unknown", "the host has no connector")
                    .with_endpoint(endpoint)
            })?;
        Ok(connector.event_source(
            self.shared.config.isolated_ssh.as_ref(),
            &self.shared.runner,
        ))
    }

    pub fn cancel(&self, endpoint: &str) -> Result<ConnectionsView, RuntimeError> {
        let target = self
            .shared
            .hub
            .spec(endpoint)
            .ok()
            .and_then(|spec| spec.target)
            .unwrap_or_else(|| endpoint.to_owned());
        let started = Instant::now();
        let result = self.shared.hub.cancel(endpoint);
        trace_attempt(
            &target,
            "cancelado pelo diálogo",
            started,
            if result.is_ok() { "ok" } else { "erro" },
        );
        result?;
        Ok(self.view())
    }

    /// Explicit disconnect of a saved SSH host (spec 029, AC-029-03): detaches only the desktop
    /// connection and drops the host's snapshot, so its workspaces leave the tree. Only a saved
    /// SSH profile can be disconnected; Local is refused.
    pub fn disconnect(&self, endpoint: &str) -> Result<ConnectionsView, RuntimeError> {
        let profile = self.ssh_profile(endpoint)?;
        self.shared.hub.disconnect(endpoint)?;
        // Spec 058 (AC-058-01): an explicit Desconectar stops the resume until the next manual
        // connect of this host.
        self.remember_resume(profile.id.as_str(), false);
        Ok(self.view())
    }

    /// Explicit new attempt for a disconnected/failed SSH host (the menu's Reconectar). Only a
    /// saved SSH profile can be reconnected.
    pub fn reconnect(&self, endpoint: &str) -> Result<ConnectionsView, RuntimeError> {
        let profile = self.ssh_profile(endpoint)?;
        self.ensure_auth(&profile)?;
        self.ensure_supervisor();
        if let Some(ticket) = self.shared.hub.request_connect(endpoint, Instant::now())? {
            self.run_attempt(ticket);
        }
        self.remember_resume(profile.id.as_str(), true);
        Ok(self.view())
    }

    /// Removes the saved profile and its host (spec 029, AC-029-03). The remote server and its
    /// sessions keep running; only the desktop forgets the profile.
    pub fn remove_profile(&self, endpoint: &str) -> Result<ConnectionsView, RuntimeError> {
        let profile = self.ssh_profile(endpoint)?;
        {
            let mut store = self.shared.store.lock().expect("store");
            let store = store.as_mut().map_err(|error| error.clone())?;
            store.remove(profile.id.as_str())?;
        }
        self.shared.hub.remove_host(endpoint)?;
        self.shared
            .connectors
            .lock()
            .expect("connectors")
            .remove(endpoint);
        Ok(self.view())
    }

    /// The saved SSH profile of `endpoint`, or the explicit refusal for Local/unknown ids.
    fn ssh_profile(&self, endpoint: &str) -> Result<SshProfile, RuntimeError> {
        let spec = self.shared.hub.spec(endpoint)?;
        if spec.kind != HostKind::Ssh {
            return Err(RuntimeError::new(
                "endpoint_local",
                "This computer is not a saved profile; nothing was changed",
            )
            .with_endpoint(endpoint));
        }
        let store = self.shared.store.lock().expect("store");
        let store = store.as_ref().map_err(|error| error.clone())?;
        store.get(endpoint).cloned().ok_or_else(|| {
            RuntimeError::new("profile_not_found", "the connection profile does not exist")
                .with_endpoint(endpoint)
        })
    }

    pub fn save_profile(
        &self,
        draft: SshProfileDraft,
        connect: bool,
    ) -> Result<ConnectionsView, RuntimeError> {
        // Nothing is written and nothing is attempted when the explicit auth choice cannot work
        // in this session (AC-031-02).
        ensure_ssh_agent(
            draft.auth.as_deref(),
            draft.target.trim(),
            draft.id.as_deref(),
        )?;
        if let Some(id) = draft.id.as_deref() {
            if self.shared.hub.is_busy(id) {
                return Err(RuntimeError::new(
                    "profile_in_use",
                    "cancel the connection before changing the profile",
                )
                .with_endpoint(id));
            }
        }
        let profile = {
            let mut store = self.shared.store.lock().expect("store");
            let store = store.as_mut().map_err(|e| e.clone())?;
            store.save(draft)?
        };
        self.register(&profile);
        if connect {
            return self.connect(profile.id.as_str());
        }
        Ok(self.view())
    }

    pub fn import_profiles(&self) -> Result<ImportView, RuntimeError> {
        let catalog = tui_catalog_path(&self.shared.config.herdr_state_dir);
        let (report, profiles) = {
            let mut store = self.shared.store.lock().expect("store");
            let store = store.as_mut().map_err(|e| e.clone())?;
            let report = store.import_tui_catalog(&catalog)?;
            (report, store.profiles().to_vec())
        };
        for profile in &profiles {
            if self.shared.hub.spec(profile.id.as_str()).is_err() {
                self.register(profile);
            }
        }
        Ok(ImportView {
            report,
            view: self.view(),
        })
    }

    pub fn send_text(
        &self,
        target: &QualifiedTarget,
        text: &str,
        submit: bool,
    ) -> Result<(), RuntimeError> {
        self.shared
            .hub
            .send_input(target, text_events(text, submit))
    }

    /// Read-only JSON API action on the host's own lane.
    pub fn workspaces(&self, endpoint: &str) -> Result<Vec<WorkspaceDto>, RuntimeError> {
        let started = Instant::now();
        let result = self
            .shared
            .hub
            .run_action(endpoint, "workspace.list", serde_json::json!({}));
        let workspaces = match result {
            Ok(result) => result
                .get("workspaces")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|w| WorkspaceDto {
                            workspace_id: w["workspace_id"].as_str().unwrap_or_default().to_owned(),
                            label: w["label"].as_str().unwrap_or_default().to_owned(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            Err(error) if error.code == "remote_api_unsupported" => {
                // Herdr 0.9.0, used by the measured mac-mini, has the visual endpoint snapshot
                // but not the optional JSON bridge. Reuse that snapshot only after the explicit
                // unsupported response; never substitute Local or invent a workspace list.
                let remaining = WORKSPACES_TIMEOUT.saturating_sub(started.elapsed());
                self.shared
                    .hub
                    .snapshot_workspaces(endpoint, remaining)?
                    .into_iter()
                    .map(|workspace| WorkspaceDto {
                        workspace_id: workspace.workspace_id,
                        label: workspace.label,
                    })
                    .collect()
            }
            Err(error) if error.code == "result_unknown" || error.code == "timeout" => {
                return Err(
                    RuntimeError::new("timeout", "no answer while reading workspaces")
                        .retryable()
                        .with_endpoint(endpoint),
                )
            }
            Err(error) => return Err(error),
        };
        trace_workspaces(&self.shared.hub, endpoint, started, "ok");
        Ok(workspaces)
    }

    /// Detaches every host (window closing). Engines and remote servers keep running.
    pub fn detach_all(&self) {
        for host in self.shared.hub.snapshot(Instant::now()).hosts {
            let _ = self.shared.hub.cancel(&host.endpoint);
        }
    }
}

/// IPC entry points awaited by the Tauri command wrappers below (spec 007). Each one runs the
/// synchronous method on the async runtime's bounded blocking pool: store lock, profile files,
/// hub locks and host sockets never wait on the IPC calling (GUI) thread, and the returned future
/// only waits. There is no shared lane: a long watch, a held host write or a slow catalog read
/// never delays another command. Parameters are owned by the call. Nothing is retried; results
/// and errors are exactly the synchronous ones. Ordering between conflicting calls
/// (connect/cancel, profile save/import, text to one host) is kept by the frontend bridge, which
/// sends such calls one at a time (`src/connections/bridge.ts`).
impl ConnectionsState {
    pub fn connections_list(
        &self,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, |state| Ok(state.view()))
    }

    pub fn connections_watch(
        &self,
        revision: u64,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(
            || RuntimeError::new("watch_failed", "could not watch connections"),
            move |state| {
                state.shared.hub.wait_changed(revision, WATCH_TIMEOUT);
                Ok(state.view())
            },
        )
    }

    pub fn connection_profile_save(
        &self,
        draft: SshProfileDraft,
        connect: bool,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| {
            state.save_profile(draft, connect)
        })
    }

    pub fn connection_profiles_import(
        &self,
    ) -> impl Future<Output = Result<ImportView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, |state| state.import_profiles())
    }

    pub fn connection_connect(
        &self,
        endpoint: String,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| state.connect(&endpoint))
    }

    pub fn connection_cancel(
        &self,
        endpoint: String,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| state.cancel(&endpoint))
    }

    pub fn connection_disconnect(
        &self,
        endpoint: String,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| {
            state.disconnect(&endpoint)
        })
    }

    pub fn connection_reconnect(
        &self,
        endpoint: String,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| state.reconnect(&endpoint))
    }

    pub fn connection_remove(
        &self,
        endpoint: String,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| {
            state.remove_profile(&endpoint)
        })
    }

    pub fn connection_send_text(
        &self,
        target: QualifiedTarget,
        text: String,
        submit: bool,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| {
            state.send_text(&target, &text, submit)
        })
    }

    pub fn connection_workspaces(
        &self,
        endpoint: String,
    ) -> impl Future<Output = Result<Vec<WorkspaceDto>, RuntimeError>> + Send + 'static {
        self.off_gui(
            || RuntimeError::new("action_failed", "could not run the action"),
            move |state| state.workspaces(&endpoint),
        )
    }

    pub fn connections_set_connect_on_open(
        &self,
        endpoint: String,
        enabled: bool,
    ) -> impl Future<Output = Result<ConnectionsView, RuntimeError>> + Send + 'static {
        self.off_gui(command_interrupted, move |state| {
            state.set_connect_on_open(&endpoint, enabled)
        })
    }

    /// Runs `work` on the blocking pool when first polled. A future dropped before that runs
    /// nothing; one dropped later lets the work finish and discards its result. If the work ends
    /// without a result (panic, runtime shutdown) the caller gets `interrupted()`, never a retry.
    fn off_gui<T: Send + 'static>(
        &self,
        interrupted: fn() -> RuntimeError,
        work: impl FnOnce(&ConnectionsState) -> Result<T, RuntimeError> + Send + 'static,
    ) -> impl Future<Output = Result<T, RuntimeError>> + Send + 'static {
        let state = self.clone();
        async move {
            tauri::async_runtime::spawn_blocking(move || work(&state))
                .await
                .unwrap_or_else(|_| Err(interrupted()))
        }
    }
}

/// The blocking work ended without a result: whether it reached the host is unknown, and it is
/// not repeated.
fn command_interrupted() -> RuntimeError {
    RuntimeError::new(
        "command_interrupted",
        "the action was interrupted without a confirmed result; check the state before repeating it",
    )
}

// ---------------------------------------------------------------------------------------
// Tauri commands: thin async wrappers (argument names and success payloads unchanged;
// `connections_list` now answers `Result`, whose `Ok` serializes exactly as the former value).
// ---------------------------------------------------------------------------------------

#[tauri::command]
pub async fn connections_list(
    state: tauri::State<'_, ConnectionsState>,
) -> Result<ConnectionsView, RuntimeError> {
    state.connections_list().await
}

#[tauri::command]
pub async fn connections_watch(
    state: tauri::State<'_, ConnectionsState>,
    revision: u64,
) -> Result<ConnectionsView, RuntimeError> {
    state.connections_watch(revision).await
}

#[tauri::command]
pub async fn connection_profile_save(
    state: tauri::State<'_, ConnectionsState>,
    draft: SshProfileDraft,
    connect: bool,
) -> Result<ConnectionsView, RuntimeError> {
    state.connection_profile_save(draft, connect).await
}

#[tauri::command]
pub async fn connection_profiles_import(
    state: tauri::State<'_, ConnectionsState>,
) -> Result<ImportView, RuntimeError> {
    state.connection_profiles_import().await
}

#[tauri::command]
pub async fn connection_connect(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
) -> Result<ConnectionsView, RuntimeError> {
    state.connection_connect(endpoint).await
}

#[tauri::command]
pub async fn connection_cancel(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
) -> Result<ConnectionsView, RuntimeError> {
    state.connection_cancel(endpoint).await
}

#[tauri::command]
pub async fn connection_disconnect(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
) -> Result<ConnectionsView, RuntimeError> {
    state.connection_disconnect(endpoint).await
}

#[tauri::command]
pub async fn connection_reconnect(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
) -> Result<ConnectionsView, RuntimeError> {
    state.connection_reconnect(endpoint).await
}

#[tauri::command]
pub async fn connection_remove(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
) -> Result<ConnectionsView, RuntimeError> {
    state.connection_remove(endpoint).await
}

#[tauri::command]
pub async fn connection_send_text(
    state: tauri::State<'_, ConnectionsState>,
    target: QualifiedTarget,
    text: String,
    submit: bool,
) -> Result<(), RuntimeError> {
    state.connection_send_text(target, text, submit).await
}

#[tauri::command]
pub async fn connection_workspaces(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
) -> Result<Vec<WorkspaceDto>, RuntimeError> {
    state.connection_workspaces(endpoint).await
}

#[tauri::command]
pub async fn connections_set_connect_on_open(
    state: tauri::State<'_, ConnectionsState>,
    endpoint: String,
    enabled: bool,
) -> Result<ConnectionsView, RuntimeError> {
    state
        .connections_set_connect_on_open(endpoint, enabled)
        .await
}

#[cfg(test)]
mod host_theme_tests {
    use super::super::hub::ApiLane;
    use super::super::state::LinkPhase;
    use super::*;
    use herdr_client::protocol::wire::ClientHostThemeUpdate;
    use herdr_client::{GatewayEvent, LiveIdentity, Negotiated};
    use std::sync::mpsc::Receiver;

    #[derive(Debug, PartialEq)]
    enum Call {
        Theme(Vec<ClientHostThemeUpdate>),
        Installed,
    }
    type Calls = Arc<Mutex<Vec<Call>>>;
    struct RecordingGateway {
        calls: Calls,
        theme_error: bool,
    }
    struct DefaultGateway;

    macro_rules! unused_gateway_methods {
        () => {
            fn endpoint(&self) -> &str {
                "local"
            }
            fn identity(&self) -> Option<LiveIdentity> {
                None
            }
            fn connect(&mut self, _: ConnectOptions) -> Result<Negotiated, RuntimeError> {
                panic!("already attached")
            }
            fn api_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
                panic!("unexpected API")
            }
            fn endpoint_request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
                panic!("unexpected endpoint")
            }
            fn send_input(
                &self,
                _: &QualifiedTarget,
                _: Vec<ClientPaneInputEvent>,
            ) -> Result<(), RuntimeError> {
                panic!("unexpected input")
            }
            fn resize(&self, _: SurfaceGeometry) -> Result<(), RuntimeError> {
                panic!("unexpected resize")
            }
            fn set_focus(&self, _: bool) -> Result<(), RuntimeError> {
                panic!("unexpected focus")
            }
            fn detach(&mut self) {}
            fn is_connected(&self) -> bool {
                true
            }
        };
    }
    impl RuntimeGateway for DefaultGateway {
        unused_gateway_methods!();
        fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
            panic!("unexpected events")
        }
    }
    impl RuntimeGateway for RecordingGateway {
        unused_gateway_methods!();
        fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
            self.calls.lock().unwrap().push(Call::Installed);
            None
        }
        fn set_host_theme(&self, updates: &[ClientHostThemeUpdate]) -> Result<(), RuntimeError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Theme(updates.to_vec()));
            if self.theme_error {
                Err(RuntimeError::new("theme_test_failure", "theme unavailable"))
            } else {
                Ok(())
            }
        }
    }
    struct FakeConnector {
        calls: Calls,
        fails: bool,
        theme_error: bool,
    }
    impl ApiLane for FakeConnector {
        fn request(&self, _: &str, _: Value) -> Result<Value, RuntimeError> {
            panic!("unexpected API")
        }
    }
    impl HostConnector for FakeConnector {
        fn connect(&self, _: ConnectOptions) -> Result<Attached, ConnectFailure> {
            if self.fails {
                return Err(ConnectFailure::transient(
                    "local",
                    "fake_failure",
                    "connection failed",
                ));
            }
            Ok(Attached {
                connected: Connected {
                    gateway: Box::new(RecordingGateway {
                        calls: self.calls.clone(),
                        theme_error: self.theme_error,
                    }),
                    api: Arc::new(FakeConnector {
                        calls: self.calls.clone(),
                        fails: false,
                        theme_error: false,
                    }),
                },
                lane: None,
                capabilities: vec![],
                server_version: "fake".into(),
                remote_binary: None,
                latency_ms: 0,
            })
        }
        fn event_source(
            &self,
            _: Option<&IsolatedSshConfig>,
            _: &Arc<dyn SshRunner>,
        ) -> EventSource {
            panic!("unexpected subscription")
        }
    }

    fn state(
        dir: &std::path::Path,
        calls: &Calls,
        fails: bool,
        theme_error: bool,
    ) -> ConnectionsState {
        let state = ConnectionsState::new(ConnectionsConfig {
            prefs_dir: dir.join("prefs"),
            herdr_config_dir: dir.join("config"),
            herdr_state_dir: dir.join("state"),
            local_session: Some(SessionName::parse("spec066-fake").unwrap()),
            local_auto_start: false,
            herdr_bin: dir.join("never-run"),
            isolated_ssh: None,
            geometry: SurfaceGeometry {
                cols: 80,
                rows: 24,
                cell_width_px: 8,
                cell_height_px: 16,
            },
        });
        state.shared.connectors.lock().unwrap().insert(
            "local".into(),
            Arc::new(FakeConnector {
                calls: calls.clone(),
                fails,
                theme_error,
            }),
        );
        state
    }

    fn attempt(state: &ConnectionsState) -> LinkPhase {
        let ticket = state
            .hub()
            .request_connect("local", Instant::now())
            .unwrap()
            .unwrap();
        state.run_attempt(ticket);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let view = state.hub().snapshot(Instant::now());
            let phase = view.hosts[0].phase;
            if !matches!(phase, LinkPhase::Connecting | LinkPhase::Reconnecting)
                || view.hosts[0].connection_error.is_some()
            {
                return phase;
            }
            assert!(Instant::now() < deadline, "attempt did not settle");
            state
                .hub()
                .wait_changed(view.revision, Duration::from_millis(20));
        }
    }

    #[test]
    fn host_theme_default_trait_method_has_no_effect() {
        assert_eq!(
            DefaultGateway.set_host_theme(&theme::host_theme_updates(None)),
            Ok(())
        );
    }

    #[test]
    fn host_theme_precedes_hub_install_once_per_successful_connection() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let state = state(dir.path(), &calls, false, false);
        let updates = theme::host_theme_updates(theme::omarchy_theme_dir().as_deref());
        assert_eq!(attempt(&state), LinkPhase::Online);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![Call::Theme(updates.clone()), Call::Installed]
        );
        state.hub().disconnect("local").unwrap();
        assert_eq!(attempt(&state), LinkPhase::Online);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                Call::Theme(updates.clone()),
                Call::Installed,
                Call::Theme(updates),
                Call::Installed
            ]
        );
    }

    #[test]
    fn host_theme_is_not_sent_when_connection_fails() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let state = state(dir.path(), &calls, true, false);
        assert_ne!(attempt(&state), LinkPhase::Online);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn host_theme_error_keeps_the_connection_online() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Calls::default();
        let state = state(dir.path(), &calls, false, true);
        assert_eq!(attempt(&state), LinkPhase::Online);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                Call::Theme(theme::host_theme_updates(
                    theme::omarchy_theme_dir().as_deref()
                )),
                Call::Installed
            ]
        );
    }
}
