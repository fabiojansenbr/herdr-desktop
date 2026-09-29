// Spec 014 — what the agents panel of the design shows, derived from engine data only.
//
// The engine owns every agent state, the terminal title, the state change sequence and the
// detection snapshot line the backend read once when the agent asked for the user. The only
// thing this client owns is *when it first saw* each state change: the engine publishes no
// timestamp, so a transition it never observed has no time ("—"), as after reconnecting.
// Nothing here interprets the snapshot text, decides a state or answers an agent.

import { normalizeStatus, statusPresentation } from "../../agents/status";
import { formatRelative, t } from "../../i18n/index.svelte";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, TabDto } from "../../agents/types";
import type { ProjectsSnapshot } from "../../projects/types";

/** Attention cards shown at once in the 256 px column; the rest is reported as "+N". */
export const ATTENTION_LIMIT = 5;
/** Shown when the session has no agent at all. */
export function emptyText(): string {
  return t("agents.panel.empty");
}
/** Badge of a list that is not live (the host is offline and this is the last one seen). */
export function cacheText(): string {
  return t("agents.cache");
}
/** Age of a transition this client never saw. */
export const UNOBSERVED_TIME = "—";

export interface Counters {
  /** Engine state working. */
  active: number;
  /** Engine state blocked (waiting for the user). */
  waiting: number;
  /** Engine states idle and done. */
  idle: number;
  /** Anything the engine did not publish as one of the four states; never counted as done. */
  unknown: number;
}

export interface ProjectTab {
  project: string;
  tab: string;
  label: string;
}

export interface PanelAgent {
  paneId: string;
  name: string;
  kind: string;
  status: AgentStatus;
  label: string;
  icon: string;
  tone: string;
  /** First letter of the engine's agent kind (the design's avatar). */
  avatar: string;
  project: string;
  tab: string;
  /** `projeto › tab`. */
  path: string;
  /** Engine's terminal title of the pane; null when the engine published none. */
  summary: string | null;
  /** Last line of the engine's detection snapshot of this transition; null when absent. */
  lastLine: string | null;
  /** Epoch ms when this client first saw the current state change; null when unobserved. */
  since: number | null;
  /** Relative time of `since`. */
  time: string;
  canFocus: boolean;
}

export interface ActivityLedger {
  /** Records the sight of one engine agent list at `now` (epoch ms). */
  observe(agents: AgentDto[], now: number): void;
  /** When this client first saw the agent's current state change; null when never observed. */
  since(paneId: string): number | null;
}

export interface PanelContext {
  ledger: ActivityLedger;
  now: number;
  snapshot: ProjectsSnapshot | null;
  tabs: TabDto[];
  /** The host is live: cards are current and focus may be requested. */
  connected?: boolean;
  /** The engine announced pane.focus for this connection. */
  canFocus?: boolean;
}

export function counters(agents: AgentDto[]): Counters {
  const tally = { active: 0, waiting: 0, idle: 0, unknown: 0 };
  for (const agent of agents) {
    switch (normalizeStatus(agent.status)) {
      case "working":
        tally.active += 1;
        break;
      case "blocked":
        tally.waiting += 1;
        break;
      case "idle":
      case "done":
        tally.idle += 1;
        break;
      default:
        tally.unknown += 1;
    }
  }
  return tally;
}

/** Project bound to the agent's workspace and the engine label of its tab (ids as fallback). */
export function projectTabLabel(agent: AgentDto, snapshot: ProjectsSnapshot | null, tabs: TabDto[]): ProjectTab {
  const bound = snapshot?.projects.find((p) => p.binding?.workspace_id === agent.workspace_id);
  const project = bound?.label ?? agent.workspace_id;
  const tabLabel = tabs.find((t) => t.tab_id === agent.tab_id)?.label;
  const tab = tabLabel && tabLabel.trim() !== "" ? tabLabel : agent.tab_id;
  return { project, tab, label: `${project} › ${tab}` };
}

/** Age of an observed transition; "—" when this client never saw the state change. */
export function relativeTime(since: number | null, now: number): string {
  return since === null ? UNOBSERVED_TIME : formatRelative(now - since);
}

