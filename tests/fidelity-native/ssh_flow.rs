//! SSH phases of the single native flow of spec 007 (AC-007-04): parent-side observation steps
//! and pure evaluators. Prepared module; it is NOT executed by `e2e_fidelity_flow` until the root
//! links it (see `.local/orchestration/native-ssh-contract.md`).
//!
//! Split of authority:
//! - The page (`src/features/fidelity/ssh-flow.ts`) acts only through the composed App UI and
//!   reports raw DOM observations. It never talks to the engines.
//! - The parent answers `harness_await` steps ([`STEPS`]) with [`parent_step`], snapshotting the
//!   fixture engines through [`HostObserver`] (API `pane.list` on each host's own socket, the
//!   deterministic agent log, the forced-command log, server PID+starttime and root file
//!   digests). The parent keeps its own copy of every answer ([`ParentLedger`]); evaluators read
//!   engine facts from that ledger, never from values relayed by the page.
//! - [`checks`] turns (page report, parent ledger, [`Expectations`]) into the named checks of
//!   `plan::FLOW`. A missing step or field is an error, never a `true`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The five SSH phases with their checks, in flow order (must equal `plan::FLOW`).
pub const PHASES: [(&str, &[&str]); 5] = [
    (
        "hosts-identity",
        &[
            "local_and_ssh_share_pane_w1_p1",
            "projects_have_distinct_roots",
            "ssh_selected_with_friendly_label",
        ],
    ),
    (
        "ssh-agent-actions",
        &[
            "agent_started_once_on_ssh",
            "prompt_delivered_once_on_ssh",
            "split_and_focus_confirmed_geometry_shown",
            "zero_actions_on_local",
        ],
    ),
    (
        "ssh-files-readonly",
        &[
            "explorer_on_selected_host",
            "diff_on_selected_host",
            "remote_marked_read_only",
        ],
    ),
    (
        "legacy-server",
        &[
            "only_dependent_actions_unavailable",
            "terminal_preserved",
            "local_preserved",
        ],
    ),
    (
        "host-switch-dirty",
        &["local_dirty_buffer_preserved_after_switch"],
    ),
];

/// `harness_await` steps the page issues (valid for `window::valid_step`), in order.
pub const STEPS: [&str; 9] = [
    "ssh-identity",
    "ssh-agent-before",
    "ssh-agent-after",
    "ssh-files-before",
    "ssh-files-after",
    "ssh-legacy-before",
    "ssh-legacy-after",
    "ssh-dirty-before",
    "ssh-dirty-after",
];

pub const SHARED_PANE: &str = "w1:p1";
/// Exact refusal of an engine without the optional JSON API over SSH (product `remote_api_unsupported`).
pub const UNSUPPORTED_CODE: &str = "remote_api_unsupported";
pub const UNSUPPORTED_MARK: &str = "(remote-api-bridge)";
/// Small files whose exact content the parent reads from each root.
pub const WATCHED_FILES: [&str; 2] = ["notas.txt", "diff-target.txt"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Local,
    Ssh,
    Legacy,
}

impl Host {
    pub fn label(self) -> &'static str {
        match self {
            Host::Local => "local",
            Host::Ssh => "ssh",
            Host::Legacy => "legacy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRow {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
}

/// One engine observed from outside the GUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSnapshot {
    pub session: String,
    /// Boot announced by the endpoint handshake when the fixture started it.
    pub boot_id: String,
    pub server_pid: u32,
    pub server_starttime: u64,
    pub server_alive: bool,
    pub root: String,
    pub panes: Vec<PaneRow>,
    pub agent_log: String,
    pub forced_command_log: String,
    /// Relative path → sha256 of every regular file under `root`.
    pub digests: BTreeMap<String, String>,
    /// [`WATCHED_FILES`] → exact content.
    pub contents: BTreeMap<String, String>,
}

