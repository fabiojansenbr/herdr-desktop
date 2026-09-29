// Spec 042 (PRD nova-lateral, P1/P2) — the nested tabs of the open workspace, as rows of the
// sidebar. Nothing is recomputed here that the center bar already computes: the title, the agent
// kind and the tab's glyph come from `workspaceTabs` (spec 026), so a tab reads the same in the
// bar and in the sidebar; the status is the 041 `sidebarStatus`, which keeps `done` apart from
// idle; and the time is the client-owned ledger of spec 014 — the engine publishes no timestamp,
// so a transition this client never saw has no time ("—").
//
// Spec 052 — the same rows serve any expanded workspace, not only the focused one: the attached
// host is read from the live connection and every other host from the snapshot the hub carries,
// and only the focused workspace of the selected host may mark a row as selected.
import type { AgentsState } from "../../agents/reducer";
import { normalizeStatus, statusLabel } from "../../agents/status";
import type { AgentDto, AgentStatus, PaneBox, TabDto } from "../../agents/types";
import type { HostDto } from "../../connections/types";
import { formatRelative } from "../../i18n/index.svelte";
import type { ActivityLedger } from "../agents/panel";
import { workspaceTabs } from "../center/model";
import { tabText } from "../center/tabs-narrow";
import { sidebarStatus, snapshotAgents, type SidebarStatusKind } from "./sidebar-model";

/** The times age without the engine sending anything; one timer for the whole list (as in 014). */
export const TAB_TIME_TICK_MS = 30_000;

/**
 * Age of an observed transition, in the user's language (spec 068): `formatRelative` of the 067
 * base, with the em dash the 014 panel already used when this client never saw the state change.
 */
export function rowTime(since: number | null, now: number): string {
  return since === null ? "—" : formatRelative(Math.max(0, now - since));
}

/** Avatar of a tab whose panes run no agent (the design's `$`). */
export const SHELL_AVATAR = "$";

/**
 * Letter of the type avatar (design `v2-01-workspace.png`): the engine kinds that share an
 * initial get the letter of the design, so `claude` and `codex` never collide in the column.
 */
export const TAB_AVATARS: Record<string, string> = {
  claude: "C",
  codex: "X",
};

/**
 * Text of each state, read from the one i18n table (spec 067) — the same words the center and the
 * palette show. The sidebar's `waiting` is the engine's `blocked`. Getters, not values: a language
 * change must reach every row without a reload.
 */
export const TAB_STATUS_TEXT: Record<SidebarStatusKind, string> = {
  get working() {
    return statusLabel("working");
  },
  get waiting() {
    return statusLabel("blocked");
  },
  get done() {
    return statusLabel("done");
  },
  get idle() {
    return statusLabel("idle");
  },
};

export interface TabRow {
  readonly tabId: string;
  /** Visible title, identical to the bar's (`tabText`): the engine label when the tab is named. */
  readonly title: string;
  /** Panes past the first two of a pane-derived title (AC-026-02), rendered as `+N`. */
  readonly more: number;
  readonly avatar: string;
  readonly kind: string | null;
  readonly status: SidebarStatusKind;
  readonly statusText: string;
  /**
   * The tab focused by this connection, in the focused workspace of the selected host: that row
   * carries the workspace's highlight (P7). Never true in a list that is merely expanded (052).
   */
  readonly selected: boolean;
  /** Epoch ms when this client first saw the tab's current state change; null when unobserved. */
  readonly since: number | null;
  readonly time: string;
  readonly label: string;
}

/**
 * Where one list reads its tabs from. Spec 052: a workspace expands independently of the focus, so
 * the sidebar lists tabs of workspaces the window is not attached to — the attached host comes
 * from the live connection (`liveSource`), every other one from the snapshot the hub carries
 * (`hostSource`, spec 039). Both shapes end in the same rows, so a tab reads the same everywhere.
 */
export interface TabSource {
  readonly tabs: readonly TabDto[];
  readonly agents: readonly AgentDto[];
  /** Tab this source reports as focused; only a selectable list turns it into the highlight. */
  readonly focusedTabId: string | null;
  /** Pane the window has confirmed as focused, as the bar reads it (glyph and kind follow it). */
  readonly focusedPaneId: string | null;
  /** Confirmed topology of the attached tab; a snapshot host has none. */
  readonly panes: readonly PaneBox[];
}

