//! Projects and collections (spec 002).
//!
//! - [`ProjectStore`]: local, versioned preferences file (`projects.json`) written
//!   atomically. It holds durable [`ProjectRef`]s (UUID + endpoint/session/root) and ordered
//!   collections of project ids. It never holds workspace/pane ids, boot ids or generations
//!   and never lives inside the engine config dir (endpoints.json, session snapshots).
//! - [`ProjectService`]: store + in-memory [`RuntimeBinding`]s. Opening a project validates
//!   the declared endpoint/session against the live identity of the gateway it is given (no
//!   fallback to Local), then reuses, rediscovers (workspace tagged with the project UUID) or
//!   creates the workspace through the JSON API, binding the current boot/generation.
//!   Collection edits never reach the engine: removing an association closes nothing.
//! - [`ProjectsState`]: harness mode (`new`, spec 002: Local only, own connection) or hosted
//!   mode (`with_gateways`, spec 007: every project opens through [`ProjectGateways`], the
//!   registered Local/SSH host, followed by one qualified focus and one root authorization).
//! - Tauri commands (`COMMANDS`) for the WebView. Registering them in the window belongs to
//!   spec 007; until then they are compiled and exercised by `src-tauri/tests/projects.rs`.
//!   Spec 007: every command is `async` and never runs on the IPC calling thread. A call takes
//!   a turn in the state's command lane when it enters; the turn is awaited without holding a
//!   thread and the blocking body (store lock, preferences file, host connection) runs on the
//!   async runtime's bounded blocking pool, so the commands of one state apply in entry order.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use herdr_client::{
    ConnectOptions, LiveIdentity, LocalGateway, ProjectRef, RuntimeBinding, RuntimeError,
    RuntimeGateway, SessionName, SurfaceGeometry, LOCAL_ENDPOINT,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// File name of the store inside the preferences directory.
pub const STORE_FILE: &str = "projects.json";
/// Current schema version of [`StoreDocument`].
pub const STORE_VERSION: u32 = 4;
/// Workspace metadata token carrying the project UUID (rediscovery within one boot).
pub const PROJECT_TOKEN: &str = "herdr_desktop_project";
/// Metadata source used for the token.
pub const METADATA_SOURCE: &str = "herdr-desktop";

/// Colours a workspace preference may carry (spec 044). Mirror of `GROUP_PALETTE` in
/// `src/components/projects/tree-model.ts`: the WebView sends one of these exact values and
/// anything else is refused, so a stored colour is always a palette colour.
pub const GROUP_PALETTE: &[&str] = &[
    "#8FA8FF", // Blue
    "#5BD68A", // Green
    "#B18CFF", // Purple
    "#F4B454", // Amber
    "#F2777A", // Red
    "#5CE1E6", // Cyan
];

/// A root longer than this is not a path the desktop will key a preference by.
const MAX_PREF_ROOT_BYTES: usize = 4096;

/// Spec 046: how many folders the "Recentes neste host" list keeps per endpoint. The bound is
/// per host, so a busy host never evicts the recents of another one.
pub const MAX_RECENT_FOLDERS_PER_HOST: usize = 8;

const MAX_COLLECTION_NAME_CHARS: usize = 80;
const MAX_PROJECT_LABEL_CHARS: usize = 120;
const MAX_ENDPOINT_PROFILE_CHARS: usize = 64;
/// Longest folder-picker title the WebView may ask for (spec 071, AC-071-03). The title is the
/// only text the host takes from the front, so it is bounded and never empty.
pub const MAX_PICKER_TITLE_CHARS: usize = 80;
/// Title used when the WebView sends none, an empty one or one above the limit.
pub const DEFAULT_PICKER_TITLE: &str = "Open project";

// ---------------------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------------------

/// Ordered list of project ids. The same project may belong to several groups.
/// (Spec 025: the UI calls these groups; the wire snapshot keeps the historical name.)
///
/// Spec 045 adds two optional fields. Both are skipped while they say nothing, so a v2 catalog
/// migrated to v3 keeps its groups byte for byte until the user actually picks a colour or
/// collapses the collection — absence is a state, not a colour to invent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub project_ids: Vec<String>,
    /// One of [`GROUP_PALETTE`], or `None` while the sidebar uses the position colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Collapsed in the sidebar (spec 045, AC-045-03); `false` is not written.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Per-workspace client preference (spec 044), keyed by `(endpoint_profile_id, normalized
/// root)` — the same key spec 025 uses for a [`ProjectRef`]. It belongs to the desktop and is
/// never sent to the engine: the engine knows nothing about colours, pinning or hiding.
/// An entry only exists while it says something; see [`ProjectStore::set_workspace_pref`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePref {
    pub endpoint_profile_id: String,
    pub root: String,
    /// One of [`GROUP_PALETTE`], or `None` for the default folder colour.
    pub color: Option<String>,
    pub pinned: bool,
    pub hidden: bool,
}

/// Fields of a [`WorkspacePref`] the caller wants changed; `None` leaves the current value.
/// The IPC layer cannot tell an absent argument from a null one, so this spec never clears a
/// colour: an entry disappears when it is neither coloured, pinned nor hidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WorkspacePrefPatch {
    pub color: Option<String>,
    pub pinned: Option<bool>,
    pub hidden: Option<bool>,
}

/// One folder the user already opened as a workspace on a host (spec 046), newest first. It is
/// a client-only list feeding "Recentes neste host" of the Novo workspace dialog: the engine is
/// never asked for it and nothing here opens anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentFolder {
    pub endpoint_profile_id: String,
    pub path: String,
}

/// Persisted document, schema [`STORE_VERSION`]. Spec 025 v2: the old `collections` are the
/// `groups` and every saved project is a closed project (a durable cwd waiting for a workspace).
/// Spec 044 v3 adds `workspace_prefs`, leaving the v2 fields exactly as they were.
/// Spec 046 v4 adds `recent_folders`, again leaving every older field untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreDocument {
    pub version: u32,
    pub projects: Vec<ProjectRef>,
    pub groups: Vec<Group>,
    pub workspace_prefs: Vec<WorkspacePref>,
    pub recent_folders: Vec<RecentFolder>,
}

impl StoreDocument {
    fn empty() -> Self {
        Self {
            version: STORE_VERSION,
            projects: Vec::new(),
            groups: Vec::new(),
            workspace_prefs: Vec::new(),
            recent_folders: Vec::new(),
        }
    }
}

/// Explicit migration entry point: every supported on-disk version is listed here and
/// converted to the current document. Unknown, newer or missing versions are refused so an
/// older GUI never rewrites a store it does not understand.
pub fn migrate(raw: Value) -> Result<StoreDocument, RuntimeError> {
    let Some(version) = raw.get("version").and_then(Value::as_u64) else {
        return Err(RuntimeError::new(
            "store_unversioned",
            "the projects file declares no version; nothing was changed",
        ));
    };
    match version {
        // v1 (spec 002): `collections` of `project_ids`. Migrated in memory: same ids and order,
        // the field becomes `groups`; the next write persists the v2 document. Each saved
        // project is a closed project already — nothing is fabricated and nothing is dropped.
        1 => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct StoreDocumentV1 {
                #[allow(dead_code)]
                version: u32,
                projects: Vec<ProjectRef>,
                collections: Vec<Group>,
            }
            let old: StoreDocumentV1 = serde_json::from_value(raw).map_err(|_| corrupt())?;
            let doc = StoreDocument {
                version: STORE_VERSION,
                projects: old.projects,
                groups: old.collections,
                workspace_prefs: Vec::new(),
                recent_folders: Vec::new(),
            };
            validate_document(&doc)?;
            Ok(doc)
        }
        // v2 (spec 025): `groups` without preferences. Migrated in memory to v3 by adding an
        // empty `workspace_prefs`; projects and groups are carried over untouched.
        2 => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct StoreDocumentV2 {
                #[allow(dead_code)]
                version: u32,
                projects: Vec<ProjectRef>,
                groups: Vec<Group>,
            }
            let old: StoreDocumentV2 = serde_json::from_value(raw).map_err(|_| corrupt())?;
            let doc = StoreDocument {
                version: STORE_VERSION,
                projects: old.projects,
                groups: old.groups,
                workspace_prefs: Vec::new(),
                recent_folders: Vec::new(),
            };
            validate_document(&doc)?;
            Ok(doc)
        }
        // v3 (spec 044): preferences without recents. Migrated in memory to v4 by adding an
        // empty `recent_folders`; projects, groups and preferences are carried over untouched.
        3 => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct StoreDocumentV3 {
                #[allow(dead_code)]
                version: u32,
                projects: Vec<ProjectRef>,
                groups: Vec<Group>,
                workspace_prefs: Vec<WorkspacePref>,
            }
            let old: StoreDocumentV3 = serde_json::from_value(raw).map_err(|_| corrupt())?;
            let doc = StoreDocument {
                version: STORE_VERSION,
                projects: old.projects,
                groups: old.groups,
                workspace_prefs: old.workspace_prefs,
                recent_folders: Vec::new(),
            };
            validate_document(&doc)?;
            Ok(doc)
        }
        4 => {
            let doc: StoreDocument = serde_json::from_value(raw).map_err(|_| corrupt())?;
            validate_document(&doc)?;
            Ok(doc)
        }
        other => Err(RuntimeError::new(
            "store_version_unsupported",
            format!(
                "the projects file uses version {other}; this desktop version reads only {STORE_VERSION}. Nothing was changed"
            ),
        )),
    }
}

