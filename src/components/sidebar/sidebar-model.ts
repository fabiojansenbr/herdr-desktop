// Pure presentation model of the new sidebar (spec 041, PRD nova-lateral). The 025 tree stays the
// source of truth for what a row is (one node per host, live workspaces plus closed projects,
// exactly one active row from 036); this model only regroups those rows by collection across
// hosts (AC-041-02) and computes the per-row status summary (AC-041-03), which — unlike
// `mapAgentStatus` — keeps `done` apart from idle.
import type { AgentDto, AgentStatus } from "../../agents/types";
import type { HostDto, LinkPhase } from "../../connections/types";
import { phaseText, t } from "../../i18n/index.svelte";
import type { CollectionDto, ProjectDto, WorkspacePrefDto } from "../../projects/types";
import type { ProjectsController } from "../../projects/controller";
import { buildWorkspaceTree, groupColor, type WorkspaceRow } from "../projects/tree-model";
import { activeDrag, planDrop, projectIdOf, type DragRow, type DropSpot } from "./drag";

/** Synthetic id of the "Sem coleção" bucket (P6); never a store collection id. */
export const UNGROUPED_ID = "__ungrouped__";
/** Name of that bucket, in the user's language (spec 068); a function, so it follows a change. */
export function ungroupedName(): string {
  return t("sidebar.ungrouped");
}
/** Spec 060 (AC-060-03): the line `Sem coleção` shows for a host that is still connecting. */
export function waitingText(): string {
  return t("sidebar.loadingWorkspaces");
}
/** Synthetic id of the `Ocultos (N)` bucket (044, P5); never a store collection id either. */
export const HIDDEN_ID = "__hidden__";

/**
 * Spec 045 (AC-045-03): a stored collection keeps its collapse in `Group.collapsed`, but the two
 * synthetic buckets have no row in the store, so theirs lives in this browser only. Storage can
 * be absent or throw (private window, blocked site data), and the sidebar must render anyway —
 * every access is wrapped and falls back to the caller's default.
 */
export function collapseKey(id: string): string {
  return `herdr.sidebar.collapsed.${id}`;
}

function readFlag(key: string, fallback: boolean): boolean {
  try {
    const stored = globalThis.localStorage?.getItem(key);
    if (stored === "1") return true;
    if (stored === "0") return false;
  } catch {
    /* no storage: the bucket just starts at its default */
  }
  return fallback;
}

function writeFlag(key: string, value: boolean): void {
  try {
    globalThis.localStorage?.setItem(key, value ? "1" : "0");
  } catch {
    /* nothing to persist to; the state stays in memory for this window */
  }
}

export function readCollapsed(id: string, fallback: boolean): boolean {
  return readFlag(collapseKey(id), fallback);
}

export function writeCollapsed(id: string, collapsed: boolean): void {
  writeFlag(collapseKey(id), collapsed);
}

export type SidebarStatusKind = "working" | "waiting" | "done" | "idle";

export interface SidebarStatus {
  readonly working: number;
  readonly waiting: number;
  readonly done: number;
  readonly idle: number;
  /** Strongest state of the row: working > waiting > done > idle. */
  readonly kind: SidebarStatusKind;
  /** `2 trabalhando`, `1 aguardando`, `1 concluído`; empty when everything is idle. */
  readonly label: string;
}

/**
 * Spec 060 (AC-060-02): the connection state a workspace line shows on its host tag. `connecting`
 * and `reconnecting` read the same to the user — the host is coming back — so they share one state.
 */
export type RowHostState = "online" | "reconnecting" | "offline" | "attention";

export interface SidebarRow extends WorkspaceRow {
  /** Host name, shown as a tag only on SSH rows. */
  readonly hostBadge: string | null;
  /** The row's host is not online: the line is dimmed and clicking it sends nothing. */
  readonly offline: boolean;
  /** Phase of the row's host, as the hub published it (spec 060). */
  readonly hostPhase: LinkPhase;
  /** What the host tag of the line says about the connection (spec 060, AC-060-02). */
  readonly hostState: RowHostState;
  /** Text the tag adds to the host name: `reconectando…`, `atenção`; null online and offline. */
  readonly hostStateText: string | null;
  /** The host's phase in the user's language, the title of a tag that is not online. */
  readonly hostStateTitle: string | null;
  /** (Re)connecting host: the line shows the cached snapshot until the connection is live. */
  readonly stale: boolean;
  readonly status: SidebarStatus;
  /** Spec 044: the key of the row's preference, `null` when the engine reported no cwd. */
  readonly prefRoot: string | null;
  /** Explicit workspace colour preference (spec 044), null when unset. */
  readonly color: string | null;
  /** Spec 062: preference, collection colour, then the UTF-16 name palette. */
  readonly tileColor: string;
  /** Pinned to the top of its collection (spec 044). */
  readonly pinned: boolean;
  /** Out of the collections, listed under `Ocultos (N)` (spec 044, P5). */
  readonly hidden: boolean;
}