export const EMPTY_SOURCE: TabSource = { tabs: [], agents: [], focusedTabId: null, focusedPaneId: null, panes: [] };

/** The attached host, read from the live connection (the 042 source). */
export function liveSource(state: AgentsState | null, focusedPaneId: string | null = null): TabSource {
  if (!state) return EMPTY_SOURCE;
  const tabs = state.tabs ?? [];
  return {
    tabs,
    agents: state.agents ?? [],
    focusedTabId: state.tabFocus?.tab_id ?? tabs.find((tab) => tab.focused)?.tab_id ?? null,
    focusedPaneId: focusedPaneId ?? state.topology?.focused_pane_id ?? null,
    panes: state.topology?.panes ?? [],
  };
}

/** Tabs one host published in the hub snapshot (039), in the shape the models read. */
export function snapshotTabs(host: HostDto | null): TabDto[] {
  return (host?.tabs ?? []).map((tab) => ({
    tab_id: tab.tab_id,
    workspace_id: tab.workspace_id,
    label: tab.label,
    number: tab.number,
    focused: tab.focused,
    pane_count: tab.pane_count,
    agent_status: normalizeStatus(tab.agent_status),
  }));
}

export interface EffectiveTabsInput {
  /** Endpoint of the host whose tabs are wanted (the sidebar's row, the bar's selected host). */
  readonly endpoint: string | null;
  /** Hub snapshot of that host (039), when the hub carries one. */
  readonly host: HostDto | null;
  /** The one agents connection this window holds. */
  readonly agents: AgentsState | null;
  /** Endpoint the window selected: the live list's host while the connection confirmed none. */
  readonly selectedEndpoint: string | null;
}

export interface EffectiveTabs {
  readonly tabs: readonly TabDto[];
  /** The tabs above are the live list (`tab.list`), not the snapshot. */
  readonly live: boolean;
  /**
   * The agents connection belongs to this host: the identity it confirmed, or the selection while
   * it has published none. Only such a host may be focused with the bar's single `tab.focus`.
   */
  readonly attached: boolean;
}

/**
 * Spec 074 (AC-074-01) — the one decision of where a host's tabs come from, for the sidebar and
 * for the top bar: the live list only while it belongs to this host, the connection is `connected`
 * and it actually carries a tab; the hub snapshot (`HostDto.tabs`) otherwise.
 *
 * The bug it fixes: selecting another host invalidates the connection (`tabs: []`,
 * `identity: null`) and the SSH attach then takes seconds of serial probes, so a rule that reads
 * "live" from the selected endpoint alone leaves the list empty while the hub already publishes the
 * tabs — the sidebar showed nothing until a tab was created. A host without the API lane (057)
 * answers no `tab.list`, so its live list is empty and this same rule lands on its snapshot.
 */
export function effectiveTabs({ endpoint, host, agents, selectedEndpoint }: EffectiveTabsInput): EffectiveTabs {
  const liveEndpoint = agents?.identity?.endpoint ?? selectedEndpoint;
  const attached = agents !== null && endpoint !== null && endpoint === liveEndpoint;
  const tabs = agents?.tabs ?? [];
  const live = attached && agents!.phase === "connected" && tabs.length > 0;
  return { tabs: live ? tabs : snapshotTabs(host), live, attached };
}

/**
 * A host the window is not attached to — or, since spec 057, one without the agents API lane
 * (`HostDto.api === false`), whose tabs live only here — read from its own snapshot
 * (`HostDto.tabs`/`agents`/`panes`): titles, states and counts are the ones the hub published,
 * never guessed here.
 *
 * `workspaceId` names the workspace the list belongs to: the focused tab is then resolved as the
 * bar resolves it (`WorkspaceTabs.svelte`), so the same tab is focused in both — the workspace's
 * own `active_tab_id` first, then a `focused` tab of that workspace, then the host's focused tab
 * and its first tab (either of which may belong to another workspace, and is then simply not in
 * this list, exactly as in the bar). Without a `workspaceId` the 052 reading is kept unchanged.
 */