fn corrupt() -> RuntimeError {
    RuntimeError::new(
        "store_corrupt",
        "the projects file is invalid; nothing was changed",
    )
}

fn validate_document(doc: &StoreDocument) -> Result<(), RuntimeError> {
    let mut seen = std::collections::HashSet::new();
    for project in &doc.projects {
        if uuid::Uuid::parse_str(&project.id).is_err() || !seen.insert(project.id.as_str()) {
            return Err(corrupt());
        }
    }
    let mut groups = std::collections::HashSet::new();
    for group in &doc.groups {
        if uuid::Uuid::parse_str(&group.id).is_err() || !groups.insert(group.id.as_str()) {
            return Err(corrupt());
        }
        // Spec 045: a colour outside the palette is a file written by something else, never
        // repainted silently.
        if let Some(color) = group.color.as_deref() {
            if palette_color(color).is_none() {
                return Err(corrupt());
            }
        }
        let mut members = std::collections::HashSet::new();
        for id in &group.project_ids {
            if !seen.contains(id.as_str()) || !members.insert(id.as_str()) {
                return Err(corrupt());
            }
        }
    }
    // Spec 044: a preference is refused rather than repainted — an unknown colour, an invalid
    // root or a duplicated key means the file was written by something else.
    let mut keys = std::collections::HashSet::new();
    for pref in &doc.workspace_prefs {
        if validate_pref_endpoint(&pref.endpoint_profile_id).is_err()
            || validate_pref_root(&pref.root).is_err()
            || !keys.insert((
                pref.endpoint_profile_id.as_str(),
                normalize_pref_root(&pref.root),
            ))
        {
            return Err(corrupt());
        }
        if let Some(color) = pref.color.as_deref() {
            if palette_color(color).is_none() {
                return Err(corrupt());
            }
        }
    }
    // Spec 046: the recents are a bounded, deduplicated list per host. A file with an invalid
    // key, the same folder twice or more than `MAX_RECENT_FOLDERS_PER_HOST` entries on one host
    // was written by something else: it is refused, never trimmed silently.
    let mut recent_keys = std::collections::HashSet::new();
    let mut per_host: HashMap<&str, usize> = HashMap::new();
    for recent in &doc.recent_folders {
        if validate_pref_endpoint(&recent.endpoint_profile_id).is_err()
            || validate_pref_root(&recent.path).is_err()
            || !recent_keys.insert((
                recent.endpoint_profile_id.as_str(),
                normalize_pref_root(&recent.path),
            ))
        {
            return Err(corrupt());
        }
        let count = per_host
            .entry(recent.endpoint_profile_id.as_str())
            .or_insert(0);
        *count += 1;
        if *count > MAX_RECENT_FOLDERS_PER_HOST {
            return Err(corrupt());
        }
    }
    Ok(())
}

/// The canonical palette entry equal to `value` ignoring ASCII case, or `None` when the colour
/// is not one the desktop offers.
fn palette_color(value: &str) -> Option<&'static str> {
    GROUP_PALETTE
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(value.trim()))
}

/// The store has no host registry of its own: an endpoint it can key a preference by is one
/// whose profile id is well formed (`local` or an SSH profile of the catalog). Anything else
/// is an endpoint this desktop does not know.
fn validate_pref_endpoint(endpoint: &str) -> Result<String, RuntimeError> {
    let endpoint = endpoint.trim();
    let known = !endpoint.is_empty()
        && endpoint.len() <= MAX_ENDPOINT_PROFILE_CHARS
        && !endpoint.starts_with('.')
        && endpoint
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !known {
        return Err(RuntimeError::new(
            "unknown_endpoint",
            "the host of this preference is not configured",
        ));
    }
    Ok(endpoint.to_owned())
}

fn validate_pref_root(root: &str) -> Result<String, RuntimeError> {
    let root = root.trim();
    if root.is_empty() || root.len() > MAX_PREF_ROOT_BYTES || root.chars().any(char::is_control) {
        return Err(RuntimeError::new(
            "invalid_root",
            format!("the workspace root must be 1 to {MAX_PREF_ROOT_BYTES} bytes long"),
        ));
    }
    Ok(root.to_owned())
}

/// Same normalization the 025 tree applies before comparing a saved root with a live cwd.
fn normalize_pref_root(root: &str) -> String {
    let trimmed = root.trim();
    let stripped = trimmed.trim_end_matches(['/', '\\']);
    if stripped.is_empty() {
        trimmed.to_owned()
    } else {
        stripped.to_owned()
    }
}

/// Writes `bytes` to a sibling temp file, syncs it and renames it over `path`, so readers see
/// either the previous or the new content. The temp file is removed on failure.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(write_failed_kind_other)?;
    std::fs::create_dir_all(parent).map_err(write_failed)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(write_failed_kind_other)?;
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(write_failed(error));
    }
    Ok(())
}

fn write_failed(error: std::io::Error) -> RuntimeError {
    let base = RuntimeError::from_io_kind(error.kind(), "could not write projects");
    RuntimeError::new("store_write_failed", base.message)
}

fn write_failed_kind_other() -> RuntimeError {
    RuntimeError::new(
        "store_write_failed",
        "could not write projects: invalid destination",
    )
}

// ---------------------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------------------

/// Form data for a new project. Validated by the backend; the WebView only checks presence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDraft {
    pub label: String,
    pub endpoint_profile_id: String,
    pub session_name: String,
    pub root: String,
}

/// `group_assign` entry: a live workspace identified by its root cwd, not by a volatile id
/// (spec 025, AC-025-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupAssignEntry {
    pub endpoint_profile_id: String,
    pub session_name: String,
    pub cwd: String,
    pub label: String,
}

/// Durable projects (closed and open) and groups in the preferences directory.
#[derive(Debug)]
pub struct ProjectStore {
    path: PathBuf,
    doc: StoreDocument,
}

