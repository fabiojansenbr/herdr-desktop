// Spec 013 — pure model of the center region. Everything here is derived from what the engine
// already published: the frame metadata (`PaneMeta.inner_rect`), the agents/tabs of the attached
// connection and the project the window opened. Nothing is polled, invented or cached: the
// component only measures the canvas box and hands it to these functions.

import { statusLabel } from "../../agents/status";
import { t } from "../../i18n/index.svelte";
import type { AgentDto, LiveIdentity, PaneBox, TabDto, Topology } from "../../agents/types";
import { HEADER_ROWS, bandOffset, headerHeightPx } from "../../terminal/pane-band";
import type { ProjectDto } from "../../projects/types";
import type { SurfaceIdentityDto } from "../../shell/types";
import type { FrameEvent, PaneMeta, RectDto } from "../../terminal/types";

export interface CellMetrics {
  cellWidth: number;
  cellHeight: number;
}
export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}
/** Top-left of the surface canvas, in the overlay's coordinates. */
export type Origin = Pick<Box, "left" | "top">;

/** The band definition is shared with the terminal's own toolbar (src/terminal/pane-band.ts). */
export { HEADER_ROWS, headerHeightPx };

/** Cell size of the committed surface: the measured canvas box divided by its cols/rows. */
export function metricsFromSurface(canvas: { width: number; height: number }, surface: { width: number; height: number }): CellMetrics | null {
  if (!(surface.width > 0) || !(surface.height > 0) || !(canvas.width > 0) || !(canvas.height > 0)) return null;
  return { cellWidth: canvas.width / surface.width, cellHeight: canvas.height / surface.height };
}

export function frameRect(inner: RectDto, metrics: CellMetrics, origin: Origin): Box {
  return {
    left: origin.left + inner.x * metrics.cellWidth,
    top: origin.top + inner.y * metrics.cellHeight,
    width: inner.width * metrics.cellWidth,
    height: inner.height * metrics.cellHeight,
  };
}

/** The frame's header band: the row above `inner_rect` (never a content row of the pane). */
export function bandRect(inner: RectDto, metrics: CellMetrics, origin: Origin): Box {
  const band = bandOffset(inner, metrics);
  return { ...band, left: origin.left + band.left, top: origin.top + band.top };
}

/** Layout the overlay draws: the panes of the committed surface revision. */
export interface SurfaceLayout {
  revision: number;
  panes: PaneMeta[];
}

/**
 * Same rule the terminal view applies to the frame events (`SurfaceMeta.apply`): a full frame
 * commits a revision, a metadata with the link table replaces the layout and a metadata without it
 * (the one a patch carries) updates ONLY the panes it lists — replacing the list there would drop
 * the panes a split had just added.
 */
export function applySurfaceMetadata(current: SurfaceLayout, event: FrameEvent): SurfaceLayout {
  if (event.type === "full") return { revision: event.revision, panes: current.panes };
  if (event.type !== "metadata" || event.revision !== current.revision) return current;
  if (event.hyperlinks) return { revision: event.revision, panes: event.panes.slice() };
  return {
    revision: event.revision,
    panes: current.panes.map((pane) => event.panes.find((updated) => updated.pane_id === pane.pane_id) ?? pane),
  };
}

export type StatusTone = "working" | "attention" | "idle" | "done" | "unknown";
export interface FrameStatus {
  label: string;
  tone: StatusTone;
  icon: string;
}
/** Tone and icon of each engine state; "unknown" is never presented as done. */
const STATUS_SHAPE: Record<AgentDto["status"], Omit<FrameStatus, "label">> = {
  working: { tone: "working", icon: "⟳" },
  blocked: { tone: "attention", icon: "✋" },
  idle: { tone: "idle", icon: "○" },
  done: { tone: "done", icon: "✓" },
  unknown: { tone: "unknown", icon: "?" },
};
/** The words are the one i18n table (spec 067), read at call time so the language can change. */
export function frameStatus(status: AgentDto["status"]): FrameStatus {
  return { label: statusLabel(status), ...STATUS_SHAPE[status] };
}
/** A pane the engine reports without an agent is a shell: idle, never done. */
const shellStatus = (): FrameStatus => frameStatus("idle");
export const SHELL_NAME = "shell";

export type FrameEdge = "accent" | "attention" | "border" | "none";

