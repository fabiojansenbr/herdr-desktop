// Agents panel state (spec 004). Pure: server data replaces local copies; errors stay on the
// resource that failed; typed prompts survive failures and unknown outcomes. Nothing here
// decides an agent's state or answers an approval.

import { t } from "../i18n/index.svelte";
import { normalizeStatus, statusPresentation } from "./status";
import type {
  AgentDto,
  AgentsEvent,
  Capabilities,
  LiveIdentity,
  Overview,
  QualifiedTarget,
  RuntimeError,
  SplitBox,
  TabDto,
  TabFocus,
  Topology,
} from "./types";

export interface PromptState {
  text: string;
  sending: boolean;
  outcome: "sent" | "unknown" | null;
  error: RuntimeError | null;
}

export interface AgentsState {
  phase: "idle" | "connecting" | "connected" | "failed" | "disconnected";
  session: string | null;
  serverVersion: string | null;
  identity: LiveIdentity | null;
  capabilities: Capabilities | null;
  kinds: string[];
  agents: AgentDto[];
  /** An `agents` event arrived during this connection (newer than the connect overview). */
  agentsFromEvents: boolean;
  /**
   * Connection on which the agent list (and its engine readiness) was observed; null while the
   * connect that the list belongs to has not confirmed its identity yet.
   */
  agentsIdentity: LiveIdentity | null;
  tabs: TabDto[];
  /** Tab focused by the attached connection (composed window); overrides the API global flag. */
  tabFocus: TabFocus | null;
  topology: Topology | null;
  connectionError: RuntimeError | null;
  streamError: RuntimeError | null;
  start: { paneId: string; kind: string; name: string; busy: boolean; error: RuntimeError | null };
  prompts: Record<string, PromptState>;
  attentionErrors: Record<string, RuntimeError>;
  pendingFocus: string | null;
  layoutBusy: boolean;
  layoutError: RuntimeError | null;
  notice: string | null;
  /**
   * pane_id → engine `zoomed` of the tab the last zoom of this window saw (spec 028). Feeds the
   * `Zoom` / `Desfazer zoom` label of the pane context menu; absent panes are not zoomed.
   */
  zoomedPanes: Record<string, boolean>;
}

export type AgentsAction =
  | { type: "connect_started" }
  | { type: "connected"; overview: Overview }
  | { type: "connect_failed"; error: RuntimeError }
  | { type: "event"; event: AgentsEvent }
  | { type: "start_edit"; field: "paneId" | "kind" | "name"; value: string }
  | { type: "start_started" }
  | { type: "start_succeeded"; agent: AgentDto }
  | { type: "start_failed"; error: RuntimeError }
  | { type: "prompt_edit"; paneId: string; text: string }
  | { type: "prompt_started"; paneId: string }
  | { type: "prompt_sent"; paneId: string; agent: AgentDto }
  | { type: "prompt_unknown"; paneId: string; error: RuntimeError }
  | { type: "prompt_failed"; paneId: string; error: RuntimeError }
  | { type: "attention_failed"; paneId: string; error: RuntimeError }
  | { type: "attention_requested"; paneId: string }
  | { type: "layout_started"; focusPaneId?: string }
  | { type: "layout_done" }
  | { type: "layout_failed"; error: RuntimeError }
  | { type: "pane_zoomed"; paneId: string; zoomed: boolean }
  | { type: "notice"; text: string | null };

export function initialState(): AgentsState {
  return {
    phase: "idle",
    session: null,
    serverVersion: null,
    identity: null,
    capabilities: null,
    kinds: [],
    agents: [],
    agentsFromEvents: false,
    agentsIdentity: null,
    tabs: [],
    tabFocus: null,
    topology: null,
    connectionError: null,
    streamError: null,
    start: { paneId: "", kind: "", name: "", busy: false, error: null },
    prompts: {},
    attentionErrors: {},
    pendingFocus: null,
    layoutBusy: false,
    layoutError: null,
    notice: null,
    zoomedPanes: {},
  };
}

function normalizeAgents(agents: AgentDto[]): AgentDto[] {
  return agents.map((a) => ({ ...a, status: normalizeStatus(a.status) }));
}

function upsert(agents: AgentDto[], agent: AgentDto): AgentDto[] {
  const next = normalizeAgents([agent])[0]!;
  const index = agents.findIndex((a) => a.pane_id === next.pane_id);
  if (index < 0) return [...agents, next];
  return agents.map((a, i) => (i === index ? next : a));
}

