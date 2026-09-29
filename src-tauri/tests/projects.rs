//! Spec 002 — collections and local project opening (seam: store + service with a fake
//! gateway that records every API command; one native E2E against a disposable session).
//!
//! The backend module is compiled here through `#[path]`: composing it into
//! `src-tauri/src/lib.rs` belongs to spec 007.
//!
//! AC-002-01: associations and their order survive a reload (restart) with distinct UUIDs;
//!            loading issues no runtime command and persists no runtime binding.
//! AC-002-02: removing one association keeps the other and the live binding; no command
//!            (in particular `workspace.close`) reaches the engine.
//! AC-002-03: opening without a binding creates/rediscovers the workspace on the declared
//!            endpoint/session and binds the current boot/generation; reopening in the same
//!            boot creates nothing. Divergent boot/generation invalidates the binding and
//!            endpoint/session mismatch refuses without any call (no Local fallback).
//! AC-044-01: the v2 store migrates to v3 keeping projects and groups untouched and adding
//!            `workspace_prefs`; `workspace_pref_set` writes/updates one entry, refuses a
//!            colour outside the palette, an empty or oversized root and an unknown endpoint,
//!            and drops an entry that is neither coloured, pinned nor hidden.
//! AC-045-02/03: a collection carries an optional `color` and a `collapsed` flag; both stay
//!            absent from the file until they say something, renaming validates the name, the
//!            colour must belong to the palette and deleting a collection keeps its projects
//!            (they fall back to "Sem coleção") and every other collection.

#[allow(dead_code)]
#[path = "../src/project_store.rs"]
mod project_store;

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../scripts/feature-harness/window.rs"]
mod window_harness;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use herdr_client::protocol::wire::ClientPaneInputEvent;
use herdr_client::{
    ConnectOptions, GatewayEvent, LiveIdentity, Negotiated, QualifiedTarget, RuntimeError,
    RuntimeGateway, SurfaceGeometry,
};
use project_store::{
    atomic_write, migrate, OpenOutcome, ProjectDraft, ProjectService, ProjectStore, RecentFolder,
    WorkspacePrefPatch, GROUP_PALETTE, MAX_RECENT_FOLDERS_PER_HOST, METADATA_SOURCE, PROJECT_TOKEN,
    STORE_FILE, STORE_VERSION,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------
// Fake engine + gateway
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FakeWorkspace {
    id: String,
    label: String,
    cwd: String,
    tokens: BTreeMap<String, String>,
}

/// Engine state shared by every gateway instance (a GUI restart builds a new gateway but
/// talks to the same engine).
#[derive(Debug, Default)]
struct FakeEngine {
    boot_id: String,
    next_workspace: u32,
    workspaces: Vec<FakeWorkspace>,
    /// Every `(endpoint, session, method, params)` the engine received.
    calls: Vec<(String, String, String, Value)>,
    fail_with: Option<RuntimeError>,
    /// When set, `workspace.create` also reboots the engine to this boot id.
    reboot_on_create: Option<String>,
}

impl FakeEngine {
    fn new(boot: &str) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            boot_id: boot.into(),
            next_workspace: 1,
            ..Self::default()
        }))
    }
}

struct FakeGateway {
    endpoint: String,
    session: String,
    generation: u64,
    engine: Arc<Mutex<FakeEngine>>,
    identity_known: bool,
}

impl FakeGateway {
    fn local(session: &str, generation: u64, engine: &Arc<Mutex<FakeEngine>>) -> Self {
        Self {
            endpoint: "local".into(),
            session: session.into(),
            generation,
            engine: engine.clone(),
            identity_known: true,
        }
    }
}

fn unsupported() -> RuntimeError {
    RuntimeError::new("unsupported_in_fake", "not used by the projects seam")
}

