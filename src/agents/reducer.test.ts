// Spec 004 — agents reducer and view model (pure; no bridge).
import { describe, expect, it } from "vitest";
import { initialState, reduce, targetFor, viewModel, type AgentsState } from "./reducer";
import type { AgentDto, Capabilities, Overview, Topology } from "./types";

const caps: Capabilities = {
  list_agents: true,
  start_agent: true,
  send_prompt: true,
  open_attention: true,
  split: true,
  focus: true,
  split_ratio: true,
  input: true,
  create_tab: true,
  focus_tab: true,
};

function agent(pane: string, status: string, extra: Partial<AgentDto> = {}): AgentDto {
  return {
    pane_id: pane,
    workspace_id: pane.split(":")[0]!,
    tab_id: `${pane.split(":")[0]}:t1`,
    name: `agente-${pane}`,
    kind: "pi",
    status: status as AgentDto["status"],
    launch_pending: false,
    ready: true,
    focused: false,
    ...extra,
  };
}

const topology: Topology = {
  revision: 4,
  width: 120,
  height: 40,
  focused_pane_id: "w1:p1",
  panes: [
    { pane_id: "w1:p1", x: 0, y: 0, width: 36, height: 40, focused: true },
    { pane_id: "w1:p2", x: 37, y: 0, width: 83, height: 40, focused: false },
  ],
  splits: [{ path: [], direction: "right", pos: 36, x: 0, y: 0, width: 120, height: 40, ratio: 0.3 }],
};

function overview(extra: Partial<Overview> = {}): Overview {
  return {
    state: "live",
    session: "hd004-alpha",
    identity: { endpoint: "local", session: "hd004-alpha", connection_generation: 3, boot_id: "boot-lumen" },
    server_version: "0.9.0",
    capabilities: caps,
    kinds: ["pi", "claude"],
    agents: [agent("w1:p1", "idle"), agent("w1:p2", "blocked")],
    tabs: [{ tab_id: "w1:t1", workspace_id: "w1", label: "1", focused: true }],
    topology,
    error: null,
    ...extra,
  };
}

function connected(extra: Partial<Overview> = {}): AgentsState {
  return reduce(reduce(initialState(), { type: "connect_started" }), { type: "connected", overview: overview(extra) });
}