function sameConnection(focus: LiveIdentity, identity: LiveIdentity | null): boolean {
  return (
    identity !== null &&
    focus.endpoint === identity.endpoint &&
    focus.session === identity.session &&
    focus.boot_id === identity.boot_id &&
    focus.connection_generation === identity.connection_generation
  );
}

/** A confirmation of another connection is dropped; an older revision never replaces a newer one. */
function acceptFocus(current: TabFocus | null, next: TabFocus, identity: LiveIdentity | null): TabFocus | null {
  if (identity && !sameConnection(next, identity)) return current;
  if (current && sameConnection(current, next) && next.revision < current.revision) return current;
  return next;
}

function prompt(state: AgentsState, paneId: string): PromptState {
  return state.prompts[paneId] ?? { text: "", sending: false, outcome: null, error: null };
}

function withPrompt(state: AgentsState, paneId: string, patch: Partial<PromptState>): AgentsState {
  return { ...state, prompts: { ...state.prompts, [paneId]: { ...prompt(state, paneId), ...patch } } };
}

function withTopology(state: AgentsState, topology: Topology | null): AgentsState {
  const start = { ...state.start };
  if (topology && !topology.panes.some((p) => p.pane_id === start.paneId)) {
    start.paneId = topology.focused_pane_id ?? topology.panes[0]?.pane_id ?? "";
  }
  const pendingFocus = state.pendingFocus && topology?.focused_pane_id === state.pendingFocus ? null : state.pendingFocus;
  return { ...state, topology, start, pendingFocus, notice: pendingFocus ? state.notice : null };
}

export function reduce(state: AgentsState, action: AgentsAction): AgentsState {
  switch (action.type) {
    case "connect_started":
      return {
        ...state,
        phase: "connecting",
        connectionError: null,
        topology: null,
        tabFocus: null,
        agentsFromEvents: false,
        agentsIdentity: null,
        zoomedPanes: {},
      };
    case "connected": {
      const o = action.overview;
      const next: AgentsState = {
        ...state,
        phase: "connected",
        session: o.session,
        serverVersion: o.server_version,
        identity: o.identity,
        capabilities: o.capabilities,
        kinds: o.kinds,
        agents: state.agentsFromEvents ? state.agents : normalizeAgents(o.agents),
        // The overview and the events of this connect call both belong to its identity.
        agentsIdentity: o.identity,
        tabs: o.tabs,
        tabFocus: [state.tabFocus, o.tab_focus ?? null].reduce<TabFocus | null>(
          (kept, candidate) => (candidate && sameConnection(candidate, o.identity) ? acceptFocus(kept, candidate, o.identity) : kept),
          null,
        ),
        connectionError: o.error,
        start: { ...state.start, kind: o.kinds.includes(state.start.kind) ? state.start.kind : (o.kinds[0] ?? "") },
      };
      // Channel events can arrive before the connect command resolves; they are newer than
      // the overview snapshot and are kept.
      return withTopology(next, state.topology ?? o.topology);
    }
    case "connect_failed":
      return { ...state, phase: "failed", connectionError: action.error };
    case "event": {
      const e = action.event;
      switch (e.type) {
        case "identity":
          return {
            ...state,
            identity: e.identity,
            tabFocus: state.tabFocus && sameConnection(state.tabFocus, e.identity) ? state.tabFocus : null,
          };
        case "topology":
          return withTopology(state, e.topology);
        case "agents":
          return {
            ...state,
            agents: normalizeAgents(e.agents),
            agentsFromEvents: true,
            agentsIdentity: state.phase === "connecting" ? null : state.identity,
            streamError: null,
          };
        case "tabs":
          // A confirmation for a tab the engine no longer lists is stale: dropped, so the bar
          // follows the focus the reconciled list itself reports (spec 027).
          return {
            ...state,
            tabs: e.tabs,
            tabFocus: state.tabFocus && e.tabs.some((t) => t.tab_id === state.tabFocus!.tab_id) ? state.tabFocus : null,
          };
        case "tab_focus":
          return { ...state, tabFocus: acceptFocus(state.tabFocus, e.focus, state.identity) };
        case "structure":
          // Forwarded engine event: the reconciliation is the controller's, the state waits for
          // the resulting `tabs` event.
          return state;
        case "state":
          // The confirmed topology/focus belonged to the connection that ended: never kept.
          if (e.state === "disconnected") {
            // Engine readiness observed on that connection is no longer confirmed either.
            return {
              ...state,
              phase: "disconnected",
              connectionError: e.error,
              topology: null,
              tabFocus: null,
              pendingFocus: null,
              agentsIdentity: null,
              zoomedPanes: {},
            };
          }
          return { ...state, streamError: e.error };
      }
      return state;
    }
    case "start_edit":
      return { ...state, start: { ...state.start, [action.field]: action.value, error: null } };
    case "start_started":
      return { ...state, start: { ...state.start, busy: true, error: null } };
    case "start_succeeded":
      return { ...state, agents: upsert(state.agents, action.agent), start: { ...state.start, busy: false, name: "", error: null } };
    case "start_failed":
      return { ...state, start: { ...state.start, busy: false, error: action.error } };
    case "prompt_edit":
      return withPrompt(state, action.paneId, { text: action.text, error: null });
    case "prompt_started":
      return withPrompt(state, action.paneId, { sending: true, error: null });
    case "prompt_sent":
      return { ...withPrompt(state, action.paneId, { text: "", sending: false, outcome: "sent", error: null }), agents: upsert(state.agents, action.agent) };
    case "prompt_unknown":
      return withPrompt(state, action.paneId, { sending: false, outcome: "unknown", error: action.error });
    case "prompt_failed":
      return withPrompt(state, action.paneId, { sending: false, error: action.error });
    case "attention_requested": {
      const errors = { ...state.attentionErrors };
      delete errors[action.paneId];
      return { ...state, attentionErrors: errors, pendingFocus: action.paneId };
    }
    case "attention_failed":
      return { ...state, attentionErrors: { ...state.attentionErrors, [action.paneId]: action.error }, pendingFocus: null };
    case "layout_started": {
      const pendingFocus =
        action.focusPaneId && state.topology?.focused_pane_id !== action.focusPaneId ? action.focusPaneId : state.pendingFocus;
      return { ...state, layoutBusy: true, layoutError: null, pendingFocus };
    }
    case "layout_done":
      return { ...state, layoutBusy: false };
    case "layout_failed":
      return { ...state, layoutBusy: false, layoutError: action.error, pendingFocus: null };
    case "pane_zoomed": {
      const zoomedPanes = { ...state.zoomedPanes };
      if (action.zoomed) zoomedPanes[action.paneId] = true;
      else delete zoomedPanes[action.paneId];
      return { ...state, zoomedPanes };
    }
    case "notice":
      return { ...state, notice: action.text };
  }
}

