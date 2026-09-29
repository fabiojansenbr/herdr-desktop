//! Contracts of the SSH phases of the spec 007 native flow (tests/fidelity-native/ssh_flow.rs):
//! pure evaluators over synthetic engine snapshots and page reports. No engine, display or GUI.
//! Each negative case names the wrong behavior it would catch.

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/plan.rs"]
mod plan;
#[cfg(target_os = "linux")]
#[allow(dead_code, unused_imports, clippy::enum_variant_names)]
#[path = "../../tests/fidelity-native/remote.rs"]
mod remote;
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/ssh_flow.rs"]
mod ssh_flow;

use std::collections::BTreeMap;

use serde_json::{json, Value};
use ssh_flow::{
    checks, parent_step, AgentLog, Expectations, Host, HostObserver, HostSnapshot, PaneRow,
    ParentLedger, PHASES, STEPS,
};

const BOOT_L: &str = "b10c41aa-0000-4000-8000-000000000001";
const BOOT_S: &str = "55e4aabb-0000-4000-8000-000000000002";
const BOOT_G: &str = "1e9ac7cc-0000-4000-8000-000000000003";
const PROFILE: &str = "0f3c2a10-7d1e-4b8a-9c55-aa0000000007";

fn exp() -> Expectations {
    let params = json!({
        "session_local": "hd007-remote-1-abc123-loc", "session_ssh": "hd007-remote-1-abc123-ssh",
        "boot_local": BOOT_L, "boot_ssh": BOOT_S,
        "root_local": "/w/fixtures/local-project", "root_ssh": "/w/fixtures/ssh-project",
        "agent_kind": "pi",
        "ssh_profile": { "id": PROFILE, "label": "Host SSH (Referencia)", "target": "u@127.0.0.1", "port": 2222, "session": "hd007-remote-1-abc123-ssh" },
        "legacy_profile": { "id": "9a9a9a9a-0000-4000-8000-00000000000a", "label": "Host SSH Legado (0.9.0)", "target": "u@127.0.0.1", "port": 2223, "session": "hd007-remote-1-abc123-leg" },
    });
    Expectations::from_fixture_params(&params, BOOT_G, "/w/fixtures/legacy-project", "n0nce7")
        .unwrap()
}

fn pane(id: &str, tab: &str, focused: bool) -> PaneRow {
    PaneRow {
        pane_id: id.into(),
        workspace_id: "w1".into(),
        tab_id: tab.into(),
        focused,
    }
}

fn snap(host: Host, e: &Expectations) -> HostSnapshot {
    let (session, boot, root, pid, notas, diff) = match host {
        Host::Local => (
            &e.session_local,
            &e.boot_local,
            &e.root_local,
            101,
            "conteudo LOCAL\n",
            "base\napenas versao local\n",
        ),
        Host::Ssh => (
            &e.session_ssh,
            &e.boot_ssh,
            &e.root_ssh,
            202,
            "conteudo REMOTO\n",
            "base\nMODIFICADA no host SSH\n",
        ),
        Host::Legacy => (
            &e.session_legacy,
            &e.boot_legacy,
            &e.root_legacy,
            303,
            "conteudo LEGADO\n",
            "",
        ),
    };
    let only = if host == Host::Local {
        "local-only.txt"
    } else {
        "remote-only.txt"
    };
    HostSnapshot {
        session: session.clone(),
        boot_id: boot.clone(),
        server_pid: pid,
        server_starttime: 9000 + pid as u64,
        server_alive: true,
        root: root.clone(),
        panes: vec![pane("w1:p1", "w1:t1", true)],
        agent_log: String::new(),
        forced_command_log: String::new(),
        digests: BTreeMap::from([
            ("notas.txt".into(), format!("sha-{notas}")),
            (only.into(), format!("sha-{}", host.label())),
        ]),
        contents: BTreeMap::from([
            ("notas.txt".into(), notas.into()),
            ("diff-target.txt".into(), diff.into()),
        ]),
    }
}