impl ProjectStore {
    /// Loads (or starts empty, without writing) the store in `prefs_dir`. Refuses a
    /// preferences directory inside the engine config dir.
    pub fn open(prefs_dir: &Path, herdr_config_dir: &Path) -> Result<Self, RuntimeError> {
        if is_within(prefs_dir, herdr_config_dir) {
            return Err(RuntimeError::new(
                "prefs_dir_forbidden",
                "the desktop preferences cannot live in Herdr's configuration directory",
            ));
        }
        let path = prefs_dir.join(STORE_FILE);
        let doc = match std::fs::read(&path) {
            Ok(bytes) => {
                let raw: Value = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
                migrate(raw)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreDocument::empty(),
            Err(error) => {
                let base = RuntimeError::from_io_kind(error.kind(), "could not read projects");
                return Err(RuntimeError::new("store_read_failed", base.message));
            }
        };
        Ok(Self { path, doc })
    }

    pub fn document(&self) -> &StoreDocument {
        &self.doc
    }

    pub fn project(&self, id: &str) -> Option<&ProjectRef> {
        self.doc.projects.iter().find(|p| p.id == id)
    }

    /// Applies `change` to a copy and persists it; memory is updated only after the atomic
    /// write succeeded.
    fn commit<T>(
        &mut self,
        change: impl FnOnce(&mut StoreDocument) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let mut next = self.doc.clone();
        let value = change(&mut next)?;
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|_| RuntimeError::new("store_write_failed", "could not serialize projects"))?;
        atomic_write(&self.path, &bytes)?;
        self.doc = next;
        Ok(value)
    }

    /// Creates an empty group (the 002 collections are the 025 groups; `group_create` uses it).
    pub fn create_collection(&mut self, name: &str) -> Result<Group, RuntimeError> {
        let name = validate_collection_name(name)?;
        let group = Group {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            project_ids: Vec::new(),
            color: None,
            collapsed: false,
        };
        self.commit(|doc| {
            doc.groups.push(group.clone());
            Ok(group)
        })
    }

    pub fn create_group(&mut self, name: &str) -> Result<Group, RuntimeError> {
        self.create_collection(name)
    }

    /// Assigns a live workspace to a group by its root cwd (AC-025-02): the saved project is
    /// upserted by (endpoint, session, root), so the same cwd reopened returns to its group and
    /// a repeated move never duplicates a row.
    pub fn assign_root(
        &mut self,
        group_id: &str,
        draft: ProjectDraft,
    ) -> Result<ProjectRef, RuntimeError> {
        let (label, endpoint, session, root) = validate_project_draft(draft)?;
        self.commit(|doc| {
            let existing = doc
                .projects
                .iter()
                .find(|p| {
                    p.endpoint_profile_id == endpoint && p.session_name == session && p.root == root
                })
                .cloned();
            let project = match existing {
                Some(project) => project,
                None => {
                    let project = ProjectRef::new(label, endpoint, session, root);
                    doc.projects.push(project.clone());
                    project
                }
            };
            let group = group_mut(doc, group_id)?;
            if !group.project_ids.iter().any(|id| id == &project.id) {
                group.project_ids.push(project.id.clone());
            }
            Ok(project)
        })
    }

    pub fn create_project(&mut self, draft: ProjectDraft) -> Result<ProjectRef, RuntimeError> {
        let (label, endpoint, session, root) = validate_project_draft(draft)?;
        let project = ProjectRef::new(label, endpoint, session, root);
        self.commit(|doc| {
            doc.projects.push(project.clone());
            Ok(project)
        })
    }

    pub fn add_to_collection(
        &mut self,
        collection_id: &str,
        project_id: &str,
        index: Option<usize>,
    ) -> Result<(), RuntimeError> {
        self.commit(|doc| {
            if !doc.projects.iter().any(|p| p.id == project_id) {
                return Err(project_not_found());
            }
            let collection = group_mut(doc, collection_id)?;
            if collection.project_ids.iter().any(|id| id == project_id) {
                return Err(RuntimeError::new(
                    "association_exists",
                    "the project is already in this collection",
                ));
            }
            let at = index
                .unwrap_or(collection.project_ids.len())
                .min(collection.project_ids.len());
            collection.project_ids.insert(at, project_id.to_owned());
            Ok(())
        })
    }

    /// Removes only the association. The project, its other associations and any live
    /// workspace are untouched; nothing is sent to the engine.
    pub fn remove_from_collection(
        &mut self,
        collection_id: &str,
        project_id: &str,
    ) -> Result<(), RuntimeError> {
        self.commit(|doc| {
            let collection = group_mut(doc, collection_id)?;
            let at = position(&collection.project_ids, project_id)?;
            collection.project_ids.remove(at);
            Ok(())
        })
    }

    /// Moves a project inside one collection (keyboard step or drag target index).
    pub fn move_project(
        &mut self,
        collection_id: &str,
        project_id: &str,
        to_index: usize,
    ) -> Result<(), RuntimeError> {
        self.commit(|doc| {
            let collection = group_mut(doc, collection_id)?;
            let from = position(&collection.project_ids, project_id)?;
            let id = collection.project_ids.remove(from);
            let to = to_index.min(collection.project_ids.len());
            collection.project_ids.insert(to, id);
            Ok(())
        })
    }

    /// Spec 044 (AC-044-01): writes or updates the preference of `(endpoint, normalized root)`.
    /// Fields absent from `patch` keep their current value; the entry is removed — never stored
    /// empty — as soon as it is neither coloured, pinned nor hidden. Answers the resulting
    /// entry, or `None` when it was dropped. Nothing here reaches the engine.
    pub fn set_workspace_pref(
        &mut self,
        endpoint_profile_id: &str,
        root: &str,
        patch: WorkspacePrefPatch,
    ) -> Result<Option<WorkspacePref>, RuntimeError> {
        let endpoint = validate_pref_endpoint(endpoint_profile_id)?;
        let root = validate_pref_root(root)?;
        let color = match patch.color.as_deref() {
            Some(value) => Some(palette_color(value).ok_or_else(|| {
                RuntimeError::new(
                    "invalid_color",
                    "this colour does not belong to the desktop palette",
                )
            })?),
            None => None,
        };
        let key = normalize_pref_root(&root);
        self.commit(move |doc| {
            let at = doc.workspace_prefs.iter().position(|pref| {
                pref.endpoint_profile_id == endpoint && normalize_pref_root(&pref.root) == key
            });
            let mut entry = match at {
                Some(at) => doc.workspace_prefs[at].clone(),
                None => WorkspacePref {
                    endpoint_profile_id: endpoint.clone(),
                    root: key.clone(),
                    color: None,
                    pinned: false,
                    hidden: false,
                },
            };
            if let Some(color) = color {
                entry.color = Some(color.to_owned());
            }
            if let Some(pinned) = patch.pinned {
                entry.pinned = pinned;
            }
            if let Some(hidden) = patch.hidden {
                entry.hidden = hidden;
            }
            let meaningful = entry.color.is_some() || entry.pinned || entry.hidden;
            match (at, meaningful) {
                (Some(at), true) => {
                    doc.workspace_prefs[at] = entry.clone();
                    Ok(Some(entry))
                }
                (Some(at), false) => {
                    doc.workspace_prefs.remove(at);
                    Ok(None)
                }
                (None, true) => {
                    doc.workspace_prefs.push(entry.clone());
                    Ok(Some(entry))
                }
                (None, false) => Ok(None),
            }
        })
    }

    /// Spec 046 (AC-046-02): records `path` as the newest folder used on `endpoint`. The entry
    /// goes to the top of the list; the same folder (normalized like the 025 roots) is moved,
    /// never duplicated, and the host keeps at most [`MAX_RECENT_FOLDERS_PER_HOST`] entries —
    /// the oldest of that host leaves, the other hosts are untouched. Nothing reaches the engine.
    pub fn record_recent_folder(
        &mut self,
        endpoint_profile_id: &str,
        path: &str,
    ) -> Result<RecentFolder, RuntimeError> {
        let endpoint = validate_pref_endpoint(endpoint_profile_id)?;
        let path = normalize_pref_root(&validate_pref_root(path)?);
        let entry = RecentFolder {
            endpoint_profile_id: endpoint,
            path,
        };
        self.commit(move |doc| {
            doc.recent_folders.retain(|recent| {
                recent.endpoint_profile_id != entry.endpoint_profile_id
                    || normalize_pref_root(&recent.path) != entry.path
            });
            doc.recent_folders.insert(0, entry.clone());
            let mut seen = 0usize;
            doc.recent_folders.retain(|recent| {
                if recent.endpoint_profile_id != entry.endpoint_profile_id {
                    return true;
                }
                seen += 1;
                seen <= MAX_RECENT_FOLDERS_PER_HOST
            });
            Ok(entry)
        })
    }