export interface PaneFrame {
  pane_id: string;
  name: string;
  status: FrameStatus;
  /** Working directory the engine reported for this pane (`pane.list`), or null. */
  path: string | null;
  focused: boolean;
  edge: FrameEdge;
  rect: Box;
  band: Box;
}

export interface PaneFramesInput {
  panes: PaneMeta[];
  agents: AgentDto[];
  panePaths: Record<string, string>;
  metrics: CellMetrics;
  origin: Origin;
}

export function paneFrames({ panes, agents, panePaths, metrics, origin }: PaneFramesInput): PaneFrame[] {
  const split = panes.length > 1;
  return panes.map((pane) => {
    const agent = agents.find((a) => a.pane_id === pane.pane_id) ?? null;
    const status = agent ? frameStatus(agent.status) : shellStatus();
    return {
      pane_id: pane.pane_id,
      name: agent ? (agent.kind ?? agent.name ?? SHELL_NAME) : SHELL_NAME,
      status,
      path: panePaths[pane.pane_id] ?? null,
      focused: pane.focused,
      edge: split ? (pane.focused ? "accent" : "border") : "none",
      rect: frameRect(pane.inner_rect, metrics, origin),
      band: bandRect(pane.inner_rect, metrics, origin),
    };
  });
}

export type TabDot = "working" | "attention" | null;
/** Orca status glyph of a tab: check when idle/done, pulse when working, attention, or a shell. */
export type TabStatusIcon = "idle" | "working" | "attention" | "shell";
export interface TabModel {
  tab_id: string;
  /** Engine label (AC-023-01): the rename draft and the title of a named tab. */
  label: string;
  /** Visible title (AC-026-01): the engine label when named, the pane names otherwise. */
  display: string;
  /** The engine named the tab (AC-023-01): it renders `label`, never a stale pane-derived title. */
  named: boolean;
  /** Panes beyond the two named in `display` (AC-026-02), shown as `+N` outside the ellipsis. */
  more: number;
  title: string;
  panes: number;
  dot: TabDot;
  icon: TabStatusIcon;
  glyph: string | null;
  kind: string | null;
  active: boolean;
}

export const AGENT_DISPLAY_TITLES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
  grok: "Grok",
  pi: "Pi",
  agy: "Antigravity",
};

export function cwdBasename(cwd: string | null | undefined): string | null {
  if (!cwd) return null;
  const trimmed = cwd.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/).filter((part) => part !== "");
  return parts[parts.length - 1] ?? trimmed;
}

export function agentDisplayTitle(kind: string | null | undefined): string | null {
  if (!kind) return null;
  return AGENT_DISPLAY_TITLES[kind] ?? kind;
}

export function tabStatusIcon({ agents, hasAgent }: { agents: AgentDto[]; hasAgent: boolean }): TabStatusIcon {
  if (!hasAgent) return "shell";
  if (agents.some((a) => a.status === "blocked")) return "attention";
  if (agents.some((a) => a.status === "working")) return "working";
  return "idle";
}

/** Visible tab title when the engine named the tab: `TabDto.label`, else its number. */
export function engineTabLabel(tab: Pick<TabDto, "label"> & Partial<Pick<TabDto, "number" | "tab_id">>): string {
  const raw = (tab.label ?? "").trim();
  if (raw) return raw;
  if (tab.number != null) return String(tab.number);
  return tab.tab_id ?? "";
}

/**
 * A pane as the tab title reads it (spec 026): the engine's `agent`,
 * `terminal_title_stripped`, `foreground_cwd` and the confirmed focus, plus the frame status
 * the tooltip shows.
 */
export interface TabPane {
  pane_id: string;
  agent: string | null;
  title: string | null;
  cwd: string | null;
  focused: boolean;
  status: FrameStatus;
}

/** Title of an unnamed tab without panes (spec 026 edge case). */
export function unnamedEmptyTitle(): string {
  return t("center.tabs.emptyTitle");
}
/** Longest engine terminal title shown as a pane name (AC-026-01). */
export const PANE_TITLE_MAX_CHARS = 24;
export const PANE_TITLE_ELLIPSIS = "…";

/** Printable text of a terminal title: escape sequences and control characters are dropped. */
export function printableTerminalTitle(value: string | null | undefined): string {
  return (value ?? "")
    .replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "")
    .replace(/[\u0000-\u001f\u007f-\u009f]/g, "")
    .trim();
}