// ---------------------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------------------

/** Qualified target on the latest identity, or null before the server identified itself. */
export function targetFor(state: AgentsState, paneId: string): QualifiedTarget | null {
  if (!state.identity || !paneId) return null;
  return {
    endpoint: state.identity.endpoint,
    session: state.identity.session,
    connection_generation: state.identity.connection_generation,
    boot_id: state.identity.boot_id,
    workspace_id: paneId.split(":")[0],
    pane_id: paneId,
  };
}

export function confirmedPane(state: AgentsState): string | null {
  return state.topology?.focused_pane_id ?? null;
}

/** Innermost split whose area contains the confirmed pane. */
export function splitOfFocusedPane(state: AgentsState): SplitBox | null {
  const topology = state.topology;
  const pane = topology?.panes.find((p) => p.pane_id === topology.focused_pane_id);
  if (!topology || !pane) return null;
  const containing = topology.splits.filter(
    (s) => pane.x >= s.x && pane.y >= s.y && pane.x + pane.width <= s.x + s.width && pane.y + pane.height <= s.y + s.height,
  );
  containing.sort((a, b) => b.path.length - a.path.length);
  return containing[0] ?? null;
}

const missing = (method: string) => t("agents.reason.missing", { method });

/**
 * Why a prompt cannot be sent to the agent of `paneId` now, or null. Readiness is the engine's
 * (`launch_pending`, then `interactive_ready` or an effective agent kind) as last observed on the
 * current connection; the engine's
 * `agent.prompt` stays the authority if it changed since. Shared by the view and the controller.
 */
export function promptDisabledReasonFor(state: AgentsState, paneId: string): string | null {
  const a = state.agents.find((candidate) => candidate.pane_id === paneId);
  const p = prompt(state, paneId);
  if (!state.capabilities?.send_prompt) return missing("agent.prompt");
  if (state.phase !== "connected") return t("agents.reason.disconnected");
  if (!a) return t("agents.reason.notFound");
  if (a.status === "blocked") return t("agents.reason.blocked");
  if (p.sending) return t("agents.reason.sending");
  if (!state.agentsIdentity || !sameConnection(state.agentsIdentity, state.identity)) {
    return t("agents.reason.awaitingAgent");
  }
  if (a.launch_pending) return t("agents.reason.launching");
  // `ready` (interactive_ready) is set only for a managed launch in its Active phase; a detected or
  // manually started agent is never marked ready, yet the engine's agent.prompt accepts it once
  // its kind is known. Without a kind reported by the engine there is nothing it would accept.
  if (!a.ready && !a.kind?.trim()) return t("agents.reason.awaitingReady");
  if (p.text.trim().length === 0) return t("agents.reason.emptyPrompt");
  return null;
}