/// Synthetic observer; `mutate` applies the behavior of the engine after the UI acted.
struct Fake(BTreeMap<&'static str, HostSnapshot>);
impl HostObserver for Fake {
    fn snapshot(&self, host: Host) -> Result<HostSnapshot, String> {
        self.0
            .get(host.label())
            .cloned()
            .ok_or_else(|| format!("{} down", host.label()))
    }
}

fn observe(ledger: &mut ParentLedger, step: &str, hosts: &BTreeMap<&'static str, HostSnapshot>) {
    let answer = parent_step(step, &json!({ "from": "page" }), &Fake(hosts.clone())).unwrap();
    ledger.record(step, answer).unwrap();
}

fn hosts(e: &Expectations) -> BTreeMap<&'static str, HostSnapshot> {
    [Host::Local, Host::Ssh, Host::Legacy]
        .into_iter()
        .map(|h| (h.label(), snap(h, e)))
        .collect()
}

fn id(pane: &str, boot: &str) -> Value {
    json!({ "pane_id": pane, "boot_prefix": &boot[..8] })
}

/// A correct run of all five phases: engine snapshots per step and the page reports.
fn good_run(e: &Expectations) -> (ParentLedger, BTreeMap<&'static str, Value>) {
    let mut ledger = ParentLedger::default();
    let mut h = hosts(e);
    observe(&mut ledger, "ssh-identity", &h);
    observe(&mut ledger, "ssh-agent-before", &h);
    let ssh = h.get_mut("ssh").unwrap();
    ssh.agent_log = format!(
        "start session={} pane=w1:p1 pid=7 argv= 1\nstate idle rc=0 2\nprompt {} 3\n",
        e.session_ssh, e.prompt_nonce
    );
    ssh.panes = vec![pane("w1:p1", "w1:t1", true), pane("w1:p2", "w1:t1", false)];
    observe(&mut ledger, "ssh-agent-after", &h);
    for step in [
        "ssh-files-before",
        "ssh-files-after",
        "ssh-legacy-before",
        "ssh-legacy-after",
        "ssh-dirty-before",
        "ssh-dirty-after",
    ] {
        observe(&mut ledger, step, &h);
    }
    let ssh_ep = "ssh:6c1d";
    let pages = BTreeMap::from([
        (
            "hosts-identity",
            json!({
                "phase": "hosts-identity", "error": null,
                "local_identity": { "pane_id": "w1:p1", "boot_prefix": &BOOT_L[..8], "pane_options": ["w1:p1"] },
                "ssh_identity": { "pane_id": "w1:p1", "boot_prefix": &BOOT_S[..8], "pane_options": ["w1:p1"] },
                "projects": { "local": { "root": e.root_local, "session": e.session_local }, "ssh": { "root": e.root_ssh, "session": e.session_ssh } },
                "selected": { "endpoint": ssh_ep, "host_kind": "SSH", "host_label": e.ssh_label, "project_badge": e.ssh_label, "agents_identity_host": e.ssh_label },
            }),
        ),
        (
            "ssh-agent-actions",
            json!({
                "phase": "ssh-agent-actions", "error": null, "start_pane": "w1:p1", "agent_status_pane": "w1:p1",
                "focus_target": "w1:p1", "ssh_identity_after": id("w1:p1", BOOT_S),
                "panes_after": [ { "pane_id": "w1:p1", "cells": [0, 0, 40, 24], "focused": true }, { "pane_id": "w1:p2", "cells": [41, 0, 39, 24], "focused": false } ],
            }),
        ),
        (
            "ssh-files-readonly",
            json!({
                "phase": "ssh-files-readonly", "error": null, "selected_endpoint": ssh_ep, "explorer_host": ssh_ep, "explorer_host_label": e.ssh_label,
                "explorer_names": ["notas.txt", "remote-only.txt"], "opened": { "path": "notas.txt", "text": "conteudo REMOTO\n" },
                "diff": { "host": ssh_ep, "path": "diff-target.txt", "lines": ["base", "MODIFICADA no host SSH"] },
                "read_only_badge": true, "save_control": false,
                "edit_attempt": { "nonce": e.edit_nonce, "contenteditable": "false", "text_before": "base", "text_after": "base" },
            }),
        ),
        (
            "legacy-server",
            json!({
                "phase": "legacy-server", "error": null,
                "legacy": { "agents_error_code": "remote_api_unsupported", "agents_error_message": "the remote Herdr does not offer the JSON API over SSH (remote-api-bridge); agent events are unavailable",
                            "start_enabled": false, "split_enabled": true, "status_phase": "live", "identity": id("w1:p1", BOOT_G) },
                "ssh_after": { "status_phase": "live", "identity": id("w1:p1", BOOT_S) },
                "local_after": { "status_phase": "live", "identity": id("w1:p1", BOOT_L) },
            }),
        ),
        (
            "host-switch-dirty",
            json!({
                "phase": "host-switch-dirty", "error": null, "file": "notas.txt",
                "dirty_before_switch": true, "dirty_after_return": true,
                "text_before_switch": format!("{}conteudo LOCAL\n", e.dirty_nonce), "text_after_return": format!("{}conteudo LOCAL\n", e.dirty_nonce),
                "ssh_during_switch": id("w1:p1", BOOT_S), "local_after_return": id("w1:p1", BOOT_L),
            }),
        ),
    ]);
    (ledger, pages)
}