impl RuntimeGateway for FakeGateway {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn identity(&self) -> Option<LiveIdentity> {
        if !self.identity_known {
            return None;
        }
        Some(LiveIdentity {
            endpoint: self.endpoint.clone(),
            session: self.session.clone(),
            connection_generation: self.generation,
            boot_id: self.engine.lock().unwrap().boot_id.clone(),
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
        engine.calls.push((
            self.endpoint.clone(),
            self.session.clone(),
            method.to_owned(),
            params.clone(),
        ));
        if let Some(error) = engine.fail_with.clone() {
            return Err(error);
        }
        match method {
            "workspace.list" => Ok(json!({
                "type": "workspace_list",
                "workspaces": engine.workspaces.iter().map(|w| json!({
                    "workspace_id": w.id,
                    "label": w.label,
                    "tokens": w.tokens,
                })).collect::<Vec<_>>(),
            })),
            "workspace.create" => {
                let id = format!("w{}", engine.next_workspace);
                engine.next_workspace += 1;
                engine.workspaces.push(FakeWorkspace {
                    id: id.clone(),
                    label: params["label"].as_str().unwrap_or_default().into(),
                    cwd: params["cwd"].as_str().unwrap_or_default().into(),
                    tokens: BTreeMap::new(),
                });
                if let Some(boot) = engine.reboot_on_create.take() {
                    engine.boot_id = boot;
                }
                Ok(json!({
                    "type": "workspace_created",
                    "workspace": { "workspace_id": id, "label": params["label"] },
                    "tab": { "tab_id": format!("{id}:t1") },
                    "root_pane": { "pane_id": format!("{id}:p1"), "workspace_id": id },
                }))
            }
            "workspace.report_metadata" => {
                let id = params["workspace_id"].as_str().unwrap().to_owned();
                let source = params["source"].as_str().unwrap_or_default().to_owned();
                assert_eq!(source, METADATA_SOURCE);
                let workspace = engine
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == id)
                    .expect("metadata for an existing workspace");
                for (key, value) in params["tokens"].as_object().unwrap() {
                    workspace
                        .tokens
                        .insert(key.clone(), value.as_str().unwrap().to_owned());
                }
                Ok(json!({ "type": "ok" }))
            }
            other => Err(RuntimeError::new(
                "unexpected_method",
                format!("fake engine refuses {other}"),
            )),
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

fn methods(engine: &Arc<Mutex<FakeEngine>>) -> Vec<String> {
    engine
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|(_, _, m, _)| m.clone())
        .collect()
}

fn clear_calls(engine: &Arc<Mutex<FakeEngine>>) {
    engine.lock().unwrap().calls.clear();
}

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

struct Dirs {
    _root: tempfile::TempDir,
    prefs: std::path::PathBuf,
    herdr_config: std::path::PathBuf,
}

/// Separate preference and engine-config directories; the engine dir holds an
/// `endpoints.json` the store must never touch.
fn dirs() -> Dirs {
    let root = tempfile::tempdir().unwrap();
    let prefs = root.path().join("prefs");
    let herdr_config = root.path().join("herdr-config");
    std::fs::create_dir_all(herdr_config.join("sessions")).unwrap();
    std::fs::write(
        herdr_config.join("endpoints.json"),
        br#"{"engine":"owned","profiles":[]}"#,
    )
    .unwrap();
    Dirs {
        _root: root,
        prefs,
        herdr_config,
    }
}

fn draft(label: &str, session: &str, root: &str) -> ProjectDraft {
    ProjectDraft {
        label: label.into(),
        endpoint_profile_id: "local".into(),
        session_name: session.into(),
        root: root.into(),
    }
}

/// Collections created in non-alphabetical order (Produto before Pessoal) and projects whose
/// labels sort opposite to their insertion order, so any sorting on save/load is caught.
struct Seeded {
    service: ProjectService,
    produto: String,
    pessoal: String,
    a: String,
    b: String,
}

fn seeded(d: &Dirs) -> Seeded {
    let store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    let mut service = ProjectService::new(store);
    let produto = service.store_mut().create_collection("Produto").unwrap().id;
    let pessoal = service.store_mut().create_collection("Pessoal").unwrap().id;
    let a = service
        .store_mut()
        .create_project(draft("Zeta API", "hd-proj-a", "/srv/zeta"))
        .unwrap()
        .id;
    let b = service
        .store_mut()
        .create_project(draft("Alfa Web", "hd-proj-a", "/srv/alfa"))
        .unwrap()
        .id;
    service
        .store_mut()
        .add_to_collection(&produto, &a, None)
        .unwrap();
    service
        .store_mut()
        .add_to_collection(&pessoal, &a, None)
        .unwrap();
    service
        .store_mut()
        .add_to_collection(&produto, &b, None)
        .unwrap();
    Seeded {
        service,
        produto,
        pessoal,
        a,
        b,
    }
}

fn collection_view(store: &ProjectStore) -> Vec<(String, Vec<String>)> {
    store
        .document()
        .groups
        .iter()
        .map(|c| (c.name.clone(), c.project_ids.clone()))
        .collect()
}

fn dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = walk(dir)
        .into_iter()
        .map(|p| p.strip_prefix(dir).unwrap().display().to_string())
        .collect();
    names.sort();
    names
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

// ---------------------------------------------------------------------------------------
// AC-002-01 — persistence of associations and order
// ---------------------------------------------------------------------------------------

/// Would catch: associations kept only in memory, order sorted by name/label on save or
/// load, a second project reusing the first UUID, collections deduplicated by project.
#[test]
fn associations_and_their_order_survive_a_restart_with_distinct_uuids() {
    let d = dirs();
    let s = seeded(&d);
    assert_ne!(s.a, s.b, "each project gets its own UUID");
    for id in [&s.a, &s.b, &s.produto, &s.pessoal] {
        let parsed = uuid::Uuid::parse_str(id).expect("ids are UUIDs");
        assert_eq!(parsed.get_version_num(), 4, "{id}");
    }
    let before = collection_view(s.service.store());
    drop(s.service);

    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(collection_view(&reloaded), before);
    assert_eq!(
        collection_view(&reloaded),
        vec![
            ("Produto".to_owned(), vec![s.a.clone(), s.b.clone()]),
            ("Pessoal".to_owned(), vec![s.a.clone()]),
        ],
        "three associations in creation order"
    );
    let projects: Vec<(&str, &str)> = reloaded
        .document()
        .projects
        .iter()
        .map(|p| (p.id.as_str(), p.label.as_str()))
        .collect();
    assert_eq!(
        projects,
        [(s.a.as_str(), "Zeta API"), (s.b.as_str(), "Alfa Web")]
    );
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(raw["version"], STORE_VERSION);
}

/// Would catch: a restart that eagerly (re)opens projects, or bindings persisted to disk and
/// resurrected on load (the binding belongs to one connection, not to the project).
#[test]
fn restart_loads_no_binding_and_sends_no_runtime_command() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let gateway = FakeGateway::local("hd-proj-a", 3, &engine);
    let opened = s.service.open_project(&s.a, &gateway).unwrap();
    assert_eq!(opened.binding.workspace_id, "w1");
    drop(s.service);
    clear_calls(&engine);

    let service = ProjectService::new(ProjectStore::open(&d.prefs, &d.herdr_config).unwrap());
    assert!(service.binding(&s.a).is_none());
    let snapshot = serde_json::to_value(service.snapshot()).unwrap();
    assert!(snapshot["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["binding"].is_null()));
    assert!(
        methods(&engine).is_empty(),
        "loading the store must not talk to the engine"
    );
    let file = std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap();
    for runtime_value in ["w1", "boot-alpha", "workspace_id", "connection_generation"] {
        assert!(
            !file.contains(runtime_value),
            "store persisted runtime data {runtime_value}: {file}"
        );
    }
}

/// AC-011-01 / Planner contract: ProjectDto projects git branch for local projects
/// via `git symbolic-ref --short HEAD`, cached in ProjectService, None for SSH or non-repo.
#[test]
fn project_dto_projects_the_git_branch_for_local_projects() {
    let d = dirs();
    let repo_dir = d.prefs.join("local-git-repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .args(["init", "-b", "spec-branch"])
        .status()
        .unwrap();
    assert!(status.success());

    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    let p_local = store
        .create_project(ProjectDraft {
            label: "Local Repo".into(),
            endpoint_profile_id: "local".into(),
            session_name: "sess-a".into(),
            root: repo_dir.display().to_string(),
        })
        .unwrap();
    let p_ssh = store
        .create_project(ProjectDraft {
            label: "SSH Repo".into(),
            endpoint_profile_id: "dev-box".into(),
            session_name: "sess-b".into(),
            root: "/remote/path".into(),
        })
        .unwrap();

    let service = ProjectService::new(store);
    let snap = serde_json::to_value(service.snapshot()).unwrap();
    let projects = snap["projects"].as_array().unwrap();

    let local_dto = projects.iter().find(|p| p["id"] == p_local.id).unwrap();
    assert_eq!(local_dto["branch"], json!("spec-branch"));

    let ssh_dto = projects.iter().find(|p| p["id"] == p_ssh.id).unwrap();
    assert_eq!(ssh_dto["branch"], Value::Null);
}

/// Keyboard (one step) and drag (arbitrary index) share the same move operation.
/// Would catch: moves applied only in memory, off-by-one on the target index, or a move that
/// duplicates/drops a project.
#[test]
fn reordering_projects_and_collections_persists_the_new_order() {
    let d = dirs();
    let mut s = seeded(&d);
    let c = s
        .service
        .store_mut()
        .create_project(draft("Meio", "hd-proj-a", "/srv/meio"))
        .unwrap()
        .id;
    s.service
        .store_mut()
        .add_to_collection(&s.produto, &c, Some(1))
        .unwrap();
    assert_eq!(
        collection_view(s.service.store())[0].1,
        vec![s.a.clone(), c.clone(), s.b.clone()],
        "insert at explicit index"
    );
    // keyboard: move A one step down
    s.service
        .store_mut()
        .move_project(&s.produto, &s.a, 1)
        .unwrap();
    // drag: move B to the top
    s.service
        .store_mut()
        .move_project(&s.produto, &s.b, 0)
        .unwrap();
    s.service
        .store_mut()
        .move_collection(&s.pessoal, 0)
        .unwrap();
    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(
        collection_view(&reloaded),
        vec![
            ("Pessoal".to_owned(), vec![s.a.clone()]),
            (
                "Produto".to_owned(),
                vec![s.b.clone(), c.clone(), s.a.clone()]
            ),
        ]
    );
}

/// Negative contracts for the store operations. Would catch: duplicate association rows,
/// silent no-ops on unknown ids, empty collection names, moves of foreign projects.
#[test]
fn invalid_collection_operations_are_rejected_without_changing_the_file() {
    let d = dirs();
    let mut s = seeded(&d);
    let file_before = std::fs::read(d.prefs.join(STORE_FILE)).unwrap();
    let store = s.service.store_mut();
    assert_eq!(
        store
            .add_to_collection(&s.produto, &s.a, None)
            .unwrap_err()
            .code,
        "association_exists"
    );
    assert_eq!(
        store
            .add_to_collection("00000000-0000-4000-8000-000000000000", &s.a, None)
            .unwrap_err()
            .code,
        "collection_not_found"
    );
    assert_eq!(
        store
            .add_to_collection(&s.produto, "00000000-0000-4000-8000-000000000000", None)
            .unwrap_err()
            .code,
        "project_not_found"
    );
    assert_eq!(
        store
            .remove_from_collection(&s.pessoal, &s.b)
            .unwrap_err()
            .code,
        "association_not_found"
    );
    assert_eq!(
        store.move_project(&s.pessoal, &s.b, 0).unwrap_err().code,
        "association_not_found"
    );
    assert_eq!(
        store.create_collection("   ").unwrap_err().code,
        "invalid_collection_name"
    );
    assert_eq!(
        std::fs::read(d.prefs.join(STORE_FILE)).unwrap(),
        file_before
    );
}

/// Would catch: the default session accepted implicitly, a relative local root resolved
/// against the GUI cwd, an empty label, or an endpoint id that could carry a path.
#[test]
fn project_form_validation_refuses_ambiguous_targets() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    let cases = [
        (draft("A", "", "/srv/a"), "invalid_session_name"),
        (draft("A", "bad name", "/srv/a"), "invalid_session_name"),
        (draft("A", "hd-proj-a", "srv/a"), "invalid_project_root"),
        (draft("A", "hd-proj-a", ""), "invalid_project_root"),
        (draft("  ", "hd-proj-a", "/srv/a"), "invalid_project_label"),
        (
            ProjectDraft {
                endpoint_profile_id: "../local".into(),
                ..draft("A", "hd-proj-a", "/srv/a")
            },
            "invalid_endpoint_profile",
        ),
    ];
    for (bad, code) in cases {
        assert_eq!(
            store.create_project(bad.clone()).unwrap_err().code,
            code,
            "{bad:?}"
        );
    }
    assert!(store.document().projects.is_empty());
    assert!(
        !d.prefs.join(STORE_FILE).exists(),
        "rejected drafts write nothing"
    );
    let ok = store
        .create_project(draft(" Zeta ", "hd-proj-a", "/srv/zeta"))
        .unwrap();
    assert_eq!(ok.label, "Zeta");
    assert_eq!(ok.session_name, "hd-proj-a");
    let default = store
        .create_project(draft("Default", "default", "/srv/default"))
        .unwrap();
    assert_eq!(default.session_name, "default");
}

// ---------------------------------------------------------------------------------------
// Versioned store, atomic write, explicit migration
// ---------------------------------------------------------------------------------------

/// Would catch: a newer store downgraded/overwritten by an older GUI, a corrupt or
/// unversioned file silently replaced by an empty store.
#[test]
fn unsupported_or_corrupt_store_is_refused_and_left_untouched() {
    for (content, code) in [
        (
            r#"{"version":5,"projects":[],"groups":[],"workspace_prefs":[],"recent_folders":[]}"#,
            "store_version_unsupported",
        ),
        (
            r#"{"version":4,"projects":[],"groups":[],"workspace_prefs":[],"recent_folders":[],"future":true}"#,
            "store_corrupt",
        ),
        (
            r#"{"version":3,"projects":[],"groups":[],"workspace_prefs":[],"future":true}"#,
            "store_corrupt",
        ),
        // Spec 046: the recents of one host are bounded and keyed; a file with a duplicated or
        // over-long list was written by something else and is refused, never trimmed silently.
        (
            r#"{"version":4,"projects":[],"groups":[],"workspace_prefs":[],"recent_folders":[{"endpoint_profile_id":"local","path":"/w/a"},{"endpoint_profile_id":"local","path":"/w/a/"}]}"#,
            "store_corrupt",
        ),
        (
            r#"{"version":4,"projects":[],"groups":[],"workspace_prefs":[],"recent_folders":[{"endpoint_profile_id":"local","path":"/w/1"},{"endpoint_profile_id":"local","path":"/w/2"},{"endpoint_profile_id":"local","path":"/w/3"},{"endpoint_profile_id":"local","path":"/w/4"},{"endpoint_profile_id":"local","path":"/w/5"},{"endpoint_profile_id":"local","path":"/w/6"},{"endpoint_profile_id":"local","path":"/w/7"},{"endpoint_profile_id":"local","path":"/w/8"},{"endpoint_profile_id":"local","path":"/w/9"}]}"#,
            "store_corrupt",
        ),
        (
            r#"{"version":2,"projects":[],"groups":[],"future":true}"#,
            "store_corrupt",
        ),
        // Spec 044: a v3 preference whose colour is not in the palette is a corrupt document,
        // never silently dropped or repainted.
        (
            r##"{"version":3,"projects":[],"groups":[],"workspace_prefs":[{"endpoint_profile_id":"local","root":"/w/a","color":"#123456","pinned":false,"hidden":false}]}"##,
            "store_corrupt",
        ),
        (
            r##"{"version":4,"projects":[],"groups":[],"workspace_prefs":[{"endpoint_profile_id":"local","root":"/w/a","color":"#123456","pinned":false,"hidden":false}],"recent_folders":[]}"##,
            "store_corrupt",
        ),
        (r#"{"projects":[],"collections":[]}"#, "store_unversioned"),
        (
            r#"{"version":0,"projects":[]}"#,
            "store_version_unsupported",
        ),
        ("{not json", "store_corrupt"),
        (
            r#"{"version":1,"projects":[{"id":"x"}],"collections":[]}"#,
            "store_corrupt",
        ),
        (
            r#"{"version":1,"projects":[],"groups":[]}"#,
            "store_corrupt",
        ),
    ] {
        let d = dirs();
        std::fs::create_dir_all(&d.prefs).unwrap();
        std::fs::write(d.prefs.join(STORE_FILE), content).unwrap();
        let error = ProjectStore::open(&d.prefs, &d.herdr_config)
            .err()
            .unwrap_or_else(|| panic!("{content} must be refused"));
        assert_eq!(error.code, code, "{content}");
        assert!(
            !error.message.contains('/'),
            "no path in errors: {}",
            error.message
        );
        assert_eq!(
            std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap(),
            content
        );
    }
}

/// The explicit migration entry point accepts the current version and rejects the others
/// by code. Would catch: `migrate` guessing a shape for unknown versions.
#[test]
fn explicit_migration_accepts_only_known_versions() {
    // Spec 025: v1 (collections) is migrated in memory to v2 (groups); nothing is dropped.
    let v1 = json!({"version": 1, "projects": [], "collections": []});
    let doc = migrate(v1).unwrap();
    assert_eq!(doc.version, STORE_VERSION);
    assert_eq!(doc.groups, Vec::new());

    // Spec 044: v2 (no preferences) is migrated in memory to v3 with an empty `workspace_prefs`.
    let v2 = json!({"version": 2, "projects": [], "groups": []});
    let from_v2 = migrate(v2).unwrap();
    assert_eq!(from_v2.version, STORE_VERSION);
    assert_eq!(from_v2.workspace_prefs, Vec::new());

    // Spec 046: v3 (no recents) is migrated in memory to v4 with an empty `recent_folders`.
    let v3 = json!({"version": 3, "projects": [], "groups": [], "workspace_prefs": []});
    let from_v3 = migrate(v3).unwrap();
    assert_eq!(from_v3.version, STORE_VERSION);
    assert_eq!(from_v3.recent_folders, Vec::new());

    let v4 = json!({"version": 4, "projects": [], "groups": [], "workspace_prefs": [], "recent_folders": []});
    assert_eq!(migrate(v4).unwrap().version, STORE_VERSION);

    assert_eq!(
        migrate(json!({"version": 99, "projects": [], "groups": []}))
            .unwrap_err()
            .code,
        "store_version_unsupported"
    );
    assert_eq!(
        migrate(json!({"version": "1"})).unwrap_err().code,
        "store_unversioned"
    );
}

/// Spec 025 AC-025-02: the 002 catalog migrates — each saved project is a closed project in the
/// group of its old collection; no group is fabricated and no empty "teste/Personal" appears.
#[test]
fn v1_collections_migrate_to_v2_groups_with_the_saved_projects() {
    let d = dirs();
    std::fs::create_dir_all(&d.prefs).unwrap();
    let project_id = "00000000-0000-4000-8000-0000000000a1";
    let group_id = "00000000-0000-4000-8000-0000000000b1";
    std::fs::write(
        d.prefs.join(STORE_FILE),
        format!(
            r#"{{"version":1,"projects":[{{"id":"{project_id}","label":"herdr","endpoint_profile_id":"local","session_name":"default","root":"/srv/herdr"}}],"collections":[{{"id":"{group_id}","name":"Meus projetos","project_ids":["{project_id}"]}}]}}"#
        ),
    )
    .unwrap();

    let store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    // v1 → the current schema in one step (v3 since spec 044); the groups keep the old ids.
    assert_eq!(store.document().version, STORE_VERSION);
    assert_eq!(
        collection_view(&store),
        vec![("Meus projetos".to_owned(), vec![project_id.to_owned()])]
    );
    assert_eq!(
        store.document().projects.len(),
        1,
        "the saved project is the closed catalog"
    );
    // The file is only rewritten on the next change; until then the v1 bytes stay intact.
    let service = ProjectService::new(store);
    let snapshot = serde_json::to_value(service.snapshot()).unwrap();
    assert_eq!(snapshot["collections"][0]["name"], json!("Meus projetos"));
    assert_eq!(snapshot["projects"][0]["root"], json!("/srv/herdr"));
}

/// Spec 025 AC-025-02: moving a live workspace to a group persists by root cwd; a reopen (same
/// cwd) lands in the same group and a repeated move never duplicates the row.
#[test]
fn group_assign_upserts_by_root_cwd_and_persists() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    let group = store.create_group("Meus projetos").unwrap().id;
    let entry = ProjectDraft {
        label: "herdr".into(),
        endpoint_profile_id: "local".into(),
        session_name: "default".into(),
        root: "/srv/herdr".into(),
    };
    let first = store.assign_root(&group, entry.clone()).unwrap();
    let again = store.assign_root(&group, entry.clone()).unwrap();
    assert_eq!(first.id, again.id, "same cwd, same saved project");
    assert_eq!(
        collection_view(&store),
        vec![("Meus projetos".to_owned(), vec![first.id.clone()])]
    );
    assert_eq!(store.document().projects.len(), 1);

    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(
        collection_view(&reloaded),
        vec![("Meus projetos".to_owned(), vec![first.id])],
        "the group and its cwd survive a restart"
    );
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(raw["version"], STORE_VERSION);
    assert_eq!(raw["groups"][0]["name"], json!("Meus projetos"));
}

// ---------------------------------------------------------------------------------------
// AC-044-01 — workspace preferences (store v3)
// ---------------------------------------------------------------------------------------

/// Writes a v2 document with two projects and one group and returns its raw value.
fn write_v2_store(d: &Dirs) -> Value {
    std::fs::create_dir_all(&d.prefs).unwrap();
    let a = "00000000-0000-4000-8000-0000000000a1";
    let b = "00000000-0000-4000-8000-0000000000a2";
    let g = "00000000-0000-4000-8000-0000000000b1";
    let raw = json!({
        "version": 2,
        "projects": [
            {"id": a, "label": "erp-api", "endpoint_profile_id": "local", "session_name": "default", "root": "/w/erp-api"},
            {"id": b, "label": "portal-web", "endpoint_profile_id": "ssh-dev", "session_name": "default", "root": "/w/portal-web"},
        ],
        "groups": [{"id": g, "name": "Acme · Clientes", "project_ids": [a, b]}],
    });
    std::fs::write(
        d.prefs.join(STORE_FILE),
        serde_json::to_vec_pretty(&raw).unwrap(),
    )
    .unwrap();
    raw
}

/// AC-044-01 and AC-046-02. Would catch: a migration that reorders, relabels or drops
/// projects/groups, one that leaves the document on v2/v3, or a current document written
/// without `workspace_prefs` / `recent_folders`.
#[test]
fn v2_store_migrates_to_the_current_schema_keeping_projects_and_groups() {
    let d = dirs();
    let before = write_v2_store(&d);

    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(store.document().version, 4);
    assert_eq!(store.document().version, STORE_VERSION);
    assert_eq!(store.document().workspace_prefs, Vec::new());
    assert_eq!(store.document().recent_folders, Vec::new());
    let migrated = serde_json::to_value(store.document()).unwrap();
    assert_eq!(
        migrated["projects"], before["projects"],
        "projects untouched"
    );
    assert_eq!(migrated["groups"], before["groups"], "groups untouched");
    assert_eq!(migrated["workspace_prefs"], json!([]));
    assert_eq!(migrated["recent_folders"], json!([]));

    // The next write persists v3 with the same old fields and the new empty list.
    store
        .set_workspace_pref(
            "local",
            "/w/erp-api",
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap();
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(on_disk["version"], json!(4));
    assert_eq!(on_disk["projects"], before["projects"]);
    assert_eq!(on_disk["groups"], before["groups"]);
    assert_eq!(
        on_disk["workspace_prefs"],
        json!([{"endpoint_profile_id": "local", "root": "/w/erp-api", "color": null, "pinned": true, "hidden": false}])
    );
    assert_eq!(on_disk["recent_folders"], json!([]));
}

/// AC-046-02. Would catch: a v3 store that stops loading, a migration that invents recents or
/// drops the 044 preferences, or a v4 document written without the new list.
#[test]
fn v3_store_migrates_to_v4_with_an_empty_recent_folders_keeping_the_preferences() {
    let d = dirs();
    std::fs::create_dir_all(&d.prefs).unwrap();
    let a = "00000000-0000-4000-8000-0000000000a1";
    let g = "00000000-0000-4000-8000-0000000000b1";
    let before = json!({
        "version": 3,
        "projects": [
            {"id": a, "label": "erp-api", "endpoint_profile_id": "local", "session_name": "default", "root": "/w/erp-api"},
        ],
        "groups": [{"id": g, "name": "Acme · Clientes", "project_ids": [a]}],
        "workspace_prefs": [
            {"endpoint_profile_id": "local", "root": "/w/erp-api", "color": null, "pinned": true, "hidden": false},
        ],
    });
    std::fs::write(
        d.prefs.join(STORE_FILE),
        serde_json::to_vec_pretty(&before).unwrap(),
    )
    .unwrap();

    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(store.document().version, 4);
    assert_eq!(store.document().recent_folders, Vec::new());
    let migrated = serde_json::to_value(store.document()).unwrap();
    assert_eq!(migrated["projects"], before["projects"]);
    assert_eq!(migrated["groups"], before["groups"]);
    assert_eq!(migrated["workspace_prefs"], before["workspace_prefs"]);

    // The next write persists v4 with the old fields intact and the new list.
    store.record_recent_folder("local", "/w/fiscal").unwrap();
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(on_disk["version"], json!(4));
    assert_eq!(on_disk["projects"], before["projects"]);
    assert_eq!(on_disk["groups"], before["groups"]);
    assert_eq!(on_disk["workspace_prefs"], before["workspace_prefs"]);
    assert_eq!(
        on_disk["recent_folders"],
        json!([{"endpoint_profile_id": "local", "path": "/w/fiscal"}])
    );
}

/// AC-046-02. Would catch: a recent appended at the end instead of the top, the same folder
/// listed twice, the list of one host growing past 8, one host evicting another host's entries,
/// or the recents lost on a restart.
#[test]
fn recent_folders_are_kept_on_top_deduped_and_bounded_per_host() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(MAX_RECENT_FOLDERS_PER_HOST, 8);

    let recorded = store.record_recent_folder("local", "/w/fiscal").unwrap();
    assert_eq!(
        recorded,
        RecentFolder {
            endpoint_profile_id: "local".into(),
            path: "/w/fiscal".into(),
        }
    );
    store.record_recent_folder("ssh-dev", "/srv/api").unwrap();
    store.record_recent_folder("local", "/w/billing").unwrap();
    assert_eq!(
        store.recent_folders("local"),
        vec!["/w/billing".to_owned(), "/w/fiscal".to_owned()],
        "the newest folder of the host comes first"
    );
    assert_eq!(store.recent_folders("ssh-dev"), vec!["/srv/api".to_owned()]);

    // The same folder again (trailing separator is the same key): moved to the top, never a
    // second entry, and the other host is untouched.
    store.record_recent_folder("local", "/w/fiscal/").unwrap();
    assert_eq!(
        store.recent_folders("local"),
        vec!["/w/fiscal".to_owned(), "/w/billing".to_owned()]
    );
    assert_eq!(store.recent_folders("ssh-dev"), vec!["/srv/api".to_owned()]);

    // Nine folders on one host: the oldest of that host leaves, the other host stays whole.
    for i in 1..=9 {
        store
            .record_recent_folder("local", &format!("/w/p{i}"))
            .unwrap();
    }
    assert_eq!(
        store.recent_folders("local"),
        vec![
            "/w/p9".to_owned(),
            "/w/p8".to_owned(),
            "/w/p7".to_owned(),
            "/w/p6".to_owned(),
            "/w/p5".to_owned(),
            "/w/p4".to_owned(),
            "/w/p3".to_owned(),
            "/w/p2".to_owned(),
        ]
    );
    assert_eq!(store.recent_folders("ssh-dev"), vec!["/srv/api".to_owned()]);

    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(
        reloaded.document().recent_folders,
        store.document().recent_folders,
        "the recents survive a restart"
    );
    assert_eq!(
        serde_json::to_value(reloaded.document()).unwrap()["version"],
        json!(4)
    );

    // The WebView reads the recents from the snapshot, in the same order.
    let snapshot = serde_json::to_value(ProjectService::new(reloaded).snapshot()).unwrap();
    assert_eq!(snapshot["version"], json!(4));
    assert_eq!(
        snapshot["recent_folders"][0],
        json!({"endpoint_profile_id": "local", "path": "/w/p9"})
    );
    assert_eq!(snapshot["recent_folders"].as_array().unwrap().len(), 9);
}

/// AC-046-02. Would catch: an empty/unbounded path or an endpoint the desktop does not know
/// keyed into the file, or a refusal that still rewrote the store.
#[test]
fn recent_folder_refuses_invalid_path_and_endpoint_without_touching_the_file() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    store.record_recent_folder("local", "/w/fiscal").unwrap();
    let before = std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap();

    assert_eq!(
        store.record_recent_folder("local", "   ").unwrap_err().code,
        "invalid_root"
    );
    assert_eq!(
        store
            .record_recent_folder("local", &"x".repeat(5000))
            .unwrap_err()
            .code,
        "invalid_root"
    );
    assert_eq!(
        store
            .record_recent_folder("../etc", "/w/fiscal")
            .unwrap_err()
            .code,
        "unknown_endpoint"
    );
    assert_eq!(
        store
            .record_recent_folder("", "/w/fiscal")
            .unwrap_err()
            .code,
        "unknown_endpoint"
    );
    assert_eq!(
        std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap(),
        before,
        "a refused recent never rewrites the store"
    );
    assert_eq!(store.recent_folders("local"), vec!["/w/fiscal".to_owned()]);
    assert_eq!(store.recent_folders("unknown-host"), Vec::<String>::new());
}

/// AC-044-01. Would catch: a second call appending a duplicate entry instead of updating the
/// one keyed by (endpoint, normalized root), a patch resetting the fields it does not carry,
/// preferences lost on reload, or an emptied entry kept in the file forever.
#[test]
fn workspace_pref_set_writes_updates_and_removes_the_entry() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();

    let written = store
        .set_workspace_pref(
            "local",
            "/w/erp-api",
            WorkspacePrefPatch {
                color: Some(GROUP_PALETTE[4].to_owned()),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap()
        .expect("entry written");
    assert_eq!(written.color.as_deref(), Some("#F2777A"));
    assert!(!written.pinned && !written.hidden);

    // A trailing separator is the same key (025 normalization): update, never a second entry.
    let updated = store
        .set_workspace_pref(
            "local",
            "/w/erp-api/",
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap()
        .expect("entry updated");
    assert_eq!(updated.color.as_deref(), Some("#F2777A"), "colour kept");
    assert!(updated.pinned);
    assert_eq!(store.document().workspace_prefs.len(), 1);

    // Another root of the same endpoint is its own entry.
    store
        .set_workspace_pref(
            "local",
            "/w/portal-web",
            WorkspacePrefPatch {
                hidden: Some(true),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap();
    assert_eq!(store.document().workspace_prefs.len(), 2);

    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(
        reloaded.document().workspace_prefs,
        store.document().workspace_prefs,
        "preferences survive a restart"
    );

    // Neither coloured, pinned nor hidden: the entry is removed, not stored empty.
    let gone = store
        .set_workspace_pref(
            "local",
            "/w/portal-web",
            WorkspacePrefPatch {
                hidden: Some(false),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap();
    assert!(gone.is_none(), "an empty preference is dropped");
    assert_eq!(
        store
            .document()
            .workspace_prefs
            .iter()
            .map(|p| p.root.clone())
            .collect::<Vec<_>>(),
        vec!["/w/erp-api".to_owned()]
    );
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(on_disk["workspace_prefs"].as_array().unwrap().len(), 1);
}

/// AC-044-01. Would catch: an arbitrary colour accepted (the palette is the contract with
/// `GROUP_PALETTE`), an empty or unbounded root keyed into the file, an endpoint the desktop
/// does not know silently accepted, or a refusal that still rewrote the store.
#[test]
fn workspace_pref_set_refuses_invalid_colour_root_and_endpoint() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    store
        .set_workspace_pref(
            "local",
            "/w/erp-api",
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap();
    let before = std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap();

    let color = |value: &str| WorkspacePrefPatch {
        color: Some(value.to_owned()),
        ..WorkspacePrefPatch::default()
    };
    let long_root = "/w/".to_owned() + &"a".repeat(4096);
    let cases: Vec<(&str, String, WorkspacePrefPatch, &str)> = vec![
        (
            "local",
            "/w/erp-api".into(),
            color("#123456"),
            "invalid_color",
        ),
        ("local", "/w/erp-api".into(), color("red"), "invalid_color"),
        (
            "local",
            "   ".into(),
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
            "invalid_root",
        ),
        (
            "local",
            long_root,
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
            "invalid_root",
        ),
        (
            "",
            "/w/erp-api".into(),
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
            "unknown_endpoint",
        ),
        (
            "não existe",
            "/w/erp-api".into(),
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
            "unknown_endpoint",
        ),
    ];
    for (endpoint, root, patch, code) in cases {
        let error = store
            .set_workspace_pref(endpoint, &root, patch)
            .err()
            .unwrap_or_else(|| panic!("{endpoint} {root} must be refused"));
        assert_eq!(error.code, code, "{endpoint} {root}");
    }
    assert_eq!(
        std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap(),
        before,
        "a refused preference changes nothing"
    );
    assert_eq!(store.document().workspace_prefs.len(), 1);

    // Every palette colour is accepted, in the exact form the WebView sends.
    for value in GROUP_PALETTE {
        store
            .set_workspace_pref("local", "/w/erp-api", color(value))
            .unwrap()
            .expect("palette colour accepted");
    }
}

// ---------------------------------------------------------------------------------------
// AC-045-02 / AC-045-03 — collection colour, name, collapse and deletion
// ---------------------------------------------------------------------------------------

/// AC-045-02/03. Would catch: `color`/`collapsed` written into every group by the migration
/// (a v2 catalog would come back repainted), the fields refused by `deny_unknown_fields`, or a
/// value that does not survive a restart.
#[test]
fn collection_color_and_collapse_are_optional_and_absent_until_set() {
    let d = dirs();
    let before = write_v2_store(&d);
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    let group = store.document().groups[0].id.clone();
    assert_eq!(store.document().groups[0].color, None);
    assert!(!store.document().groups[0].collapsed);

    // The first write persists v3 without inventing the two new fields.
    store.rename_collection(&group, "Clientes").unwrap();
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(raw["groups"][0]["name"], json!("Clientes"));
    assert_eq!(
        raw["groups"][0]["project_ids"],
        before["groups"][0]["project_ids"]
    );
    assert!(
        raw["groups"][0].get("color").is_none(),
        "absence is preserved: {raw}"
    );
    assert!(raw["groups"][0].get("collapsed").is_none(), "{raw}");

    store
        .set_collection_color(&group, GROUP_PALETTE[2])
        .unwrap();
    store.set_collection_collapsed(&group, true).unwrap();
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert_eq!(raw["groups"][0]["color"], json!("#B18CFF"));
    assert_eq!(raw["groups"][0]["collapsed"], json!(true));

    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(reloaded.document().groups[0].name, "Clientes");
    assert_eq!(
        reloaded.document().groups[0].color.as_deref(),
        Some("#B18CFF")
    );
    assert!(reloaded.document().groups[0].collapsed);

    // Expanding it again drops the flag from the file instead of storing `false`.
    let mut reloaded = reloaded;
    reloaded.set_collection_collapsed(&group, false).unwrap();
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap()).unwrap();
    assert!(raw["groups"][0].get("collapsed").is_none(), "{raw}");
}

/// AC-045-02. Would catch: an empty or oversized collection name accepted, an arbitrary colour
/// stored (the palette is the contract with `GROUP_PALETTE`), an unknown collection silently
/// created, or a refusal that still rewrote the file.
#[test]
fn collection_rename_and_colour_are_validated_and_leave_the_file_untouched_on_refusal() {
    let d = dirs();
    let mut store = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    let group = store.create_group("Produto").unwrap().id;
    store
        .set_collection_color(&group, GROUP_PALETTE[0])
        .unwrap();
    let before = std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap();

    let long = "n".repeat(81);
    assert_eq!(
        store.rename_collection(&group, "   ").unwrap_err().code,
        "invalid_collection_name"
    );
    assert_eq!(
        store.rename_collection(&group, &long).unwrap_err().code,
        "invalid_collection_name"
    );
    assert_eq!(
        store
            .set_collection_color(&group, "#123456")
            .unwrap_err()
            .code,
        "invalid_color"
    );
    assert_eq!(
        store.set_collection_color(&group, "roxo").unwrap_err().code,
        "invalid_color"
    );
    let missing = "00000000-0000-4000-8000-0000000000ff";
    assert_eq!(
        store.rename_collection(missing, "X").unwrap_err().code,
        "collection_not_found"
    );
    assert_eq!(
        store
            .set_collection_color(missing, GROUP_PALETTE[1])
            .unwrap_err()
            .code,
        "collection_not_found"
    );
    assert_eq!(
        store
            .set_collection_collapsed(missing, true)
            .unwrap_err()
            .code,
        "collection_not_found"
    );
    assert_eq!(
        store.delete_collection(missing).unwrap_err().code,
        "collection_not_found"
    );
    assert_eq!(
        std::fs::read_to_string(d.prefs.join(STORE_FILE)).unwrap(),
        before,
        "a refused edit changes nothing"
    );

    // The name is trimmed, like the one `group_create` takes.
    store.rename_collection(&group, "  Produto novo  ").unwrap();
    assert_eq!(store.document().groups[0].name, "Produto novo");
    for color in GROUP_PALETTE {
        store.set_collection_color(&group, color).unwrap();
    }
}

/// AC-045-02. Would catch: deleting a collection deleting its projects (the workspaces would
/// vanish from the sidebar instead of falling back to "Sem coleção"), touching another
/// collection's membership, or dropping the workspace preferences.
#[test]
fn deleting_a_collection_keeps_its_projects_preferences_and_the_other_collections() {
    let d = dirs();
    let s = seeded(&d);
    let mut service = s.service;
    service
        .store_mut()
        .set_workspace_pref(
            "local",
            "/srv/zeta",
            WorkspacePrefPatch {
                pinned: Some(true),
                ..WorkspacePrefPatch::default()
            },
        )
        .unwrap();
    let projects_before: Vec<String> = service
        .store()
        .document()
        .projects
        .iter()
        .map(|p| p.id.clone())
        .collect();

    service.store_mut().delete_collection(&s.produto).unwrap();

    let doc = service.store().document();
    assert_eq!(
        doc.groups.iter().map(|g| g.id.clone()).collect::<Vec<_>>(),
        vec![s.pessoal.clone()],
        "only the deleted collection is gone"
    );
    assert_eq!(
        doc.groups[0].project_ids,
        vec![s.a.clone()],
        "the other collection keeps its members"
    );
    assert_eq!(
        doc.projects
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>(),
        projects_before,
        "no project is deleted: `b` simply has no collection now"
    );
    assert_eq!(doc.workspace_prefs.len(), 1, "preferences are untouched");

    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(reloaded.document().groups.len(), 1);
    assert_eq!(reloaded.document().projects.len(), projects_before.len());
}

/// Would catch: writing in place (a failed write truncating the previous store), temp files
/// left behind, or memory diverging from disk after a failed save.
#[cfg(unix)]
#[test]
fn failed_atomic_save_keeps_previous_file_and_memory() {
    use std::os::unix::fs::PermissionsExt;
    let d = dirs();
    let mut s = seeded(&d);
    let path = d.prefs.join(STORE_FILE);
    let before_file = std::fs::read(&path).unwrap();
    let before_view = collection_view(s.service.store());
    assert_eq!(
        dir_listing(&d.prefs),
        [STORE_FILE],
        "no temp files after saves"
    );

    std::fs::set_permissions(&d.prefs, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = s.service.store_mut().create_collection("Terceira");
    std::fs::set_permissions(&d.prefs, std::fs::Permissions::from_mode(0o755)).unwrap();

    let error = result.unwrap_err();
    assert_eq!(error.code, "store_write_failed");
    assert!(!error.message.contains('/'), "{}", error.message);
    assert_eq!(std::fs::read(&path).unwrap(), before_file);
    assert_eq!(collection_view(s.service.store()), before_view);
    assert_eq!(dir_listing(&d.prefs), [STORE_FILE]);
}

/// Would catch: `atomic_write` appending instead of replacing, or not creating the parent.
#[test]
fn atomic_write_replaces_whole_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("file.json");
    atomic_write(&path, b"first-long-content").unwrap();
    atomic_write(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    assert_eq!(dir_listing(&dir.path().join("nested")), ["file.json"]);
}

// ---------------------------------------------------------------------------------------
// TASK-002-04 — ProjectRef durable vs RuntimeBinding; no engine persistence touched
// ---------------------------------------------------------------------------------------

/// Would catch: preferences placed in the engine config dir (where endpoints.json and the
/// session snapshots live).
#[test]
fn preferences_inside_the_engine_config_dir_are_refused() {
    let d = dirs();
    for inside in [
        d.herdr_config.clone(),
        d.herdr_config.join("desktop"),
        d.herdr_config.join("sessions").join("x"),
    ] {
        let error = ProjectStore::open(&inside, &d.herdr_config)
            .err()
            .unwrap_or_else(|| panic!("{inside:?} must be refused"));
        assert_eq!(error.code, "prefs_dir_forbidden");
    }
    assert!(ProjectStore::open(&d.prefs, &d.herdr_config).is_ok());
}

/// Would catch: the store or the open flow writing endpoints.json, snapshots or any file in
/// the engine config dir.
#[test]
fn store_and_open_never_write_engine_files() {
    let d = dirs();
    let listing_before = dir_listing(&d.herdr_config);
    let endpoints_before = std::fs::read(d.herdr_config.join("endpoints.json")).unwrap();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    s.service
        .open_project(&s.a, &FakeGateway::local("hd-proj-a", 1, &engine))
        .unwrap();
    s.service
        .store_mut()
        .remove_from_collection(&s.produto, &s.a)
        .unwrap();
    assert_eq!(dir_listing(&d.herdr_config), listing_before);
    assert_eq!(
        std::fs::read(d.herdr_config.join("endpoints.json")).unwrap(),
        endpoints_before
    );
    assert_eq!(dir_listing(&d.prefs), [STORE_FILE]);
}

// ---------------------------------------------------------------------------------------
// AC-002-02 — removing an association
// ---------------------------------------------------------------------------------------

/// Would catch: removal implemented as "delete project" (A disappears from Pessoal), removal
/// that drops the live binding, or any runtime command (e.g. workspace.close) sent.
#[test]
fn removing_from_one_collection_keeps_the_other_and_the_live_workspace() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let gateway = FakeGateway::local("hd-proj-a", 2, &engine);
    let opened = s.service.open_project(&s.a, &gateway).unwrap();
    clear_calls(&engine);

    s.service
        .store_mut()
        .remove_from_collection(&s.produto, &s.a)
        .unwrap();

    assert_eq!(
        methods(&engine),
        Vec::<String>::new(),
        "removing an association sends no command to the engine"
    );
    assert_eq!(s.service.binding(&s.a), Some(&opened.binding));
    assert_eq!(engine.lock().unwrap().workspaces.len(), 1);
    assert_eq!(
        collection_view(s.service.store()),
        vec![
            ("Produto".to_owned(), vec![s.b.clone()]),
            ("Pessoal".to_owned(), vec![s.a.clone()]),
        ]
    );
    assert!(s.service.store().project(&s.a).is_some());
    let reloaded = ProjectStore::open(&d.prefs, &d.herdr_config).unwrap();
    assert_eq!(
        collection_view(&reloaded),
        collection_view(s.service.store())
    );
    // Reopening after removal still reuses the same workspace.
    let again = s.service.open_project(&s.a, &gateway).unwrap();
    assert_eq!(again.outcome, OpenOutcome::Reused);
    assert!(!methods(&engine).iter().any(|m| m == "workspace.close"));
}

// ---------------------------------------------------------------------------------------
// AC-002-03 — open, reopen, rediscover, invalidate
// ---------------------------------------------------------------------------------------

/// Would catch: workspace created on a session other than the project's, cwd not the project
/// root, binding without the live boot/generation, or no rediscovery tag.
#[test]
fn opening_without_binding_creates_workspace_on_declared_target_and_binds_identity() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    engine.lock().unwrap().next_workspace = 7;
    let gateway = FakeGateway::local("hd-proj-a", 3, &engine);

    let opened = s.service.open_project(&s.a, &gateway).unwrap();

    assert_eq!(opened.outcome, OpenOutcome::Created);
    assert_eq!(opened.binding.project_id, s.a);
    assert_eq!(opened.binding.workspace_id, "w7");
    assert_eq!(opened.binding.boot_id, "boot-alpha");
    assert_eq!(opened.binding.connection_generation, 3);
    assert_eq!(
        methods(&engine),
        [
            "workspace.list",
            "workspace.create",
            "workspace.report_metadata"
        ]
    );
    let engine_state = engine.lock().unwrap();
    for (endpoint, session, _, _) in &engine_state.calls {
        assert_eq!(
            (endpoint.as_str(), session.as_str()),
            ("local", "hd-proj-a")
        );
    }
    let created = &engine_state.workspaces[0];
    assert_eq!(created.cwd, "/srv/zeta");
    assert_eq!(created.label, "Zeta API");
    assert_eq!(created.tokens.get(PROJECT_TOKEN), Some(&s.a));
}

/// Would catch: reopen in the same boot creating a second workspace (duplicate processes).
#[test]
fn reopening_within_the_same_boot_creates_no_duplicate_workspace() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let gateway = FakeGateway::local("hd-proj-a", 3, &engine);
    let first = s.service.open_project(&s.a, &gateway).unwrap();
    clear_calls(&engine);

    let second = s.service.open_project(&s.a, &gateway).unwrap();

    assert_eq!(second.outcome, OpenOutcome::Reused);
    assert_eq!(second.binding, first.binding);
    assert_eq!(methods(&engine), ["workspace.list"]);
    assert_eq!(engine.lock().unwrap().workspaces.len(), 1);
}

/// A GUI restart has no binding and a new connection; the tagged workspace of the same boot
/// is rediscovered. Would catch: restart creating a duplicate workspace, or rediscovery
/// matching another project's workspace (B is opened first and must not be picked for A).
#[test]
fn restart_rediscovers_the_tagged_workspace_of_the_same_boot() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let before = FakeGateway::local("hd-proj-a", 5, &engine);
    let b_ws = s.service.open_project(&s.b, &before).unwrap().binding;
    let a_ws = s.service.open_project(&s.a, &before).unwrap().binding;
    assert_ne!(a_ws.workspace_id, b_ws.workspace_id);
    drop(s.service);
    clear_calls(&engine);

    let mut restarted = ProjectService::new(ProjectStore::open(&d.prefs, &d.herdr_config).unwrap());
    let after = FakeGateway::local("hd-proj-a", 1, &engine);
    let reopened = restarted.open_project(&s.a, &after).unwrap();

    assert_eq!(reopened.outcome, OpenOutcome::Rediscovered);
    assert_eq!(reopened.binding.workspace_id, a_ws.workspace_id);
    assert_eq!(reopened.binding.connection_generation, 1);
    assert_eq!(reopened.binding.boot_id, "boot-alpha");
    assert_eq!(methods(&engine), ["workspace.list"]);
    assert_eq!(engine.lock().unwrap().workspaces.len(), 2);
}

/// Would catch: a binding from a previous boot reused because the new boot happens to have a
/// workspace with the same id (`w1`), i.e. retargeting a foreign workspace.
#[test]
fn divergent_boot_invalidates_binding_and_never_reuses_the_old_workspace_id() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let gateway = FakeGateway::local("hd-proj-a", 4, &engine);
    let old = s.service.open_project(&s.a, &gateway).unwrap().binding;
    assert_eq!(old.workspace_id, "w1");
    {
        let mut e = engine.lock().unwrap();
        e.boot_id = "boot-beta".into();
        e.workspaces = vec![FakeWorkspace {
            id: "w1".into(),
            label: "unrelated after reboot".into(),
            cwd: "/tmp".into(),
            tokens: BTreeMap::new(),
        }];
        e.next_workspace = 2;
        e.calls.clear();
    }

    let reopened = s.service.open_project(&s.a, &gateway).unwrap();

    assert_eq!(reopened.invalidated.as_ref(), Some(&old));
    assert_eq!(reopened.outcome, OpenOutcome::Created);
    assert_eq!(reopened.binding.workspace_id, "w2");
    assert_eq!(reopened.binding.boot_id, "boot-beta");
    assert_eq!(
        methods(&engine),
        [
            "workspace.list",
            "workspace.create",
            "workspace.report_metadata"
        ]
    );
}

/// Same boot, renewed connection: the binding is invalid (not `Reused`) and the workspace is
/// found again by tag. Would catch: validating only the boot and ignoring the generation.
#[test]
fn divergent_generation_invalidates_binding_before_rediscovery() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let old = s
        .service
        .open_project(&s.a, &FakeGateway::local("hd-proj-a", 4, &engine))
        .unwrap()
        .binding;
    let renewed = FakeGateway::local("hd-proj-a", 5, &engine);

    let reopened = s.service.open_project(&s.a, &renewed).unwrap();

    assert_eq!(reopened.invalidated.as_ref(), Some(&old));
    assert_eq!(reopened.outcome, OpenOutcome::Rediscovered);
    assert_eq!(reopened.binding.workspace_id, old.workspace_id);
    assert_eq!(reopened.binding.connection_generation, 5);
}

/// A binding whose workspace was closed elsewhere is replaced, not trusted.
/// Would catch: returning `Reused` for a workspace the engine no longer has.
#[test]
fn binding_to_a_workspace_closed_elsewhere_is_replaced() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let gateway = FakeGateway::local("hd-proj-a", 1, &engine);
    s.service.open_project(&s.a, &gateway).unwrap();
    engine.lock().unwrap().workspaces.clear();

    let reopened = s.service.open_project(&s.a, &gateway).unwrap();

    assert_eq!(reopened.outcome, OpenOutcome::Created);
    assert_eq!(reopened.binding.workspace_id, "w2");
}

/// Would catch: a fallback to the local gateway for a project of another endpoint or
/// session, or any API call made before the target is validated.
#[test]
fn endpoint_or_session_mismatch_is_refused_before_any_command() {
    let d = dirs();
    let mut s = seeded(&d);
    let remote = s
        .service
        .store_mut()
        .create_project(ProjectDraft {
            endpoint_profile_id: "ssh-build".into(),
            ..draft("Build", "hd-proj-a", "/home/ci/build")
        })
        .unwrap()
        .id;
    let other_session = s
        .service
        .store_mut()
        .create_project(draft("Outra", "hd-proj-b", "/srv/outra"))
        .unwrap()
        .id;
    let engine = FakeEngine::new("boot-alpha");
    let local = FakeGateway::local("hd-proj-a", 1, &engine);

    let error = s.service.open_project(&remote, &local).unwrap_err();
    assert_eq!(error.code, "target_endpoint_mismatch");
    assert_eq!(error.endpoint.as_deref(), Some("ssh-build"));
    let error = s.service.open_project(&other_session, &local).unwrap_err();
    assert_eq!(error.code, "target_session_mismatch");

    let mut lying = FakeGateway::local("hd-proj-a", 1, &engine);
    lying.endpoint = "ssh-build".into();
    let error = s.service.open_project(&s.a, &lying).unwrap_err();
    assert_eq!(error.code, "target_endpoint_mismatch");

    assert!(methods(&engine).is_empty(), "{:?}", methods(&engine));
    assert!(s.service.binding(&remote).is_none());
    assert!(s.service.binding(&other_session).is_none());
}

/// Would catch: binding with an empty boot id when the handshake has not produced one.
#[test]
fn unknown_identity_is_retryable_and_sends_nothing() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let mut gateway = FakeGateway::local("hd-proj-a", 1, &engine);
    gateway.identity_known = false;
    let error = s.service.open_project(&s.a, &gateway).unwrap_err();
    assert_eq!(error.code, "boot_unknown");
    assert!(error.retryable);
    assert!(methods(&engine).is_empty());
}

/// Would catch: an engine failure on one project clearing other projects' bindings or
/// mutating the store; the error stays on the affected project.
#[test]
fn engine_unavailable_error_stays_on_the_affected_project() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    let gateway = FakeGateway::local("hd-proj-a", 1, &engine);
    let a_binding = s.service.open_project(&s.a, &gateway).unwrap().binding;
    let file_before = std::fs::read(d.prefs.join(STORE_FILE)).unwrap();
    engine.lock().unwrap().fail_with = Some(
        RuntimeError::new("server_unavailable", "the Herdr API is unavailable")
            .retryable()
            .with_endpoint("local"),
    );

