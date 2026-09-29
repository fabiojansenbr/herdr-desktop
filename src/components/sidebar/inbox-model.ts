// Spec 043 (PRD nova-lateral) — "Precisa de você": every agent of every host that is waiting for
// the user or has finished, in one box at the top of the sidebar.
//
// What counts as needing the user is the 041 split (`sidebarStatus`), which keeps `done` apart
// from idle — without it a finished agent would be invisible (the PRD's own risk). The state text
// is the engine presentation of spec 014 (`statusPresentation`), the avatar the one the nested
// tabs use (042), and the time the client-owned ledger of spec 014: the engine publishes no
// timestamp, so a transition this client never saw has no time ("—").
//
// Only the host the window is attached to publishes `detection_last_line` (spec 014); every other
// host is read from the snapshot it carries (`HostDto.agents`/`panes`), never from the live list.
import { normalizeStatus, statusPresentation } from "../../agents/status";
import type { AgentDto, AgentStatus } from "../../agents/types";
import type { HostDto } from "../../connections/types";
import { t } from "../../i18n/index.svelte";
import type { ActivityLedger } from "../agents/panel";
import { sidebarStatus } from "./sidebar-model";
import { rowTime, tabAvatar } from "./tab-rows";

/** Title of the box, in the user's language (spec 068); a function, so it follows a change. */
export function inboxTitle(): string {
  return t("sidebar.inbox.title");
}
/** The times age without the engine sending anything; one timer for the whole box (as in 014). */
export const INBOX_TICK_MS = 30_000;

/** Waiting for an answer in the terminal, or finished: the two reasons an agent is listed. */
export type InboxKind = "blocked" | "done";

/** One agent of one host, normalized from the live list or from the host's own snapshot. */
export interface InboxAgent {
  /** Unique across hosts: two hosts can publish the same pane id. */
  readonly id: string;
  readonly endpoint: string;
  readonly hostLabel: string;
  readonly paneId: string;
  readonly tabId: string;
  readonly workspaceId: string;
  readonly workspaceLabel: string;
  readonly kind: string;
  readonly status: AgentStatus;
  readonly stateChangeSeq?: number;
  /** Detection line of the transition, published only by the attached host; null otherwise. */
  readonly lastLine: string | null;
  readonly selectedHost: boolean;
}

export interface InboxItem {
  readonly id: string;
  readonly endpoint: string;
  readonly hostLabel: string;
  readonly workspaceId: string;
  readonly tabId: string;
  readonly paneId: string;
  readonly kind: InboxKind;
  readonly avatar: string;
  /** `codex · api`: the agent and the workspace it is in. */
  readonly text: string;
  /** Workspace alone (spec 051): the line shows it, the agent stays in the avatar and the label. */
  readonly workspace: string;
  /**
   * The engine's detection line, when this host published one for a blocked agent; null
   * otherwise (spec 051: without it the state is only the icon, never a second line).
   */
  readonly detection: string | null;
  /** The engine's detection line when there is one, else what the state means. */
  readonly detail: string;
  readonly since: number | null;
  readonly time: string;
  /** `state_change_seq` + status: what makes a visited item stay away (AC-043-03). */
  readonly key: string;
  readonly selectedHost: boolean;
  readonly label: string;
}

export interface InboxAgentsInput {
  readonly hosts: readonly HostDto[];
  readonly selectedEndpoint: string | null;
  /** Live agent list of the attached host (the only one with `detection_last_line`). */
  readonly live?: readonly AgentDto[];
  /**
   * Endpoint the live list belongs to. Defaults to the selected one; during a host switch the
   * window still holds the previous host's list, and attributing it to the new host would show
   * the wrong agents — so the caller passes the identity the list was observed on.
   */
  readonly liveEndpoint?: string | null;
}

/** State key of one agent, the same shape the activity ledger keys its transitions with. */
export function agentStateKey(agent: { status: AgentStatus; stateChangeSeq?: number }): string {
  return `${agent.stateChangeSeq ?? "?"}:${normalizeStatus(agent.status)}`;
}

function workspaceLabelOf(host: HostDto, workspaceId: string): string {
  return (host.workspaces ?? []).find((workspace) => workspace.workspace_id === workspaceId)?.label || workspaceId;
}

/** Agents one host publishes when the window is not attached to it: its own snapshot. */
function snapshotAgents(host: HostDto): readonly {
  pane_id: string;
  workspace_id: string;
  tab_id: string;
  kind: string;
  status: AgentStatus;
}[] {
  if (host.agents && host.agents.length > 0) {
    return host.agents.map((agent) => ({
      pane_id: agent.pane_id,
      workspace_id: agent.workspace_id,
      tab_id: agent.tab_id,
      kind: agent.agent ?? agent.display_agent ?? agent.name ?? agent.pane_id,
      status: normalizeStatus(agent.agent_status),
    }));
  }
  return (host.panes ?? [])
    .filter((pane) => pane.agent)
    .map((pane) => ({
      pane_id: pane.pane_id,
      workspace_id: pane.workspace_id,
      tab_id: pane.tab_id ?? "",
      kind: pane.agent!,
      status: normalizeStatus(pane.agent_status),
    }));
}

/**
 * Every agent of every host as one list (all states: the ledger dates the transitions it sees,
 * not only the ones the box shows). Pane ids are qualified by endpoint, so two hosts that publish
 * the same pane id never share a row or a date.
 */