fn verdict(phase: &str, page: &Value, ledger: &ParentLedger) -> BTreeMap<&'static str, bool> {
    checks(phase, page, ledger, &exp())
        .unwrap()
        .into_iter()
        .collect()
}

fn with_host(
    ledger: &mut ParentLedger,
    step: &str,
    label: &str,
    edit: impl FnOnce(&mut HostSnapshot),
) {
    let entry = ledger.steps.get_mut(step).unwrap();
    let mut s: HostSnapshot = serde_json::from_value(entry["hosts"][label].clone()).unwrap();
    edit(&mut s);
    entry["hosts"][label] = json!(s);
}

#[test]
fn phases_and_checks_equal_the_plan_and_steps_are_valid_names() {
    for (name, checks) in PHASES {
        let spec = plan::FLOW
            .iter()
            .find(|p| p.name == name)
            .expect("phase in plan");
        assert_eq!(spec.checks, checks, "{name}");
    }
    for step in STEPS {
        assert!(
            step.len() <= 64
                && step
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{step}"
        );
    }
}

#[test]
fn correct_run_passes_every_check_through_the_plan_evaluator() {
    let (ledger, pages) = good_run(&exp());
    let mut outcomes = BTreeMap::new();
    for (name, _) in PHASES {
        let checks = checks(name, &pages[name], &ledger, &exp()).unwrap();
        assert!(checks.iter().all(|(_, ok)| *ok), "{name}: {checks:?}");
        outcomes.insert(name.to_owned(), plan::PhaseOutcome::with(&checks));
    }
    let v = plan::evaluate(plan::FLOW, &outcomes);
    // Linked phases are Prepared: exactly the five SSH phases pass from these outcomes, and the
    // flow still is not a pass while the other phases' observations are absent (ESCALAR22).
    let ssh: Vec<String> = PHASES.iter().map(|(n, _)| (*n).to_owned()).collect();
    assert!(
        v.passed == ssh && v.failed.is_empty() && !v.is_pass(plan::FLOW),
        "{v:?}"
    );
}

#[test]
fn missing_parent_step_or_page_field_is_an_error_not_a_pass() {
    let (mut ledger, pages) = good_run(&exp());
    ledger.steps.remove("ssh-agent-after");
    assert!(checks(
        "ssh-agent-actions",
        &pages["ssh-agent-actions"],
        &ledger,
        &exp()
    )
    .is_err());
    let (ledger, pages) = good_run(&exp());
    let mut page = pages["hosts-identity"].clone();
    page["selected"]
        .as_object_mut()
        .unwrap()
        .remove("project_badge");
    assert!(checks("hosts-identity", &page, &ledger, &exp()).is_err());
    let mut failed = pages["legacy-server"].clone();
    failed["error"] = json!("timed out");
    assert!(checks("legacy-server", &failed, &ledger, &exp()).is_err());
    assert!(checks("native-keys", &pages["legacy-server"], &ledger, &exp()).is_err());
}

