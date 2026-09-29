//! Spec 007 — routing of project opening through the selected host and dynamic root
//! authorization of local files (contracts, no E2E).
//!
//! Seam: `ProjectsState::with_gateways` + `ProjectsState::open` with fake gateways (one fake
//! engine per host, both handing out the *same* workspace/pane ids) and `FilesState` over a
//! real temp directory. The backend modules are compiled through `#[path]`, as in the
//! feature suites; the composition in `lib.rs` supplies the concrete adapter later.
//!
//! Wrong behaviours each test would catch are named in the test doc comments.

#[allow(dead_code)]
#[path = "../src/project_store.rs"]
mod project_store;

#[allow(dead_code)]
#[path = "../src/files/local.rs"]
mod files_local;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use herdr_client::protocol::wire::ClientPaneInputEvent;
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, ProjectRef, QualifiedTarget,
    RuntimeBinding, RuntimeError, RuntimeGateway, SurfaceGeometry,
};
use project_store::{
    OpenOutcome, ProjectDraft, ProjectGateways, ProjectStore, ProjectsState, PROJECT_TOKEN,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------
// Fake hosts
// ---------------------------------------------------------------------------------------

const LOCAL: &str = "local";
const SSH: &str = "ssh-prod";
const LOCAL_SESSION: &str = "desk-local";
const SSH_SESSION: &str = "desk-remote";

#[derive(Debug, Default)]
struct Engine {
    endpoint: String,
    session: String,
    boot_id: String,
    generation: u64,
    next_workspace: u32,
    /// `(workspace_id, project token)`.
    workspaces: Vec<(String, Option<String>)>,
    /// Every API method this engine received.
    calls: Vec<String>,
    /// When set, serving `workspace.list` reboots the engine to this boot id and forgets its
    /// workspaces except for `w1`, which a fresh boot creates again (same id, other boot).
    reboot_on_list: Option<String>,
}

type Shared<T> = Arc<Mutex<T>>;

fn engine(endpoint: &str, session: &str, boot: &str) -> Shared<Engine> {
    Arc::new(Mutex::new(Engine {
        endpoint: endpoint.into(),
        session: session.into(),
        boot_id: boot.into(),
        generation: 1,
        next_workspace: 1,
        ..Engine::default()
    }))
}

fn calls(engine: &Shared<Engine>) -> Vec<String> {
    engine.lock().unwrap().calls.clone()
}

fn count(engine: &Shared<Engine>, method: &str) -> usize {
    calls(engine)
        .iter()
        .filter(|m| m.as_str() == method)
        .count()
}

/// Gateway view of one engine. `endpoint`/`session` may be forged to simulate a host that
/// handed out the wrong connection.
struct FakeGateway {
    endpoint: String,
    session: String,
    engine: Shared<Engine>,
}

fn unsupported() -> RuntimeError {
    RuntimeError::new("unsupported_in_fake", "not used by the projects seam")
}

impl RuntimeGateway for FakeGateway {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn identity(&self) -> Option<LiveIdentity> {
        let engine = self.engine.lock().unwrap();
        Some(LiveIdentity {
            endpoint: self.endpoint.clone(),
            session: self.session.clone(),
            connection_generation: engine.generation,
            boot_id: engine.boot_id.clone(),
        })
    }

    fn connect(&mut self, _options: ConnectOptions) -> Result<Negotiated, RuntimeError> {
        Err(unsupported())
    }

    fn take_events(&mut self) -> Option<std::sync::mpsc::Receiver<GatewayEvent>> {
        None
    }

    fn api_request(&self, method: &str, params: Value) -> Result<Value, RuntimeError> {
        let mut engine = self.engine.lock().unwrap();
        engine.calls.push(method.to_owned());
        match method {
            "workspace.list" => {
                if let Some(boot) = engine.reboot_on_list.take() {
                    engine.boot_id = boot;
                    engine.workspaces = vec![("w1".into(), None)];
                }
                Ok(json!({
                    "workspaces": engine.workspaces.iter().map(|(id, project)| {
                        let mut tokens = serde_json::Map::new();
                        if let Some(project) = project {
                            tokens.insert(PROJECT_TOKEN.into(), json!(project));
                        }
                        json!({ "workspace_id": id, "tokens": tokens })
                    }).collect::<Vec<_>>(),
                }))
            }
            "workspace.create" => {
                let id = format!("w{}", engine.next_workspace);
                engine.next_workspace += 1;
                engine.workspaces.push((id.clone(), None));
                Ok(json!({
                    "workspace": { "workspace_id": id },
                    "root_pane": { "pane_id": format!("{id}:p1"), "workspace_id": id },
                }))
            }
            "workspace.report_metadata" => {
                let id = params["workspace_id"].as_str().unwrap().to_owned();
                let project = params["tokens"][PROJECT_TOKEN].as_str().map(str::to_owned);
                let workspace = engine
                    .workspaces
                    .iter_mut()
                    .find(|(w, _)| *w == id)
                    .expect("metadata for an existing workspace");
                workspace.1 = project;
                Ok(json!({ "type": "ok" }))
            }
            other => Err(RuntimeError::new("unexpected_method", other.to_owned())),
        }
    }

    fn endpoint_request(&self, _method: &str, _params: Value) -> Result<Value, RuntimeError> {
        Err(unsupported())
    }

    fn send_input(
        &self,
        _target: &QualifiedTarget,
        _events: Vec<ClientPaneInputEvent>,
    ) -> Result<(), RuntimeError> {
        Err(unsupported())
    }

    fn resize(&self, _geometry: SurfaceGeometry) -> Result<(), RuntimeError> {
        Err(unsupported())
    }

    fn set_focus(&self, _focused: bool) -> Result<(), RuntimeError> {
        Err(unsupported())
    }

    fn detach(&mut self) {}

    fn is_connected(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostCall {
    Gateway {
        project: String,
    },
    Focus {
        project: String,
        binding: RuntimeBinding,
    },
    Authorize {
        project: String,
        root: String,
    },
}

/// Registered hosts by endpoint profile id. What the fake hands out and how it fails is
/// configurable per test.
#[derive(Default)]
struct FakeHosts {
    engines: BTreeMap<String, Shared<Engine>>,
    /// Endpoint whose `gateway()` fails with this error.
    gateway_error: Option<(String, RuntimeError)>,
    /// Forged `(endpoint, session)` handed out instead of the real connection.
    forged: Option<(String, String)>,
    focus_error: Mutex<Option<RuntimeError>>,
    authorize_error: Mutex<Option<RuntimeError>>,
    log: Mutex<Vec<HostCall>>,
}

impl FakeHosts {
    fn log(&self) -> Vec<HostCall> {
        self.log.lock().unwrap().clone()
    }

    fn focus_calls(&self) -> Vec<(String, RuntimeBinding)> {
        self.log()
            .into_iter()
            .filter_map(|call| match call {
                HostCall::Focus { project, binding } => Some((project, binding)),
                _ => None,
            })
            .collect()
    }

    fn authorize_calls(&self) -> Vec<(String, String)> {
        self.log()
            .into_iter()
            .filter_map(|call| match call {
                HostCall::Authorize { project, root } => Some((project, root)),
                _ => None,
            })
            .collect()
    }
}

impl ProjectGateways for FakeHosts {
    fn gateway(&self, project: &ProjectRef) -> Result<Box<dyn RuntimeGateway>, RuntimeError> {
        self.log.lock().unwrap().push(HostCall::Gateway {
            project: project.id.clone(),
        });
        if let Some((endpoint, error)) = &self.gateway_error {
            if *endpoint == project.endpoint_profile_id {
                return Err(error.clone());
            }
        }
        let engine = self
            .engines
            .get(&project.endpoint_profile_id)
            .ok_or_else(|| RuntimeError::new("endpoint_unavailable", "no such fake host"))?;
        let (endpoint, session) = match &self.forged {
            Some(forged) => forged.clone(),
            None => {
                let engine = engine.lock().unwrap();
                (engine.endpoint.clone(), engine.session.clone())
            }
        };
        Ok(Box::new(FakeGateway {
            endpoint,
            session,
            engine: engine.clone(),
        }))
    }

    fn focus_workspace(
        &self,
        project: &ProjectRef,
        binding: &RuntimeBinding,
    ) -> Result<(), RuntimeError> {
        self.log.lock().unwrap().push(HostCall::Focus {
            project: project.id.clone(),
            binding: binding.clone(),
        });
        match self.focus_error.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn authorize_root(&self, project: &ProjectRef) -> Result<(), RuntimeError> {
        self.log.lock().unwrap().push(HostCall::Authorize {
            project: project.id.clone(),
            root: project.root.clone(),
        });
        match self.authorize_error.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    prefs: PathBuf,
    herdr_config: PathBuf,
    local: ProjectRef,
    ssh: ProjectRef,
    local_engine: Shared<Engine>,
    ssh_engine: Shared<Engine>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("prefs");
        // Engine config dir without any socket: a Local connection attempt could never succeed.
        let herdr_config = dir.path().join("herdr-config");
        std::fs::create_dir_all(&prefs).unwrap();
        std::fs::create_dir_all(&herdr_config).unwrap();
        let mut store = ProjectStore::open(&prefs, &herdr_config).unwrap();
        let local = store
            .create_project(ProjectDraft {
                label: "api local".into(),
                endpoint_profile_id: LOCAL.into(),
                session_name: LOCAL_SESSION.into(),
                root: "/srv/local-api".into(),
            })
            .unwrap();
        let ssh = store
            .create_project(ProjectDraft {
                label: "api prod".into(),
                endpoint_profile_id: SSH.into(),
                session_name: SSH_SESSION.into(),
                root: "/home/deploy/api".into(),
            })
            .unwrap();
        Self {
            _dir: dir,
            prefs,
            herdr_config,
            local,
            ssh,
            local_engine: engine(LOCAL, LOCAL_SESSION, "boot-local-A"),
            ssh_engine: engine(SSH, SSH_SESSION, "boot-ssh-B"),
        }
    }

    fn hosts(&self) -> FakeHosts {
        let mut engines = BTreeMap::new();
        engines.insert(LOCAL.to_owned(), self.local_engine.clone());
        engines.insert(SSH.to_owned(), self.ssh_engine.clone());
        FakeHosts {
            engines,
            ..FakeHosts::default()
        }
    }

    fn state(&self, hosts: &Arc<FakeHosts>) -> ProjectsState {
        let gateways: Arc<dyn ProjectGateways> = hosts.clone();
        ProjectsState::with_gateways(self.prefs.clone(), self.herdr_config.clone(), gateways)
    }

    fn binding_of(state: &ProjectsState, project: &ProjectRef) -> Option<RuntimeBinding> {
        state
            .snapshot()
            .unwrap()
            .projects
            .into_iter()
            .find(|dto| dto.project.id == project.id)
            .and_then(|dto| dto.binding)
    }
}

// ---------------------------------------------------------------------------------------
// Projects: routing through the selected host
// ---------------------------------------------------------------------------------------

mod projects {
    use super::*;

    /// Catches: opening an SSH project through the Local connection (or any fallback), and
    /// confusing two hosts whose engines hand out the same `w1`/`w1:p1` ids.
    #[test]
    fn ssh_and_local_projects_with_same_workspace_ids_each_reach_only_their_own_host() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        let state = fx.state(&hosts);

        let opened = state.open(&fx.ssh.id).expect("ssh project opens");
        assert_eq!(opened.result.outcome, OpenOutcome::Created);
        assert_eq!(opened.result.binding.workspace_id, "w1");
        assert_eq!(opened.result.binding.boot_id, "boot-ssh-B");
        assert_eq!(
            calls(&fx.ssh_engine),
            [
                "workspace.list",
                "workspace.create",
                "workspace.report_metadata"
            ]
        );
        assert!(
            calls(&fx.local_engine).is_empty(),
            "no command may reach the Local host"
        );
        let ssh_binding = opened.result.binding.clone();
        assert_eq!(
            hosts.log(),
            vec![
                HostCall::Gateway {
                    project: fx.ssh.id.clone()
                },
                HostCall::Focus {
                    project: fx.ssh.id.clone(),
                    binding: ssh_binding.clone()
                },
                HostCall::Authorize {
                    project: fx.ssh.id.clone(),
                    root: "/home/deploy/api".into()
                },
            ]
        );

        let local = state.open(&fx.local.id).expect("local project opens");
        assert_eq!(local.result.binding.workspace_id, "w1");
        assert_eq!(local.result.binding.boot_id, "boot-local-A");
        assert_eq!(count(&fx.ssh_engine, "workspace.create"), 1);
        assert_eq!(count(&fx.local_engine, "workspace.create"), 1);
        assert_eq!(
            hosts.focus_calls().last().unwrap(),
            &(fx.local.id.clone(), local.result.binding.clone())
        );
        assert_eq!(
            Fixture::binding_of(&state, &fx.ssh),
            Some(ssh_binding),
            "opening Local leaves the SSH binding untouched"
        );
    }

    /// Catches: a hosted state that still dials a Local connection itself, or that answers a
    /// failed host with some other host's error/binding.
    #[test]
    fn gateway_error_is_returned_without_local_connection_focus_or_authorization() {
        let fx = Fixture::new();
        let mut hosts = fx.hosts();
        hosts.gateway_error = Some((
            LOCAL.into(),
            RuntimeError::new("fake_local_offline", "host offline").retryable(),
        ));
        let hosts = Arc::new(hosts);
        let state = fx.state(&hosts);

        let error = state.open(&fx.local.id).expect_err("host is offline");
        assert_eq!(error.code, "fake_local_offline");
        assert!(calls(&fx.local_engine).is_empty());
        assert!(calls(&fx.ssh_engine).is_empty());
        assert_eq!(
            hosts.log(),
            vec![HostCall::Gateway {
                project: fx.local.id.clone()
            }]
        );
        assert_eq!(Fixture::binding_of(&state, &fx.local), None);
        assert_eq!(
            state.snapshot().unwrap().projects.len(),
            2,
            "errors never drop stored projects"
        );
    }

    /// Catches: trusting whatever connection the host hands out for the project.
    #[test]
    fn wrong_endpoint_or_session_from_host_sends_nothing_and_skips_focus_and_authorization() {
        for (forged, code) in [
            ((LOCAL, SSH_SESSION), "target_endpoint_mismatch"),
            ((SSH, LOCAL_SESSION), "target_session_mismatch"),
        ] {
            let fx = Fixture::new();
            let mut hosts = fx.hosts();
            hosts.forged = Some((forged.0.into(), forged.1.into()));
            let hosts = Arc::new(hosts);
            let state = fx.state(&hosts);

            let error = state
                .open(&fx.ssh.id)
                .expect_err("forged connection refused");
            assert_eq!(error.code, code);
            assert!(calls(&fx.ssh_engine).is_empty(), "{code}: no API call");
            assert!(calls(&fx.local_engine).is_empty(), "{code}: no API call");
            assert!(hosts.focus_calls().is_empty(), "{code}: no focus");
            assert!(
                hosts.authorize_calls().is_empty(),
                "{code}: no authorization"
            );
            assert_eq!(Fixture::binding_of(&state, &fx.ssh), None);
        }
    }

    /// Catches: re-creating the workspace on reopen, or focusing/authorizing zero or several
    /// times per open.
    #[test]
    fn reopening_reuses_and_focuses_and_authorizes_exactly_once_per_open() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        let state = fx.state(&hosts);

        let first = state.open(&fx.ssh.id).unwrap();
        let second = state.open(&fx.ssh.id).unwrap();
        assert_eq!(second.result.outcome, OpenOutcome::Reused);
        assert_eq!(second.result.binding, first.result.binding);
        assert_eq!(count(&fx.ssh_engine, "workspace.create"), 1);
        assert_eq!(count(&fx.ssh_engine, "workspace.report_metadata"), 1);
        assert_eq!(
            hosts.focus_calls(),
            vec![
                (fx.ssh.id.clone(), first.result.binding.clone()),
                (fx.ssh.id.clone(), first.result.binding.clone()),
            ]
        );
        assert_eq!(hosts.authorize_calls().len(), 2);
        assert!(calls(&fx.local_engine).is_empty());
    }

    /// Catches: a new GUI state that recreates instead of rediscovering the tagged workspace.
    #[test]
    fn fresh_state_rediscovers_the_tagged_workspace_on_the_ssh_host() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        let first = fx.state(&hosts).open(&fx.ssh.id).unwrap();

        let restarted = fx.state(&hosts);
        let again = restarted.open(&fx.ssh.id).unwrap();
        assert_eq!(again.result.outcome, OpenOutcome::Rediscovered);
        assert_eq!(
            again.result.binding.workspace_id,
            first.result.binding.workspace_id
        );
        assert_eq!(count(&fx.ssh_engine, "workspace.create"), 1);
        assert_eq!(hosts.focus_calls().len(), 2);
    }

    /// Catches: focusing with a binding of the previous connection generation after the host
    /// reconnected (the action must carry the renewed generation).
    #[test]
    fn reconnect_before_reopen_focuses_only_the_renewed_binding() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        let state = fx.state(&hosts);
        let first = state.open(&fx.ssh.id).unwrap();

        fx.ssh_engine.lock().unwrap().generation = 2;
        let again = state.open(&fx.ssh.id).unwrap();
        assert_eq!(again.result.invalidated, Some(first.result.binding.clone()));
        assert_eq!(again.result.binding.connection_generation, 2);
        let focused: Vec<u64> = hosts
            .focus_calls()
            .iter()
            .map(|(_, binding)| binding.connection_generation)
            .collect();
        assert_eq!(focused, vec![1, 2]);
    }

    /// Catches: sending focus to `w1` of a new boot because the rebooted engine reused the id
    /// while the project was being reopened.
    #[test]
    fn reboot_during_reopen_sends_no_focus_nor_authorization() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        let state = fx.state(&hosts);
        let first = state.open(&fx.ssh.id).unwrap();
        assert_eq!(first.result.binding.workspace_id, "w1");

        fx.ssh_engine.lock().unwrap().reboot_on_list = Some("boot-ssh-C".into());
        let error = state.open(&fx.ssh.id).expect_err("stale boot refused");
        assert!(
            error.code == "target_boot_stale",
            "unexpected error {}",
            error.code
        );
        assert!(error.retryable);
        assert_eq!(hosts.focus_calls().len(), 1, "no focus after the reboot");
        assert_eq!(hosts.authorize_calls().len(), 1);
        assert_eq!(count(&fx.ssh_engine, "workspace.create"), 1);
        assert!(calls(&fx.local_engine).is_empty());
    }

    /// Catches: dropping the binding just created when focus fails, retrying the focus
    /// automatically, or authorizing the root of a project whose focus failed.
    #[test]
    fn focus_failure_keeps_binding_and_is_not_retried_nor_authorized() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        *hosts.focus_error.lock().unwrap() =
            Some(RuntimeError::new("result_unknown", "focus timed out"));
        let state = fx.state(&hosts);

        let error = state.open(&fx.ssh.id).expect_err("focus failed");
        assert_eq!(error.code, "result_unknown");
        assert_eq!(hosts.focus_calls().len(), 1);
        assert!(hosts.authorize_calls().is_empty());
        let binding = Fixture::binding_of(&state, &fx.ssh).expect("binding kept");
        assert_eq!(binding.workspace_id, "w1");

        let again = state.open(&fx.ssh.id).unwrap();
        assert_eq!(again.result.outcome, OpenOutcome::Reused);
        assert_eq!(count(&fx.ssh_engine, "workspace.create"), 1);
        assert_eq!(hosts.focus_calls().len(), 2);
        assert_eq!(hosts.authorize_calls().len(), 1);
    }

    /// Catches: dropping the binding or retrying when root authorization fails.
    #[test]
    fn authorization_failure_keeps_binding_and_is_not_retried() {
        let fx = Fixture::new();
        let hosts = Arc::new(fx.hosts());
        *hosts.authorize_error.lock().unwrap() =
            Some(RuntimeError::new("file_uri_invalid", "root missing"));
        let state = fx.state(&hosts);

        let error = state.open(&fx.local.id).expect_err("authorization failed");
        assert_eq!(error.code, "file_uri_invalid");
        assert_eq!(hosts.focus_calls().len(), 1);
        assert_eq!(hosts.authorize_calls().len(), 1);
        assert!(Fixture::binding_of(&state, &fx.local).is_some());
        assert!(calls(&fx.ssh_engine).is_empty());
    }

    /// Catches: the harness mode (`ProjectsState::new`) silently opening remote projects.
    #[test]
    fn harness_mode_still_reports_remote_endpoints_unavailable() {
        let fx = Fixture::new();
        let state = ProjectsState::new(fx.prefs.clone(), fx.herdr_config.clone());
        let error = state
            .open(&fx.ssh.id)
            .expect_err("no SSH adapter in harness mode");
        assert_eq!(error.code, "endpoint_unavailable");
        assert!(calls(&fx.ssh_engine).is_empty());
    }
}