export function inboxAgents({ hosts, selectedEndpoint, live = [], liveEndpoint }: InboxAgentsInput): InboxAgent[] {
  const attachedTo = liveEndpoint === undefined ? selectedEndpoint : liveEndpoint;
  const result: InboxAgent[] = [];
  for (const host of hosts) {
    const attached = attachedTo !== null && host.endpoint === attachedTo && live.length > 0;
    const source = attached
      ? live.map((agent) => ({
          pane_id: agent.pane_id,
          workspace_id: agent.workspace_id,
          tab_id: agent.tab_id,
          kind: agent.kind ?? agent.name ?? agent.pane_id,
          status: normalizeStatus(agent.status),
          stateChangeSeq: agent.state_change_seq,
          lastLine: agent.detection_last_line?.trim() || null,
        }))
      : snapshotAgents(host).map((agent) => ({ ...agent, stateChangeSeq: undefined, lastLine: null }));
    for (const agent of source) {
      result.push({
        id: `${host.endpoint}::${agent.pane_id}`,
        endpoint: host.endpoint,
        hostLabel: host.label || host.endpoint,
        paneId: agent.pane_id,
        tabId: agent.tab_id,
        workspaceId: agent.workspace_id,
        workspaceLabel: workspaceLabelOf(host, agent.workspace_id),
        kind: agent.kind,
        status: agent.status,
        stateChangeSeq: agent.stateChangeSeq,
        lastLine: agent.lastLine,
        selectedHost: host.endpoint === selectedEndpoint,
      });
    }
  }
  return result;
}

/** The list as the activity ledger reads it: one `AgentDto` per agent, keyed by the qualified id. */
export function ledgerAgents(agents: readonly InboxAgent[]): AgentDto[] {
  return agents.map((agent) => ({
    pane_id: agent.id,
    workspace_id: agent.workspaceId,
    tab_id: agent.tabId,
    name: agent.kind,
    kind: agent.kind,
    status: agent.status,
    launch_pending: false,
    ready: true,
    focused: false,
    state_change_seq: agent.stateChangeSeq,
  }));
}

/**
 * Items of the box (AC-043-01): the blocked ones first, then the finished ones, each group with
 * the newest observed transition first (an unobserved one last, by id, as the 014 panel orders).
 */
export function inboxItems(
  agents: readonly InboxAgent[],
  { ledger, now }: { ledger: ActivityLedger; now: number },
): InboxItem[] {
  const items: InboxItem[] = [];
  for (const agent of agents) {
    // The 041 split is what makes a finished agent visible at all: `mapAgentStatus` folds it into
    // idle, `sidebarStatus` does not.
    const summary = sidebarStatus([agent.status]).kind;
    if (summary !== "waiting" && summary !== "done") continue;
    const kind: InboxKind = summary === "waiting" ? "blocked" : "done";
    const state = statusPresentation(agent.status);
    const detection = kind === "blocked" && agent.selectedHost && agent.lastLine ? agent.lastLine : null;
    const detail = detection ?? state.label;
    const since = ledger.since(agent.id);
    const time = rowTime(since, now);
    const text = `${agent.kind} · ${agent.workspaceLabel}`;
    items.push({
      id: agent.id,
      endpoint: agent.endpoint,
      hostLabel: agent.hostLabel,
      workspaceId: agent.workspaceId,
      tabId: agent.tabId,
      paneId: agent.paneId,
      kind,
      avatar: tabAvatar(agent.kind),
      text,
      workspace: agent.workspaceLabel,
      detection,
      detail,
      since,
      time,
      key: agentStateKey(agent),
      selectedHost: agent.selectedHost,
      label: [text, agent.hostLabel, state.label, detail === state.label ? null : detail].filter(Boolean).join(", "),
    });
  }
  const rank = (item: InboxItem) => (item.kind === "blocked" ? 0 : 1);
  return items.sort((a, b) => {
    if (rank(a) !== rank(b)) return rank(a) - rank(b);
    if (a.since !== b.since) {
      if (a.since === null) return 1;
      if (b.since === null) return -1;
      return b.since - a.since;
    }
    return a.id.localeCompare(b.id);
  });
}

export interface InboxVisit {
  /** Host the window is attached to right now; null while none is selected. */
  readonly endpoint: string | null;
  /** Tab focused on that host; null when unknown. */
  readonly tabId: string | null;
  readonly items: readonly InboxItem[];
}

export interface InboxSeen {
  /** Records that the user is looking at `tabId` of `endpoint`: its finished items leave. */
  visit(input: InboxVisit): void;
  /** The user already saw this exact state of this agent (AC-043-03). */
  dismissed(item: InboxItem): boolean;
}

/**
 * What the user already looked at. A finished item leaves when its tab is visited and stays away
 * while the agent keeps the same state (`state_change_seq` + status); the next transition brings
 * it back. A blocked item is never dismissed by a visit: it still needs an answer in the terminal,
 * and it leaves the box only when the engine stops publishing it as blocked.
 */
export function createInboxSeen(): InboxSeen {
  const seen = new Map<string, string>();
  return {
    visit({ endpoint, tabId, items }) {
      if (!endpoint || !tabId) return;
      for (const item of items) {
        if (item.kind !== "done") continue;
        if (item.endpoint !== endpoint || item.tabId !== tabId) continue;
        seen.set(item.id, item.key);
      }
    },
    dismissed: (item) => item.kind === "done" && seen.get(item.id) === item.key,
  };
}
