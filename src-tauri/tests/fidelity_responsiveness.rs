//! Spec 007 — responsiveness of the agents and projects IPC commands (seam: the entry points
//! the real `#[tauri::command]` wrappers await, `AgentsState::<command>` and
//! `ProjectsState::<command>`, driven by a single-threaded executor written here; no GUI, no
//! engine, no default session).
//!
//! What the seam must show, deterministically (holds/barriers, never wall-time claims):
//! - a command whose host blocks returns `Pending` to the calling thread instead of blocking
//!   it; getters that wait the same service lock are `Pending` too; unrelated ready work and
//!   another channel keep progressing on that thread;
//! - the eventual result or error is delivered once, with no retry/replay of the action;
//! - the channel handed to `agents_connect` is the one the service's callbacks reach;
//! - commands of one service apply in entry order, even when the older one starts late: an
//!   older detach never undoes a newer attach, a dropped command never blocks the next;
//! - clones of `ProjectsState` share the one store (persistence) and the runtime bindings.
//!
//! Fixture values differ on purpose: host `ssh-hd007r` / boot `boot-hd007r-5`, projects host
//! `ssh-hd007p` / boot `boot-hd007p-2`, workspace `w9`, error codes `fake_events_off` and
//! `host_offline_hd007p`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use herdr_client::protocol::wire::{ClientPaneInputEvent, PaneSurfaceFrame};
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, ProjectRef, QualifiedTarget,
    RuntimeBinding, RuntimeError, RuntimeGateway, SurfaceGeometry,
};
use herdr_desktop::bridge::agent_commands::{AgentsHost, AgentsState, EventStream, PromptOutcome};
use herdr_desktop::project_store::{
    OpenOutcome, ProjectDraft, ProjectGateways, ProjectStore, ProjectsState,
};
use herdr_desktop::terminal::GeometryDto;
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};

/// Upper bound for anything that must eventually happen (a deadline, not a performance claim).
const WITHIN: Duration = Duration::from_secs(10);
/// How long a held host call waits for its release before giving up. An implementation that
/// runs the command inline on the calling thread returns `hold_timeout` after this instead of
/// hanging the test, so the `Pending` assertions fail instead of deadlocking.
const HOLD_LIMIT: Duration = Duration::from_secs(3);

// =======================================================================================
// Single-threaded executor
// =======================================================================================

#[derive(Default)]
struct Signal {
    wakes: Mutex<u64>,
    cv: Condvar,
}

impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.wakes.lock().unwrap() += 1;
        self.cv.notify_all();
    }
}

/// Polls futures only on the test thread; a wake is the only way to learn progress.
struct Executor {
    signal: Arc<Signal>,
    waker: Waker,
}

type Task<T> = Pin<Box<dyn Future<Output = T> + Send>>;

impl Executor {
    fn new() -> Self {
        let signal = Arc::new(Signal::default());
        Self {
            waker: Waker::from(signal.clone()),
            signal,
        }
    }

    fn poll<T>(&self, task: &mut Task<T>) -> Poll<T> {
        task.as_mut().poll(&mut Context::from_waker(&self.waker))
    }

    fn assert_pending<T>(&self, task: &mut Task<T>, what: &str) {
        assert!(
            self.poll(task).is_pending(),
            "{what}: the command must hand the calling thread back while its work waits"
        );
    }

    /// Polls until ready, sleeping only on wakes.
    fn wait<T>(&self, task: &mut Task<T>, what: &str) -> T {
        let deadline = Instant::now() + WITHIN;
        loop {
            let seen = *self.signal.wakes.lock().unwrap();
            if let Poll::Ready(value) = self.poll(task) {
                return value;
            }
            let mut wakes = self.signal.wakes.lock().unwrap();
            while *wakes == seen {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_else(|| panic!("{what}: never completed"));
                wakes = self.signal.cv.wait_timeout(wakes, left).unwrap().0;
            }
        }
    }

    /// True when `task` stays pending across every wake for `window` (used only to show a
    /// newer command does not overtake an older one that was not polled yet).
    fn stays_pending<T>(&self, task: &mut Task<T>, window: Duration) -> bool {
        let deadline = Instant::now() + window;
        loop {
            let seen = *self.signal.wakes.lock().unwrap();
            if self.poll(task).is_ready() {
                return false;
            }
            let mut wakes = self.signal.wakes.lock().unwrap();
            while *wakes == seen {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return true;
                };
                wakes = self.signal.cv.wait_timeout(wakes, left).unwrap().0;
            }
        }
    }
}