// ---------------------------------------------------------------------------------------
// Files: dynamic root authorization
// ---------------------------------------------------------------------------------------

mod files {
    use super::files_local::{FilesState, SaveOutcome};
    use super::*;
    use herdr_client::FileUri;

    struct Tree {
        _dir: tempfile::TempDir,
        base: PathBuf,
        first: PathBuf,
        second: PathBuf,
        outside: PathBuf,
    }

    fn tree() -> Tree {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let first = base.join("first");
        let second = base.join("second");
        let outside = base.join("outside");
        for (root, name, body) in [
            (&first, "a.txt", "alpha\n"),
            (&second, "b.txt", "bravo\n"),
            (&outside, "secret.txt", "secret\n"),
        ] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join(name), body).unwrap();
        }
        Tree {
            _dir: dir,
            base,
            first,
            second,
            outside,
        }
    }

    fn uri(path: &Path) -> FileUri {
        FileUri::local(path.to_string_lossy().into_owned())
    }

    fn denied(state: &FilesState, path: &Path) -> bool {
        matches!(state.read(&uri(path)), Err(e) if e.code == "path_outside_root")
    }

    /// Catches: an initial state that authorizes cwd/home/anything before a project opens, and
    /// changing the approved `new(vec![])` refusal.
    #[test]
    fn empty_state_denies_everything_and_new_without_roots_still_refuses() {
        let t = tree();
        let state = FilesState::empty();
        assert!(state.authorized_roots().is_empty());
        assert!(denied(&state, &t.first.join("a.txt")));
        let cwd = std::env::current_dir().unwrap();
        assert!(matches!(state.list(&uri(&cwd), None), Err(e) if e.code == "path_outside_root"));
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            assert!(
                matches!(state.list(&uri(&home), None), Err(e) if e.code == "path_outside_root")
            );
        }
        assert!(FilesState::new(vec![]).is_err());
    }

    /// Catches: authorizing a parent/sibling, or a prefix match (`first` authorizing
    /// `first-extra`).
    #[test]
    fn authorized_root_allows_only_that_root() {
        let t = tree();
        let sibling = t.base.join("first-extra");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("x.txt"), "x\n").unwrap();
        let state = FilesState::empty();

        let root = state.authorize_root(&t.first).expect("real directory");
        assert_eq!(root, t.first);
        assert_eq!(
            state.read(&uri(&t.first.join("a.txt"))).unwrap().content,
            "alpha\n"
        );
        assert!(state.list(&uri(&t.first), None).is_ok());
        assert!(denied(&state, &t.second.join("b.txt")));
        assert!(denied(&state, &sibling.join("x.txt")));
        assert!(matches!(state.list(&uri(&t.base), None), Err(e) if e.code == "path_outside_root"));
    }

    /// Catches: `..` traversal and symlinks (file and directory) that leave the root.
    #[cfg(unix)]
    #[test]
    fn traversal_and_symlink_escapes_are_denied() {
        let t = tree();
        std::os::unix::fs::symlink(&t.outside, t.first.join("dir-link")).unwrap();
        std::os::unix::fs::symlink(t.outside.join("secret.txt"), t.first.join("file-link"))
            .unwrap();
        let state = FilesState::empty();
        state.authorize_root(&t.first).unwrap();

        assert!(denied(&state, &t.first.join("../outside/secret.txt")));
        assert!(denied(&state, &t.first.join("dir-link/secret.txt")));
        assert!(denied(&state, &t.first.join("file-link")));
        assert!(matches!(
            state.list(&uri(&t.first.join("dir-link")), None),
            Err(e) if e.code == "path_outside_root"
        ));
    }

    /// Catches: adding a root that rebuilds the provider and drops live snapshots (breaking
    /// save/recovery of open tabs), loses the first root, or disables conflict detection; and
    /// a clone that duplicates state instead of sharing it.
    #[test]
    fn second_root_preserves_first_root_snapshots_edits_conflicts_and_recovery() {
        let t = tree();
        let state = FilesState::empty();
        let shared = state.clone();
        state.authorize_root(&t.first).unwrap();
        let a = t.first.join("a.txt");
        let clean = state.read(&uri(&a)).unwrap();
        let dirty = state.read(&uri(&a)).unwrap();

        shared.authorize_root(&t.second).unwrap();
        assert_eq!(
            state.authorized_roots(),
            vec![t.first.clone(), t.second.clone()]
        );
        assert_eq!(shared.snapshot(&clean.id), Some(clean.clone()));
        assert_eq!(
            state.read(&uri(&t.second.join("b.txt"))).unwrap().content,
            "bravo\n"
        );

        match state.save(&clean.id, "alpha edited\n").unwrap() {
            SaveOutcome::Saved { snapshot } => assert_eq!(snapshot.content, "alpha edited\n"),
            other => panic!("expected save, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "alpha edited\n");
        assert!(matches!(
            shared.save(&dirty.id, "stale buffer\n").unwrap(),
            SaveOutcome::Conflict { .. }
        ));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "alpha edited\n");
        let copy = shared.save_recovery(&dirty.id, "stale buffer\n").unwrap();
        assert!(Path::new(&copy.path).starts_with(&t.first));
        assert_eq!(
            std::fs::read_to_string(&copy.path).unwrap(),
            "stale buffer\n"
        );
        assert!(denied(&state, &t.outside.join("secret.txt")));
    }

    /// Catches: authorizing twice registering duplicates (or a symlinked alias of the same
    /// directory as a new root).
    #[cfg(unix)]
    #[test]
    fn authorizing_the_same_root_is_idempotent() {
        let t = tree();
        let alias = t.base.join("alias");
        std::os::unix::fs::symlink(&t.first, &alias).unwrap();
        let state = FilesState::empty();
        state.authorize_root(&t.first).unwrap();
        state.authorize_root(&t.first).unwrap();
        assert_eq!(state.authorize_root(&alias).unwrap(), t.first);
        assert_eq!(state.authorized_roots(), vec![t.first.clone()]);
    }

    /// Catches: relative, empty, missing, non-directory or filesystem-root paths widening the
    /// authorized set (for example by resolving against cwd).
    #[test]
    fn relative_or_invalid_roots_are_refused_and_do_not_widen() {
        let t = tree();
        let state = FilesState::empty();
        state.authorize_root(&t.first).unwrap();
        let filesystem_root = t
            .base
            .ancestors()
            .last()
            .expect("filesystem root")
            .to_path_buf();
        for invalid in [
            PathBuf::from(""),
            PathBuf::from("second"),
            PathBuf::from("./outside"),
            t.base.join("missing"),
            t.second.join("b.txt"),
            filesystem_root,
        ] {
            let error = state
                .authorize_root(&invalid)
                .expect_err(&format!("{} must be refused", invalid.display()));
            assert_eq!(error.code, "file_uri_invalid", "{}", invalid.display());
        }
        assert_eq!(state.authorized_roots(), vec![t.first.clone()]);
        assert!(denied(&state, &t.second.join("b.txt")));
        assert!(denied(&state, &t.outside.join("secret.txt")));
    }
}