export interface SidebarCollection {
  readonly id: string;
  readonly name: string;
  /** `Group.color` when the user picked one (spec 045), otherwise the colour of the position. */
  readonly color: string;
  readonly count: number;
  readonly empty: boolean;
  /** The "Sem coleção" bucket, which is hidden when empty (unlike a stored collection). */
  readonly ungrouped: boolean;
  /** Spec 045 (AC-045-03): collapse as the store recorded it; the buckets carry `false` here. */
  readonly collapsed: boolean;
  readonly rows: readonly SidebarRow[];
}

export interface SidebarInput {
  readonly hosts: readonly HostDto[];
  readonly groups: readonly CollectionDto[];
  readonly projects: readonly ProjectDto[];
  readonly agents?: readonly AgentDto[];
  readonly opening?: Record<string, true>;
  readonly selectedEndpoint?: string | null;
  /** Spec 044: client-only preferences of the store (colour, pinned, hidden). */
  readonly prefs?: readonly WorkspacePrefDto[];
}

/**
 * Spec 060 (AC-060-03): a host that is opening its connection and has no workspace in the snapshot
 * yet — neither a live one nor a cached one. It has no line of its own to dim, so `Sem coleção`
 * shows this one waiting line for it until the host is online (or gives up).
 */
export interface SidebarWaitingHost {
  readonly endpoint: string;
  readonly name: string;
  /** `mac-mini · Carregando workspaces…`. */
  readonly label: string;
}

/** The WORKSPACES section: its collections plus the rows hidden out of them (spec 044, P5). */
export interface SidebarSections {
  readonly collections: readonly SidebarCollection[];
  /** Every hidden row, in the order its collection would have shown it. */
  readonly hidden: readonly SidebarRow[];
  /** Spec 060: the hosts `Sem coleção` is waiting for, in the order of the hub. */
  readonly waiting: readonly SidebarWaitingHost[];
}

/** Same normalization the store and the 025 tree apply before comparing roots. */
export function normalizeRoot(root: string): string {
  return root.trim().replace(/[\\/]+$/, "") || root.trim();
}

function prefKey(endpoint: string, root: string): string {
  return `${endpoint}\u0000${normalizeRoot(root)}`;
}

/**
 * Status summary of one row (AC-041-03). The engine publishes `idle|working|blocked|done|unknown`
 * (../herdr/src/api/schema/common.rs); `done` is a state of its own here, so a finished agent is
 * visible without opening the workspace.
 */
export function sidebarStatus(statuses: readonly AgentStatus[]): SidebarStatus {
  let working = 0;
  let waiting = 0;
  let done = 0;
  let idle = 0;
  for (const status of statuses) {
    if (status === "working") working += 1;
    else if (status === "blocked") waiting += 1;
    else if (status === "done") done += 1;
    else idle += 1;
  }
  const kind: SidebarStatusKind = working > 0 ? "working" : waiting > 0 ? "waiting" : done > 0 ? "done" : "idle";
  const label =
    kind === "working"
      ? t("sidebar.status.working", { count: working })
      : kind === "waiting"
        ? t("sidebar.status.waiting", { count: waiting })
        : kind === "done"
          ? t("sidebar.status.done", { count: done })
          : "";
  return { working, waiting, done, idle, kind, label };
}

/**
 * The agents the sidebar reasons about: the live list when the API answered, otherwise the
 * snapshot the hosts carry (a host without `agent.list` still shows its panes). Same rule the
 * 011 region already applied to the tree; it returns the live list untouched when there is one.
 */
export function effectiveAgents(live: readonly AgentDto[], hosts: readonly HostDto[]): readonly AgentDto[] {
  if (live.length > 0) return live;
  return hosts.flatMap((host) => snapshotAgents(host));
}

