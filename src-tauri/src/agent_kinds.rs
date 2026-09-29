//! Local availability of agent kinds (spec 073).
//!
//! The engine publishes which agent kinds it knows (`server.agent_manifests`) but not which of
//! them are installed on the machine, so the "New agent" list would offer every kind equally.
//! This is the host's own answer for the Local host: for each kind, is one of its binaries on the
//! `PATH`? The binary table mirrors the engine's registry
//! (`../herdr/src/integration/registry.rs:37-62`), and a kind with no entry there uses its own
//! name as the binary, as the engine does.
//!
//! It is read-only, bounded and never a shell: no process is spawned, no output is read, only
//! directory entries of the `PATH` are stat'ed. A kind name is validated before it reaches the
//! filesystem, so nothing the WebView sends can become a path. The answer is advisory — a false
//! `available` only dims a row, it never blocks starting an agent — and remote hosts are not
//! probed at all (the front treats them as available).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use herdr_client::RuntimeError;
use serde::{Deserialize, Serialize};

/// Commands this module exposes to the WebView (kept in sync with `lib.rs` by the registry test).
pub const COMMANDS: &[&str] = &["agent_kinds_available"];

/// Longest list one query may carry: the engine publishes a couple of dozen kinds, and this is an
/// IPC bound, not an engine one.
pub const MAX_KINDS: usize = 64;

/// Longest accepted kind name.
pub const MAX_KIND_CHARS: usize = 32;

/// Kinds whose binaries the engine names explicitly. Anything absent here is looked up under its
/// own name (`gemini`, `cline`, `kiro`), which is also what the engine does for a target it has no
/// entry for.
const BINARIES: &[(&str, &[&str])] = &[
    ("pi", &["pi"]),
    ("omp", &["omp"]),
    ("claude", &["claude"]),
    ("codex", &["codex"]),
    ("copilot", &["copilot"]),
    ("devin", &["devin"]),
    ("droid", &["droid"]),
    ("kimi", &["kimi"]),
    ("opencode", &["opencode"]),
    ("kilo", &["kilo", "kilo-code"]),
    ("hermes", &["hermes"]),
    ("qodercli", &["qodercli"]),
    ("qwen", &["qwen"]),
    ("cursor", &["cursor-agent"]),
    ("mastracode", &["mastracode"]),
    ("agy", &["agy"]),
    ("grok", &["grok"]),
];

/// One kind's answer. `available` is `false` for "no binary found", never "unknown": the front
/// asks only the Local host and drops the whole section when the call fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentKindAvailability {
    pub kind: String,
    pub available: bool,
}

/// Binaries that would launch `kind`, in the order the engine tries them.
pub fn binaries_of(kind: &str) -> Vec<&str> {
    match BINARIES.iter().find(|(name, _)| *name == kind) {
        Some((_, commands)) => commands.to_vec(),
        None => vec![kind],
    }
}

/// `[a-z0-9_-]{1,32}`: the shape of every kind the engine publishes. Anything else (a path, a
/// separator, an upper-case letter, an empty name) is refused before any filesystem lookup.
fn validate_kind(kind: &str) -> Result<(), RuntimeError> {
    if kind.is_empty() || kind.chars().count() > MAX_KIND_CHARS {
        return Err(RuntimeError::new(
            "invalid_input",
            format!("agent kind must be 1..={MAX_KIND_CHARS} characters"),
        ));
    }
    if !kind
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(RuntimeError::new(
            "invalid_input",
            "agent kind accepts only [a-z0-9_-]".to_owned(),
        ));
    }
    Ok(())
}

/// Paths an executable named `command` could have inside `dir` (the engine's own candidates).
fn command_path_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    let base = dir.join(command);

    #[cfg(not(windows))]
    {
        vec![base]
    }

    #[cfg(windows)]
    {
        let mut candidates = vec![base];
        for extension in [".exe", ".cmd", ".bat", ".ps1"] {
            candidates.push(dir.join(format!("{command}{extension}")));
        }
        candidates
    }
}

