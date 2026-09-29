//! Selection of the composed window (007) over the one connection hub of `ConnectionsState`.
//!
//! - [`SelectionState`] keeps the selected endpoint: only that host keeps a surface, the others
//!   stay metadata-only. It never opens a second gateway or surface.
//! - [`SelectionState::gateway_for`] hands out a `RuntimeGateway` view of one host bound to the
//!   identity captured when it was created (endpoint, session, hub generation, boot). Every
//!   action revalidates that identity against the hub before anything is sent; the view never
//!   follows a reconnect, reboot or another host. `connect` is refused and `detach` does nothing:
//!   the hub owns the connection.
//! - [`SelectionState::agents_host`] is the `AgentsHost` of the hosted agents.
//! - [`SelectionState::forward_notice`] hands the hub notices the hosted agents need (full
//!   surfaces and the end/replacement of connections, never patches) to their surface listener,
//!   labelled with the connection they were produced on. It is called by the window's one hub
//!   observer (the composed surface), without any hub lock held.

use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use herdr_client::protocol::wire::{ClientPaneInputEvent, PaneSurfaceFrame};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry,
};
use serde_json::Value;

use super::agent_commands::{AgentsHost, EventStream, SurfaceListener, SurfaceSignal};
use super::events::{open_event_stream, Fenced};
use crate::connections::commands::ConnectionsState;
use crate::connections::hub::{EndpointScope, HostNotice, NoticeOrigin};

/// JSON API methods without consequences: they never enter the action ledger.
const READ_ONLY_API: &[&str] = &[
    "agent.list",
    "agent.get",
    // One detection snapshot per transition to waiting (spec 014): a read with no consequence.
    "agent.read",
    // One static screen snapshot per home thumbnail (spec 012): a read with no consequence.
    "pane.read",
    "tab.list",
    "workspace.list",
    "server.agent_manifests",
    "pane.process_info",
];

/// Methods the agents discovery probes with an empty params object, which the engine rejects for
/// missing required fields before executing anything (see `AgentsCore::discover`).
const PROBED_WITH_EMPTY_PARAMS: &[&str] = &["agent.start", "agent.prompt"];

fn records(method: &str, params: &Value) -> bool {
    if READ_ONLY_API.contains(&method) {
        return false;
    }
    let probe = params.as_object().is_some_and(|p| p.is_empty());
    !(probe && PROBED_WITH_EMPTY_PARAMS.contains(&method))
}

struct Shared {
    connections: ConnectionsState,
    selected: RwLock<Option<String>>,
    /// Serializes selection transitions (never taken by frame observers or readers).
    transition: Mutex<()>,
    /// Identity of the last gateway handed to the agents host; its surface is paired with it.
    issued: Mutex<Option<LiveIdentity>>,
    /// Connections (endpoint → hub generation) whose server reported no remote JSON API.
    api_missing: Mutex<BTreeMap<String, u64>>,
    /// Surface listener of the hosted agents (at most one per window).
    surface_listener: RwLock<Option<SurfaceListener>>,
}

impl Shared {
    fn selected(&self) -> Option<String> {
        self.selected.read().expect("selection lock").clone()
    }

    fn view(self: &Arc<Self>, endpoint: &str) -> Result<HubGateway, RuntimeError> {
        let identity = self.connections.hub().live_identity(endpoint)?;
        Ok(HubGateway {
            shared: self.clone(),
            identity,
        })
    }
}

/// Selected host of the window and gateway views of the shared hub.
#[derive(Clone)]
pub struct SelectionState {
    shared: Arc<Shared>,
}

fn no_selection() -> RuntimeError {
    RuntimeError::new("no_selection", "no host is selected")
}

impl SelectionState {
    pub fn new(connections: ConnectionsState) -> Self {
        Self {
            shared: Arc::new(Shared {
                connections,
                selected: RwLock::new(None),
                transition: Mutex::new(()),
                issued: Mutex::new(None),
                api_missing: Mutex::new(BTreeMap::new()),
                surface_listener: RwLock::new(None),
            }),
        }
    }

    pub fn connections(&self) -> &ConnectionsState {
        &self.shared.connections
    }

    pub fn selected(&self) -> Option<String> {
        self.shared.selected()
    }