    let error = s.service.open_project(&s.b, &gateway).unwrap_err();

    assert_eq!(error.code, "server_unavailable");
    assert!(error.retryable);
    assert!(s.service.binding(&s.b).is_none());
    assert_eq!(s.service.binding(&s.a), Some(&a_binding));
    assert_eq!(
        std::fs::read(d.prefs.join(STORE_FILE)).unwrap(),
        file_before
    );
}

/// Would catch: binding a workspace to the boot observed *before* the create when the engine
/// rebooted in between.
#[test]
fn identity_change_during_open_does_not_bind() {
    let d = dirs();
    let mut s = seeded(&d);
    let engine = FakeEngine::new("boot-alpha");
    engine.lock().unwrap().reboot_on_create = Some("boot-gamma".into());
    let gateway = FakeGateway::local("hd-proj-a", 1, &engine);
    let error = s.service.open_project(&s.a, &gateway).unwrap_err();
    assert_eq!(error.code, "target_boot_stale");
    assert!(s.service.binding(&s.a).is_none());
}

// ---------------------------------------------------------------------------------------
// IPC surface of the module
// ---------------------------------------------------------------------------------------

/// Would catch: a module command without a handler function, a generic shell/fs command, or
/// the WebView bridge invoking a name the backend does not declare.
#[test]
fn project_commands_are_limited_and_match_the_frontend_bridge() {
    let source = include_str!("../src/project_store.rs");
    let forbidden = [
        "shell",
        "exec",
        "spawn",
        "open_url",
        "read_file",
        "write_file",
        "http",
        "fetch",
        "close",
    ];
    for command in project_store::COMMANDS {
        assert!(
            source.contains(&format!("#[tauri::command]\npub async fn {command}("))
                || source.contains(&format!("#[tauri::command]\npub fn {command}(")),
            "{command} has no #[tauri::command] handler"
        );
        for word in forbidden {
            assert!(!command.contains(word), "{command} looks like {word}");
        }
    }
    let handlers = source.matches("#[tauri::command]").count();
    assert_eq!(handlers, project_store::COMMANDS.len());

    let bridge = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/projects/bridge.ts"
    ))
    .unwrap();
    let mut invoked: Vec<&str> = bridge
        .split("invoke<")
        .skip(1)
        .filter_map(|rest| rest.split('"').nth(1))
        .collect();
    invoked.sort_unstable();
    let mut declared: Vec<&str> = project_store::COMMANDS.to_vec();
    declared.sort_unstable();
    assert_eq!(invoked, declared);
}