fn executable_file_exists(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

/// Is `command` an executable in one of the directories of `path`?
pub fn command_available_in(path: &OsStr, command: &str) -> bool {
    std::env::split_paths(path).any(|dir| {
        command_path_candidates(&dir, command)
            .into_iter()
            .any(|candidate| executable_file_exists(&candidate))
    })
}

/// Availability of each kind against an injected `PATH`, in the order asked.
pub fn availability(
    kinds: &[String],
    path: &OsStr,
) -> Result<Vec<AgentKindAvailability>, RuntimeError> {
    if kinds.len() > MAX_KINDS {
        return Err(RuntimeError::new(
            "invalid_input",
            format!("at most {MAX_KINDS} agent kinds per query"),
        ));
    }
    for kind in kinds {
        validate_kind(kind)?;
    }
    Ok(kinds
        .iter()
        .map(|kind| AgentKindAvailability {
            kind: kind.clone(),
            available: binaries_of(kind)
                .into_iter()
                .any(|command| command_available_in(path, command)),
        })
        .collect())
}

/// Which of the WebView's agent kinds have a binary on this machine's `PATH`. Only the Local host
/// is described: the front never asks for a remote one.
#[tauri::command]
pub fn agent_kinds_available(
    kinds: Vec<String>,
) -> Result<Vec<AgentKindAvailability>, RuntimeError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    availability(&kinds, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// A `PATH` with one directory holding `claude` and `cursor-agent` as executables.
    fn path_with_claude_and_cursor() -> (tempfile::TempDir, std::ffi::OsString) {
        let dir = tempfile::tempdir().unwrap();
        for command in ["claude", "cursor-agent"] {
            let file = dir.path().join(command);
            fs::write(&file, b"#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path().to_path_buf()]).unwrap();
        (dir, path)
    }

    fn kinds(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| (*k).to_owned()).collect()
    }

    /// AC-073-01 — would catch: the binary table losing `cursor` → `cursor-agent`, a kind with no
    /// entry not falling back to its own name, or the answer losing the caller's order.
    #[test]
    fn ac_073_01_availability_follows_the_injected_path_in_the_asked_order() {
        let (_dir, path) = path_with_claude_and_cursor();
        let answer = availability(&kinds(&["claude", "cursor", "codex", "gemini"]), &path).unwrap();
        assert_eq!(
            answer
                .iter()
                .map(|a| (a.kind.as_str(), a.available))
                .collect::<Vec<_>>(),
            [
                ("claude", true),
                ("cursor", true),
                ("codex", false),
                ("gemini", false)
            ]
        );
    }

    /// AC-073-01 — would catch: an empty query erroring instead of answering nothing.
    #[test]
    fn ac_073_01_an_empty_query_answers_nothing() {
        let (_dir, path) = path_with_claude_and_cursor();
        assert_eq!(availability(&[], &path).unwrap(), Vec::new());
    }

    /// AC-073-01 — would catch: an unbounded query, or a kind name carrying a path, a separator or
    /// an upper-case letter reaching the filesystem lookup.
    #[test]
    fn ac_073_01_the_query_is_bounded_and_the_names_validated() {
        let (_dir, path) = path_with_claude_and_cursor();
        let too_many = vec!["claude".to_owned(); MAX_KINDS + 1];
        let refused = availability(&too_many, &path).unwrap_err();
        assert_eq!(refused.code, "invalid_input");
        for bad in ["", "Claude", "../claude", "a/b", "cla ude", &"a".repeat(33)] {
            let refused = availability(&kinds(&[bad]), &path).unwrap_err();
            assert_eq!(refused.code, "invalid_input", "{bad}");
        }
        let ok = availability(&vec!["claude".to_owned(); MAX_KINDS], &path).unwrap();
        assert_eq!(ok.len(), MAX_KINDS);
    }

    /// Would catch: the command dropped from the module registry (the window would not expose it).
    #[test]
    fn the_command_is_declared_by_the_module() {
        assert_eq!(COMMANDS, ["agent_kinds_available"]);
    }

    /// Would catch: a kind of the engine's table mapped to the wrong binary, or `kilo` losing its
    /// second name (`../herdr/src/integration/registry.rs:37-62`).
    #[test]
    fn the_binary_table_mirrors_the_engine_registry() {
        assert_eq!(binaries_of("cursor"), ["cursor-agent"]);
        assert_eq!(binaries_of("kilo"), ["kilo", "kilo-code"]);
        assert_eq!(binaries_of("agy"), ["agy"]);
        assert_eq!(binaries_of("gemini"), ["gemini"]);
    }
}