export function createActivityLedger(): ActivityLedger {
  const seen = new Map<string, { key: string; at: number }>();
  return {
    observe(agents, now) {
      const live = new Set(agents.map((a) => a.pane_id));
      for (const paneId of Array.from(seen.keys())) {
        if (!live.has(paneId)) seen.delete(paneId);
      }
      for (const agent of agents) {
        // Without a sequence from the engine, the published state is the change key: a server
        // that omits it still ages its cards on every state change.
        const key = `${agent.state_change_seq ?? "?"}:${normalizeStatus(agent.status)}`;
        const previous = seen.get(agent.pane_id);
        if (!previous || previous.key !== key) seen.set(agent.pane_id, { key, at: now });
      }
    },
    since: (paneId) => seen.get(paneId)?.at ?? null,
  };
}

function panelAgent(agent: AgentDto, ctx: PanelContext): PanelAgent {
  const status = normalizeStatus(agent.status);
  const presentation = statusPresentation(status);
  const { project, tab, label } = projectTabLabel(agent, ctx.snapshot, ctx.tabs);
  const kind = agent.kind ?? "";
  const since = ctx.ledger.since(agent.pane_id);
  const line = agent.detection_last_line?.trim();
  const title = agent.terminal_title?.trim();
  return {
    paneId: agent.pane_id,
    name: agent.name ?? agent.pane_id,
    kind,
    status,
    label: presentation.label,
    icon: presentation.icon,
    tone: presentation.tone,
    avatar: (kind || agent.name || agent.pane_id).trim().slice(0, 1).toLowerCase(),
    project,
    tab,
    path: label,
    summary: title && title !== "" ? title : null,
    lastLine: line && line !== "" ? line : null,
    since,
    time: relativeTime(since, ctx.now),
    canFocus: (ctx.connected ?? true) && (ctx.canFocus ?? true),
  };
}

/** Newest observed transition first; an unobserved transition goes last, by pane id. */
function byActivity(a: PanelAgent, b: PanelAgent): number {
  if (a.since === b.since) return a.paneId.localeCompare(b.paneId);
  if (a.since === null) return 1;
  if (b.since === null) return -1;
  return b.since - a.since;
}

export interface AttentionQueue {
  cards: PanelAgent[];
  /** Waiting agents beyond `ATTENTION_LIMIT`. */
  overflow: number;
}

export function attentionQueue(agents: AgentDto[], ctx: PanelContext): AttentionQueue {
  const waiting = agents
    .filter((a) => normalizeStatus(a.status) === "blocked")
    .map((a) => panelAgent(a, ctx))
    .sort(byActivity);
  return { cards: waiting.slice(0, ATTENTION_LIMIT), overflow: Math.max(0, waiting.length - ATTENTION_LIMIT) };
}

export function runningRows(agents: AgentDto[], ctx: PanelContext): PanelAgent[] {
  return agents.map((a) => panelAgent(a, ctx)).sort(byActivity);
}

export interface PanelView {
  counters: Counters;
  attention: AttentionQueue;
  running: PanelAgent[];
  empty: boolean;
  emptyText: string;
  /** The panel shows the last known list of a host that is not live. */
  cache: boolean;
  cacheText: string;
}

export function panelView(state: AgentsState | null, ctx: Omit<PanelContext, "tabs"> & { tabs?: TabDto[] }): PanelView {
  const agents = state?.agents ?? [];
  const connected = (ctx.connected ?? true) && state?.phase === "connected";
  const full: PanelContext = {
    ...ctx,
    tabs: ctx.tabs ?? state?.tabs ?? [],
    connected,
    canFocus: (ctx.canFocus ?? state?.capabilities?.focus ?? false) && connected,
  };
  return {
    counters: counters(agents),
    attention: attentionQueue(agents, full),
    running: runningRows(agents, full),
    empty: agents.length === 0,
    emptyText: emptyText(),
    cache: !connected,
    cacheText: cacheText(),
  };
}