    /// The recent folders of one host, newest first (spec 046).
    pub fn recent_folders(&self, endpoint_profile_id: &str) -> Vec<String> {
        self.doc
            .recent_folders
            .iter()
            .filter(|recent| recent.endpoint_profile_id == endpoint_profile_id)
            .map(|recent| recent.path.clone())
            .collect()
    }

    /// Spec 045 (AC-045-02): renames one collection. Same bounds as `create_collection`, so a
    /// name the catalog would refuse on creation is refused here too.
    pub fn rename_collection(
        &mut self,
        collection_id: &str,
        name: &str,
    ) -> Result<(), RuntimeError> {
        let name = validate_collection_name(name)?;
        self.commit(|doc| {
            group_mut(doc, collection_id)?.name = name;
            Ok(())
        })
    }

    /// Spec 045 (AC-045-02): the collection colour, always one of [`GROUP_PALETTE`].
    pub fn set_collection_color(
        &mut self,
        collection_id: &str,
        color: &str,
    ) -> Result<(), RuntimeError> {
        let color = palette_color(color).ok_or_else(|| {
            RuntimeError::new(
                "invalid_color",
                "this colour does not belong to the desktop palette",
            )
        })?;
        self.commit(|doc| {
            group_mut(doc, collection_id)?.color = Some(color.to_owned());
            Ok(())
        })
    }

    /// Spec 045 (AC-045-03): the collapse state of a stored collection, so it survives a reload.
    pub fn set_collection_collapsed(
        &mut self,
        collection_id: &str,
        collapsed: bool,
    ) -> Result<(), RuntimeError> {
        self.commit(|doc| {
            group_mut(doc, collection_id)?.collapsed = collapsed;
            Ok(())
        })
    }

    /// Spec 045 (AC-045-02): removes the collection only. Its projects stay in the catalog and
    /// simply have no collection ("Sem coleção"); no workspace is closed and nothing reaches the
    /// engine — the same rule `remove_from_collection` has followed since 002.
    pub fn delete_collection(&mut self, collection_id: &str) -> Result<(), RuntimeError> {
        self.commit(|doc| {
            let at = doc
                .groups
                .iter()
                .position(|g| g.id == collection_id)
                .ok_or_else(collection_not_found)?;
            doc.groups.remove(at);
            Ok(())
        })
    }

    pub fn move_collection(
        &mut self,
        collection_id: &str,
        to_index: usize,
    ) -> Result<(), RuntimeError> {
        self.commit(|doc| {
            let from = doc
                .groups
                .iter()
                .position(|c| c.id == collection_id)
                .ok_or_else(collection_not_found)?;
            let collection = doc.groups.remove(from);
            let to = to_index.min(doc.groups.len());
            doc.groups.insert(to, collection);
            Ok(())
        })
    }
}

/// Trimmed collection name, bounded like the 002 catalog (`invalid_collection_name`).
fn validate_collection_name(name: &str) -> Result<String, RuntimeError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_COLLECTION_NAME_CHARS {
        return Err(RuntimeError::new(
            "invalid_collection_name",
            format!("the group name must be 1 to {MAX_COLLECTION_NAME_CHARS} characters long"),
        ));
    }
    Ok(name.to_owned())
}

fn group_mut<'a>(
    doc: &'a mut StoreDocument,
    collection_id: &str,
) -> Result<&'a mut Group, RuntimeError> {
    doc.groups
        .iter_mut()
        .find(|c| c.id == collection_id)
        .ok_or_else(collection_not_found)
}

fn position(ids: &[String], project_id: &str) -> Result<usize, RuntimeError> {
    ids.iter().position(|id| id == project_id).ok_or_else(|| {
        RuntimeError::new(
            "association_not_found",
            "the project does not belong to this collection",
        )
    })
}

fn collection_not_found() -> RuntimeError {
    RuntimeError::new("collection_not_found", "collection not found")
}

fn project_not_found() -> RuntimeError {
    RuntimeError::new("project_not_found", "project not found")
}

/// Validates and normalizes a project/form draft: trimmed label, endpoint profile, session name
/// and root (absolute on the endpoint machine, no control characters).
fn validate_project_draft(
    draft: ProjectDraft,
) -> Result<(String, String, String, String), RuntimeError> {
    let label = draft.label.trim();
    if label.is_empty() || label.chars().count() > MAX_PROJECT_LABEL_CHARS {
        return Err(RuntimeError::new(
            "invalid_project_label",
            format!("the project name must be 1 to {MAX_PROJECT_LABEL_CHARS} characters long"),
        ));
    }
    let endpoint = draft.endpoint_profile_id.trim();
    if endpoint.is_empty()
        || endpoint.len() > MAX_ENDPOINT_PROFILE_CHARS
        || endpoint.starts_with('.')
        || !endpoint
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(RuntimeError::new(
            "invalid_endpoint_profile",
            "invalid endpoint profile",
        ));
    }
    let session = SessionName::parse(draft.session_name.trim())?;
    let root = draft.root.trim();
    let root_ok = !root.is_empty()
        && (endpoint != LOCAL_ENDPOINT || Path::new(root).is_absolute())
        && !root.chars().any(char::is_control);
    if !root_ok {
        return Err(RuntimeError::new(
            "invalid_project_root",
            "the project root must be an absolute path on the endpoint's machine",
        ));
    }
    Ok((
        label.to_owned(),
        endpoint.to_owned(),
        session.as_str().to_owned(),
        root.to_owned(),
    ))
}

/// Lexical containment after resolving what exists on disk (`..`, symlinks).
fn is_within(candidate: &Path, base: &Path) -> bool {
    let resolve = |p: &Path| -> PathBuf {
        // Canonicalize the longest existing ancestor and re-append the rest.
        let mut existing = p.to_path_buf();
        let mut rest = Vec::new();
        while !existing.exists() {
            match (existing.file_name(), existing.parent()) {
                (Some(name), Some(parent)) => {
                    rest.push(name.to_owned());
                    existing = parent.to_path_buf();
                }
                _ => break,
            }
        }
        let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
        for name in rest.into_iter().rev() {
            resolved.push(name);
        }
        resolved
    };
    resolve(candidate).starts_with(resolve(base))
}