#[test]
fn ledger_refuses_replayed_step_and_parent_refuses_unknown_step_or_dead_host() {
    let (mut ledger, _) = good_run(&exp());
    assert!(ledger.record("ssh-identity", json!({})).is_err());
    let h = hosts(&exp());
    assert!(parent_step("native-keys", &json!({}), &Fake(h.clone())).is_err());
    let mut no_legacy = h;
    no_legacy.remove("legacy");
    assert!(parent_step("ssh-legacy-before", &json!({}), &Fake(no_legacy.clone())).is_err());
    assert!(parent_step("ssh-agent-before", &json!({}), &Fake(no_legacy)).is_ok());
}

#[test]
fn duplicated_start_or_prompt_on_ssh_fails() {
    let e = exp();
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-agent-after", "ssh", |s| {
        s.agent_log = format!("start session={0} pane=w1:p1 pid=7 argv= 1\nstart session={0} pane=w1:p1 pid=8 argv= 2\nprompt {1} 3\nprompt {1} 4\n", e.session_ssh, e.prompt_nonce);
    });
    let v = verdict("ssh-agent-actions", &pages["ssh-agent-actions"], &ledger);
    assert!(
        !v["agent_started_once_on_ssh"] && !v["prompt_delivered_once_on_ssh"],
        "{v:?}"
    );
}

#[test]
fn prompt_with_extra_text_is_not_the_nonce() {
    let e = exp();
    let log = AgentLog::parse(&format!(
        "prompt {}x 3\nprompt x{} 4\n",
        e.prompt_nonce, e.prompt_nonce
    ));
    assert_eq!(log.prompts.len(), 2);
    assert!(log.prompts.iter().all(|p| *p != e.prompt_nonce));
}

#[test]
fn action_reaching_local_fails_zero_actions_on_local() {
    let e = exp();
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-agent-after", "local", |s| {
        s.agent_log = format!(
            "start session={} pane=w1:p1 pid=9 argv= 5\n",
            e.session_local
        );
    });
    assert!(
        !verdict("ssh-agent-actions", &pages["ssh-agent-actions"], &ledger)
            ["zero_actions_on_local"]
    );
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-agent-after", "local", |s| {
        s.panes.push(pane("w1:p2", "w1:t1", false))
    });
    assert!(
        !verdict("ssh-agent-actions", &pages["ssh-agent-actions"], &ledger)
            ["zero_actions_on_local"]
    );
}

#[test]
fn wrong_boot_or_restarted_server_fails() {
    let e = exp();
    let (ledger, pages) = good_run(&e);
    let mut page = pages["hosts-identity"].clone();
    page["ssh_identity"]["boot_prefix"] = json!(&BOOT_L[..8]); // UI shows Local's boot as SSH.
    assert!(!verdict("hosts-identity", &page, &ledger)["local_and_ssh_share_pane_w1_p1"]);
    let mut absent = pages["hosts-identity"].clone();
    absent["ssh_identity"]["pane_options"] = json!(["w2:p1"]); // w1:p1 not offered on SSH.
    assert!(!verdict("hosts-identity", &absent, &ledger)["local_and_ssh_share_pane_w1_p1"]);
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-agent-after", "ssh", |s| {
        s.server_starttime += 1
    });
    let v = verdict("ssh-agent-actions", &pages["ssh-agent-actions"], &ledger);
    assert!(
        !v["agent_started_once_on_ssh"] && !v["split_and_focus_confirmed_geometry_shown"],
        "{v:?}"
    );
}