fn task<T>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
    Box::pin(future)
}

// =======================================================================================
// Barriers and channels
// =======================================================================================

/// A host call that blocks while armed until the test releases it.
#[derive(Default)]
struct Hold {
    state: Mutex<HoldState>,
    cv: Condvar,
}

#[derive(Default)]
struct HoldState {
    armed: usize,
    entered: usize,
    released: bool,
}

impl Hold {
    fn arm(&self) {
        let mut state = self.state.lock().unwrap();
        state.armed += 1;
        state.released = false;
    }

    fn pass(&self) -> Result<(), RuntimeError> {
        let mut state = self.state.lock().unwrap();
        if state.armed == 0 {
            return Ok(());
        }
        state.armed -= 1;
        state.entered += 1;
        self.cv.notify_all();
        let deadline = Instant::now() + HOLD_LIMIT;
        while !state.released {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RuntimeError::new("hold_timeout", "hold never released"));
            };
            state = self.cv.wait_timeout(state, left).unwrap().0;
        }
        Ok(())
    }

    fn wait_entered(&self, count: usize) {
        let deadline = Instant::now() + WITHIN;
        let mut state = self.state.lock().unwrap();
        while state.entered < count {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("the held host call was never reached");
            state = self.cv.wait_timeout(state, left).unwrap().0;
        }
    }

    fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.cv.notify_all();
    }
}

/// Records every message a `tauri::ipc::Channel` delivers to its callback.
struct Recorder {
    messages: Arc<Mutex<Vec<Value>>>,
    arrived: Arc<Condvar>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            messages: Arc::new(Mutex::new(Vec::new())),
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

    fn messages(&self) -> Vec<Value> {
        self.messages.lock().unwrap().clone()
    }

    fn wait_for(&self, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + WITHIN;
        let mut messages = self.messages.lock().unwrap();
        while messages.len() < count {
            let left = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("channel received {:?}", *messages));
            messages = self.arrived.wait_timeout(messages, left).unwrap().0;
        }
        messages.clone()
    }
}

// =======================================================================================
// Agents fixture
// =======================================================================================

const AGENTS_HOST: &str = "ssh-hd007r";
const AGENTS_BOOT: &str = "boot-hd007r-5";
const PANE: &str = "w1:p1";

fn agents_identity() -> LiveIdentity {
    LiveIdentity {
        endpoint: AGENTS_HOST.into(),
        session: "hd007r-remote".into(),
        connection_generation: 9,
        boot_id: AGENTS_BOOT.into(),
    }
}

#[derive(Default)]
struct AgentsHostFake {
    gateway_hold: Hold,
    prompt_hold: Arc<Hold>,
    gateway_calls: AtomicUsize,
    prompts: Arc<AtomicUsize>,
}

struct AgentsGateway {
    prompt_hold: Arc<Hold>,
    prompts: Arc<AtomicUsize>,
}

fn not_in_fake() -> RuntimeError {
    RuntimeError::new("unsupported_in_fake", "not used by the responsiveness seam")
}

impl RuntimeGateway for AgentsGateway {
    fn endpoint(&self) -> &str {
        AGENTS_HOST
    }
    fn identity(&self) -> Option<LiveIdentity> {
        Some(agents_identity())
    }
    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(not_in_fake())
    }
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let agent = json!({"pane_id": PANE, "workspace_id": "w1", "tab_id": "w1:t1",
            "name": "agente-hd007r", "agent": "pi", "agent_status": "idle"});
        match method {
            "server.agent_manifests" => Ok(json!({"manifests": [{"agent": "pi"}]})),
            "agent.list" => Ok(json!({"agents": [agent]})),
            "tab.list" => Ok(json!({"tabs": [{"tab_id": "w1:t1", "workspace_id": "w1",
                "label": "hd007r", "focused": true}]})),
            "agent.prompt" if params.get("text").is_some() => {
                self.prompt_hold.pass()?;
                self.prompts.fetch_add(1, Ordering::SeqCst);
                Err(RuntimeError::new("timeout", "sem resposta do agente").retryable())
            }
            _ => Err(RuntimeError::new(
                "invalid_request",
                "missing field `target`",
            )),
        }
    }
    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
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