// ---------------------------------------------------------------------------------------
// Service: bindings and open
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenOutcome {
    /// A new workspace was created on the declared endpoint/session.
    Created,
    /// No valid binding; the workspace tagged with this project was found in the same boot.
    Rediscovered,
    /// The binding was valid for the live identity and its workspace still exists.
    Reused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenResult {
    pub project_id: String,
    pub binding: RuntimeBinding,
    pub outcome: OpenOutcome,
    /// Previous binding discarded because boot/generation diverged or its workspace vanished.
    pub invalidated: Option<RuntimeBinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectDto {
    #[serde(flatten)]
    pub project: ProjectRef,
    pub binding: Option<RuntimeBinding>,
    /// Git branch of the project repository (spec 011); `None` when unknown, unborn or not local.
    pub branch: Option<String>,
}

/// Everything the WebView sees about projects. No paths of the preferences file.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectsSnapshot {
    pub version: u32,
    pub projects: Vec<ProjectDto>,
    /// The 025 groups (kept as `collections` on the wire for the WebView contract).
    pub collections: Vec<Group>,
    /// Spec 044: client-only preferences of each workspace, keyed by endpoint and root.
    pub workspace_prefs: Vec<WorkspacePref>,
    /// Spec 046: folders already opened as a workspace on each host, newest first.
    pub recent_folders: Vec<RecentFolder>,
}

/// Computes the git branch for a local project root using `git symbolic-ref --short HEAD`.
/// Bounded by a 500ms timeout, isolated environment without global git config, works without commits.
fn resolve_git_branch(root: &str) -> Option<String> {
    let path = Path::new(root);
    if !path.is_dir() {
        return None;
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["--no-optional-locks", "symbolic-ref", "--short", "HEAD"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    let mut stdout = child.stdout.take()?;
    let handle = std::thread::spawn(move || {
        let mut buf = String::new();
        use std::io::Read;
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    match rx.recv_timeout(Duration::from_millis(500)) {
        Ok(out) => {
            let _ = child.wait();
            let _ = handle.join();
            let branch = out.trim().to_owned();
            if !branch.is_empty() {
                Some(branch)
            } else {
                None
            }
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

pub struct ProjectService {
    store: ProjectStore,
    bindings: HashMap<String, RuntimeBinding>,
    branches: HashMap<String, Option<String>>,
}

impl ProjectService {
    pub fn new(store: ProjectStore) -> Self {
        let mut branches = HashMap::new();
        for p in &store.doc.projects {
            if p.endpoint_profile_id == "local" {
                branches.insert(p.id.clone(), resolve_git_branch(&p.root));
            } else {
                branches.insert(p.id.clone(), None);
            }
        }
        Self {
            store,
            bindings: HashMap::new(),
            branches,
        }
    }

    pub fn store(&self) -> &ProjectStore {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut ProjectStore {
        &mut self.store
    }

    pub fn binding(&self, project_id: &str) -> Option<&RuntimeBinding> {
        self.bindings.get(project_id)
    }

    pub fn refresh_branch(&mut self, project_id: &str) {
        if let Some(p) = self.store.project(project_id) {
            if p.endpoint_profile_id == "local" {
                self.branches
                    .insert(project_id.to_owned(), resolve_git_branch(&p.root));
            } else {
                self.branches.insert(project_id.to_owned(), None);
            }
        }
    }

    pub fn snapshot(&self) -> ProjectsSnapshot {
        ProjectsSnapshot {
            version: self.store.doc.version,
            projects: self
                .store
                .doc
                .projects
                .iter()
                .map(|project| ProjectDto {
                    project: project.clone(),
                    binding: self.bindings.get(&project.id).cloned(),
                    branch: self.branches.get(&project.id).cloned().flatten(),
                })
                .collect(),
            collections: self.store.doc.groups.clone(),
            workspace_prefs: self.store.doc.workspace_prefs.clone(),
            recent_folders: self.store.doc.recent_folders.clone(),
        }
    }

    /// Opens `project_id` through `gateway`, which must be the connection of the project's
    /// declared endpoint and session. See the module docs for the reuse/rediscover/create
    /// order. Errors belong to this project only; other bindings are untouched.
    pub fn open_project(
        &mut self,
        project_id: &str,
        gateway: &dyn RuntimeGateway,
    ) -> Result<OpenResult, RuntimeError> {
        let project = self
            .store
            .project(project_id)
            .cloned()
            .ok_or_else(project_not_found)?;
        self.refresh_branch(project_id);
        let live = validate_target(&project, gateway)?;

        let mut invalidated = None;
        if let Some(binding) = self.bindings.get(project_id).cloned() {
            if !binding.is_valid_for(&live) {
                self.bindings.remove(project_id);
                invalidated = Some(binding);
            }
        }

        let workspaces = list_workspaces(gateway)?;
        if let Some(binding) = self.bindings.get(project_id).cloned() {
            if workspaces.iter().any(|w| w.id == binding.workspace_id) {
                return Ok(OpenResult {
                    project_id: project_id.to_owned(),
                    binding,
                    outcome: OpenOutcome::Reused,
                    invalidated,
                });
            }
            self.bindings.remove(project_id);
            invalidated = Some(binding);
        }

        let (workspace_id, outcome) = match workspaces
            .iter()
            .find(|w| w.project.as_deref() == Some(project_id))
        {
            Some(found) => (found.id.clone(), OpenOutcome::Rediscovered),
            None => (create_workspace(gateway, &project)?, OpenOutcome::Created),
        };

        // The engine may have rebooted or the connection renewed while we talked to it.
        let after = validate_target(&project, gateway)?;
        if after.boot_id != live.boot_id {
            return Err(RuntimeError::new(
                "target_boot_stale",
                "the server restarted while opening; open the project again",
            )
            .retryable()
            .with_endpoint(project.endpoint_profile_id.clone()));
        }
        if after.connection_generation != live.connection_generation {
            return Err(RuntimeError::new(
                "target_generation_stale",
                "the connection was renewed while opening; open the project again",
            )
            .retryable()
            .with_endpoint(project.endpoint_profile_id.clone()));
        }

        let binding = RuntimeBinding {
            project_id: project_id.to_owned(),
            connection_generation: live.connection_generation,
            boot_id: live.boot_id,
            workspace_id,
        };
        self.bindings.insert(project_id.to_owned(), binding.clone());
        Ok(OpenResult {
            project_id: project_id.to_owned(),
            binding,
            outcome,
            invalidated,
        })
    }
}

/// Endpoint and session of the gateway and of its live identity must be the project's.
fn validate_target(
    project: &ProjectRef,
    gateway: &dyn RuntimeGateway,
) -> Result<LiveIdentity, RuntimeError> {
    let endpoint = project.endpoint_profile_id.as_str();
    let mismatch_endpoint = || {
        RuntimeError::new(
            "target_endpoint_mismatch",
            "the available connection belongs to another endpoint; the project was not opened",
        )
        .with_endpoint(endpoint)
    };
    if gateway.endpoint() != endpoint {
        return Err(mismatch_endpoint());
    }
    let live = gateway.identity().ok_or_else(|| {
        RuntimeError::new("boot_unknown", "the server identity is not established yet")
            .retryable()
            .with_endpoint(endpoint)
    })?;
    if live.endpoint != endpoint {
        return Err(mismatch_endpoint());
    }
    if live.session != project.session_name {
        return Err(RuntimeError::new(
            "target_session_mismatch",
            "the available connection belongs to another session; the project was not opened",
        )
        .with_endpoint(endpoint));
    }
    if live.boot_id.is_empty() {
        return Err(RuntimeError::new(
            "boot_unknown",
            "the server identity is not established yet",
        )
        .retryable()
        .with_endpoint(endpoint));
    }
    Ok(live)
}

struct WorkspaceSummary {
    id: String,
    project: Option<String>,
}

fn list_workspaces(gateway: &dyn RuntimeGateway) -> Result<Vec<WorkspaceSummary>, RuntimeError> {
    let result = gateway.api_request("workspace.list", json!({}))?;
    let list = result
        .get("workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error(gateway, "workspace.list without workspaces"))?;
    list.iter()
        .map(|w| {
            let id = w
                .get("workspace_id")
                .and_then(Value::as_str)
                .ok_or_else(|| protocol_error(gateway, "workspace without id"))?;
            Ok(WorkspaceSummary {
                id: id.to_owned(),
                project: w
                    .pointer(&format!("/tokens/{PROJECT_TOKEN}"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn create_workspace(
    gateway: &dyn RuntimeGateway,
    project: &ProjectRef,
) -> Result<String, RuntimeError> {
    let created = gateway.api_request(
        "workspace.create",
        json!({ "cwd": project.root, "label": project.label, "focus": false }),
    )?;
    let workspace_id = created
        .pointer("/workspace/workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error(gateway, "workspace.create without workspace_id"))?
        .to_owned();
    gateway.api_request(
        "workspace.report_metadata",
        json!({
            "workspace_id": workspace_id,
            "source": METADATA_SOURCE,
            "tokens": { PROJECT_TOKEN: project.id },
        }),
    )?;
    Ok(workspace_id)
}

fn protocol_error(gateway: &dyn RuntimeGateway, message: &str) -> RuntimeError {
    RuntimeError::new("protocol_error", format!("unexpected response: {message}"))
        .with_endpoint(gateway.endpoint().to_owned())
}

// ---------------------------------------------------------------------------------------
// Local runtime bridge
// ---------------------------------------------------------------------------------------

/// Attaches `gateway` without an active surface (no frames requested) and waits until the
/// handshake snapshot establishes the boot id. Events are drained on a background thread
/// that ends with the connection. Nothing is started if the session is absent.
pub fn attach_metadata_gateway(
    gateway: &mut LocalGateway,
    timeout: Duration,
) -> Result<LiveIdentity, RuntimeError> {
    gateway.connect(ConnectOptions {
        geometry: SurfaceGeometry {
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
        },
        surface_active: false,
    })?;
    if let Some(events) = gateway.take_event_stream() {
        std::thread::Builder::new()
            .name("herdr-desktop-projects-drain".into())
            .spawn(move || while events.recv().is_ok() {})
            .map_err(|e| RuntimeError::from_io_kind(e.kind(), "events thread"))?;
    }
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(identity) = gateway.identity() {
            return Ok(identity);
        }
        if Instant::now() >= deadline {
            gateway.detach();
            return Err(RuntimeError::new(
                "boot_unknown",
                "the server did not report its identity in time",
            )
            .retryable()
            .with_endpoint(LOCAL_ENDPOINT));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------------------
// Hosted opening (spec 007)
// ---------------------------------------------------------------------------------------

/// Registered hosts as seen by the projects service. Implemented by the window composition
/// over the connections hub (Local and SSH alike); fakes in the contract tests.
///
/// Implementations must not call back into [`ProjectsState`] (its lock is held during the
/// open) and must never substitute another endpoint/session for the project's own.
pub trait ProjectGateways: Send + Sync {
    /// Connection of the project's declared endpoint and session. Unavailable host → error;
    /// never a Local fallback and never a new connection to a different host.
    fn gateway(&self, project: &ProjectRef) -> Result<Box<dyn RuntimeGateway>, RuntimeError>;
    /// Focuses the bound workspace on the project's host (qualified by the binding's boot and
    /// generation). Called once per successful open; never retried by the service.
    fn focus_workspace(
        &self,
        project: &ProjectRef,
        binding: &RuntimeBinding,
    ) -> Result<(), RuntimeError>;
    /// Authorizes the project root for its file provider. Called once per open, after focus.
    fn authorize_root(&self, project: &ProjectRef) -> Result<(), RuntimeError>;
}

// ---------------------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------------------

/// Commands this module exposes to the WebView (registered by the window composition, 007).
pub const COMMANDS: &[&str] = &[
    "projects_list",
    "project_create",
    "collection_create",
    "collection_add_project",
    "collection_remove_project",
    "collection_move_project",
    "collection_move",
    "project_open",
    "project_pick_folder",
    // Spec 025: groups are the desktop folders; a live workspace is assigned by its root cwd.
    "group_create",
    "group_assign",
    // Spec 044: client-only preference of one workspace (colour, pinned, hidden).
    "workspace_pref_set",
    // Spec 045: the collection menu — rename, palette colour, collapse state and deletion.
    "group_rename",
    "group_set_color",
    "group_set_collapsed",
    "group_delete",
    // Spec 046: the folder just opened as a workspace on a host, for "Recentes neste host".
    "recent_folder_add",
];

/// Native folder picker for the "Open project" dialog. The product injects the Tauri dialog;
/// harnesses leave it unset (the command then reports `picker_unavailable`).
///
/// Spec 071 (AC-071-03): the title arrives already translated from the WebView, validated by
/// [`picker_title`] — the only text this host takes from the front.
pub trait FolderPicker: Send + Sync {
    fn pick_folder(&self, title: &str) -> Result<Option<String>, RuntimeError>;
}

/// The title to show: the one the front sent when it says something and fits, English otherwise.
fn picker_title(requested: &str) -> String {
    let trimmed = requested.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_PICKER_TITLE_CHARS {
        return DEFAULT_PICKER_TITLE.to_owned();
    }
    trimmed.to_owned()
}

struct ProjectsInner {
    prefs_dir: PathBuf,
    herdr_config_dir: PathBuf,
    service: Option<ProjectService>,
    /// One gateway per local session, reused so its connection generation keeps increasing.
    /// Harness mode only: a hosted state never creates Local connections of its own.
    gateways: HashMap<String, LocalGateway>,
    /// Hosted mode: every project opens through the registered Local/SSH host.
    hosted: Option<Arc<dyn ProjectGateways>>,
    picker: Option<Arc<dyn FolderPicker>>,
}

/// Managed state. The store is loaded lazily by the first command, never on construction.
/// Clones share the one store, its bindings and the command lane.
#[derive(Clone)]
pub struct ProjectsState {
    inner: Arc<Mutex<ProjectsInner>>,
    lane: Arc<CommandLane>,
}

impl ProjectsState {
    /// Harness mode (spec 002): only `local` projects open, through a Local connection owned
    /// by this state; other endpoints report `endpoint_unavailable`.
    pub fn new(prefs_dir: PathBuf, herdr_config_dir: PathBuf) -> Self {
        Self::build(prefs_dir, herdr_config_dir, None)
    }

    /// Hosted mode (spec 007): projects open through `gateways` (see [`ProjectGateways`]).
    pub fn with_gateways(
        prefs_dir: PathBuf,
        herdr_config_dir: PathBuf,
        gateways: Arc<dyn ProjectGateways>,
    ) -> Self {
        Self::build(prefs_dir, herdr_config_dir, Some(gateways))
    }

    fn build(
        prefs_dir: PathBuf,
        herdr_config_dir: PathBuf,
        hosted: Option<Arc<dyn ProjectGateways>>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProjectsInner {
                prefs_dir,
                herdr_config_dir,
                service: None,
                gateways: HashMap::new(),
                hosted,
                picker: None,
            })),
            lane: Arc::new(CommandLane::default()),
        }
    }

    /// Installs the native folder picker (product window only; never on render).
    pub fn set_folder_picker(&self, picker: Arc<dyn FolderPicker>) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .picker = Some(picker);
    }

    /// Native folder picker of the "Open project" dialog, with the title the WebView sends
    /// (already translated). Cancel is `Ok(None)`. Off the command lane so a dialog held open
    /// does not stall list/create/open.
    pub fn pick_folder(
        &self,
        title: String,
    ) -> impl Future<Output = Result<Option<String>, RuntimeError>> + Send + 'static {
        let picker = self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .picker
            .clone();
        let title = picker_title(&title);
        async move {
            let Some(picker) = picker else {
                return Err(RuntimeError::new(
                    "picker_unavailable",
                    "the native folder picker is unavailable",
                ));
            };
            tauri::async_runtime::spawn_blocking(move || picker.pick_folder(&title))
                .await
                .map_err(|_| command_interrupted())?
        }
    }

    /// Current projects, collections and bindings.
    pub fn snapshot(&self) -> Result<ProjectsSnapshot, RuntimeError> {
        with_service(self, |inner| Ok(service(inner).snapshot()))
    }

    /// Explicit user action (the `project_open` command).
    pub fn open(&self, project_id: &str) -> Result<OpenResponse, RuntimeError> {
        with_service(self, |inner| {
            let project = service(inner)
                .store()
                .project(project_id)
                .cloned()
                .ok_or_else(project_not_found)?;
            if let Some(hosted) = inner.hosted.clone() {
                return open_hosted(inner, hosted.as_ref(), &project);
            }
            open_harness_local(inner, &project)
        })
    }
}

/// Hosted open: the host's connection for the project's endpoint/session, the approved
/// reuse/rediscover/create order, then — only while the binding still matches the live
/// boot/generation — one qualified focus and one root authorization. Nothing is retried and
/// no error removes stored projects or the binding already established.
fn open_hosted(
    inner: &mut ProjectsInner,
    hosts: &dyn ProjectGateways,
    project: &ProjectRef,
) -> Result<OpenResponse, RuntimeError> {
    let gateway = hosts.gateway(project)?;
    let result = service(inner).open_project(&project.id, gateway.as_ref())?;
    // The host may have rebooted or reconnected after the workspace was resolved (a new boot
    // can hand out the same workspace id): never act on a stale binding.
    let live = validate_target(project, gateway.as_ref())?;
    if live.boot_id != result.binding.boot_id {
        return Err(RuntimeError::new(
            "target_boot_stale",
            "the server restarted while opening; open the project again",
        )
        .retryable()
        .with_endpoint(project.endpoint_profile_id.clone()));
    }
    if live.connection_generation != result.binding.connection_generation {
        return Err(RuntimeError::new(
            "target_generation_stale",
            "the connection was renewed while opening; open the project again",
        )
        .retryable()
        .with_endpoint(project.endpoint_profile_id.clone()));
    }
    hosts.focus_workspace(project, &result.binding)?;
    hosts.authorize_root(project)?;
    Ok(OpenResponse {
        result,
        snapshot: service(inner).snapshot(),
    })
}

fn open_harness_local(
    inner: &mut ProjectsInner,
    project: &ProjectRef,
) -> Result<OpenResponse, RuntimeError> {
    let project_id = project.id.as_str();
    if project.endpoint_profile_id != LOCAL_ENDPOINT {
        return Err(RuntimeError::new(
            "endpoint_unavailable",
            "this endpoint has no connection available in the desktop yet",
        )
        .with_endpoint(project.endpoint_profile_id.clone()));
    }
    let session = SessionName::parse(&project.session_name)?;
    let config_dir = inner.herdr_config_dir.clone();
    let gateway = inner
        .gateways
        .entry(project.session_name.clone())
        .or_insert_with(|| LocalGateway::new(&config_dir, session));
    if !gateway.is_connected() || gateway.identity().is_none() {
        attach_metadata_gateway(gateway, Duration::from_secs(10))?;
    }
    let gateway = inner
        .gateways
        .remove(&project.session_name)
        .expect("inserted above");
    let opened = service(inner).open_project(project_id, &gateway);
    if matches!(&opened, Err(error) if error.retryable) {
        // Connection-level failure: drop the connection so the next attempt renews it
        // (new generation → current bindings are revalidated, never trusted).
        let mut gateway = gateway;
        gateway.detach();
        inner.gateways.insert(project.session_name.clone(), gateway);
    } else {
        inner.gateways.insert(project.session_name.clone(), gateway);
    }
    let result = opened?;
    Ok(OpenResponse {
        result,
        snapshot: service(inner).snapshot(),
    })
}

fn with_service<T>(
    state: &ProjectsState,
    f: impl FnOnce(&mut ProjectsInner) -> Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    let mut inner = state.inner.lock().map_err(|_| {
        RuntimeError::new("state_poisoned", "the projects' internal state is invalid")
    })?;
    if inner.service.is_none() {
        let store = ProjectStore::open(&inner.prefs_dir, &inner.herdr_config_dir)?;
        inner.service = Some(ProjectService::new(store));
    }
    f(&mut inner)
}

fn service(inner: &mut ProjectsInner) -> &mut ProjectService {
    inner.service.as_mut().expect("loaded by with_service")
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenResponse {
    pub result: OpenResult,
    pub snapshot: ProjectsSnapshot,
}

/// Blocking bodies of the store-editing commands.
impl ProjectsState {
    fn create_project_blocking(
        &self,
        draft: ProjectDraft,
        collection_id: Option<String>,
    ) -> Result<ProjectsSnapshot, RuntimeError> {
        with_service(self, |inner| {
            let service = service(inner);
            if let Some(collection) = collection_id.as_deref() {
                if !service
                    .store()
                    .document()
                    .groups
                    .iter()
                    .any(|c| c.id == collection)
                {
                    return Err(collection_not_found());
                }
            }
            let project = service.store_mut().create_project(draft)?;
            service.refresh_branch(&project.id);
            if let Some(collection) = collection_id.as_deref() {
                service
                    .store_mut()
                    .add_to_collection(collection, &project.id, None)?;
            }
            Ok(service.snapshot())
        })
    }

    /// Spec 025: upserts the closed project of a live workspace by its root cwd and adds it to
    /// the group; resolves the branch and answers the resulting snapshot.
    fn group_assign_blocking(
        &self,
        group_id: String,
        entry: GroupAssignEntry,
    ) -> Result<ProjectsSnapshot, RuntimeError> {
        with_service(self, |inner| {
            let service = service(inner);
            let project = service.store_mut().assign_root(
                &group_id,
                ProjectDraft {
                    label: entry.label,
                    endpoint_profile_id: entry.endpoint_profile_id,
                    session_name: entry.session_name,
                    root: entry.cwd,
                },
            )?;
            service.refresh_branch(&project.id);
            Ok(service.snapshot())
        })
    }

    /// Applies one change to the store and answers the resulting snapshot.
    fn edit_blocking(
        &self,
        change: impl FnOnce(&mut ProjectStore) -> Result<(), RuntimeError>,
    ) -> Result<ProjectsSnapshot, RuntimeError> {
        with_service(self, |inner| {
            let service = service(inner);
            change(service.store_mut())?;
            Ok(service.snapshot())
        })
    }
}

/// IPC entry points awaited by the Tauri command wrappers below. Each one takes its turn
/// in the command lane when called (see [`CommandLane`]) and runs the blocking body on the
/// runtime's blocking pool; the returned future only waits.
impl ProjectsState {
    pub fn projects_list(
        &self,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(|state| state.snapshot())
    }

    pub fn project_create(
        &self,
        draft: ProjectDraft,
        collection_id: Option<String>,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| state.create_project_blocking(draft, collection_id))
    }

    pub fn collection_create(
        &self,
        name: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.create_collection(&name).map(drop))
        })
    }

    pub fn collection_add_project(
        &self,
        collection_id: String,
        project_id: String,
        index: Option<usize>,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.add_to_collection(&collection_id, &project_id, index))
        })
    }

    pub fn collection_remove_project(
        &self,
        collection_id: String,
        project_id: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.remove_from_collection(&collection_id, &project_id))
        })
    }

    pub fn collection_move_project(
        &self,
        collection_id: String,
        project_id: String,
        to_index: usize,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.move_project(&collection_id, &project_id, to_index))
        })
    }

    pub fn collection_move(
        &self,
        collection_id: String,
        to_index: usize,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.move_collection(&collection_id, to_index))
        })
    }

    /// Spec 025: creates an empty group (`Novo grupo`).
    pub fn group_create(
        &self,
        name: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| state.edit_blocking(|store| store.create_group(&name).map(drop)))
    }

    /// Spec 025: moves a workspace into a group by its root cwd (upsert of the closed project).
    pub fn group_assign(
        &self,
        group_id: String,
        entry: GroupAssignEntry,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| state.group_assign_blocking(group_id, entry))
    }

    /// Spec 045: the collection menu. Each one is a single store edit answering the snapshot.
    pub fn group_rename(
        &self,
        group_id: String,
        name: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.rename_collection(&group_id, &name))
        })
    }

    pub fn group_set_color(
        &self,
        group_id: String,
        color: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.set_collection_color(&group_id, &color))
        })
    }

    pub fn group_set_collapsed(
        &self,
        group_id: String,
        collapsed: bool,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| store.set_collection_collapsed(&group_id, collapsed))
        })
    }

    pub fn group_delete(
        &self,
        group_id: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| state.edit_blocking(|store| store.delete_collection(&group_id)))
    }

    /// Spec 046: records the folder used by a successful creation as the newest of its host.
    pub fn recent_folder_add(
        &self,
        endpoint_profile_id: String,
        path: String,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| {
                store
                    .record_recent_folder(&endpoint_profile_id, &path)
                    .map(drop)
            })
        })
    }

    /// Spec 044: colour / pinned / hidden of one workspace, by endpoint and root.
    pub fn workspace_pref_set(
        &self,
        endpoint_profile_id: String,
        root: String,
        patch: WorkspacePrefPatch,
    ) -> impl Future<Output = Result<ProjectsSnapshot, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| {
            state.edit_blocking(|store| {
                store
                    .set_workspace_pref(&endpoint_profile_id, &root, patch)
                    .map(drop)
            })
        })
    }

    pub fn project_open(
        &self,
        project_id: String,
    ) -> impl Future<Output = Result<OpenResponse, RuntimeError>> + Send + 'static {
        self.in_lane(move |state| state.open(&project_id))
    }

    fn in_lane<T: Send + 'static>(
        &self,
        work: impl FnOnce(&ProjectsState) -> Result<T, RuntimeError> + Send + 'static,
    ) -> impl Future<Output = Result<T, RuntimeError>> + Send + 'static {
        run_in_lane(&self.lane, self.clone(), work)
    }
}

