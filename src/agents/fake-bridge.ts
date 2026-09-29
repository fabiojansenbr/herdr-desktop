// In-memory AgentsBridge for the isolated preview and the controller tests. Records every
// command with the exact argument object the Tauri bridge would send. With `simulate` it also
// plays a scripted engine (start → idle, prompt → working → blocked, "y" typed in the blocked
// pane → idle) so the preview shows every state; tests keep it off and emit events explicitly.
// It is not the backend: identity checks and topology belong to agent_commands.rs.

import type { AgentsBridge } from "./bridge";
import type { AgentDto, AgentsEvent, AgentStatus, Capabilities, Overview, QualifiedTarget, RuntimeError, TabDto, Topology } from "./types";

export interface RecordedCall {
  command: string;
  args: Record<string, unknown>;
}

export interface FakeAgentsOptions {
  bootId: string;
  generation: number;
  session: string;
  endpoint?: string;
  kinds?: string[];
  /** [pane, status] pairs; default: one idle agent in w1:p1. */
  agents?: [string, AgentStatus][];
  /** Kind of the seeded agents (default pi). */
  agentKind?: string;
  /** Engine `tab.list` label of the single seeded tab. */
  tabLabel?: string;
  tabNumber?: number;
  paneCount?: number;
  /** Full `tab.list` fixture (spec 025: more than one workspace); overrides the single tab. */
  tabs?: TabDto[];
  /** Manual pane names the engine reports in `pane.list` (spec 028), per pane. */
  labels?: Record<string, string>;
  /** Capability names reported as absent. */
  missing?: (keyof Capabilities)[];
  simulate?: boolean;
}

export interface FakeAgentsBridge extends AgentsBridge {
  calls: RecordedCall[];
  emit(event: AgentsEvent): void;
  agent(paneId: string, status: AgentStatus): AgentDto;
  topology(focusedPaneId: string, ratio: number): Topology;
  failPrompt(paneId: string, error: RuntimeError, unknown: boolean): void;
  clearPromptFailure(paneId: string): void;
  /** Replaces the `tab.list` fixture every `overview` returns (spec 027). */
  setTabs(tabs: TabDto[]): void;
  /** Makes `tab.focus` of `tabId` fail like an engine that no longer has it (spec 027). */
  failFocusTab(tabId: string, error: RuntimeError): void;
}

const FULL: Capabilities = {
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
  close_tab: true,
  rename_tab: true,
  zoom: true,
  rename_pane: true,
  swap: true,
  input_set: true,
};

