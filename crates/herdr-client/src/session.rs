//! Session naming and socket resolution. Ports of `session::validate_name`,
//! `session::data_dir_for` and `config::io::config_dir` from the reference engine
//! (03749ae); the desktop must derive the same paths the engine binds.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::contracts::RuntimeError;

pub const DEFAULT_SESSION_NAME: &str = "default";
const MAX_SESSION_NAME_LEN: usize = 64;
const APP_DIR_NAME: &str = "herdr";

/// Validated session name. `default` is valid and addresses the engine's default session
/// (spec 016: the zero-config window uses it exactly like the TUI).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionName(String);

impl SessionName {
    /// Port of `validate_name`.
    pub fn parse(name: &str) -> Result<Self, RuntimeError> {
        validate_name(name)
            .map_err(|message| RuntimeError::new("invalid_session_name", message))?;
        Ok(Self(name.to_owned()))
    }

    /// The engine's default session (the TUI's implicit target).
    pub fn default_session() -> Self {
        Self(DEFAULT_SESSION_NAME.to_owned())
    }

    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_SESSION_NAME
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for SessionName {
    type Error = RuntimeError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        SessionName::parse(&value)
    }
}

impl From<SessionName> for String {
    fn from(value: SessionName) -> Self {
        value.0
    }
}

/// Exact port of the engine rule. Returns the engine's message on failure.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("session name cannot be empty".to_string());
    }
    if name.len() > MAX_SESSION_NAME_LEN {
        return Err(format!(
            "session name cannot be longer than {MAX_SESSION_NAME_LEN} bytes"
        ));
    }
    if name == "." || name == ".." {
        return Err("session name cannot be . or ..".to_string());
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(
            "session name may only contain ASCII letters, numbers, '.', '_' and '-'".to_string(),
        );
    }
    Ok(())
}

/// Port of `config::io::config_dir()` driven by an explicit environment lookup so it is
/// testable without mutating the process environment.
pub fn herdr_config_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(dir) = env("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join(APP_DIR_NAME);
    }
    #[cfg(windows)]
    {
        if let Some(dir) = env("APPDATA") {
            return PathBuf::from(dir).join(APP_DIR_NAME);
        }
        if let Some(profile) = env("USERPROFILE") {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join(APP_DIR_NAME);
        }
    }
    if let Some(home) = env("HOME") {
        return PathBuf::from(home).join(".config").join(APP_DIR_NAME);
    }
    std::env::temp_dir().join(APP_DIR_NAME)
}

/// Socket locations of one named session under the engine config dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPaths {
    pub data_dir: PathBuf,
    pub api_socket: PathBuf,
    pub client_socket: PathBuf,
}

impl SessionPaths {
    /// Port of `data_dir_for`: `Some(name)` = `<config_dir>/sessions/<name>`, the default
    /// session (`None` in the engine) = the config dir itself.
    pub fn for_session(config_dir: &Path, session: &SessionName) -> Self {
        let data_dir = if session.is_default() {
            config_dir.to_path_buf()
        } else {
            config_dir.join("sessions").join(session.as_str())
        };
        Self {
            api_socket: data_dir.join("herdr.sock"),
            client_socket: data_dir.join("herdr-client.sock"),
            data_dir,
        }
    }
}