#[test]
fn internal_profile_id_as_label_fails_friendly_label() {
    let (ledger, pages) = good_run(&exp());
    let mut page = pages["hosts-identity"].clone();
    page["selected"]["project_badge"] = json!(PROFILE);
    assert!(!verdict("hosts-identity", &page, &ledger)["ssh_selected_with_friendly_label"]);
    let mut same_root = pages["hosts-identity"].clone();
    same_root["projects"]["ssh"]["root"] = same_root["projects"]["local"]["root"].clone();
    assert!(!verdict("hosts-identity", &same_root, &ledger)["projects_have_distinct_roots"]);
}

#[test]
fn divergent_geometry_or_unconfirmed_focus_fails() {
    let (ledger, pages) = good_run(&exp());
    let geometry = "split_and_focus_confirmed_geometry_shown";
    let mut overlap = pages["ssh-agent-actions"].clone();
    overlap["panes_after"][1]["cells"] = json!([30, 0, 50, 24]);
    assert!(!verdict("ssh-agent-actions", &overlap, &ledger)[geometry]);
    let mut focus = pages["ssh-agent-actions"].clone();
    focus["panes_after"][0]["focused"] = json!(false);
    focus["panes_after"][1]["focused"] = json!(true);
    focus["focus_target"] = json!("w1:p2");
    assert!(
        !verdict("ssh-agent-actions", &focus, &ledger)[geometry],
        "engine focus is w1:p1"
    );
    let mut extra = pages["ssh-agent-actions"].clone();
    extra["panes_after"].as_array_mut().unwrap().pop();
    assert!(
        !verdict("ssh-agent-actions", &extra, &ledger)[geometry],
        "split not shown"
    );
}

#[test]
fn local_file_or_remote_write_fails_files_checks() {
    let e = exp();
    let (ledger, pages) = good_run(&e);
    let mut page = pages["ssh-files-readonly"].clone();
    page["opened"]["text"] = json!("conteudo LOCAL\n");
    page["explorer_names"] = json!(["local-only.txt", "notas.txt"]);
    page["diff"]["lines"] = json!(["base", "apenas versao local"]);
    let v = verdict("ssh-files-readonly", &page, &ledger);
    assert!(
        !v["explorer_on_selected_host"] && !v["diff_on_selected_host"],
        "{v:?}"
    );
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-files-after", "ssh", |s| {
        s.contents.insert(
            "notas.txt".into(),
            format!("{}conteudo REMOTO\n", e.edit_nonce),
        );
    });
    assert!(
        !verdict("ssh-files-readonly", &pages["ssh-files-readonly"], &ledger)
            ["remote_marked_read_only"]
    );
    let (ledger, pages) = good_run(&e);
    let mut editable = pages["ssh-files-readonly"].clone();
    editable["edit_attempt"]["contenteditable"] = json!("true");
    assert!(!verdict("ssh-files-readonly", &editable, &ledger)["remote_marked_read_only"]);
}

#[test]
fn generic_connection_error_is_not_capability_absence() {
    let (ledger, pages) = good_run(&exp());
    let mut page = pages["legacy-server"].clone();
    page["legacy"]["agents_error_code"] = json!("connection_lost");
    page["legacy"]["agents_error_message"] = json!("conexão perdida");
    assert!(!verdict("legacy-server", &page, &ledger)["only_dependent_actions_unavailable"]);
    let mut disconnected = pages["legacy-server"].clone();
    disconnected["legacy"]["status_phase"] = json!("disconnected");
    let v = verdict("legacy-server", &disconnected, &ledger);
    assert!(
        !v["only_dependent_actions_unavailable"] && !v["terminal_preserved"],
        "{v:?}"
    );
    let mut local_down = pages["legacy-server"].clone();
    local_down["local_after"]["identity"] = id("w1:p1", BOOT_G);
    assert!(!verdict("legacy-server", &local_down, &ledger)["local_preserved"]);
}

