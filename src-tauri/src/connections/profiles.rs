//! SSH profiles owned by the desktop (`connections.json` in its preferences directory).
//!
//! Profiles of the Herdr TUI catalog (`<state>/client/endpoints.json`) are imported by
//! reading that file only: nothing is written back, no selection file is touched and no
//! `herdr machine add` runs (it prepares the remote server as a side effect). Edits land in
//! the desktop store.

use std::io::Write;
use std::path::{Path, PathBuf};

use herdr_client::{RuntimeError, SessionName};
use serde::{Deserialize, Serialize};

use super::ssh_options::{ProfileId, SshIdentity, SshTarget};

pub const STORE_FILE: &str = "connections.json";
pub const STORE_VERSION: u32 = 1;
/// The only special authentication choice of the dialog: use the session's ssh-agent.
pub const SSH_AGENT_AUTH: &str = "ssh-agent";
/// Any other explicit choice means "the OpenSSH default key files"; it is stored as `None`.
pub const KEY_AUTH: &str = "key";
const MAX_LABEL_BYTES: usize = 128;
const MAX_CATALOG_BYTES: u64 = 64 * 1024;
const MAX_PROFILES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshProfile {
    pub id: ProfileId,
    pub label: String,
    pub target: SshTarget,
    #[serde(default)]
    pub port: Option<u16>,
    pub session: SessionName,
    /// Authentication chosen in the dialog: `Some("ssh-agent")` or `None` (OpenSSH default key
    /// files). Stored so a retry/reconnect repeats the same explicit choice (AC-031-02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// Id of the TUI catalog profile this one was imported from (dedupe only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// "Conectar ao abrir" of the host menu (spec 058, AC-058-01): on by default, so a store
    /// written by the previous format keeps behaving like the TUI, where the SSH link is up as
    /// soon as the app is.
    #[serde(default = "connect_on_open_default")]
    pub connect_on_open: bool,
    /// Whether this host was connected when the app was last used. Set by a connect, cleared by
    /// an explicit `Desconectar`; a profile that is removed takes it along. It is the only thing
    /// the resume reads besides the preference above; no secret is ever stored.
    #[serde(default)]
    pub resume_on_open: bool,
}

fn connect_on_open_default() -> bool {
    true
}

impl SshProfile {
    pub fn identity(&self) -> SshIdentity {
        SshIdentity {
            profile_id: self.id.clone(),
            target: self.target.clone(),
            port: self.port,
            session: self.session.clone(),
        }
    }
}

/// Form payload from the WebView.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SshProfileDraft {
    #[serde(default)]
    pub id: Option<String>,
    pub label: String,
    pub target: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub session: String,
    /// `None`/`"key"` = the OpenSSH default key files; `"ssh-agent"` = the session agent.
    /// Unknown values are refused before anything is written or attempted.
    #[serde(default)]
    pub auth: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedProfile {
    pub label: String,
    pub code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub imported: Vec<String>,
    pub skipped: Vec<SkippedProfile>,
    pub already_present: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    #[serde(default)]
    profiles: Vec<SshProfile>,
}

#[derive(Debug, Deserialize)]
struct TuiCatalog {
    #[serde(default)]
    ssh: Vec<TuiProfile>,
}

#[derive(Debug, Deserialize)]
struct TuiProfile {
    id: String,
    label: String,
    target: String,
    session: String,
    #[serde(default = "enabled_default")]
    enabled: bool,
}

fn enabled_default() -> bool {
    true
}

/// `<state dir>/client/endpoints.json` (engine `catalog_path`).
pub fn tui_catalog_path(herdr_state_dir: &Path) -> PathBuf {
    herdr_state_dir.join("client").join("endpoints.json")
}

/// Port of the engine's `config::io::state_dir` (release app dir name).
pub fn herdr_state_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(dir) = env("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("herdr");
    }
    #[cfg(windows)]
    {
        if let Some(dir) = env("LOCALAPPDATA") {
            return PathBuf::from(dir).join("herdr");
        }
        if let Some(profile) = env("USERPROFILE") {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("herdr");
        }
    }
    if let Some(home) = env("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("herdr");
    }
    std::env::temp_dir().join("herdr-state")
}

#[derive(Debug)]
pub struct ProfileStore {
    dir: PathBuf,
    profiles: Vec<SshProfile>,
}

fn store_error(code: &str, message: &str) -> RuntimeError {
    RuntimeError::new(code, message)
}