impl AgentsHost for AgentsHostFake {
    fn endpoint(&self) -> String {
        AGENTS_HOST.into()
    }
    fn gateway(&self) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        self.gateway_hold.pass()?;
        self.gateway_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(AgentsGateway {
            prompt_hold: self.prompt_hold.clone(),
            prompts: self.prompts.clone(),
        }))
    }
    fn endpoint_methods(&self) -> Result<Vec<String>, RuntimeError> {
        Ok(vec!["pane.split".into()])
    }
    fn surface(&self) -> Option<PaneSurfaceFrame> {
        None
    }
    fn event_stream(&self, attached: &LiveIdentity) -> Result<Box<dyn EventStream>, RuntimeError> {
        Err(RuntimeError::new(
            "fake_events_off",
            format!("sem eventos em {}", attached.boot_id),
        ))
    }
}

fn geometry() -> GeometryDto {
    GeometryDto {
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
    }
}

fn agents() -> (Arc<AgentsHostFake>, AgentsState) {
    let host = Arc::new(AgentsHostFake::default());
    let state = AgentsState::hosted(host.clone());
    (host, state)
}

/// The watcher of an attach reports its (fake) unavailable event stream once on the channel
/// the attach received.
fn assert_events_unavailable(message: &Value) {
    assert_eq!(message["type"], "state", "{message}");
    assert_eq!(message["state"], "events_unavailable", "{message}");
    assert_eq!(message["error"]["code"], "fake_events_off", "{message}");
}

// =======================================================================================
// Agents
// =======================================================================================

/// Would catch: `agents_connect`/`agents_overview` still running on the IPC calling thread
/// (a Blocking command, or an `async fn` that locks and talks to the host inline) — the first
/// poll would block on the held host instead of returning `Pending`; a getter answered before
/// the connect that entered earlier; the connect retried against the host; or the channel of
/// the IPC call not being the one the service's watcher reports to.
#[test]
fn agents_connect_on_a_held_host_is_pending_and_the_overview_waits_off_the_calling_thread() {
    let exec = Executor::new();
    let (host, state) = agents();
    let events = Recorder::new();
    host.gateway_hold.arm();

    let mut connect = task(state.agents_connect(geometry(), events.channel()));
    exec.assert_pending(&mut connect, "agents_connect on a held host");
    host.gateway_hold.wait_entered(1);

    let mut overview = task(state.agents_overview(false));
    exec.assert_pending(&mut overview, "agents_overview behind a held connect");

    // Unrelated work on the same thread: a ready future and another (terminal) channel.
    let terminal = Recorder::new();
    let terminal_channel: Channel<Value> = terminal.channel();
    let mut unrelated = task(async move {
        terminal_channel
            .send(json!({"frame": "hd007r-terminal"}))
            .unwrap();
        7_u32
    });
    assert_eq!(exec.poll(&mut unrelated), Poll::Ready(7));
    assert_eq!(
        terminal.messages(),
        vec![json!({"frame": "hd007r-terminal"})]
    );
    exec.assert_pending(&mut connect, "agents_connect still held");
    exec.assert_pending(
        &mut overview,
        "agents_overview still behind the held connect",
    );
    assert!(events.messages().is_empty());

    host.gateway_hold.release();
    let connected = exec
        .wait(&mut connect, "agents_connect")
        .expect("attach once the host answers");
    assert_eq!(connected.identity, Some(agents_identity()));
    assert_ne!(connected.state, "disconnected");
    assert_eq!(connected.kinds, vec!["pi".to_owned()]);
    let seen = exec
        .wait(&mut overview, "agents_overview")
        .expect("overview never fails");
    assert_eq!(
        seen.identity,
        Some(agents_identity()),
        "the overview entered after the connect and must see it"
    );
    assert_eq!(host.gateway_calls.load(Ordering::SeqCst), 1, "no retry");

    let delivered = events.wait_for(1);
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert_events_unavailable(&delivered[0]);
}