export function createFakeAgentsBridge(options: FakeAgentsOptions): FakeAgentsBridge {
  const endpoint = options.endpoint ?? "local";
  const calls: RecordedCall[] = [];
  const failures = new Map<string, { error: RuntimeError; unknown: boolean }>();
  const capabilities: Capabilities = { ...FULL };
  for (const name of options.missing ?? []) capabilities[name] = false;
  const kinds = options.kinds ?? ["pi", "claude"];
  let listener: (event: AgentsEvent) => void = () => {};
  let panes = ["w1:p1", "w1:p2"];
  let focused = "w1:p1";
  let ratio = 0.5;

  const makeAgent = (paneId: string, status: AgentStatus, name?: string, kind?: string): AgentDto => ({
    pane_id: paneId,
    workspace_id: paneId.split(":")[0] ?? "w1",
    tab_id: `${paneId.split(":")[0] ?? "w1"}:t1`,
    name: name ?? `agente-${paneId}`,
    kind: kind ?? options.agentKind ?? "pi",
    status,
    launch_pending: false,
    ready: true,
    focused: paneId === focused,
  });
  let agents: AgentDto[] = (options.agents ?? [["w1:p1", "idle"]]).map(([pane, status]) => makeAgent(pane, status));
  let tabs: TabDto[] = options.tabs ?? [
    {
      tab_id: "w1:t1",
      workspace_id: "w1",
      label: options.tabLabel ?? "principal",
      number: options.tabNumber ?? 1,
      pane_count: options.paneCount ?? 1,
      focused: true,
    },
  ];
  const tabFailures = new Map<string, RuntimeError>();

  const makeTopology = (focusedPaneId: string, r: number): Topology => {
    const width = 120;
    const height = 40;
    if (panes.length === 1) {
      return { revision: 1, width, height, focused_pane_id: focusedPaneId, panes: [{ pane_id: panes[0]!, x: 0, y: 0, width, height, focused: true, ...paneLabel(panes[0]!) }], splits: [] };
    }
    const first = Math.round(width * r);
    return {
      revision: 1,
      width,
      height,
      focused_pane_id: focusedPaneId,
      panes: panes.slice(0, 2).map((pane, i) => ({
        pane_id: pane,
        x: i === 0 ? 0 : first + 1,
        y: 0,
        width: i === 0 ? first : width - first - 1,
        height,
        focused: pane === focusedPaneId,
        ...paneLabel(pane),
      })),
      splits: [{ path: [], direction: "right", pos: first, x: 0, y: 0, width, height, ratio: first / width }],
    };
  };

  const paneLabel = (pane: string) => (options.labels?.[pane] ? { label: options.labels[pane] } : {});

  const record = (command: string, args: Record<string, unknown>) => calls.push({ command, args: structuredClone(args) });
  const later = (fn: () => void, ms: number) => {
    if (options.simulate) setTimeout(fn, ms);
  };
  const emitAgents = () => listener({ type: "agents", agents: structuredClone(agents) });
  const setStatus = (paneId: string, status: AgentStatus) => {
    agents = agents.map((a) => (a.pane_id === paneId ? { ...a, status, launch_pending: false, ready: true } : a));
    emitAgents();
  };
  const refuse = (target: QualifiedTarget): RuntimeError | null =>
    target.boot_id !== options.bootId || target.endpoint !== endpoint
      ? { code: "target_boot_stale", message: "o servidor reiniciou; selecione o alvo novamente", retryable: false, endpoint }
      : null;

  const overview = (): Overview => ({
    state: "live",
    session: options.session,
    identity: { endpoint, session: options.session, connection_generation: options.generation, boot_id: options.bootId },
    server_version: "fake",
    capabilities,
    kinds: capabilities.start_agent ? kinds : [],
    agents: structuredClone(agents),
    tabs: structuredClone(tabs),
    topology: makeTopology(focused, ratio),
    error: null,
  });

  return {
    calls,
    emit: (event) => listener(event),
    agent: (paneId, status) => makeAgent(paneId, status),
    topology: (focusedPaneId, r) => makeTopology(focusedPaneId, r),
    failPrompt: (paneId, error, unknown) => failures.set(paneId, { error, unknown }),
    clearPromptFailure: (paneId) => failures.delete(paneId),
    setTabs: (next) => {
      tabs = structuredClone(next);
    },
    failFocusTab: (tabId, error) => tabFailures.set(tabId, error),
    async connect(_geometry, onEvent) {
      record("agents_connect", {});
      listener = onEvent;
      return overview();
    },
    async overview() {
      // The real bridge always asks for the reconciliation read (spec 027).
      record("agents_overview", { reconcile: true });
      return overview();
    },
    async detach() {
      record("agents_detach", {});
      return { ...overview(), state: "disconnected" };
    },
    async startAgent(target, kind, name, autonomous) {
      // The autonomy is recorded as the backend receives it; the flags themselves are its table.
      record("agent_start", { target, kind, name, autonomous: autonomous ?? false });
      const refused = refuse(target);
      if (refused) throw refused;
      const agent = { ...makeAgent(target.pane_id, "unknown", name, kind), launch_pending: true, ready: false };
      agents = [...agents.filter((a) => a.pane_id !== target.pane_id), agent];
      later(() => setStatus(target.pane_id, "idle"), 400);
      return structuredClone(agent);
    },
    async prompt(target, text, resendAfterUnknown) {
      record("agent_prompt", { target, text, resendAfterUnknown });
      const failure = failures.get(target.pane_id);
      if (failure?.unknown) return { outcome: "unknown", error: failure.error };
      if (failure) throw failure.error;
      later(() => setStatus(target.pane_id, "working"), 100);
      later(() => setStatus(target.pane_id, "blocked"), 1500);
      return { outcome: "sent", agent: structuredClone(agents.find((a) => a.pane_id === target.pane_id) ?? makeAgent(target.pane_id, "idle")) };
    },
    async openAttention(target) {
      record("agent_open_attention", { target });
      later(() => {
        focused = target.pane_id;
        listener({ type: "topology", topology: makeTopology(focused, ratio) });
      }, 50);
    },
    async split(target, direction) {
      record("pane_split", { target, direction });
      // Like the engine (spec 075): the created pane is answered at once, its topology arrives later.
      const created = panes.length < 2 ? `w1:p${panes.length + 1}` : null;
      later(() => {
        if (created) panes = [...panes, created];
        listener({ type: "topology", topology: makeTopology(focused, ratio) });
      }, 50);
      return created;
    },
    async focusPane(target) {
      record("pane_focus", { target });
      later(() => {
        focused = target.pane_id;
        listener({ type: "topology", topology: makeTopology(focused, ratio) });
      }, 50);
    },
    async setSplitRatio(target, path, r) {
      record("pane_set_split_ratio", { target, path, ratio: r });
      later(() => {
        ratio = r;
        listener({ type: "topology", topology: makeTopology(focused, ratio) });
      }, 50);
    },
    async input(target, events) {
      record("pane_input", { target, events });
      const blocked = agents.find((a) => a.pane_id === target.pane_id && a.status === "blocked");
      if (blocked && events.some((e) => e.kind === "text" && e.text === "y")) later(() => setStatus(target.pane_id, "idle"), 200);
    },
    async createTab(target) {
      record("tab_create", { target });
    },
    async focusTab(target, tabId) {
      record("tab_focus", { target, tabId });
      const failure = tabFailures.get(tabId);
      if (failure) throw failure;
    },
    async closeTab(target, tabId) {
      record("tab_close", { target, tabId });
    },
    async renameTab(target, tabId, label) {
      record("tab_rename", { target, tabId, label });
    },
    async zoomPane(target, mode) {
      record("pane_zoom", { target, mode });
      return { pane_id: target.pane_id, zoomed: mode !== "off" };
    },
    async renamePane(target, paneId, label) {
      record("pane_rename", { target, paneId, label });
    },
    async swapPane(target, paneId) {
      record("pane_swap", { target, paneId });
    },
    async setPaneRightClick(target, paneId, passthrough) {
      record("pane_input_set", { target, paneId, passthrough });
    },
    async closePane(target) {
      record("pane_close", { target });
      later(() => {
        panes = panes.filter((p) => p !== target.pane_id);
        if (panes.length === 0) panes = ["w1:p1"];
        if (!panes.includes(focused)) focused = panes[0]!;
        listener({ type: "topology", topology: makeTopology(focused, ratio) });
      }, 50);
    },
  };
}