/**
 * The agents of one host as the snapshot it carries published them (`agent.list` when the host
 * answered it, otherwise its panes). Spec 052 reads a workspace of another host from here: it is
 * the only source the sidebar has for a host the window is not attached to.
 */
export function snapshotAgents(host: HostDto): AgentDto[] {
  if (host.agents && host.agents.length > 0) {
    return host.agents.map((a) => ({
      pane_id: a.pane_id,
      workspace_id: a.workspace_id,
      tab_id: a.tab_id,
      name: a.name,
      kind: a.agent ?? a.display_agent ?? a.name,
      status: (a.agent_status as AgentStatus) ?? "idle",
      launch_pending: false,
      ready: true,
      focused: a.focused,
      terminal_title: a.terminal_title_stripped ?? a.terminal_title ?? a.title ?? null,
    }));
  }
  return (host.panes ?? []).flatMap((p) =>
    p.agent
      ? [
          {
            pane_id: p.pane_id,
            workspace_id: p.workspace_id,
            tab_id: p.tab_id ?? "",
            name: p.agent,
            kind: p.agent,
            status: (p.agent_status as AgentStatus) ?? "idle",
            launch_pending: false,
            ready: true,
            focused: p.focused,
            terminal_title: p.terminal_title ?? p.title ?? null,
          } satisfies AgentDto,
        ]
      : [],
  );
}

/** Raw engine statuses of the agents that belong to this row (a closed row is keyed by project). */
function statusesOf(row: WorkspaceRow, agents: readonly AgentDto[]): AgentStatus[] {
  const key = row.kind === "closed" ? row.projectId : row.workspaceId;
  if (!key) return [];
  return agents.filter((agent) => agent.workspace_id === key).map((agent) => agent.status);
}

/**
 * Collections of the WORKSPACES section, in the store's order and then "Sem coleção" (P6).
 * Inside a collection the rows follow the host order of the 025 tree (Local before SSH, each host
 * by workspace `number`), so a collection reads as one list even when it spans hosts.
 */
export function buildSidebar(input: SidebarInput): readonly SidebarCollection[] {
  return buildSidebarSections(input).collections;
}

/**
 * The same section plus the hidden rows (spec 044, AC-044-03): inside each collection the pinned
 * rows come first, keeping the 025 order among equals, and a hidden row leaves its collection for
 * the `Ocultos (N)` item at the end of the section.
 */