/// Would catch: `agent_prompt` blocking the calling thread on the API, an unknown outcome
/// turned into an error or resent by the async wrapper, or the per-pane "unknown" fence lost
/// across wrapper calls (a second prompt reaching the engine without `resend_after_unknown`).
#[test]
fn agent_prompt_on_a_held_api_is_pending_and_resolves_once_as_unknown_without_resend() {
    let exec = Executor::new();
    let (host, state) = agents();
    state.attach_hosted(None).expect("attach (sync seam)");
    let target = QualifiedTarget::new(&agents_identity(), None, PANE);

    host.prompt_hold.arm();
    let mut prompt = task(state.agent_prompt(target.clone(), "olá hd007r".into(), false));
    exec.assert_pending(&mut prompt, "agent_prompt on a held API");
    host.prompt_hold.wait_entered(1);
    exec.assert_pending(&mut prompt, "agent_prompt still held");
    host.prompt_hold.release();

    match exec.wait(&mut prompt, "agent_prompt") {
        Ok(PromptOutcome::Unknown { error }) => assert_eq!(error.code, "timeout"),
        other => panic!("expected an unknown outcome, got {other:?}"),
    }
    assert_eq!(host.prompts.load(Ordering::SeqCst), 1);

    let mut again = task(state.agent_prompt(target, "olá hd007r".into(), false));
    let refused = exec
        .wait(&mut again, "second agent_prompt")
        .expect_err("refused until the user confirms a resend");
    assert_eq!(refused.code, "prompt_outcome_unknown");
    assert_eq!(host.prompts.load(Ordering::SeqCst), 1, "never resent");
}

/// Would catch: attach/detach applied in worker start order instead of entry order — an older
/// detach whose worker starts after a newer attach would detach it (and silence its channel);
/// a command dropped before running blocking every later command; a dropped connect attaching
/// anyway; or a held older connect committing after a newer detach.
#[test]
fn attach_and_detach_apply_in_entry_order_even_when_the_older_command_starts_late() {
    let exec = Executor::new();
    let (host, state) = agents();

    // 1. Older detach entered first but polled (started) only after the newer connect.
    let mut first = task(state.agents_connect(geometry(), Recorder::new().channel()));
    exec.wait(&mut first, "initial connect").expect("attach");
    let mut late_detach = task(state.agents_detach());
    let newest = Recorder::new();
    let mut connect = task(state.agents_connect(geometry(), newest.channel()));
    assert!(
        exec.stays_pending(&mut connect, Duration::from_millis(300)),
        "the newer connect must not overtake the detach that entered before it"
    );
    let detached = exec
        .wait(&mut late_detach, "late detach")
        .expect("detach never fails");
    assert_eq!(detached.state, "disconnected");
    let attached = exec
        .wait(&mut connect, "newer connect")
        .expect("attach after the older detach");
    assert_ne!(attached.state, "disconnected");
    let mut overview = task(state.agents_overview(false));
    let now = exec.wait(&mut overview, "overview").expect("overview");
    assert_ne!(
        now.state, "disconnected",
        "the newest command (attach) wins"
    );
    assert_eq!(now.identity, Some(agents_identity()));
    assert_events_unavailable(&newest.wait_for(1)[0]);
    let calls = host.gateway_calls.load(Ordering::SeqCst);

    // 2. A connect dropped before it ran neither blocks the next command nor attaches.
    let dropped_events = Recorder::new();
    let dropped = task(state.agents_connect(geometry(), dropped_events.channel()));
    let mut detach = task(state.agents_detach());
    drop(dropped);
    let detached = exec
        .wait(&mut detach, "detach after a dropped connect")
        .unwrap();
    assert_eq!(detached.state, "disconnected");
    assert_eq!(host.gateway_calls.load(Ordering::SeqCst), calls);
    assert!(dropped_events.messages().is_empty());

    // 3. A held older connect finishes first; the newer detach still has the last word.
    host.gateway_hold.arm();
    let mut held = task(state.agents_connect(geometry(), Recorder::new().channel()));
    exec.assert_pending(&mut held, "held connect");
    host.gateway_hold.wait_entered(1);
    let mut newer_detach = task(state.agents_detach());
    exec.assert_pending(&mut newer_detach, "detach behind a held connect");
    host.gateway_hold.release();
    exec.wait(&mut held, "held connect").expect("attach");
    let last = exec.wait(&mut newer_detach, "newer detach").unwrap();
    assert_eq!(last.state, "disconnected");
    let mut overview = task(state.agents_overview(false));
    assert_eq!(
        exec.wait(&mut overview, "overview").unwrap().state,
        "disconnected"
    );
}

// =======================================================================================
// Projects fixture
// =======================================================================================

const PROJECTS_HOST: &str = "ssh-hd007p";
const PROJECTS_SESSION: &str = "hd007p-remote";
const PROJECTS_BOOT: &str = "boot-hd007p-2";