/** API tab list with the pressed tab of the attached connection when it confirmed one. */
function pressedTabs(state: AgentsState): TabDto[] {
  const focus = state.tabFocus && sameConnection(state.tabFocus, state.identity) ? state.tabFocus : null;
  if (!focus) return state.tabs;
  return state.tabs.map((t) => ({ ...t, focused: t.tab_id === focus.tab_id }));
}

export function viewModel(state: AgentsState) {
  const caps = state.capabilities;
  const topology = state.topology;
  const confirmed = confirmedPane(state);
  const agents = state.agents.map((a) => {
    const p = prompt(state, a.pane_id);
    const presentation = statusPresentation(a.status);
    const promptDisabledReason = promptDisabledReasonFor(state, a.pane_id);
    return {
      paneId: a.pane_id,
      name: a.name ?? a.pane_id,
      kind: a.kind ?? t("agents.kind.unknown"),
      status: a.status,
      label: presentation.label,
      icon: presentation.icon,
      tone: presentation.tone,
      readiness: a.launch_pending ? t("agents.readiness.launching") : a.ready ? t("agents.readiness.ready") : "",
      promptText: p.text,
      canPrompt: promptDisabledReason === null,
      promptDisabledReason,
      promptLabel: p.outcome === "unknown" ? t("agents.prompt.resend") : t("agents.prompt.send"),
      outcome:
        p.outcome === "unknown"
          ? t("agents.prompt.unknown")
          : p.outcome === "sent"
            ? t("agents.prompt.sent")
            : null,
      canOpen: Boolean(caps?.open_attention),
      error: p.outcome === "unknown" ? null : (p.error?.message ?? state.attentionErrors[a.pane_id]?.message ?? null),
    };
  });
  const occupied = new Set(state.agents.map((a) => a.pane_id));
  let startReason: string | null = null;
  if (!caps?.start_agent) startReason = state.kinds.length === 0 ? t("agents.reason.noKinds") : missing("agent.start");
  else if (!state.start.kind) startReason = t("agents.reason.chooseKind");
  else if (!state.start.paneId) startReason = t("agents.reason.choosePane");
  else if (occupied.has(state.start.paneId)) startReason = t("agents.reason.paneOccupied");
  else if (!state.start.name.trim()) startReason = t("agents.reason.nameRequired");
  else if (state.start.busy) startReason = t("agents.reason.starting");
  const focusedSplit = splitOfFocusedPane(state);
  return {
    onboarding:
      state.phase === "connected" && state.agents.length === 0
        ? t("agents.onboarding")
        : null,
    agents,
    attention: agents.filter((a) => a.status === "blocked"),
    start: {
      enabled: startReason === null,
      reason: startReason,
      error: state.start.error?.message ?? null,
      panes: topology?.panes.map((p) => p.pane_id) ?? [],
    },
    panes: (topology?.panes ?? []).map((p) => ({
      paneId: p.pane_id,
      left: (p.x / (topology?.width || 1)) * 100,
      top: (p.y / (topology?.height || 1)) * 100,
      width: (p.width / (topology?.width || 1)) * 100,
      height: (p.height / (topology?.height || 1)) * 100,
      cells: { x: p.x, y: p.y, width: p.width, height: p.height },
      confirmed: p.pane_id === confirmed,
      pending: p.pane_id === state.pendingFocus,
      agent: agents.find((a) => a.paneId === p.pane_id) ?? null,
    })),
    focusedSplit,
    split: { enabled: Boolean(caps?.split && confirmed) },
    ratio: { enabled: Boolean(caps?.split_ratio && focusedSplit) },
    tabs: pressedTabs(state),
    canCreateTab: Boolean(caps?.create_tab && confirmed),
    canFocusTab: Boolean(caps?.focus_tab),
    connectionError: state.connectionError?.message ?? null,
    streamError: state.streamError?.message ?? null,
    layoutError: state.layoutError?.message ?? null,
    confirmed,
  };
}
