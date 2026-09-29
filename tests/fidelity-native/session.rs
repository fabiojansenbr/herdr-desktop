//! Disposable sessions of the flow and the Local/SSH pair precondition of AC-007-04.
//!
//! The Local session comes from the gate (`scripts/feature-harness/session.sh start hd007`); a
//! second disposable session backs the SSH host. Both must expose the same pane id `w1:p1`
//! while roots and boots differ, so an action reaching the wrong host is observable.

use herdr_client::session::SessionName;

/// Prefix of every session this flow may attach to, create or stop.
pub const PREFIX: &str = "hd007-";
/// Pane id both hosts must share.
pub const SHARED_PANE: &str = "w1:p1";

/// A disposable session name of this spec (never the default session).
pub fn disposable(raw: &str) -> Result<SessionName, String> {
    let rest = raw
        .strip_prefix(PREFIX)
        .ok_or_else(|| format!("session {raw:?} is not a disposable {PREFIX}* session"))?;
    if rest.is_empty() {
        return Err(format!("session {raw:?} has no suffix"));
    }
    SessionName::parse(raw).map_err(|e| format!("session {raw:?} rejected: {}", e.code))
}

/// Only sessions this run created may be stopped.
pub fn may_stop(session: &str, created: &[String]) -> bool {
    disposable(session).is_ok() && created.iter().any(|c| c == session)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFixture {
    pub session: String,
    pub pane_id: String,
    pub root: std::path::PathBuf,
    pub boot_id: String,
}

/// Problems that would make the Local/SSH pair unable to discriminate hosts (empty = usable).
pub fn pair_problems(local: &HostFixture, ssh: &HostFixture) -> Vec<String> {
    let mut problems = Vec::new();
    for (label, host) in [("local", local), ("ssh", ssh)] {
        if let Err(e) = disposable(&host.session) {
            problems.push(format!("{label}: {e}"));
        }
        if host.pane_id != SHARED_PANE {
            problems.push(format!(
                "{label}: pane {} is not {SHARED_PANE}",
                host.pane_id
            ));
        }
        if !host.root.is_absolute() {
            problems.push(format!(
                "{label}: root {} is not absolute",
                host.root.display()
            ));
        }
        if host.boot_id.is_empty() {
            problems.push(format!("{label}: empty boot id"));
        }
    }
    if local.session == ssh.session {
        problems.push("hosts share the same session".into());
    }
    if local.root.starts_with(&ssh.root) || ssh.root.starts_with(&local.root) {
        problems.push("project roots are equal or nested".into());
    }
    if local.boot_id == ssh.boot_id {
        problems.push("hosts share the same boot id".into());
    }
    problems
}