impl HostSnapshot {
    /// Panes of an API `pane.list` result (`{"panes":[…]}`).
    pub fn panes_from_list(result: &Value) -> Result<Vec<PaneRow>, String> {
        let panes = result["panes"]
            .as_array()
            .ok_or_else(|| format!("pane.list without panes: {result}"))?;
        panes
            .iter()
            .map(|p| {
                let text = |k: &str| {
                    p[k].as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("pane without {k}: {p}"))
                };
                Ok(PaneRow {
                    pane_id: text("pane_id")?,
                    workspace_id: text("workspace_id")?,
                    tab_id: text("tab_id")?,
                    focused: p["focused"]
                        .as_bool()
                        .ok_or_else(|| format!("pane without focused: {p}"))?,
                })
            })
            .collect()
    }

    fn pane(&self, id: &str) -> Option<&PaneRow> {
        self.panes.iter().find(|p| p.pane_id == id)
    }

    fn same_server(&self, other: &HostSnapshot) -> bool {
        self.server_alive
            && other.server_alive
            && self.session == other.session
            && self.boot_id == other.boot_id
            && self.server_pid == other.server_pid
            && self.server_starttime == other.server_starttime
    }
}

/// Source of engine facts; implemented for `remote::RemoteFixture` by [`fixture::FixtureObserver`]
/// and by synthetic fixtures in the contract tests.
pub trait HostObserver {
    fn snapshot(&self, host: Host) -> Result<HostSnapshot, String>;
}

/// Hosts the parent snapshots at each step.
pub fn step_hosts(step: &str) -> Result<&'static [Host], String> {
    match step {
        "ssh-legacy-before" | "ssh-legacy-after" => Ok(&[Host::Local, Host::Ssh, Host::Legacy]),
        s if STEPS.contains(&s) => Ok(&[Host::Local, Host::Ssh]),
        other => Err(format!("{other}: not an ssh-flow step")),
    }
}

/// Parent answer to a page step: fresh snapshots of the step's hosts plus the page's detail.
pub fn parent_step(
    step: &str,
    detail: &Value,
    observer: &dyn HostObserver,
) -> Result<Value, String> {
    let hosts = step_hosts(step)?;
    let mut snaps = serde_json::Map::new();
    for host in hosts {
        let snap = observer.snapshot(*host)?;
        snaps.insert(host.label().into(), json!(snap));
    }
    Ok(json!({ "step": step, "detail": detail, "hosts": snaps }))
}

/// Answers the parent gave, kept by the parent (the page's relayed copies are ignored).
#[derive(Debug, Clone, Default)]
pub struct ParentLedger {
    pub steps: BTreeMap<String, Value>,
}

impl ParentLedger {
    /// Records one answer; a step answered twice is an error (no replayed observation).
    pub fn record(&mut self, step: &str, answer: Value) -> Result<(), String> {
        if self.steps.contains_key(step) {
            return Err(format!("{step}: answered twice"));
        }
        self.steps.insert(step.to_owned(), answer);
        Ok(())
    }

    fn host(&self, step: &str, host: Host) -> Result<HostSnapshot, String> {
        let answer = self
            .steps
            .get(step)
            .ok_or_else(|| format!("{step}: parent never observed this step"))?;
        if answer["step"] != step {
            return Err(format!("{step}: ledger entry of step {}", answer["step"]));
        }
        serde_json::from_value(answer["hosts"][host.label()].clone())
            .map_err(|e| format!("{step}/{}: {e}", host.label()))
    }
}

/// Values fixed by the parent before the window starts (fixture identity + run nonces).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expectations {
    pub session_local: String,
    pub session_ssh: String,
    pub session_legacy: String,
    pub boot_local: String,
    pub boot_ssh: String,
    pub boot_legacy: String,
    pub root_local: String,
    pub root_ssh: String,
    pub root_legacy: String,
    pub ssh_label: String,
    pub legacy_label: String,
    /// Fixture profile ids (internal; must not be the visible label).
    pub ssh_profile_id: String,
    pub legacy_profile_id: String,
    pub agent_kind: String,
    pub agent_name: String,
    pub prompt_nonce: String,
    pub edit_nonce: String,
    pub dirty_nonce: String,
    pub dirty_file: String,
}