/**
 * Name of one pane (AC-026-01): the engine's `agent` (lowercase, as the TUI shows it), else
 * `terminal_title_stripped` when printable and up to 24 characters (longer titles keep the
 * ellipsis), else the basename of `foreground_cwd`, else `shell`.
 */
export function paneTitle(pane: Pick<TabPane, "agent" | "title" | "cwd">): string {
  const agent = pane.agent?.trim().toLowerCase();
  if (agent) return agent;
  const title = printableTerminalTitle(pane.title);
  if (title) {
    return title.length > PANE_TITLE_MAX_CHARS
      ? `${title.slice(0, PANE_TITLE_MAX_CHARS).trimEnd()}${PANE_TITLE_ELLIPSIS}`
      : title;
  }
  return cwdBasename(pane.cwd) ?? SHELL_NAME;
}

/**
 * Engine tab without a name of its own: the label is empty or holds only digits — `tab.list`
 * sends the tab position as `label` ("2") and the monotonic id as `number` (21), so the label
 * never needs to equal the number for the tab to be unnamed (spec 028, r4b).
 */
export function isUnnamedTab(tab: Pick<TabDto, "label">): boolean {
  const label = (tab.label ?? "").trim();
  return label === "" || /^\d+$/.test(label);
}

/**
 * Visible title of one tab (AC-026-01/02), split so the component can keep the `+N` of a
 * multi-pane title outside the truncating text: a named tab shows its engine label; an unnamed
 * one lists its panes' names in layout order (focused pane NOT moved to the front), two names
 * and `+N` beyond.
 */
export function tabTitleParts(tab: Pick<TabDto, "label" | "number">, panes: readonly TabPane[]): { text: string; more: number } {
  if (!isUnnamedTab(tab)) return { text: engineTabLabel(tab), more: 0 };
  if (panes.length === 0) return { text: unnamedEmptyTitle(), more: 0 };
  const names = panes.map(paneTitle);
  if (names.length === 1) return { text: names[0]!, more: 0 };
  return { text: `${names[0]} · ${names[1]}`, more: names.length - 2 };
}

/** Visible title of one tab (AC-026-01/02): the engine label when named, the pane names else. */
export function tabTitle(tab: Pick<TabDto, "label" | "number">, panes: readonly TabPane[]): string {
  const { text, more } = tabTitleParts(tab, panes);
  return more > 0 ? `${text} +${more}` : text;
}

/**
 * Tooltip of one tab (AC-026-01/02): a named tab keeps its engine label (023) and every pane the
 * engine reported is listed with its state (`claude · Working`); an unnamed tab lists them
 * only.
 */
export function tabTooltip(tab: Pick<TabDto, "label" | "number">, panes: readonly TabPane[]): string {
  const entries = panes.map((pane) => `${paneTitle(pane)} · ${pane.status.label}`);
  if (!isUnnamedTab(tab)) {
    const label = engineTabLabel(tab);
    return entries.length > 0 ? `${label} · ${entries.join(" · ")}` : label;
  }
  return entries.length > 0 ? entries.join(" · ") : unnamedEmptyTitle();
}

/** Minimal host snapshot the center reads to scope the workspace (spec 025). */
export interface FocusedWorkspaceHost {
  endpoint: string;
  workspaces?: readonly { workspace_id: string; focused: boolean }[];
}

/**
 * Workspace the bar belongs to (spec 025 AC-025-04): the focused workspace of the selected host,
 * taken from the live host snapshot first (a focus changed in the TUI lands here) and from the
 * connection's confirmed tab as fallback. `null` = unknown (the whole engine list is used).
 */
export function focusedWorkspaceId(input: {
  hosts?: readonly FocusedWorkspaceHost[] | null;
  endpoint: string | null;
  tabs: readonly Pick<TabDto, "tab_id" | "workspace_id">[];
  focusedTabId: string | null;
}): string | null {
  const host = input.hosts?.find((candidate) => candidate.endpoint === input.endpoint);
  const focused = host?.workspaces?.find((workspace) => workspace.focused);
  if (focused) return focused.workspace_id;
  const tab = input.tabs.find((candidate) => candidate.tab_id === input.focusedTabId);
  return tab?.workspace_id ?? null;
}

/** Public pane ids are `{workspace_id}:p{n}` (engine `public_pane_id_for_number`). */
export function paneWorkspaceId(paneId: string): string {
  return paneId.split(":")[0] ?? "";
}