describe("agents reducer", () => {
  // Would catch: an empty session rendering start controls enabled (or creating anything).
  it("shows onboarding and a disabled start when there is nothing to act on", () => {
    const view = viewModel(connected({ agents: [], kinds: [] }));
    expect(view.onboarding).toMatch(/Nenhum agente/);
    expect(view.start.enabled).toBe(false);
    expect(view.start.reason).toMatch(/tipo/);
  });

  // Would catch: geometry recomputed/reflowed by the GUI instead of the server's cells.
  it("places panes exactly where the server topology says", () => {
    const view = viewModel(connected());
    expect(view.panes.map((p) => [p.paneId, p.left, p.top, p.width, p.height, p.confirmed])).toEqual([
      ["w1:p1", 0, 0, 30, 100, true],
      ["w1:p2", (37 / 120) * 100, 0, (83 / 120) * 100, 100, false],
    ]);
    expect(view.focusedSplit?.ratio).toBe(0.3);
  });

  // Would catch: a blocked agent not surfaced for attention, or attention on other states.
  it("lists blocked agents for attention only", () => {
    const view = viewModel(connected());
    expect(view.attention.map((a) => a.paneId)).toEqual(["w1:p2"]);
    const rows = Object.fromEntries(view.agents.map((a) => [a.paneId, a]));
    expect(rows["w1:p2"]!.canPrompt).toBe(false);
    expect(rows["w1:p1"]!.canPrompt).toBe(false); // no text yet
    const typed = reduce(connected(), { type: "prompt_edit", paneId: "w1:p1", text: "oi" });
    expect(viewModel(typed).agents.find((a) => a.paneId === "w1:p1")!.canPrompt).toBe(true);
  });

  // Would catch: an unexpected status from an event shown as done.
  it("normalises event statuses and never shows unknown as done", () => {
    const state = reduce(connected(), { type: "event", event: { type: "agents", agents: [agent("w1:p1", "finished")] } });
    const row = viewModel(state).agents[0]!;
    expect(row.status).toBe("unknown");
    expect(row.label).not.toBe("Concluído");
  });

  // Would catch: a missing method disabling every control, or leaving its own button enabled.
  it("disables only the action whose method is absent", () => {
    let state = connected({ capabilities: { ...caps, send_prompt: false, split_ratio: false }, agents: [agent("w1:p1", "idle")] });
    state = reduce(state, { type: "prompt_edit", paneId: "w1:p1", text: "oi" });
    state = reduce(state, { type: "start_edit", field: "paneId", value: "w1:p2" });
    const view = viewModel(state);
    const p1 = view.agents.find((a) => a.paneId === "w1:p1")!;
    expect(p1.canPrompt).toBe(false);
    expect(p1.promptDisabledReason).toMatch(/não oferece/);
    expect(view.ratio.enabled).toBe(false);
    state = reduce(state, { type: "start_edit", field: "name", value: "revisor" });
    expect(viewModel(state).start.enabled).toBe(true);
    expect(view.split.enabled).toBe(true);
    expect(p1.canOpen).toBe(true);
  });

  // Would catch: an unknown outcome clearing the typed prompt or offering a silent resend.
  it("keeps the prompt and asks for an explicit resend after an unknown outcome", () => {
    let state = reduce(connected(), { type: "prompt_edit", paneId: "w1:p1", text: "Gerar relatório" });
    state = reduce(state, { type: "prompt_started", paneId: "w1:p1" });
    state = reduce(state, {
      type: "prompt_unknown",
      paneId: "w1:p1",
      error: { code: "timeout", message: "o endpoint não respondeu a tempo", retryable: true, endpoint: "local" },
    });
    const row = viewModel(state).agents.find((a) => a.paneId === "w1:p1")!;
    expect(state.prompts["w1:p1"]!.text).toBe("Gerar relatório");
    expect(row.promptLabel).toBe("Reenviar mesmo assim");
    expect(row.outcome).toMatch(/desconhecido/);
    state = reduce(state, { type: "prompt_sent", paneId: "w1:p1", agent: agent("w1:p1", "working") });
    expect(state.prompts["w1:p1"]!.text).toBe("");
    expect(viewModel(state).agents.find((a) => a.paneId === "w1:p1")!.promptLabel).toBe("Enviar prompt");
  });

  // Would catch: errors spilling from one agent to another or to the whole panel.
  it("keeps errors on the affected resource", () => {
    const error = { code: "server_unavailable", message: "API do Herdr indisponível", retryable: true, endpoint: "local" };
    const state = reduce(connected(), { type: "prompt_failed", paneId: "w1:p1", error });
    const view = viewModel(state);
    expect(view.agents.find((a) => a.paneId === "w1:p1")!.error).toBe("API do Herdr indisponível");
    expect(view.agents.find((a) => a.paneId === "w1:p2")!.error).toBeNull();
    expect(view.start.error).toBeNull();
    expect(view.connectionError).toBeNull();
  });

  // Would catch: targets built from a stale identity after the server rebooted.
  it("qualifies targets with the latest identity", () => {
    let state = connected();
    expect(targetFor(state, "w1:p2")).toEqual({
      endpoint: "local",
      session: "hd004-alpha",
      connection_generation: 3,
      boot_id: "boot-lumen",
      workspace_id: "w1",
      pane_id: "w1:p2",
    });
    state = reduce(state, {
      type: "event",
      event: { type: "identity", identity: { endpoint: "local", session: "hd004-alpha", connection_generation: 3, boot_id: "boot-nova" } },
    });
    expect(targetFor(state, "w1:p2")!.boot_id).toBe("boot-nova");
    expect(targetFor(initialState(), "w1:p2")).toBeNull();
  });

  // Spec 027. Would catch: a forwarded structural event changing the state on its own (the list
  // only changes when the reconciliation's `tabs` event lands).
  it("keeps the state on a forwarded structural event", () => {
    const state = reduce(initialState(), { type: "connected", overview: overview() });
    expect(reduce(state, { type: "event", event: { type: "structure", events: ["tab_closed", "workspace_closed"] } })).toBe(state);
  });

  // Would catch: channel events that arrived before the connect command resolved being
  // overwritten by the older overview (topology lost, input never enabled).
  it("keeps topology and agents events received before the connect result", () => {
    let state = reduce(initialState(), { type: "connect_started" });
    state = reduce(state, { type: "event", event: { type: "topology", topology } });
    state = reduce(state, { type: "event", event: { type: "agents", agents: [agent("w1:p1", "working")] } });
    state = reduce(state, { type: "connected", overview: overview({ topology: null, agents: [agent("w1:p1", "idle")] }) });
    expect(state.topology).toEqual(topology);
    expect(state.agents.map((a) => a.status)).toEqual(["working"]);
    expect(viewModel(state).confirmed).toBe("w1:p1");
  });
});