impl Expectations {
    /// From `RemoteFixture::harness_params_json()` (after `start_legacy_host`), the legacy
    /// `HostFixture` boot/root (absent from those params) and a run nonce (`[a-z0-9]{6,}`), so the
    /// same values reach the page params and the evaluator.
    pub fn from_fixture_params(
        params: &Value,
        legacy_boot: &str,
        legacy_root: &str,
        nonce: &str,
    ) -> Result<Self, String> {
        if nonce.len() < 6
            || !nonce
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!("invalid run nonce {nonce:?}"));
        }
        let text = |v: &Value, k: &str| {
            v[k].as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("fixture params without {k}"))
        };
        let legacy = &params["legacy_profile"];
        if legacy_boot.is_empty() || legacy_root.is_empty() {
            return Err("legacy boot and root are required".into());
        }
        if !legacy.is_object() {
            return Err("fixture params without legacy_profile (start_legacy_host first)".into());
        }
        Ok(Self {
            session_local: text(params, "session_local")?,
            session_ssh: text(params, "session_ssh")?,
            session_legacy: text(legacy, "session")?,
            boot_local: text(params, "boot_local")?,
            boot_ssh: text(params, "boot_ssh")?,
            boot_legacy: legacy_boot.to_owned(),
            root_local: text(params, "root_local")?,
            root_ssh: text(params, "root_ssh")?,
            root_legacy: legacy_root.to_owned(),
            ssh_label: text(&params["ssh_profile"], "label")?,
            legacy_label: text(legacy, "label")?,
            ssh_profile_id: text(&params["ssh_profile"], "id")?,
            legacy_profile_id: text(legacy, "id")?,
            agent_kind: text(params, "agent_kind")?,
            agent_name: format!("hd007-agent-{nonce}"),
            prompt_nonce: format!("hd007-prompt-{nonce}"),
            edit_nonce: format!("hd007-remote-edit-{nonce}"),
            dirty_nonce: format!("hd007-dirty-{nonce}"),
            dirty_file: "notas.txt".into(),
        })
    }

    /// `params.ssh_flow` for the page: connection form values and nonces only (no key paths).
    pub fn page_params(&self, fixture: &Value) -> Value {
        let profile = |p: &Value| json!({ "label": p["label"], "target": p["target"], "port": p["port"], "session": p["session"] });
        json!({
            "ssh_profile": profile(&fixture["ssh_profile"]),
            "legacy_profile": profile(&fixture["legacy_profile"]),
            "local": { "label": "Fidelidade Local SSH-flow", "session": self.session_local, "root": self.root_local },
            "ssh": { "label": "Fidelidade SSH", "session": self.session_ssh, "root": self.root_ssh },
            "legacy": { "label": "Fidelidade Legado", "session": self.session_legacy, "root": self.root_legacy },
            "shared_pane": SHARED_PANE,
            "agent_kind": self.agent_kind,
            "agent_name": self.agent_name,
            "prompt_nonce": self.prompt_nonce,
            "edit_nonce": self.edit_nonce,
            "dirty_nonce": self.dirty_nonce,
            "dirty_file": self.dirty_file,
        })
    }
}

/// Agent helper log lines, parsed strictly (`start session=S pane=P pid=N argv=… TS`, `prompt TEXT TS`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentLog {
    pub starts: Vec<(String, String)>,
    pub prompts: Vec<String>,
}

impl AgentLog {
    pub fn parse(log: &str) -> Self {
        let mut out = Self::default();
        for line in log.lines() {
            if let Some(rest) = line.strip_prefix("start ") {
                let field = |k: &str| {
                    rest.split(' ')
                        .find_map(|w| w.strip_prefix(k))
                        .unwrap_or("")
                        .to_owned()
                };
                out.starts.push((field("session="), field("pane=")));
            } else if let Some(rest) = line.strip_prefix("prompt ") {
                if let Some((text, ts)) = rest.rsplit_once(' ') {
                    if !ts.is_empty() && ts.chars().all(|c| c.is_ascii_digit()) {
                        out.prompts.push(text.to_owned());
                    }
                }
            }
        }
        out
    }

    fn starts_in(&self, session: &str) -> Vec<&str> {
        self.starts
            .iter()
            .filter(|(s, _)| s == session)
            .map(|(_, p)| p.as_str())
            .collect()
    }

    fn prompts_equal(&self, text: &str) -> usize {
        self.prompts.iter().filter(|p| *p == text).count()
    }
}

type Checks = Vec<(&'static str, bool)>;

fn page_str<'a>(page: &'a Value, path: &str) -> Result<&'a str, String> {
    page.pointer(path)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("page report without {path}"))
}

fn page_bool(page: &Value, path: &str) -> Result<bool, String> {
    page.pointer(path)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("page report without {path}"))
}