#[test]
fn lost_or_saved_dirty_buffer_fails() {
    let e = exp();
    let key = "local_dirty_buffer_preserved_after_switch";
    let (ledger, pages) = good_run(&e);
    let mut lost = pages["host-switch-dirty"].clone();
    lost["text_after_return"] = json!("conteudo LOCAL\n");
    lost["dirty_after_return"] = json!(false);
    assert!(!verdict("host-switch-dirty", &lost, &ledger)[key]);
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-dirty-after", "local", |s| {
        s.contents.insert(
            "notas.txt".into(),
            format!("{}conteudo LOCAL\n", e.dirty_nonce),
        );
    });
    assert!(
        !verdict("host-switch-dirty", &pages["host-switch-dirty"], &ledger)[key],
        "saved to disk"
    );
    let (ledger, pages) = good_run(&e);
    let mut no_switch = pages["host-switch-dirty"].clone();
    no_switch["ssh_during_switch"] = id("w1:p1", BOOT_L);
    assert!(
        !verdict("host-switch-dirty", &no_switch, &ledger)[key],
        "never left Local"
    );
}

#[test]
fn expectations_require_legacy_and_a_valid_nonce() {
    let params = json!({ "session_local": "a" });
    assert!(Expectations::from_fixture_params(&params, BOOT_G, "/r", "n0nce7").is_err());
    let e = exp();
    assert_ne!(e.prompt_nonce, e.dirty_nonce);
    let page = e.page_params(
        &json!({ "ssh_profile": { "label": "x", "id": PROFILE }, "legacy_profile": {} }),
    );
    assert!(
        !page.to_string().contains(PROFILE),
        "page params carry no internal profile id or key path"
    );
}

#[test]
fn start_of_another_session_in_the_ssh_log_is_not_once() {
    let e = exp();
    let (mut ledger, pages) = good_run(&e);
    with_host(&mut ledger, "ssh-agent-after", "ssh", |s| {
        s.agent_log = format!(
            "start session=hd007-remote-9-zzzzzz-ssh pane=w1:p1 pid=6 argv= 0\n{}",
            s.agent_log
        );
    });
    assert!(
        !verdict("ssh-agent-actions", &pages["ssh-agent-actions"], &ledger)
            ["agent_started_once_on_ssh"]
    );
}

#[test]
fn bridge_text_under_another_error_code_is_not_capability_absence() {
    let (ledger, pages) = good_run(&exp());
    let mut page = pages["legacy-server"].clone();
    page["legacy"]["agents_error_code"] = json!("connection_lost"); // message still names the bridge
    assert!(!verdict("legacy-server", &page, &ledger)["only_dependent_actions_unavailable"]);
}

#[test]
fn dirty_flag_kept_but_text_changed_fails() {
    let e = exp();
    let (ledger, pages) = good_run(&e);
    let mut page = pages["host-switch-dirty"].clone();
    page["text_after_return"] =
        json!(format!("{}conteudo LOCAL\n", e.dirty_nonce).replace("LOCAL", "REMOTO"));
    assert!(
        !verdict("host-switch-dirty", &page, &ledger)["local_dirty_buffer_preserved_after_switch"]
    );
}

/// ESCALAR24: the legacy server lacks only the JSON API. `pane.split` is an endpoint command the
/// reference welcome announces (AgentsCore::capabilities), so it stays available there.
/// Would catch: an oracle (or product) that disables a still supported endpoint action, or that
/// accepts the API dependent start still enabled.
#[test]
fn legacy_keeps_supported_split_and_refuses_only_api_dependent_start() {
    let (ledger, pages) = good_run(&exp());
    assert!(
        verdict("legacy-server", &pages["legacy-server"], &ledger)
            ["only_dependent_actions_unavailable"]
    );
    let mut split_disabled = pages["legacy-server"].clone();
    split_disabled["legacy"]["split_enabled"] = json!(false);
    assert!(
        !verdict("legacy-server", &split_disabled, &ledger)["only_dependent_actions_unavailable"]
    );
    let mut start_enabled = pages["legacy-server"].clone();
    start_enabled["legacy"]["start_enabled"] = json!(true);
    assert!(
        !verdict("legacy-server", &start_enabled, &ledger)["only_dependent_actions_unavailable"]
    );
}