impl ProfileStore {
    /// Opens (without creating) the store under `dir`. `engine_dirs` are the Herdr config and
    /// state directories: the desktop never keeps its preferences inside them.
    pub fn open(dir: &Path, engine_dirs: &[&Path]) -> Result<Self, RuntimeError> {
        if engine_dirs.iter().any(|engine| dir.starts_with(engine)) {
            return Err(store_error(
                "prefs_inside_engine_dir",
                "the desktop preferences cannot live inside Herdr's directories",
            ));
        }
        let path = dir.join(STORE_FILE);
        let profiles = match std::fs::read(&path) {
            Ok(bytes) => {
                let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                    store_error(
                        "connections_store_corrupt",
                        "the connections file is corrupt",
                    )
                })?;
                if raw.get("version").and_then(serde_json::Value::as_u64)
                    != Some(u64::from(STORE_VERSION))
                {
                    return Err(store_error(
                        "connections_store_unsupported",
                        "unsupported connections file version",
                    ));
                }
                let file: StoreFile = serde_json::from_value(raw).map_err(|_| {
                    store_error(
                        "connections_store_corrupt",
                        "the connections file is corrupt",
                    )
                })?;
                file.profiles
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(RuntimeError::from_io_kind(
                    error.kind(),
                    "could not read the connections file",
                ))
            }
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            profiles,
        })
    }

    pub fn profiles(&self) -> &[SshProfile] {
        &self.profiles
    }

    pub fn get(&self, id: &str) -> Option<&SshProfile> {
        self.profiles.iter().find(|p| p.id.as_str() == id)
    }

    fn index_of(&self, id: &str) -> Result<usize, RuntimeError> {
        self.profiles
            .iter()
            .position(|profile| profile.id.as_str() == id)
            .ok_or_else(|| {
                store_error("profile_not_found", "the connection profile does not exist")
            })
    }

    /// Records whether this host should be dialed again at the next start (spec 058, AC-058-01).
    /// Nothing is written when the value already is the one asked for.
    pub fn set_resume_on_open(&mut self, id: &str, resume: bool) -> Result<(), RuntimeError> {
        let index = self.index_of(id)?;
        if self.profiles[index].resume_on_open == resume {
            return Ok(());
        }
        let mut next = self.profiles.clone();
        next[index].resume_on_open = resume;
        self.persist(next)
    }

    /// "Conectar ao abrir" of the host menu (spec 058, AC-058-03). Only this preference changes;
    /// the memory of having been connected is kept, so turning it back on resumes the host again.
    pub fn set_connect_on_open(
        &mut self,
        id: &str,
        connect_on_open: bool,
    ) -> Result<SshProfile, RuntimeError> {
        let index = self.index_of(id)?;
        if self.profiles[index].connect_on_open != connect_on_open {
            let mut next = self.profiles.clone();
            next[index].connect_on_open = connect_on_open;
            self.persist(next)?;
        }
        Ok(self.profiles[index].clone())
    }

    fn validate_label(label: &str) -> Result<String, RuntimeError> {
        let label = label.trim();
        if label.is_empty() || label.len() > MAX_LABEL_BYTES || label.chars().any(char::is_control)
        {
            return Err(store_error(
                "profile_label_invalid",
                "the display name must be 1 to 128 bytes long, with no control characters",
            ));
        }
        Ok(label.to_owned())
    }

    /// Creates or updates a profile. Nothing is written when validation fails.
    pub fn save(&mut self, draft: SshProfileDraft) -> Result<SshProfile, RuntimeError> {
        let label = Self::validate_label(&draft.label)?;
        let index = match draft.id.as_deref() {
            Some(id) => {
                let id = ProfileId::parse(id)?;
                Some(
                    self.profiles
                        .iter()
                        .position(|p| p.id == id)
                        .ok_or_else(|| {
                            store_error(
                                "profile_not_found",
                                "the connection profile does not exist",
                            )
                        })?,
                )
            }
            None => None,
        };
        let id = match index {
            Some(i) => self.profiles[i].id.clone(),
            None => ProfileId::generate(),
        };
        let identity = SshIdentity::new(id, draft.target.trim(), draft.port, draft.session.trim())?;
        let auth = match draft.auth.as_deref() {
            None | Some("") | Some(KEY_AUTH) => None,
            Some(SSH_AGENT_AUTH) => Some(SSH_AGENT_AUTH.to_owned()),
            Some(_) => {
                return Err(store_error(
                    "profile_auth_invalid",
                    "unknown authentication method",
                ))
            }
        };
        let mut next = self.profiles.clone();
        let profile = SshProfile {
            id: identity.profile_id,
            label,
            target: identity.target,
            port: identity.port,
            session: identity.session,
            auth,
            imported_from: index.and_then(|i| next[i].imported_from.clone()),
            // Editing a profile in the dialog never changes what the resume decided (spec 058).
            connect_on_open: index.is_none_or(|i| next[i].connect_on_open),
            resume_on_open: index.is_some_and(|i| next[i].resume_on_open),
        };
        match index {
            Some(i) => next[i] = profile.clone(),
            None => {
                if next.len() >= MAX_PROFILES {
                    return Err(store_error(
                        "profile_limit",
                        "the connection profile limit was reached",
                    ));
                }
                next.push(profile.clone());
            }
        }
        self.persist(next)?;
        Ok(profile)
    }

    /// Removes a saved profile (spec 029, AC-029-03). The engine and the remote host are not
    /// touched; nothing is written when the id is unknown or the store cannot be persisted.
    pub fn remove(&mut self, id: &str) -> Result<SshProfile, RuntimeError> {
        let id = ProfileId::parse(id)?;
        let index = self
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or_else(|| {
                store_error("profile_not_found", "the connection profile does not exist")
            })?;
        let mut next = self.profiles.clone();
        let removed = next.remove(index);
        self.persist(next)?;
        Ok(removed)
    }

    /// Reads the TUI catalog and adds its valid profiles that are not present yet.
    pub fn import_tui_catalog(&mut self, catalog: &Path) -> Result<ImportReport, RuntimeError> {
        let metadata = match std::fs::metadata(catalog) {
            Ok(m) => m,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ImportReport::default())
            }
            Err(error) => {
                return Err(RuntimeError::from_io_kind(
                    error.kind(),
                    "could not read Herdr's profiles",
                ))
            }
        };
        if metadata.len() > MAX_CATALOG_BYTES {
            return Err(store_error(
                "tui_catalog_too_large",
                "Herdr's profile catalog exceeds the limit",
            ));
        }
        let bytes = std::fs::read(catalog).map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "could not read Herdr's profiles")
        })?;
        let parsed: TuiCatalog = serde_json::from_slice(&bytes).map_err(|_| {
            store_error("tui_catalog_invalid", "Herdr's profile catalog is invalid")
        })?;
        let mut report = ImportReport::default();
        let mut next = self.profiles.clone();
        for tui in parsed.ssh {
            if !tui.enabled {
                report.skipped.push(SkippedProfile {
                    label: tui.label,
                    code: "tui_profile_disabled".into(),
                });
                continue;
            }
            if next.iter().any(|p| {
                p.imported_from.as_deref() == Some(tui.id.as_str())
                    || (p.target.as_str() == tui.target && p.session.as_str() == tui.session)
            }) {
                report.already_present += 1;
                continue;
            }
            let label = match Self::validate_label(&tui.label) {
                Ok(label) => label,
                Err(error) => {
                    report.skipped.push(SkippedProfile {
                        label: tui.label,
                        code: error.code,
                    });
                    continue;
                }
            };
            match SshIdentity::new(ProfileId::generate(), &tui.target, None, &tui.session) {
                Ok(identity) => {
                    if next.len() >= MAX_PROFILES {
                        report.skipped.push(SkippedProfile {
                            label,
                            code: "profile_limit".into(),
                        });
                        continue;
                    }
                    report.imported.push(label.clone());
                    next.push(SshProfile {
                        id: identity.profile_id,
                        label,
                        target: identity.target,
                        port: None,
                        session: identity.session,
                        auth: None,
                        imported_from: Some(tui.id),
                        connect_on_open: true,
                        // An imported host was never connected by the desktop: nothing to resume
                        // until the user connects it once.
                        resume_on_open: false,
                    });
                }
                Err(error) => report.skipped.push(SkippedProfile {
                    label,
                    code: error.code,
                }),
            }
        }
        if !report.imported.is_empty() {
            self.persist(next)?;
        }
        Ok(report)
    }

    fn persist(&mut self, profiles: Vec<SshProfile>) -> Result<(), RuntimeError> {
        let file = StoreFile {
            version: STORE_VERSION,
            profiles,
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|_| {
            store_error(
                "serialization_error",
                "could not serialize the connections file",
            )
        })?;
        atomic_write(&self.dir, STORE_FILE, &bytes)?;
        self.profiles = file.profiles;
        Ok(())
    }
}

/// temp file + fsync + rename in the same directory.
fn atomic_write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), RuntimeError> {
    let io = |error: std::io::Error| {
        RuntimeError::from_io_kind(error.kind(), "could not write the connections file")
    };
    std::fs::create_dir_all(dir).map_err(io)?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, dir.join(name))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}