export function hostSource(host: HostDto | null, workspaceId: string | null = null): TabSource {
  if (!host) return EMPTY_SOURCE;
  const tabs = snapshotTabs(host);
  const scoped = workspaceId === null ? tabs : tabs.filter((tab) => tab.workspace_id === workspaceId);
  const active =
    workspaceId === null
      ? null
      : (host.workspaces ?? []).find((workspace) => workspace.workspace_id === workspaceId)?.active_tab_id ?? null;
  return {
    tabs,
    agents: snapshotAgents(host),
    focusedTabId:
      (active !== null && tabs.some((tab) => tab.tab_id === active) ? active : null) ??
      scoped.find((tab) => tab.focused)?.tab_id ??
      tabs.find((tab) => tab.focused)?.tab_id ??
      (workspaceId === null ? null : (tabs[0]?.tab_id ?? null)),
    focusedPaneId: null,
    panes: [],
  };
}

export interface TabRowsInput {
  readonly source: TabSource;
  /** Workspace of the row; without one there is no workspace to list and no rows. */
  readonly workspaceId: string | null;
  /**
   * Spec 052 (AC-052-02): only the focused workspace of the selected host may mark a row as
   * selected — the highlight belongs to the tab this window is showing, not to a list that merely
   * happens to be expanded.
   */
  readonly selectable?: boolean;
  readonly ledger: ActivityLedger;
  readonly now: number;
}

/** Letter of the avatar: the design's letter for the kind, else its initial, else the shell `$`. */
export function tabAvatar(kind: string | null, glyph: string | null = null): string {
  const key = (kind ?? "").trim().toLowerCase();
  if (key) return TAB_AVATARS[key] ?? key.slice(0, 1).toUpperCase();
  const initial = (glyph ?? "").trim();
  return initial ? initial.slice(0, 1).toUpperCase() : SHELL_AVATAR;
}

/** Tabs of one workspace in `number` order; the engine list order is not relied upon. */
function byNumber(tabs: readonly TabDto[]): TabDto[] {
  return tabs
    .map((tab, index) => ({ tab, index }))
    .sort((a, b) => (a.tab.number ?? a.index) - (b.tab.number ?? b.index) || a.index - b.index)
    .map((entry) => entry.tab);
}

/**
 * Rows of one workspace's tabs (AC-042-01/02), from the live connection or from a host snapshot
 * (AC-052-02). Only the tabs of `workspaceId` are listed: a list never leaks another workspace's.
 */
export function tabRows({ source, workspaceId, selectable = true, ledger, now }: TabRowsInput): TabRow[] {
  if (!workspaceId) return [];
  const tabs = byNumber(source.tabs.filter((tab) => tab.workspace_id === workspaceId));
  const agents = source.agents;
  const panes = source.panes;
  const models = workspaceTabs({
    tabs: [...tabs],
    agents: [...agents],
    focusedTabId: source.focusedTabId,
    focusedPaneId: source.focusedPaneId,
    layoutPanes: panes,
    panePaths: Object.fromEntries(panes.flatMap((pane) => (pane.cwd ? [[pane.pane_id, pane.cwd] as const] : []))),
    workspaceId,
  });

  return models.map((model) => {
    const tab = tabs.find((candidate) => candidate.tab_id === model.tab_id)!;
    const own = agents.filter((agent) => agent.tab_id === model.tab_id);
    // A tab whose panes the client does not hold as agents is read from the state the engine
    // published for the tab itself (`tab.list`), never guessed as idle.
    const statuses: AgentStatus[] = own.length > 0 ? own.map((agent) => agent.status) : [tab.agent_status ?? "idle"];
    const status = sidebarStatus(statuses);
    const since = own.reduce<number | null>((newest, agent) => {
      const at = ledger.since(agent.pane_id);
      return at === null ? newest : newest === null ? at : Math.max(newest, at);
    }, null);
    const time = rowTime(since, now);
    const statusText = TAB_STATUS_TEXT[status.kind];
    return {
      tabId: model.tab_id,
      title: tabText(model),
      more: model.more,
      avatar: tabAvatar(model.kind, model.glyph),
      kind: model.kind,
      status: status.kind,
      statusText,
      selected: selectable && model.active,
      since,
      time,
      label: [tabText(model), statusText, since === null ? null : time].filter(Boolean).join(", "),
    };
  });
}