    /// Selects `endpoint`: it becomes the only host with a surface, the others metadata-only.
    /// A live connection that announced `surface_interest` only toggles its lease in place
    /// (spec 035, like the TUI); any other online host keeps the renegotiation path. An unknown
    /// endpoint changes nothing.
    ///
    /// Transitions are serialized: a concurrent call waits for the running one, so the last
    /// transition to run decides the only visible host and an earlier one can never re-enable
    /// its host afterwards. Readers of the selection and frame observers never wait on it.
    pub fn select(&self, endpoint: &str) -> Result<(), RuntimeError> {
        let _transition = self.shared.transition.lock().expect("selection transition");
        let hub = self.shared.connections.hub();
        hub.spec(endpoint)?;
        *self.shared.selected.write().expect("selection lock") = Some(endpoint.to_owned());
        let hosts = hub.snapshot(Instant::now()).hosts;
        // Hide every other host first so two hosts never hold a surface at once; a failure to
        // hide one does not stop hiding the rest, and the selected host is shown only after all
        // others were hidden.
        let mut hide_error = None;
        for host in hosts.iter().filter(|h| h.endpoint != endpoint) {
            if let Err(error) = self.shared.connections.set_visible(&host.endpoint, false) {
                hide_error.get_or_insert(error);
            }
        }
        if let Some(error) = hide_error {
            return Err(error);
        }
        self.shared.connections.set_visible(endpoint, true)
    }

    /// Gateway view of `endpoint` bound to its current connection (for the project service too).
    pub fn gateway_for(&self, endpoint: &str) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        Ok(Box::new(self.shared.view(endpoint)?))
    }

    pub fn selected_gateway(&self) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        let endpoint = self.selected().ok_or_else(no_selection)?;
        self.gateway_for(&endpoint)
    }

    /// Forwards one hub notice to the hosted agents' surface listener. Only full surfaces and
    /// connection ends/replacements are forwarded (a patch never changes panes, splits or focus
    /// in surface v1); the listener judges them by the origin connection. No frame is cloned.
    pub fn forward_notice(&self, origin: &NoticeOrigin, notice: &HostNotice) {
        let Some(listener) = self
            .shared
            .surface_listener
            .read()
            .expect("surface listener")
            .clone()
        else {
            return;
        };
        let signal = match (notice, origin.identity.as_ref()) {
            (HostNotice::Full(frame), Some(identity)) => SurfaceSignal::Full {
                identity,
                frame,
                // Host lock only (released before the listener runs): never the agents core.
                confirmed_tab: self
                    .shared
                    .connections
                    .hub()
                    .confirmed_tab_focus(identity, frame.projection_revision),
            },
            (HostNotice::Lost(error) | HostNotice::Invalidated(error), Some(identity)) => {
                SurfaceSignal::Ended { identity, error }
            }
            (HostNotice::Connected { generation }, _) => SurfaceSignal::Replaced {
                endpoint: &origin.endpoint,
                generation: *generation,
            },
            _ => return,
        };
        listener(signal);
    }

    /// Host of the hosted agents: always the selected endpoint.
    pub fn agents_host(&self) -> Arc<dyn AgentsHost> {
        Arc::new(SelectedHost {
            shared: self.shared.clone(),
        })
    }
}

/// `RuntimeGateway` view of one hub connection.
struct HubGateway {
    shared: Arc<Shared>,
    identity: LiveIdentity,
}

impl HubGateway {
    fn endpoint_scope(&self, params: &Value) -> Result<EndpointScope, RuntimeError> {
        let field = |name: &str| params.get(name).and_then(Value::as_str).map(str::to_owned);
        let endpoint = self.identity.endpoint.as_str();
        let pane = match (field("target_pane_id"), field("pane_id")) {
            (Some(a), Some(b)) if a != b => {
                return Err(RuntimeError::new(
                    "target_ambiguous",
                    "the action names two different panes; nothing was sent",
                )
                .with_endpoint(endpoint))
            }
            (Some(pane), _) | (None, Some(pane)) => Some(pane),
            (None, None) => None,
        };
        Ok(match (pane, field("tab_id"), field("workspace_id")) {
            (Some(pane_id), _, workspace_id) => EndpointScope::Pane {
                pane_id,
                workspace_id,
            },
            (None, Some(tab), _) => EndpointScope::Tab(tab),
            (None, None, Some(workspace)) => EndpointScope::Workspace(workspace),
            (None, None, None) => {
                return Err(RuntimeError::new(
                    "target_required",
                    "the action names no pane, tab or workspace; nothing was sent",
                )
                .with_endpoint(endpoint))
            }
        })
    }
}

impl RuntimeGateway for HubGateway {
    fn endpoint(&self) -> &str {
        &self.identity.endpoint
    }

