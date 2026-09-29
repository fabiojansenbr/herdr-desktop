// Spec 007 — the pressed tab of the composed agents panel is the tab focused by the attached
// CONNECTION (snapshot confirmed with the committed full surface), not the JSON API global flag.
// Distinct values: API reports w1:t1 focused globally, this connection focuses w1:t2, another
// client/host focuses w1:t3; Local and SSH share tab ids; revisions 6/7/8 differ.
import { describe, expect, it } from "vitest";
import { initialState, reduce, viewModel, type AgentsState } from "./reducer";
import type { AgentsEvent, LiveIdentity, Overview, TabDto, TabFocus } from "./types";

const ssh: LiveIdentity = { endpoint: "ssh-tab", session: "hd007t-remote", connection_generation: 4, boot_id: "boot-ssh-8" };

function tabs(globallyFocused: string): TabDto[] {
  return ["w1:t1", "w1:t2", "w1:t3"].map((tab_id) => ({
    tab_id,
    workspace_id: "w1",
    label: tab_id.slice(4),
    focused: tab_id === globallyFocused,
  }));
}

function focus(tab_id: string | null, revision: number, identity: Partial<LiveIdentity> = {}): TabFocus {
  return { ...ssh, ...identity, revision, tab_id };
}

function overview(extra: Partial<Overview> = {}): Overview {
  return {
    state: "live",
    session: ssh.session,
    identity: ssh,
    server_version: "0.9.0",
    capabilities: null,
    kinds: [],
    agents: [],
    tabs: tabs("w1:t1"),
    topology: null,
    tab_focus: focus("w1:t2", 7),
    error: null,
    ...extra,
  };
}

function event(state: AgentsState, e: AgentsEvent): AgentsState {
  return reduce(state, { type: "event", event: e });
}

function connected(extra: Partial<Overview> = {}): AgentsState {
  return reduce(reduce(initialState(), { type: "connect_started" }), { type: "connected", overview: overview(extra) });
}

function pressed(state: AgentsState): string[] {
  return viewModel(state)
    .tabs.filter((t) => t.focused)
    .map((t) => t.tab_id);
}

describe("connection-confirmed tab focus", () => {
  // Would catch: the r2 native defect — pressing the API's globally focused w1:t1 while the
  // attached SSH connection shows w1:t2.
  it("presses the tab confirmed by the attached connection, not the global API flag", () => {
    expect(pressed(connected())).toEqual(["w1:t2"]);
  });

  // Would catch: a late tab.list refresh (another client focused w1:t3) overriding the
  // connection's focus.
  it("keeps the confirmed focus when a late API tab list arrives", () => {
    const state = event(connected(), { type: "tabs", tabs: tabs("w1:t3") });
    expect(pressed(state)).toEqual(["w1:t2"]);
    expect(state.tabs.find((t) => t.tab_id === "w1:t3")?.focused).toBe(true);
  });

  // Would catch: an older confirmation replacing a newer one, or the event being ignored.
  it("follows newer confirmations of the same connection and ignores older revisions", () => {
    let state = event(connected(), { type: "tab_focus", focus: focus("w1:t3", 8) });
    expect(pressed(state)).toEqual(["w1:t3"]);
    state = event(state, { type: "tab_focus", focus: focus("w1:t1", 6) });
    expect(pressed(state)).toEqual(["w1:t3"]);
  });

  // Would catch: focus of another host/session/boot/connection with the same tab ids moving this panel.
  it("ignores focus of another endpoint, session, boot or connection generation", () => {
    let state = connected();
    state = event(state, { type: "tab_focus", focus: focus("w1:t3", 9, { endpoint: "local" }) });
    state = event(state, { type: "tab_focus", focus: focus("w1:t3", 9, { boot_id: "boot-ssh-9" }) });
    state = event(state, { type: "tab_focus", focus: focus("w1:t3", 9, { connection_generation: 5 }) });
    state = event(state, { type: "tab_focus", focus: focus("w1:t3", 9, { session: "hd007t-other" }) });
    expect(pressed(state)).toEqual(["w1:t2"]);
    expect(pressed(connected({ tab_focus: focus("w1:t3", 7, { session: "hd007t-other" }) }))).toEqual(["w1:t1"]);
    expect(pressed(connected({ tab_focus: focus("w1:t3", 7, { boot_id: "boot-local-5" }) }))).toEqual(["w1:t1"]);
  });

  // AC-027-01. Would catch: the bar keeping a confirmation for a tab the engine no longer lists
  // (closed in another client) instead of following the focus the engine reports in `tab.list`.
  it("drops the confirmation of a tab the reconciled list no longer has", () => {
    const state = event(connected(), { type: "tabs", tabs: tabs("w1:t3").filter((t) => t.tab_id !== "w1:t2") });
    expect(pressed(state)).toEqual(["w1:t3"]);
  });

  // Would catch: a confirmed focus surviving the end of its connection or a reboot.
  it("clears the confirmation when the connection ends or the identity changes", () => {
    const ended = event(connected(), { type: "state", state: "disconnected", error: null });
    expect(ended.tabFocus).toBeNull();
    const rebooted = event(connected(), { type: "identity", identity: { ...ssh, boot_id: "boot-ssh-9" } });
    expect(rebooted.tabFocus).toBeNull();
    const again = reduce(ended, { type: "connect_started" });
    expect(again.tabFocus).toBeNull();
  });

  // Would catch: the connect overview (captured earlier) replacing a newer channel confirmation.
  it("keeps a newer channel confirmation over the connect overview", () => {
    let state = reduce(initialState(), { type: "connect_started" });
    state = event(state, { type: "tab_focus", focus: focus("w1:t3", 8) });
    state = reduce(state, { type: "connected", overview: overview() });
    expect(pressed(state)).toEqual(["w1:t3"]);
  });

  // Would catch: breaking the standalone 004 panel, which has no connection confirmation.
  it("keeps the API flag when no connection confirmation exists", () => {
    expect(pressed(connected({ tab_focus: null }))).toEqual(["w1:t1"]);
    const { tab_focus: _omitted, ...standalone } = overview();
    const state = reduce(reduce(initialState(), { type: "connect_started" }), { type: "connected", overview: standalone });
    expect(pressed(state)).toEqual(["w1:t1"]);
  });
});