#[derive(Default)]
struct ProjectHostsFake {
    hold: Hold,
    offline: Mutex<bool>,
    gateway_calls: AtomicUsize,
    api: Arc<Mutex<Vec<String>>>,
    focus: AtomicUsize,
    authorize: AtomicUsize,
}

struct ProjectGateway {
    api: Arc<Mutex<Vec<String>>>,
}

impl RuntimeGateway for ProjectGateway {
    fn endpoint(&self) -> &str {
        PROJECTS_HOST
    }
    fn identity(&self) -> Option<LiveIdentity> {
        Some(LiveIdentity {
            endpoint: PROJECTS_HOST.into(),
            session: PROJECTS_SESSION.into(),
            connection_generation: 4,
            boot_id: PROJECTS_BOOT.into(),
        })
    }
    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(not_in_fake())
    }
    fn take_events(&mut self) -> Option<Receiver<GatewayEvent>> {
        None
    }
    fn api_request(&self, method: &str, _params: Value) -> Result<Value, RuntimeError> {
        self.api.lock().unwrap().push(method.to_owned());
        match method {
            "workspace.list" => Ok(json!({"workspaces": []})),
            "workspace.create" => Ok(json!({"workspace": {"workspace_id": "w9"}})),
            "workspace.report_metadata" => Ok(json!({})),
            _ => Err(not_in_fake()),
        }
    }
    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(not_in_fake())
    }
    fn send_input(
        &self,
        _target: &QualifiedTarget,
        _events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        Err(not_in_fake())
    }
    fn resize(&self, _geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        Err(not_in_fake())
    }
    fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
        Err(not_in_fake())
    }
    fn detach(&mut self) {}
    fn is_connected(&self) -> bool {
        true
    }
}

impl ProjectGateways for ProjectHostsFake {
    fn gateway(&self, project: &ProjectRef) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        self.hold.pass()?;
        self.gateway_calls.fetch_add(1, Ordering::SeqCst);
        if *self.offline.lock().unwrap() {
            return Err(RuntimeError::new("host_offline_hd007p", "host fora do ar")
                .retryable()
                .with_endpoint(project.endpoint_profile_id.clone()));
        }
        Ok(Box::new(ProjectGateway {
            api: self.api.clone(),
        }))
    }
    fn focus_workspace(
        &self,
        _project: &ProjectRef,
        binding: &RuntimeBinding,
    ) -> Result<(), RuntimeError> {
        assert_eq!(binding.workspace_id, "w9");
        self.focus.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn authorize_root(&self, _project: &ProjectRef) -> Result<(), RuntimeError> {
        self.authorize.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct ProjectsFixture {
    _dir: tempfile::TempDir,
    prefs: std::path::PathBuf,
    herdr_config: std::path::PathBuf,
    project: ProjectRef,
    hosts: Arc<ProjectHostsFake>,
}

impl ProjectsFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("prefs");
        let herdr_config = dir.path().join("herdr-config");
        std::fs::create_dir_all(&prefs).unwrap();
        std::fs::create_dir_all(&herdr_config).unwrap();
        let project = ProjectStore::open(&prefs, &herdr_config)
            .unwrap()
            .create_project(ProjectDraft {
                label: "api hd007p".into(),
                endpoint_profile_id: PROJECTS_HOST.into(),
                session_name: PROJECTS_SESSION.into(),
                root: "/srv/hd007p".into(),
            })
            .unwrap();
        Self {
            _dir: dir,
            prefs,
            herdr_config,
            project,
            hosts: Arc::new(ProjectHostsFake::default()),
        }
    }

    fn state(&self) -> ProjectsState {
        let gateways: Arc<dyn ProjectGateways> = self.hosts.clone();
        ProjectsState::with_gateways(self.prefs.clone(), self.herdr_config.clone(), gateways)
    }

    fn api_count(&self, method: &str) -> usize {
        self.hosts
            .api
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.as_str() == method)
            .count()
    }
}

fn collection_names(snapshot: &herdr_desktop::project_store::ProjectsSnapshot) -> Vec<String> {
    snapshot
        .collections
        .iter()
        .map(|c| c.name.clone())
        .collect()
}

// =======================================================================================
// Projects
// =======================================================================================

