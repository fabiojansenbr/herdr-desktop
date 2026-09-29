//! Home screen of the design (spec 012): the system user of the greeting and one static
//! `pane.read` snapshot of a pane for the project-card thumbnails.
//!
//! Both commands are read-only and bounded: `system_user` returns the name of the environment
//! and nothing else (no home, no credentials, and no invented name when there is none);
//! `pane_read` addresses one pane through the qualified target the window already holds
//! (endpoint, session, generation, boot and pane are validated before anything is sent) and
//! never polls, never retries and never falls back to another host.

use herdr_client::{QualifiedTarget, RuntimeError};
use serde::Serialize;
use tauri::State;

use super::agent_commands::{AgentsState, PaneSnapshotDto};

/// Commands this module exposes to the WebView (registered by the window composition).
pub const COMMANDS: &[&str] = &["system_user", "pane_read"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemUserDto {
    pub user: String,
}

/// System user for the greeting, in the order the platforms publish it. An environment that
/// publishes none returns the empty string (spec 072, AC-072-02): the host never invents a name,
/// and the WebView says who the user is in their own language (`home.user`).
pub fn user_from(get: &dyn Fn(&str) -> Option<String>) -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .filter_map(|key| get(key))
        .map(|value| value.trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

#[tauri::command]
pub fn system_user() -> SystemUserDto {
    SystemUserDto {
        user: user_from(&|key| std::env::var(key).ok()),
    }
}

/// One static `pane.read` snapshot for a home thumbnail (spec 012). The pane must be one the
/// engine listed as an agent of the qualified host; a refused call sends nothing.
#[tauri::command]
pub async fn pane_read(
    state: State<'_, AgentsState>,
    target: QualifiedTarget,
    pane_id: String,
    lines: u32,
) -> Result<PaneSnapshotDto, RuntimeError> {
    state.pane_read(target, pane_id, lines).await
}

#[cfg(test)]
mod tests {
    use super::user_from;

    fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        move |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    /// Spec 072 (AC-072-02): an environment that publishes no user leaves the name to the front,
    /// which says it in the user's language (`home.user`).
    ///
    /// Would catch: the host inventing a name again for a missing user.
    #[test]
    fn ac_072_02_an_unnamed_environment_returns_the_empty_string() {
        assert_eq!(user_from(&env(&[])), "");
        assert_eq!(
            user_from(&env(&[("USER", "   "), ("LOGNAME", ""), ("USERNAME", "")])),
            ""
        );
        assert_eq!(user_from(&env(&[("HOME", "/home/ana")])), "");
    }

    /// Would catch: the order of the variables changing, or a trimmed name being lost.
    #[test]
    fn ac_072_02_the_first_non_empty_variable_names_the_user() {
        assert_eq!(user_from(&env(&[("USER", " ana ")])), "ana");
        assert_eq!(user_from(&env(&[("USER", ""), ("LOGNAME", "bea")])), "bea");
        assert_eq!(
            user_from(&env(&[("LOGNAME", " "), ("USERNAME", "cid")])),
            "cid"
        );
        assert_eq!(
            user_from(&env(&[("USER", "ana"), ("LOGNAME", "bea")])),
            "ana"
        );
    }
}