/** Panes and tabs of one workspace (spec 025 AC-025-04): the status bar counts the focus only. */
export function workspaceCounts(input: {
  tabs: readonly Pick<TabDto, "workspace_id">[];
  panes: readonly { pane_id: string }[];
  workspaceId: string | null;
}): { tabs: number; panes: number } {
  if (input.workspaceId === null) return { tabs: input.tabs.length, panes: input.panes.length };
  return {
    tabs: input.tabs.filter((tab) => tab.workspace_id === input.workspaceId).length,
    panes: input.panes.filter((pane) => paneWorkspaceId(pane.pane_id) === input.workspaceId).length,
  };
}

/**
 * Tabs of one workspace: an unnamed tab is titled by the app each pane runs (AC-026-01/02), a
 * named one keeps the engine label (AC-023-01); glyph/icon follow the focused pane. With
 * `workspaceId` (spec 025) only that workspace's tabs are returned; `null` keeps the whole engine
 * list for callers that have no focus yet. `layoutPanes` is the confirmed tab's topology: only it
 * knows the panes' order and shells; every other tab is read from the engine's agent metadata and
 * its pane count.
 */
export function workspaceTabs({
  tabs,
  agents,
  focusedTabId,
  focusedPaneId = null,
  layoutPanes = [],
  panePaths = {},
  workspaceId = null,
}: {
  tabs: TabDto[];
  agents: AgentDto[];
  focusedTabId: string | null;
  focusedPaneId?: string | null;
  layoutPanes?: readonly PaneBox[];
  panePaths?: Record<string, string>;
  workspaceId?: string | null;
}): TabModel[] {
  const scoped = workspaceId === null ? tabs : tabs.filter((tab) => tab.workspace_id === workspaceId);
  return scoped.map((tab) => {
    const own = agents.filter((a) => a.tab_id === tab.tab_id);
    const focusedAgent =
      own.find((a) => a.pane_id === focusedPaneId) ?? own.find((a) => a.focused) ?? own[0] ?? null;
    const dot: TabDot =
      tab.agent_status === "blocked" || own.some((a) => a.status === "blocked")
        ? "attention"
        : tab.agent_status === "working" || own.some((a) => a.status === "working")
          ? "working"
          : null;
    const hasAgent = own.length > 0;
    const statusIcon = hasAgent
      ? tabStatusIcon({ agents: own, hasAgent: true })
      : tab.agent_status === "working"
        ? "working"
        : tab.agent_status === "blocked"
          ? "attention"
          : "shell";
    const kind = focusedAgent?.kind ?? null;
    const label = engineTabLabel(tab);
    const glyph = kind ? kind.trim().slice(0, 1).toLowerCase() : hasAgent ? (focusedAgent?.name ?? "").trim().slice(0, 1).toLowerCase() || null : null;
    const panes = tab.pane_count ?? 0;
    const titlePanes = tabTitlePanes(tab, own, tab.tab_id === focusedTabId ? layoutPanes : [], panePaths);
    const parts = tabTitleParts(tab, titlePanes);
    return {
      tab_id: tab.tab_id,
      label,
      display: parts.text,
      named: !isUnnamedTab(tab),
      more: parts.more,
      title: tabTooltip(tab, titlePanes),
      panes,
      dot,
      icon: statusIcon,
      glyph: glyph || null,
      kind,
      active: tab.tab_id === focusedTabId,
    };
  });
}

/** Panes of one tab as the title reads them: the confirmed layout when there is one, else the
 * engine's agents; a pane only known by count enters as a shell, never dropped. */
function tabTitlePanes(
  tab: TabDto,
  own: AgentDto[],
  layout: readonly PaneBox[],
  panePaths: Record<string, string>,
): TabPane[] {
  if (layout.length > 0) {
    return layout.map((pane) => {
      const agent = own.find((a) => a.pane_id === pane.pane_id) ?? null;
      return {
        pane_id: pane.pane_id,
        agent: agent?.kind ?? null,
        title: agent?.terminal_title ?? null,
        cwd: panePaths[pane.pane_id] ?? pane.cwd ?? null,
        focused: pane.focused,
        status: agent ? frameStatus(agent.status) : shellStatus(),
      };
    });
  }
  const known: TabPane[] = own.map((agent) => ({
    pane_id: agent.pane_id,
    agent: agent.kind,
    title: agent.terminal_title ?? null,
    cwd: panePaths[agent.pane_id] ?? null,
    focused: agent.focused,
    status: frameStatus(agent.status),
  }));
  const missing = Math.max(0, (tab.pane_count ?? known.length) - known.length);
  for (let index = 0; index < missing; index += 1) {
    known.push({ pane_id: "", agent: null, title: null, cwd: null, focused: false, status: shellStatus() });
  }
  return known;
}