/// Would catch: `project_open` blocking the calling thread on the host, `projects_list` or a
/// persisted collection change blocking on the service lock inline, clones of the managed
/// state holding separate stores/bindings (the list on the clone would miss the binding and a
/// fresh load would miss the collection), or the open focusing/authorizing/creating twice.
#[test]
fn project_open_on_a_held_host_is_pending_while_list_and_persistence_on_a_clone_wait_off_thread() {
    let exec = Executor::new();
    let fx = ProjectsFixture::new();
    let state = fx.state();
    let clone = state.clone();
    fx.hosts.hold.arm();

    let mut open = task(state.project_open(fx.project.id.clone()));
    exec.assert_pending(&mut open, "project_open on a held host");
    fx.hosts.hold.wait_entered(1);

    let mut list = task(clone.projects_list());
    exec.assert_pending(&mut list, "projects_list behind a held open");
    let mut create = task(clone.collection_create("favoritos hd007p".into()));
    exec.assert_pending(&mut create, "collection_create behind a held open");

    let mut unrelated = task(async { "hd007p-ready" });
    assert_eq!(exec.poll(&mut unrelated), Poll::Ready("hd007p-ready"));
    exec.assert_pending(&mut open, "project_open still held");
    exec.assert_pending(&mut list, "projects_list still behind the held open");

    fx.hosts.hold.release();
    let opened = exec
        .wait(&mut open, "project_open")
        .expect("open on the released host");
    assert_eq!(opened.result.outcome, OpenOutcome::Created);
    assert_eq!(opened.result.binding.workspace_id, "w9");
    assert_eq!(opened.result.binding.boot_id, PROJECTS_BOOT);

    let listed = exec.wait(&mut list, "projects_list").expect("list");
    let binding = listed
        .projects
        .iter()
        .find(|p| p.project.id == fx.project.id)
        .and_then(|p| p.binding.clone())
        .expect("the clone sees the binding established by the open that entered first");
    assert_eq!(binding.workspace_id, "w9");
    let created = exec.wait(&mut create, "collection_create").expect("create");
    assert_eq!(collection_names(&created), vec!["favoritos hd007p"]);

    assert_eq!(fx.hosts.gateway_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fx.api_count("workspace.create"), 1);
    assert_eq!(fx.hosts.focus.load(Ordering::SeqCst), 1);
    assert_eq!(fx.hosts.authorize.load(Ordering::SeqCst), 1);

    // The collection written through the clone is on disk; bindings never are.
    let reloaded = ProjectsState::new(fx.prefs.clone(), fx.herdr_config.clone());
    let mut fresh = task(reloaded.projects_list());
    let fresh = exec.wait(&mut fresh, "reloaded list").expect("reload");
    assert_eq!(collection_names(&fresh), vec!["favoritos hd007p"]);
    assert_eq!(fresh.projects.len(), 1);
    assert!(fresh.projects[0].binding.is_none());
}

/// Would catch: a host error swallowed, retried or turned into a join/panic error by the async
/// wrapper; a store validation error lost; or commands of one store applied in worker start
/// order (the collection that entered second would be listed first).
#[test]
fn project_errors_arrive_once_and_store_changes_apply_in_entry_order() {
    let exec = Executor::new();
    let fx = ProjectsFixture::new();
    let state = fx.state();
    *fx.hosts.offline.lock().unwrap() = true;

    let mut open = task(state.project_open(fx.project.id.clone()));
    let error = exec
        .wait(&mut open, "project_open on an offline host")
        .expect_err("offline host");
    assert_eq!(error.code, "host_offline_hd007p");
    assert_eq!(error.endpoint.as_deref(), Some(PROJECTS_HOST));
    assert_eq!(fx.hosts.gateway_calls.load(Ordering::SeqCst), 1, "no retry");
    assert_eq!(fx.api_count("workspace.list"), 0);

    let mut missing = task(state.collection_add_project(
        "colecao-inexistente".into(),
        fx.project.id.clone(),
        None,
    ));
    assert_eq!(
        exec.wait(&mut missing, "add to a missing collection")
            .expect_err("unknown collection")
            .code,
        "collection_not_found"
    );

    let clone = state.clone();
    let mut older = task(clone.collection_create("primeira hd007p".into()));
    let mut newer = task(state.collection_create("segunda hd007p".into()));
    assert!(
        exec.stays_pending(&mut newer, Duration::from_millis(300)),
        "the newer change must not overtake the one that entered before it"
    );
    exec.wait(&mut older, "older create").expect("create");
    let last = exec.wait(&mut newer, "newer create").expect("create");
    assert_eq!(
        collection_names(&last),
        vec!["primeira hd007p", "segunda hd007p"]
    );
}