/// `pane P · geração G · boot B` as shown by the status bar; returns (pane, boot prefix).
fn page_identity<'a>(page: &'a Value, path: &str) -> Result<(&'a str, &'a str), String> {
    let pane = page_str(page, &format!("{path}/pane_id"))?;
    let boot = page_str(page, &format!("{path}/boot_prefix"))?;
    Ok((pane, boot))
}

fn boot_matches(prefix: &str, boot: &str) -> bool {
    prefix.len() >= 8 && boot.starts_with(prefix)
}

/// Named checks of one SSH phase. `Err` when the page failed or an observation is missing.
pub fn checks(
    phase: &str,
    page: &Value,
    ledger: &ParentLedger,
    exp: &Expectations,
) -> Result<Checks, String> {
    if !page["error"].is_null() {
        return Err(format!("{phase}: page error {}", page["error"]));
    }
    if page["phase"] != phase {
        return Err(format!("{phase}: report of phase {}", page["phase"]));
    }
    match phase {
        "hosts-identity" => hosts_identity(page, ledger, exp),
        "ssh-agent-actions" => agent_actions(page, ledger, exp),
        "ssh-files-readonly" => files_readonly(page, ledger, exp),
        "legacy-server" => legacy_server(page, ledger, exp),
        "host-switch-dirty" => dirty_switch(page, ledger, exp),
        other => Err(format!("{other}: not an ssh-flow phase")),
    }
}

fn hosts_identity(
    page: &Value,
    ledger: &ParentLedger,
    exp: &Expectations,
) -> Result<Checks, String> {
    let local = ledger.host("ssh-identity", Host::Local)?;
    let ssh = ledger.host("ssh-identity", Host::Ssh)?;
    let (lp, lb) = page_identity(page, "/local_identity")?;
    let (sp, sb) = page_identity(page, "/ssh_identity")?;
    let shared = local.session == exp.session_local
        && ssh.session == exp.session_ssh
        && local.boot_id == exp.boot_local
        && ssh.boot_id == exp.boot_ssh
        && local.boot_id != ssh.boot_id
        && local.server_alive
        && ssh.server_alive
        && local.pane(SHARED_PANE).is_some()
        && ssh.pane(SHARED_PANE).is_some()
        && local.pane(lp).is_some()
        && ssh.pane(sp).is_some()
        && page_names(page, "/local_identity/pane_options")?.contains(SHARED_PANE)
        && page_names(page, "/ssh_identity/pane_options")?.contains(SHARED_PANE)
        && boot_matches(lb, &exp.boot_local)
        && boot_matches(sb, &exp.boot_ssh)
        && !boot_matches(sb, &exp.boot_local);
    let row_root = |key: &str| page_str(page, &format!("/projects/{key}/root"));
    let row_session = |key: &str| page_str(page, &format!("/projects/{key}/session"));
    let roots = row_root("local")? == exp.root_local
        && row_root("ssh")? == exp.root_ssh
        && row_session("local")? == exp.session_local
        && row_session("ssh")? == exp.session_ssh
        && local.root == exp.root_local
        && ssh.root == exp.root_ssh
        && exp.root_local != exp.root_ssh
        && local.digests != ssh.digests;
    let endpoint = page_str(page, "/selected/endpoint")?;
    let labels = [
        page_str(page, "/selected/host_label")?,
        page_str(page, "/selected/project_badge")?,
        page_str(page, "/selected/agents_identity_host")?,
    ];
    let friendly = endpoint != "local"
        && !endpoint.is_empty()
        && page_str(page, "/selected/host_kind")? == "SSH"
        && labels.iter().all(|l| *l == exp.ssh_label)
        && labels
            .iter()
            .all(|l| !l.contains(endpoint) && !l.contains(&exp.ssh_profile_id));
    Ok(vec![
        ("local_and_ssh_share_pane_w1_p1", shared),
        ("projects_have_distinct_roots", roots),
        ("ssh_selected_with_friendly_label", friendly),
    ])
}

#[derive(Debug, Deserialize)]
struct UiPane {
    pane_id: String,
    cells: [u32; 4],
    focused: bool,
}