/** Pane every action of the center addresses: the one the surface confirmed. */
export interface ConfirmedTarget {
  pane_id: string;
}

/** Why an action is unavailable while the surface has not confirmed a pane yet. */
export function waitingTarget(): string {
  return t("center.reason.waitingTarget");
}

/**
 * The pane the actions may address: the one this surface confirmed, and only while the agents
 * connection is the same one (endpoint, session, generation, boot) and its topology agrees on the
 * focus and lists that pane. A topology left over from another host/attachment addresses a pane the
 * engine no longer lists, and the action is refused after being sent (spec 013, gate r3).
 */
export function confirmedTarget({
  identity,
  agents,
}: {
  identity: SurfaceIdentityDto | null;
  agents: { identity: LiveIdentity | null; topology: Topology | null } | null;
}): ConfirmedTarget | null {
  const topology = agents?.topology ?? null;
  const live = agents?.identity ?? null;
  if (!identity || !topology || !live) return null;
  const sameConnection =
    live.endpoint === identity.endpoint &&
    live.session === identity.session &&
    live.connection_generation === identity.connection_generation &&
    live.boot_id === identity.boot_id;
  if (!sameConnection) return null;
  if (topology.focused_pane_id !== identity.pane_id) return null;
  if (!topology.panes.some((pane) => pane.pane_id === identity.pane_id)) return null;
  return { pane_id: identity.pane_id };
}

/**
 * Name of the agent the header starts, in the engine's own rule: it must start with a lowercase
 * letter and hold only lowercase letters, digits, `-` or `_`, up to 32 characters (`agent.start`
 * refuses anything else — spec 013, gate r5). The pane is kept in the name so two panes never
 * collide.
 */
export function agentName(kind: string, paneId: string): string {
  const clean = (value: string) => value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  const base = [clean(kind), clean(paneId)].filter((part) => part !== "").join("-") || "agente";
  const named = /^[a-z]/.test(base) ? base : `a-${base}`;
  return named.slice(0, 32).replace(/-+$/g, "");
}

export type HeaderAction = "split" | "newTab" | "newAgent";

/** Reason each action is unavailable (null = enabled): the window's own reasons first, then the
 * confirmed target, so no button is offered for an action that would be refused unsent. */
export function actionAvailability({
  target,
  unavailable,
}: {
  target: ConfirmedTarget | null;
  unavailable: Readonly<Record<HeaderAction, string | null>>;
}): Record<HeaderAction, string | null> {
  const reason = (action: HeaderAction) => unavailable[action] ?? (target ? null : waitingTarget());
  return { split: reason("split"), newTab: reason("newTab"), newAgent: reason("newAgent") };
}
export interface HeaderModel {
  crumbs: string[];
  branch: string | null;
  path: string | null;
  empty: boolean;
  emptyText: string;
  actions: HeaderAction[];
}

export function projectHeader({
  project,
  collection,
  branch,
  tabs,
  session = null,
  workspace = null,
  cwd = null,
}: {
  project: ProjectDto | null;
  collection: string | null;
  branch: string | null;
  tabs: number;
  session?: string | null;
  workspace?: string | null;
  cwd?: string | null;
}): HeaderModel {
  const empty = tabs === 0;
  if (project) {
    return {
      crumbs: [collection, project.label ?? null].filter((c): c is string => typeof c === "string" && c !== ""),
      branch: branch && branch !== "—" ? branch : null,
      path: project.root ?? null,
      empty,
      emptyText: t("center.header.empty"),
      actions: empty ? ["newTab", "newAgent"] : ["split", "newTab", "newAgent"],
    };
  }
  return {
    crumbs: [session ? t("center.header.session", { name: session }) : null, workspace].filter((c): c is string => typeof c === "string" && c !== ""),
    branch: null,
    path: cwd,
    empty,
    emptyText: empty ? t("center.header.empty") : "",
    actions: empty ? ["newTab", "newAgent"] : ["split", "newTab", "newAgent"],
  };
}