// ---------------------------------------------------------------------------------------
// Command lane (spec 007). Mirrored in `bridge/agent_commands.rs`: both modules are also compiled in
// isolation by their feature tests, so neither can import the other.
// ---------------------------------------------------------------------------------------

/// Entry-ordered execution of one service's IPC commands off the calling thread.
///
/// A call takes the next turn synchronously (a short lock, never the service lock). Its future
/// waits for the turn without occupying a thread, then runs the work on the async runtime's
/// bounded blocking pool (`tauri::async_runtime::spawn_blocking`) and releases the turn when
/// the work ends, panics included. A future dropped before its turn releases it unrun; one
/// dropped after the work started lets the work finish (the result is discarded, nothing is
/// retried). At most one command of the service runs at a time, as before on the GUI thread.
#[derive(Default)]
struct CommandLane {
    turns: Mutex<Turns>,
}

#[derive(Default)]
struct Turns {
    issued: u64,
    next: u64,
    finished: BTreeSet<u64>,
    waiting: BTreeMap<u64, Waker>,
}

impl CommandLane {
    fn turns(&self) -> MutexGuard<'_, Turns> {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Turn {
    lane: Arc<CommandLane>,
    number: u64,
}

impl Turn {
    fn take(lane: &Arc<CommandLane>) -> Self {
        let mut turns = lane.turns();
        let number = turns.issued;
        turns.issued += 1;
        Self {
            lane: lane.clone(),
            number,
        }
    }

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut turns = self.lane.turns();
        if turns.next == self.number {
            return Poll::Ready(());
        }
        turns.waiting.insert(self.number, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut turns = self.lane.turns();
        turns.waiting.remove(&self.number);
        turns.finished.insert(self.number);
        loop {
            let next = turns.next;
            if !turns.finished.remove(&next) {
                break;
            }
            turns.next += 1;
        }
        let next = turns.next;
        let waker = turns.waiting.remove(&next);
        drop(turns);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

fn run_in_lane<S: Send + 'static, T: Send + 'static>(
    lane: &Arc<CommandLane>,
    state: S,
    work: impl FnOnce(&S) -> Result<T, RuntimeError> + Send + 'static,
) -> impl Future<Output = Result<T, RuntimeError>> + Send + 'static {
    let turn = Turn::take(lane);
    async move {
        std::future::poll_fn(|cx| turn.poll_ready(cx)).await;
        let job = tauri::async_runtime::spawn_blocking(move || {
            let _turn = turn;
            work(&state)
        });
        job.await.unwrap_or_else(|_| Err(command_interrupted()))
    }
}

/// The blocking work ended without a result (it panicked or the runtime shut down): whether
/// the action reached the engine is unknown, and it is not repeated.
fn command_interrupted() -> RuntimeError {
    RuntimeError::new(
        "command_interrupted",
        "the action was interrupted without a confirmed result; check the state before repeating it",
    )
}

// ---------------------------------------------------------------------------------------
// Tauri commands: thin async wrappers (argument names, success and error payloads unchanged).
// ---------------------------------------------------------------------------------------

#[tauri::command]
pub async fn projects_list(
    state: tauri::State<'_, ProjectsState>,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.projects_list().await
}

#[tauri::command]
pub async fn project_create(
    state: tauri::State<'_, ProjectsState>,
    draft: ProjectDraft,
    collection_id: Option<String>,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.project_create(draft, collection_id).await
}

#[tauri::command]
pub async fn collection_create(
    state: tauri::State<'_, ProjectsState>,
    name: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.collection_create(name).await
}

#[tauri::command]
pub async fn collection_add_project(
    state: tauri::State<'_, ProjectsState>,
    collection_id: String,
    project_id: String,
    index: Option<usize>,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state
        .collection_add_project(collection_id, project_id, index)
        .await
}

#[tauri::command]
pub async fn collection_remove_project(
    state: tauri::State<'_, ProjectsState>,
    collection_id: String,
    project_id: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state
        .collection_remove_project(collection_id, project_id)
        .await
}

#[tauri::command]
pub async fn collection_move_project(
    state: tauri::State<'_, ProjectsState>,
    collection_id: String,
    project_id: String,
    to_index: usize,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state
        .collection_move_project(collection_id, project_id, to_index)
        .await
}

#[tauri::command]
pub async fn collection_move(
    state: tauri::State<'_, ProjectsState>,
    collection_id: String,
    to_index: usize,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.collection_move(collection_id, to_index).await
}

/// Spec 025: `Novo grupo` — an empty group in the desktop catalog.
#[tauri::command]
pub async fn group_create(
    state: tauri::State<'_, ProjectsState>,
    name: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.group_create(name).await
}

/// Spec 025: moves a workspace into a group by its root cwd (upsert, so a reopen returns to the
/// same group and a repeated move never duplicates the row).
#[tauri::command]
pub async fn group_assign(
    state: tauri::State<'_, ProjectsState>,
    group_id: String,
    entry: GroupAssignEntry,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.group_assign(group_id, entry).await
}

/// Spec 045: renames one collection (`Renomear` of the collection menu).
#[tauri::command]
pub async fn group_rename(
    state: tauri::State<'_, ProjectsState>,
    group_id: String,
    name: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.group_rename(group_id, name).await
}

/// Spec 045: the palette colour of one collection.
#[tauri::command]
pub async fn group_set_color(
    state: tauri::State<'_, ProjectsState>,
    group_id: String,
    color: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.group_set_color(group_id, color).await
}

/// Spec 045: persists whether the collection is collapsed in the sidebar.
#[tauri::command]
pub async fn group_set_collapsed(
    state: tauri::State<'_, ProjectsState>,
    group_id: String,
    collapsed: bool,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.group_set_collapsed(group_id, collapsed).await
}

/// Spec 045: removes one collection. Its projects survive without a collection and no workspace
/// is closed (the engine is not told).
#[tauri::command]
pub async fn group_delete(
    state: tauri::State<'_, ProjectsState>,
    group_id: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.group_delete(group_id).await
}

/// Spec 044: writes the desktop-only preference of one workspace (colour from the palette,
/// pinned, hidden). Nothing is sent to the engine and no shell path is exposed.
#[tauri::command]
pub async fn workspace_pref_set(
    state: tauri::State<'_, ProjectsState>,
    endpoint_profile_id: String,
    root: String,
    color: Option<String>,
    pinned: Option<bool>,
    hidden: Option<bool>,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state
        .workspace_pref_set(
            endpoint_profile_id,
            root,
            WorkspacePrefPatch {
                color,
                pinned,
                hidden,
            },
        )
        .await
}

/// Spec 046: records the folder of a successful creation as the newest recent of its host.
/// Client-only list: nothing is sent to the engine and no workspace is opened here.
#[tauri::command]
pub async fn recent_folder_add(
    state: tauri::State<'_, ProjectsState>,
    endpoint_profile_id: String,
    path: String,
) -> Result<ProjectsSnapshot, RuntimeError> {
    state.recent_folder_add(endpoint_profile_id, path).await
}

/// Explicit user action. Harness mode opens only local projects (other endpoints report
/// `endpoint_unavailable`); hosted mode routes through the registered host of the project.
#[tauri::command]
pub async fn project_open(
    state: tauri::State<'_, ProjectsState>,
    project_id: String,
) -> Result<OpenResponse, RuntimeError> {
    state.project_open(project_id).await
}

/// Native folder picker of the "Open project" dialog (AC-016-03). Directory-only stays fixed in
/// the injected picker so the WebView cannot open an arbitrary dialog; the title is the
/// translated one the front sends, validated here (spec 071, AC-071-03). Cancel is `Ok(None)`.
#[tauri::command]
pub async fn project_pick_folder(
    state: tauri::State<'_, ProjectsState>,
    title: String,
) -> Result<Option<String>, RuntimeError> {
    state.pick_folder(title).await
}