/// Panes shown by the agents surface tile without overlap and match a right split of one pane.
fn tiles_right_split(panes: &[UiPane]) -> bool {
    if panes.len() != 2 {
        return false;
    }
    let mut sorted: Vec<&UiPane> = panes.iter().collect();
    sorted.sort_by_key(|p| p.cells[0]);
    let (a, b) = (sorted[0].cells, sorted[1].cells);
    a[2] > 0 && b[2] > 0 && a[3] > 0 && a[1] == b[1] && a[3] == b[3] && a[0] + a[2] <= b[0]
}

fn agent_actions(
    page: &Value,
    ledger: &ParentLedger,
    exp: &Expectations,
) -> Result<Checks, String> {
    let before_ssh = ledger.host("ssh-agent-before", Host::Ssh)?;
    let after_ssh = ledger.host("ssh-agent-after", Host::Ssh)?;
    let before_local = ledger.host("ssh-agent-before", Host::Local)?;
    let after_local = ledger.host("ssh-agent-after", Host::Local)?;
    let start_pane = page_str(page, "/start_pane")?;
    let (bs, as_) = (
        AgentLog::parse(&before_ssh.agent_log),
        AgentLog::parse(&after_ssh.agent_log),
    );
    let new_starts = as_.starts_in(&exp.session_ssh);
    let started = before_ssh.same_server(&after_ssh)
        && bs.starts_in(&exp.session_ssh).is_empty()
        && new_starts == [start_pane]
        && as_.starts.len() == 1
        && page_str(page, "/agent_status_pane")? == start_pane;
    let prompted = bs.prompts_equal(&exp.prompt_nonce) == 0
        && as_.prompts_equal(&exp.prompt_nonce) == 1
        && as_.prompts.len() == bs.prompts.len() + 1;

    let ui: Vec<UiPane> = serde_json::from_value(page["panes_after"].clone())
        .map_err(|e| format!("page report panes_after: {e}"))?;
    let focus_target = page_str(page, "/focus_target")?;
    let (surface_pane, surface_boot) = page_identity(page, "/ssh_identity_after")?;
    let ui_ids: BTreeSet<&str> = ui.iter().map(|p| p.pane_id.as_str()).collect();
    let tab = after_ssh
        .pane(SHARED_PANE)
        .map(|p| (p.workspace_id.clone(), p.tab_id.clone()));
    let engine_tab: BTreeSet<&str> = after_ssh
        .panes
        .iter()
        .filter(|p| Some((p.workspace_id.clone(), p.tab_id.clone())) == tab)
        .map(|p| p.pane_id.as_str())
        .collect();
    let engine_focused: Vec<&str> = after_ssh
        .panes
        .iter()
        .filter(|p| p.focused)
        .map(|p| p.pane_id.as_str())
        .collect();
    let ui_focused: Vec<&str> = ui
        .iter()
        .filter(|p| p.focused)
        .map(|p| p.pane_id.as_str())
        .collect();
    let geometry = before_ssh.same_server(&after_ssh)
        && after_ssh.panes.len() == before_ssh.panes.len() + 1
        && ui_ids == engine_tab
        && ui_ids.contains(SHARED_PANE)
        && tiles_right_split(&ui)
        && engine_focused == [focus_target]
        && ui_focused == [focus_target]
        && surface_pane == focus_target
        && boot_matches(surface_boot, &exp.boot_ssh);

    let (bl, al) = (
        AgentLog::parse(&before_local.agent_log),
        AgentLog::parse(&after_local.agent_log),
    );
    let untouched = before_local.same_server(&after_local)
        && before_local.panes == after_local.panes
        && bl == al
        && al.starts.is_empty()
        && !after_local.agent_log.contains(&exp.prompt_nonce)
        && before_local.digests == after_local.digests;
    Ok(vec![
        ("agent_started_once_on_ssh", started),
        ("prompt_delivered_once_on_ssh", prompted),
        ("split_and_focus_confirmed_geometry_shown", geometry),
        ("zero_actions_on_local", untouched),
    ])
}

fn page_names(page: &Value, path: &str) -> Result<BTreeSet<String>, String> {
    page.pointer(path)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("page report without {path}"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{path}: non-text entry"))
        })
        .collect()
}