export function buildSidebarSections(input: SidebarInput): SidebarSections {
  const agents = input.agents ?? [];
  const tree = buildWorkspaceTree({
    hosts: input.hosts,
    groups: input.groups,
    projects: input.projects,
    agents,
    opening: input.opening ?? {},
    selectedEndpoint: input.selectedEndpoint ?? null,
  });
  const hostOf = new Map(input.hosts.map((host) => [host.endpoint, host]));
  const prefOf = new Map((input.prefs ?? []).map((pref) => [prefKey(pref.endpoint_profile_id, pref.root), pref]));

  const decorate = (node: (typeof tree)[number], row: WorkspaceRow, collectionColor?: string): SidebarRow => {
    const host = hostOf.get(node.endpoint);
    const prefRoot = row.cwd ? normalizeRoot(row.cwd) : null;
    const pref = prefRoot === null ? undefined : prefOf.get(prefKey(row.endpoint, prefRoot));
    const phase: LinkPhase = host?.phase ?? "offline";
    const hostState = rowHostState(phase);
    return {
      ...row,
      hostBadge: node.typeBadge === "SSH" ? (host?.label || node.endpoint) : null,
      offline: node.offline,
      hostPhase: phase,
      hostState,
      hostStateText:
        hostState === "reconnecting" ? t("sidebar.host.reconnecting") : hostState === "attention" ? t("sidebar.host.attention") : null,
      // P3 of the PRD: the phase is said in the user's language, never as the engine's
      // `phase_label` (which spec 071 turns into English).
      hostStateTitle: hostState === "online" ? null : phaseText(phase),
      stale: hostState === "reconnecting",
      status: sidebarStatus(statusesOf(row, agents)),
      prefRoot,
      color: pref?.color ?? null,
      tileColor: pref?.color ?? collectionColor ?? groupColor(row.name.split("").reduce((sum, unit) => sum + unit.charCodeAt(0), 0)),
      pinned: pref?.pinned === true,
      hidden: pref?.hidden === true,
    };
  };

  /**
   * Spec 045: inside one host's block the rows follow the collection's own order, so dragging a
   * row to a new position shows it there. The host blocks themselves stay in the 025 order
   * (Local before SSH), which is what AC-041-02 pinned; a row the collection does not list keeps
   * its place at the end of its block.
   */
  const inStoreOrder = (rows: readonly SidebarRow[], ids: readonly string[]): SidebarRow[] => {
    const rank = new Map(ids.map((id, index) => [id, index]));
    return rows
      .map((row, index) => ({ row, index, at: (row.projectId !== null ? rank.get(row.projectId) : undefined) ?? ids.length }))
      .sort((a, b) => a.at - b.at || a.index - b.index)
      .map((entry) => entry.row);
  };

  const hidden: SidebarRow[] = [];
  /** Pinned first, otherwise the 025 order; hidden rows leave for the `Ocultos` item. */
  const arrange = (rows: readonly SidebarRow[]): SidebarRow[] => {
    const visible: SidebarRow[] = [];
    for (const row of rows) {
      if (row.hidden) hidden.push(row);
      else visible.push(row);
    }
    return [...visible.filter((row) => row.pinned), ...visible.filter((row) => !row.pinned)];
  };

  const sections: SidebarCollection[] = input.groups.map((group, position) => {
    const color = group.color || groupColor(position);
    const rows = arrange(
      tree.flatMap((node) =>
        inStoreOrder(
          (node.groups.find((g) => g.id === group.id)?.rows ?? []).map((row) => decorate(node, row, color)),
          group.project_ids,
        ),
      ),
    );
    return {
      id: group.id,
      name: group.name,
      color,
      count: rows.length,
      empty: rows.length === 0,
      ungrouped: false,
      collapsed: group.collapsed === true,
      rows,
    };
  });

  /**
   * Spec 060 (AC-060-01): `Sem coleção` is the bucket of every loose row again, of Local and of the
   * SSH hosts alike — the 059 section that pulled the remote ones out is gone, and what it made
   * visible now rides on the row itself (`hostState`) or on the waiting line below.
   */
  const loose = arrange(tree.flatMap((node) => node.rows.map((row) => decorate(node, row))));

  /**
   * AC-060-03: a host still opening its connection with nothing in the snapshot has no line to
   * dim, so the bucket is shown for its waiting line alone — and never counts it as a workspace.
   */
  const waiting: SidebarWaitingHost[] = input.hosts
    .filter((host) => (host.phase === "connecting" || host.phase === "reconnecting") && (host.workspaces?.length ?? 0) === 0)
    .map((host) => {
      const name = host.label || host.endpoint;
      return { endpoint: host.endpoint, name, label: `${name} · ${waitingText()}` };
    });

  if (loose.length > 0 || waiting.length > 0) {
    sections.push({
      id: UNGROUPED_ID,
      name: ungroupedName(),
      color: groupColor(input.groups.length),
      count: loose.length,
      empty: loose.length === 0,
      ungrouped: true,
      collapsed: false,
      rows: loose,
    });
  }

  return { collections: sections, hidden, waiting };
}

/** The phase of a host as the line reads it (AC-060-02); `connecting` says `reconectando…` too. */
function rowHostState(phase: LinkPhase): RowHostState {
  if (phase === "online") return "online";
  if (phase === "connecting" || phase === "reconnecting") return "reconnecting";
  if (phase === "attention") return "attention";
  return "offline";
}

/**
 * Runs the plan of a drop in order (spec 045); every step is an existing catalog command.
 *
 * The row may have had no ProjectRef at all: `assign` upserts it by root, and the `move` that
 * follows needs that id, or the line lands at the end instead of where the indicator was.
 */
export async function runDropPlan(projects: ProjectsController, spot: DropSpot, source: DragRow | null = activeDrag()): Promise<void> {
  if (!source || !source.cwd) return;
  const plan = planDrop(source, spot);
  if (plan.refused !== null) return;
  let projectId = source.projectId;
  for (const step of plan.steps) {
    if (step.kind === "remove") {
      if (projectId) await projects.removeFromCollection(step.collectionId, projectId);
    } else if (step.kind === "assign") {
      await projects.assignToGroup(step.groupId, {
        endpoint_profile_id: source.endpoint,
        session_name: source.session,
        cwd: source.cwd,
        label: source.label,
      });
      projectId = projectIdOf(projects.state.snapshot?.projects ?? [], source);
    } else if (projectId) {
      await projects.moveByDrag(step.collectionId, projectId, step.overProjectId, step.placement);
    }
  }
}