    fn identity(&self) -> Option<LiveIdentity> {
        Some(self.identity.clone())
    }

    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(RuntimeError::new(
            "hub_owned",
            "this host's connection belongs to the window's hub",
        )
        .with_endpoint(self.identity.endpoint.clone()))
    }

    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        None
    }

    fn api_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let record = records(method, &params);
        let result = self
            .shared
            .connections
            .hub()
            .run_api(&self.identity, method, params, record);
        if matches!(&result, Err(error) if error.code == "remote_api_unsupported") {
            self.shared.api_missing.lock().expect("api missing").insert(
                self.identity.endpoint.clone(),
                self.identity.connection_generation,
            );
        }
        result
    }

    fn endpoint_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let scope = self.endpoint_scope(&params)?;
        self.shared
            .connections
            .hub()
            .run_endpoint_scoped(&self.identity, &scope, method, params)
    }

    fn send_input(
        &self,
        target: &QualifiedTarget,
        events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        target.validate(&self.identity)?;
        self.shared.connections.hub().send_input(target, events)
    }

    fn send_clipboard_image(
        &self,
        target: &QualifiedTarget,
        extension: &str,
        data: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        target.validate(&self.identity)?;
        self.shared
            .connections
            .hub()
            .send_clipboard_image(target, extension, data)
    }

    fn resize(&self, geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        let hub = self.shared.connections.hub();
        hub.check_identity(&self.identity)?;
        hub.resize(&self.identity.endpoint, geometry)
    }

    fn set_focus(&self, focused: bool) -> Result<(), RuntimeError> {
        self.shared
            .connections
            .hub()
            .set_focus(&self.identity, focused)
    }

    /// The hub owns the connection; dropping the view detaches nothing.
    fn detach(&mut self) {}

    fn is_connected(&self) -> bool {
        self.shared
            .connections
            .hub()
            .check_identity(&self.identity)
            .is_ok()
    }
}

/// The agents host: whatever endpoint is selected when called.
struct SelectedHost {
    shared: Arc<Shared>,
}

impl AgentsHost for SelectedHost {
    fn endpoint(&self) -> String {
        self.shared.selected().unwrap_or_default()
    }

    fn gateway(&self) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        let endpoint = self.shared.selected().ok_or_else(no_selection)?;
        let view = self.shared.view(&endpoint)?;
        *self.shared.issued.lock().expect("issued") = Some(view.identity.clone());
        Ok(Box::new(view))
    }

    fn listen_surface(&self, listener: Option<SurfaceListener>) {
        *self
            .shared
            .surface_listener
            .write()
            .expect("surface listener") = listener;
    }

    /// Only for the selected host; the hub checks connection, boot, presentation and revision.
    fn confirmed_tab_focus(
        &self,
        identity: &LiveIdentity,
        projection_revision: u64,
    ) -> Option<Option<String>> {
        if self.shared.selected().as_deref() != Some(identity.endpoint.as_str()) {
            return None;
        }
        self.shared
            .connections
            .hub()
            .confirmed_tab_focus(identity, projection_revision)
    }

    fn endpoint_methods(&self) -> Result<Vec<String>, RuntimeError> {
        let endpoint = self.shared.selected().ok_or_else(no_selection)?;
        self.shared.connections.hub().endpoint_methods(&endpoint)
    }

    /// Only the surface of the connection whose gateway was issued last, and only while that
    /// host is still selected.
    fn surface(&self) -> Option<PaneSurfaceFrame> {
        let issued = self.shared.issued.lock().expect("issued").clone()?;
        if self.shared.selected().as_deref() != Some(issued.endpoint.as_str()) {
            return None;
        }
        self.shared.connections.hub().surface_for(&issued)
    }

    /// A stream on the connection `attached` only, opened while that host is still selected and
    /// that connection/boot still current; it ends once either changes. Nothing is opened or
    /// written on the currently selected host instead. No bridge process is started for a
    /// connection whose server reported no remote JSON API.
    fn event_stream(&self, attached: &LiveIdentity) -> Result<Box<dyn EventStream>, RuntimeError> {
        let endpoint = attached.endpoint.clone();
        if self.shared.selected().as_deref() != Some(endpoint.as_str()) {
            return Err(RuntimeError::new(
                "selection_changed",
                "the selected host changed; events of the attached host will not be opened",
            )
            .with_endpoint(endpoint));
        }
        let hub = self.shared.connections.hub();
        hub.check_identity(attached)?;
        let identity = attached.clone();
        let missing = self
            .shared
            .api_missing
            .lock()
            .expect("api missing")
            .get(&endpoint)
            .is_some_and(|generation| *generation == identity.connection_generation);
        if missing {
            return Err(RuntimeError::new(
                "remote_api_unsupported",
                "the remote Herdr does not offer the JSON API over SSH (remote-api-bridge); agent events are unavailable",
            )
            .with_endpoint(endpoint));
        }
        let source = self.shared.connections.event_source(&endpoint)?;
        let stream = open_event_stream(&source, &endpoint)?;
        let shared = self.shared.clone();
        Ok(Box::new(Fenced::new(
            stream,
            Box::new(move || {
                shared.selected().as_deref() == Some(identity.endpoint.as_str())
                    && shared.connections.hub().check_identity(&identity).is_ok()
            }),
        )))
    }
}