fn files_readonly(
    page: &Value,
    ledger: &ParentLedger,
    exp: &Expectations,
) -> Result<Checks, String> {
    let before_ssh = ledger.host("ssh-files-before", Host::Ssh)?;
    let after_ssh = ledger.host("ssh-files-after", Host::Ssh)?;
    let local = ledger.host("ssh-files-after", Host::Local)?;
    let content =
        |snap: &HostSnapshot, name: &str| snap.contents.get(name).cloned().unwrap_or_default();
    let host_endpoint = page_str(page, "/selected_endpoint")?;
    let names = page_names(page, "/explorer_names")?;
    let top = |snap: &HostSnapshot| -> BTreeSet<String> {
        snap.digests
            .keys()
            .map(|k| k.split('/').next().unwrap_or(k).to_owned())
            .collect()
    };
    let opened = page_str(page, "/opened/text")?;
    let explorer = page_str(page, "/explorer_host")? == host_endpoint
        && host_endpoint != "local"
        && page_str(page, "/explorer_host_label")? == exp.ssh_label
        && names == top(&after_ssh)
        && names != top(&local)
        && page_str(page, "/opened/path")? == "notas.txt"
        && opened.trim_end_matches('\n') == content(&after_ssh, "notas.txt").trim_end_matches('\n')
        && opened != content(&local, "notas.txt");
    let diff_lines = page_names(page, "/diff/lines")?;
    let lines = |snap: &HostSnapshot| -> BTreeSet<String> {
        content(snap, "diff-target.txt")
            .lines()
            .map(str::to_owned)
            .collect()
    };
    let local_only: BTreeSet<String> = lines(&local)
        .difference(&lines(&after_ssh))
        .cloned()
        .collect();
    let diff = page_str(page, "/diff/host")? == host_endpoint
        && page_str(page, "/diff/path")? == "diff-target.txt"
        && !diff_lines.is_empty()
        && diff_lines == lines(&after_ssh)
        && diff_lines.is_disjoint(&local_only);
    let unchanged = before_ssh.digests == after_ssh.digests
        && before_ssh.contents == after_ssh.contents
        && after_ssh
            .contents
            .values()
            .all(|c| !c.contains(&exp.edit_nonce))
        && !after_ssh.forced_command_log.contains(&exp.edit_nonce);
    let read_only = page_bool(page, "/read_only_badge")?
        && !page_bool(page, "/save_control")?
        && page_str(page, "/edit_attempt/contenteditable")? == "false"
        && page_str(page, "/edit_attempt/nonce")? == exp.edit_nonce
        && page_str(page, "/edit_attempt/text_before")?
            == page_str(page, "/edit_attempt/text_after")?
        && !page_str(page, "/edit_attempt/text_after")?.contains(&exp.edit_nonce)
        && unchanged;
    Ok(vec![
        ("explorer_on_selected_host", explorer),
        ("diff_on_selected_host", diff),
        ("remote_marked_read_only", read_only),
    ])
}

fn legacy_server(
    page: &Value,
    ledger: &ParentLedger,
    exp: &Expectations,
) -> Result<Checks, String> {
    let before = |h| ledger.host("ssh-legacy-before", h);
    let after = |h| ledger.host("ssh-legacy-after", h);
    let (bleg, aleg) = (before(Host::Legacy)?, after(Host::Legacy)?);
    let code = page_str(page, "/legacy/agents_error_code")?;
    let message = page_str(page, "/legacy/agents_error_message")?;
    let (lp, lb) = page_identity(page, "/legacy/identity")?;
    let legacy_live = page_str(page, "/legacy/status_phase")? == "live"
        && aleg.pane(lp).is_some()
        && boot_matches(lb, &exp.boot_legacy)
        && bleg.same_server(&aleg)
        && aleg.boot_id == exp.boot_legacy;
    let dependent = code == UNSUPPORTED_CODE
        && message.contains(UNSUPPORTED_MARK)
        && !page_bool(page, "/legacy/start_enabled")?
        // Endpoint command announced by the legacy welcome: independent of the JSON API (ESCALAR24).
        && page_bool(page, "/legacy/split_enabled")?
        && legacy_live
        && AgentLog::parse(&aleg.agent_log).starts.is_empty()
        && aleg.panes == bleg.panes;
    let (sp, sb) = page_identity(page, "/ssh_after/identity")?;
    let (bssh, assh) = (before(Host::Ssh)?, after(Host::Ssh)?);
    let terminal = legacy_live
        && page_str(page, "/ssh_after/status_phase")? == "live"
        && assh.pane(sp).is_some()
        && boot_matches(sb, &exp.boot_ssh)
        && bssh.same_server(&assh);
    let (lcp, lcb) = page_identity(page, "/local_after/identity")?;
    let (bloc, aloc) = (before(Host::Local)?, after(Host::Local)?);
    let local = page_str(page, "/local_after/status_phase")? == "live"
        && aloc.pane(lcp).is_some()
        && boot_matches(lcb, &exp.boot_local)
        && bloc.same_server(&aloc)
        && bloc.panes == aloc.panes
        && AgentLog::parse(&aloc.agent_log) == AgentLog::parse(&bloc.agent_log);
    Ok(vec![
        ("only_dependent_actions_unavailable", dependent),
        ("terminal_preserved", terminal),
        ("local_preserved", local),
    ])
}