/// Spec 046 (round 2). Would catch a handler inserted between an existing doc comment and the
/// function it documents, so one command answers for another's documentation.
#[test]
fn every_command_handler_keeps_its_own_documentation() {
    let source = include_str!("../src/project_store.rs");
    let doc_of = |command: &str| -> String {
        let at = source
            .find(&format!("pub async fn {command}("))
            .unwrap_or_else(|| panic!("{command} has no handler"));
        let head = &source[..at];
        let attr = head
            .rfind("#[tauri::command]")
            .unwrap_or_else(|| panic!("{command} has no #[tauri::command]"));
        head[..attr]
            .lines()
            .rev()
            .take_while(|line| line.trim_start().starts_with("///"))
            .map(|line| line.trim().trim_start_matches("///").trim().to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    };

    let open = doc_of("project_open");
    assert!(
        open.contains("Harness mode"),
        "project_open lost its own documentation: {open:?}"
    );
    let recent = doc_of("recent_folder_add");
    assert!(
        recent.to_lowercase().contains("recent"),
        "recent_folder_add has no documentation of its own: {recent:?}"
    );
    assert!(
        !recent.contains("Harness mode"),
        "recent_folder_add carries the documentation of project_open: {recent:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Native E2E: real Tauri window with ProjectNavigator and the real backend/IPC:
// create collections and projects → open → reopen; close the GUI; reopen the GUI → open
// (rediscover) → remove association; close; reopen the GUI → persisted state.
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod e2e {
    use super::native_harness::{self, run_phase};
    use super::project_store::{self, ProjectsState};
    use super::window_harness;
    use herdr_client::{LocalGateway, RuntimeGateway, SessionName, SessionPaths};
    use serde_json::{json, Value};
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::Duration;

    const WINDOW_PHASE_TEST: &str = "e2e::e2e_projects_window";

    /// One GUI process: a Tauri window over the built frontend whose page runs
    /// `src/features/projects/e2e.ts` against the real project commands.
    #[test]
    #[ignore = "window phase of e2e_projects_flow; fails when run outside the harness"]
    fn e2e_projects_window() {
        let phase = native_harness::current_phase();
        let prefs = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_PREFS"));
        let params: Value =
            serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS")).unwrap();
        let herdr_config = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());
        let builder = tauri::Builder::default()
            .manage(ProjectsState::new(prefs, herdr_config))
            .invoke_handler(tauri::generate_handler![
                project_store::projects_list,
                project_store::project_create,
                project_store::collection_create,
                project_store::collection_add_project,
                project_store::collection_remove_project,
                project_store::collection_move_project,
                project_store::collection_move,
                project_store::project_open,
                window_harness::harness_report,
            ]);
        window_harness::run_feature_window(
            tauri::generate_context!(),
            builder,
            "projects",
            &phase,
            params,
            PathBuf::from(native_harness::required(native_harness::RESULT_ENV)),
            Duration::from_secs(90),
        );
        panic!("the harness window returned without reporting done");
    }

    fn workspace_ids(gateway: &LocalGateway) -> Vec<String> {
        let mut ids: Vec<String> = gateway.api_request("workspace.list", json!({})).unwrap()
            ["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["workspace_id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    }

    fn shell_pid_of(gateway: &LocalGateway, workspace_id: &str) -> u32 {
        let panes = gateway
            .api_request("pane.list", json!({ "workspace_id": workspace_id }))
            .unwrap();
        let pane_id = panes["panes"][0]["pane_id"]
            .as_str()
            .unwrap_or_else(|| panic!("workspace {workspace_id} has no pane: {panes}"))
            .to_owned();
        for _ in 0..50 {
            if let Some(pid) = gateway.api().pane_shell_pid(&pane_id).unwrap() {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("pane {pane_id} never reported a shell pid");
    }

    fn names_and_labels(collections: &Value) -> Value {
        json!(collections
            .as_array()
            .unwrap()
            .iter()
            .map(|c| json!([
                c["name"],
                c["projects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p["label"].clone())
                    .collect::<Vec<_>>()
            ]))
            .collect::<Vec<_>>())
    }

    fn ids(collections: &Value) -> Value {
        json!(collections
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["projects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["id"].clone())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>())
    }

    fn store_view(prefs: &std::path::Path) -> Value {
        let raw: Value = serde_json::from_str(
            &std::fs::read_to_string(prefs.join(project_store::STORE_FILE)).unwrap(),
        )
        .unwrap();
        json!(raw["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| json!([c["name"], c["project_ids"]]))
            .collect::<Vec<_>>())
    }

    #[test]
    #[ignore = "needs the disposable session created by scripts/feature-harness/session.sh; run by just check-spec 002"]
    fn e2e_projects_flow() {
        let session = native_harness::required("HERDR_DESKTOP_E2E_SESSION");
        let report = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_REPORT"));
        let work = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_DIR")).join("work");
        assert!(
            session.starts_with("hd002-"),
            "never run against a non-disposable session: {session}"
        );
        std::fs::create_dir_all(&report).unwrap();
        let session_name = SessionName::parse(&session).unwrap();
        let herdr_config = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());
        let paths = SessionPaths::for_session(&herdr_config, &session_name);
        let endpoints = herdr_config.join("endpoints.json");
        let endpoints_before = std::fs::read(&endpoints).ok();

        // Observer connection of the test itself (not the GUI's), used only to check the engine.
        let mut observer = LocalGateway::new(&herdr_config, session_name.clone());
        project_store::attach_metadata_gateway(&mut observer, Duration::from_secs(20))
            .expect("observer attach to the disposable session");
        let boot_id = observer.identity().unwrap().boot_id;

        let root_a = work.join("projeto-a");
        let root_b = work.join("projeto-b");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();
        let prefs_root = tempfile::tempdir().unwrap();
        let prefs = prefs_root.path().join("prefs");
        let prefs_str = prefs.display().to_string();
        let params = json!({
            "session": session,
            "root_a": root_a.display().to_string(),
            "root_b": root_b.display().to_string(),
        })
        .to_string();
        let env = [
            ("HERDR_DESKTOP_E2E_PREFS", prefs_str.as_str()),
            ("HERDR_DESKTOP_E2E_PARAMS", params.as_str()),
            ("HERDR_DESKTOP_E2E_SESSION", session.as_str()),
            (
                "HERDR_DESKTOP_E2E_DIR",
                work.parent().unwrap().to_str().unwrap(),
            ),
        ];

        let mut log = std::fs::File::create(report.join("e2e-projects.log")).unwrap();
        let mut note = |line: String| {
            eprintln!("{line}");
            writeln!(log, "{line}").unwrap();
        };
        note(format!(
            "session={session} boot={boot_id} prefs=<tempdir>/prefs (removed at the end)"
        ));

        // --- GUI 1: create collections and projects in the window, open A, open A again --------
        let workspaces_before = workspace_ids(&observer);
        let gui1 = run_phase(WINDOW_PHASE_TEST, "create", prefs_root.path(), &env);
        let c = &gui1.result;
        note(format!("GUI 1 (pid {}) window report: {c}", gui1.pid));
        assert!(c["error"].is_null(), "{c}");
        assert_eq!(c["loaded"], json!([]), "empty store on first launch");
        let expected_initial = json!([
            ["Produto", ["Projeto A", "Projeto B"]],
            ["Pessoal", ["Projeto A"]]
        ]);
        assert_eq!(
            names_and_labels(&c["collections_before_open"]),
            expected_initial
        );
        let initial_ids = ids(&c["collections"]);
        let a = initial_ids[0][0].as_str().unwrap().to_owned();
        let b = initial_ids[0][1].as_str().unwrap().to_owned();
        assert_ne!(a, b, "distinct UUIDs");
        assert_eq!(initial_ids, json!([[a, b], [a]]));
        assert_eq!(c["first_open"]["outcome"], "created");
        assert_eq!(
            c["second_open"]["outcome"], "reused",
            "reopen in the same boot"
        );
        let workspace = c["first_open"]["workspace"].as_str().unwrap().to_owned();
        assert_eq!(c["second_open"]["workspace"], workspace.as_str());
        for collection in c["collections"].as_array().unwrap() {
            for project in collection["projects"].as_array().unwrap() {
                if project["id"] == a.as_str() {
                    assert_eq!(project["workspace"], workspace.as_str(), "{project}");
                    assert_eq!(project["endpoint"], "Local");
                }
            }
        }
        let workspaces_after_gui1 = workspace_ids(&observer);
        let mut expected = workspaces_before.clone();
        expected.push(workspace.clone());
        expected.sort();
        assert_eq!(
            workspaces_after_gui1, expected,
            "exactly one workspace created by the two opens"
        );
        let pid = shell_pid_of(&observer, &workspace);
        assert!(native_harness::process_alive(pid));
        note(format!(
            "engine after GUI 1: workspaces {workspaces_before:?} -> {workspaces_after_gui1:?}; workspace {workspace} shell pid {pid}"
        ));
        let store_file = std::fs::read_to_string(prefs.join(project_store::STORE_FILE)).unwrap();
        assert!(!store_file.contains(&workspace) && !store_file.contains(&boot_id));
        assert_eq!(
            store_view(&prefs),
            json!([["Produto", [a, b]], ["Pessoal", [a]]])
        );
        assert!(
            native_harness::process_alive(pid),
            "closing the GUI kept the shell"
        );

        // --- GUI 2: restart; state loaded in the window; open A (rediscover); remove from Produto --
        let gui2 = run_phase(WINDOW_PHASE_TEST, "restart", prefs_root.path(), &env);
        let r = &gui2.result;
        note(format!("GUI 2 (pid {}) window report: {r}", gui2.pid));
        assert_ne!(gui2.pid, gui1.pid, "restart is a new GUI process");
        assert!(r["error"].is_null(), "{r}");
        assert_eq!(
            names_and_labels(&r["loaded"]),
            expected_initial,
            "three associations and their order after restart"
        );
        assert_eq!(ids(&r["loaded"]), json!([[a, b], [a]]));
        for collection in r["loaded"].as_array().unwrap() {
            for project in collection["projects"].as_array().unwrap() {
                assert_eq!(
                    project["status"], "closed",
                    "no binding survives a restart: {project}"
                );
            }
        }
        assert_eq!(r["open"]["outcome"], "rediscovered");
        assert_eq!(
            r["open"]["workspace"],
            workspace.as_str(),
            "same workspace after restart"
        );
        assert_eq!(
            workspace_ids(&observer),
            workspaces_after_gui1,
            "restart + open + removal created or closed nothing"
        );
        assert_eq!(
            shell_pid_of(&observer, &workspace),
            pid,
            "same shell process"
        );
        assert!(native_harness::process_alive(pid));
        let expected_after_remove = json!([["Produto", ["Projeto B"]], ["Pessoal", ["Projeto A"]]]);
        assert_eq!(names_and_labels(&r["collections"]), expected_after_remove);
        let pessoal_a = &r["collections"][1]["projects"][0];
        assert_eq!(pessoal_a["status"], "open");
        assert_eq!(pessoal_a["workspace"], workspace.as_str());
        note(format!(
            "engine after GUI 2: workspaces {:?}; shell pid {pid} alive",
            workspace_ids(&observer)
        ));

        // --- GUI 3: restart again; the removal persisted ------------------------------------------
        let gui3 = run_phase(WINDOW_PHASE_TEST, "reload", prefs_root.path(), &env);
        let l = &gui3.result;
        note(format!("GUI 3 (pid {}) window report: {l}", gui3.pid));
        assert!(l["error"].is_null(), "{l}");
        assert_eq!(names_and_labels(&l["loaded"]), expected_after_remove);
        assert_eq!(ids(&l["loaded"]), json!([[b], [a]]));
        assert_eq!(
            store_view(&prefs),
            json!([["Produto", [b]], ["Pessoal", [a]]])
        );
        assert_eq!(workspace_ids(&observer), workspaces_after_gui1);
        assert!(native_harness::process_alive(pid));

        // Engine persistence untouched.
        assert_eq!(
            std::fs::read(&endpoints).ok(),
            endpoints_before,
            "endpoints.json changed"
        );
        assert!(
            !paths.data_dir.join(project_store::STORE_FILE).exists()
                && !herdr_config.join(project_store::STORE_FILE).exists()
        );
        note("endpoints.json unchanged; no store file in the engine config dir".into());

        std::fs::write(
            report.join("e2e-projects-summary.json"),
            serde_json::to_string_pretty(&json!({
                "session": session,
                "boot_id": boot_id,
                "gui_processes": { "create": gui1.pid, "restart": gui2.pid, "reload": gui3.pid },
                "project_ids": { "a": a, "b": b },
                "workspaces_before": workspaces_before,
                "workspaces_after": workspaces_after_gui1,
                "workspace_id": workspace,
                "opens": {
                    "gui1_first": c["first_open"],
                    "gui1_second": c["second_open"],
                    "gui2_after_restart": r["open"],
                },
                "shell_pid": pid,
                "collections_gui2_loaded": names_and_labels(&r["loaded"]),
                "collections_gui3_loaded": names_and_labels(&l["loaded"]),
                "endpoints_json_unchanged": true,
            }))
            .unwrap(),
        )
        .unwrap();
        observer.detach();
    }
}