fn dirty_switch(page: &Value, ledger: &ParentLedger, exp: &Expectations) -> Result<Checks, String> {
    let before = ledger.host("ssh-dirty-before", Host::Local)?;
    let after = ledger.host("ssh-dirty-after", Host::Local)?;
    let original = before
        .contents
        .get(&exp.dirty_file)
        .ok_or_else(|| format!("parent did not read {}", exp.dirty_file))?;
    let typed = page_str(page, "/text_before_switch")?;
    let returned = page_str(page, "/text_after_return")?;
    let (_, via) = page_identity(page, "/ssh_during_switch")?;
    let (_, back) = page_identity(page, "/local_after_return")?;
    let preserved = page_str(page, "/file")? == exp.dirty_file
        && page_bool(page, "/dirty_before_switch")?
        && page_bool(page, "/dirty_after_return")?
        && typed.contains(&exp.dirty_nonce)
        && typed != original
        && returned == typed
        && boot_matches(via, &exp.boot_ssh)
        && boot_matches(back, &exp.boot_local)
        && after.contents.get(&exp.dirty_file) == Some(original)
        && before.digests == after.digests
        && after
            .contents
            .values()
            .all(|c| !c.contains(&exp.dirty_nonce));
    Ok(vec![(
        "local_dirty_buffer_preserved_after_switch",
        preserved,
    )])
}

/// [`HostObserver`] over the real fixture (`remote::RemoteFixture`). Linux only, like the fixture.
#[cfg(target_os = "linux")]
pub mod fixture {
    use std::collections::BTreeMap;
    use std::path::Path;

    use serde_json::json;

    use super::super::remote::{compute_sha256, is_proc_alive, RemoteFixture};
    use super::{Host, HostObserver, HostSnapshot, WATCHED_FILES};

    pub struct FixtureObserver<'a>(pub &'a RemoteFixture);

    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
                out.insert(rel.display().to_string(), compute_sha256(&path)?);
            }
        }
        Ok(())
    }

    impl HostObserver for FixtureObserver<'_> {
        fn snapshot(&self, host: Host) -> Result<HostSnapshot, String> {
            let fx = self.0;
            let (session, root) = match host {
                Host::Local => (&fx.local_session, &fx.local_root),
                Host::Ssh => (&fx.ssh_session, &fx.ssh_root),
                Host::Legacy => (
                    fx.legacy_session
                        .as_ref()
                        .ok_or("legacy host not started")?,
                    &fx.legacy_root,
                ),
            };
            let process = session
                .tracked_process(host.label())
                .ok_or_else(|| format!("{}: server process not tracked", host.label()))?;
            let label = host.label();
            let result = fx
                .host_api(label)?
                .request("pane.list", json!({}))
                .map_err(|e| format!("{label} pane.list: {e:?}"))?;
            let mut digests = BTreeMap::new();
            walk(root, root, &mut digests)?;
            let mut contents = BTreeMap::new();
            for name in WATCHED_FILES {
                if let Ok(text) = std::fs::read_to_string(root.join(name)) {
                    contents.insert(name.to_owned(), text);
                }
            }
            Ok(HostSnapshot {
                session: session.name.clone(),
                boot_id: session.boot_id.clone(),
                server_pid: process.pid,
                server_starttime: process.starttime,
                server_alive: is_proc_alive(process.pid, process.starttime),
                root: root.display().to_string(),
                panes: HostSnapshot::panes_from_list(&result)?,
                agent_log: fx.read_agent_log(label)?,
                forced_command_log: fx.read_forced_command_log(label).unwrap_or_default(),
                digests,
                contents,
            })
        }
    }
}
