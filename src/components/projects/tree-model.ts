// Pure presentation model for the projects tree and connections footer (spec 011, AC-011-01, AC-011-02).
import type { AgentDto, AgentStatus } from "../../agents/types";
import { failureReason } from "../../connections/presentation";
import { phaseText, t } from "../../i18n/index.svelte";
import type { HostDto } from "../../connections/types";
import type { CollectionDto, ProjectDto } from "../../projects/types";

export interface AgentDot {
  readonly status: "working" | "blocked" | "idle";
  readonly color: string;
  readonly label: string;
}

export interface ProjectItem {
  readonly id: string;
  readonly name: string;
  readonly endpoint: string;
  readonly isSsh: boolean;
  readonly hostBadge: string | null;
  readonly branch: string;
  readonly active: boolean;
  readonly allDots: readonly AgentDot[];
  readonly visibleDots: readonly AgentDot[];
  readonly overflowCount: number;
  readonly agentSummary: string;
  readonly session: string;
  readonly root: string;
  readonly workspaceId: string;
  readonly opening: boolean;
}

export interface GroupItem {
  readonly id: string;
  readonly name: string;
  readonly color: string;
  readonly count: number;
  readonly empty: boolean;
  readonly projects: readonly ProjectItem[];
}

export interface ConnectionItem {
  readonly endpoint: string;
  readonly name: string;
  readonly typeBadge: "Local" | "SSH";
  readonly latencyText: string;
  readonly statusText: string;
  readonly tone: "ok" | "idle" | "warn" | "attention";
  readonly active: boolean;
  /** Live connection: the menu offers Desconectar; otherwise Reconectar (spec 029, AC-029-03). */
  readonly online: boolean;
  /** Failed connection: the row offers Tentar novamente with the reason (AC-029-02). */
  readonly retryable: boolean;
  /** Row tooltip; the discovered herdr binary when known (`herdr <versão> · <caminho>`). */
  readonly tooltip: string;
}

export const GROUP_PALETTE = [
  "#8FA8FF", // Blue (e.g. Acme / Clientes)
  "#5BD68A", // Green (e.g. Open source)
  "#B18CFF", // Purple (e.g. Pessoal)
  "#F4B454", // Amber
  "#F2777A", // Red
  "#5CE1E6", // Cyan
] as const;

export function groupColor(index: number): string {
  return GROUP_PALETTE[index % GROUP_PALETTE.length] ?? "#8FA8FF";
}

export function mapAgentStatus(status: AgentStatus): AgentDot | null {
  switch (status) {
    case "working":
      return { status: "working", color: "var(--working, #5BD68A)", label: t("projects.agent.working") };
    case "blocked":
      return { status: "blocked", color: "var(--attention, #F4B454)", label: t("projects.agent.blocked") };
    case "idle":
    case "done":
      return { status: "idle", color: "var(--idle, #6B7280)", label: t("projects.agent.idle") };
    case "unknown":
    default:
      return null;
  }
}

export function formatAgentSummary(dots: readonly AgentDot[]): string {
  if (dots.length === 0) return t("projects.agents.none");
  const working = dots.filter((d) => d.status === "working").length;
  const blocked = dots.filter((d) => d.status === "blocked").length;
  const idle = dots.filter((d) => d.status === "idle").length;

  const parts: string[] = [];
  if (working > 0) parts.push(t("projects.agents.working", { count: working }));
  if (blocked > 0) parts.push(t("projects.agents.blocked", { count: blocked }));
  if (idle > 0) parts.push(t("projects.agents.idle", { count: idle }));

  const total = t("projects.agents.total", { count: dots.length });
  return parts.length > 0 ? `${total}: ${parts.join(", ")}` : total;
}

export function buildProjectItem(
  project: ProjectDto,
  activeProjectId: string | null,
  hostLabels: Record<string, string>,
  allAgents: readonly AgentDto[],
  projectBranch: string | null | undefined,
  opening: boolean = false,
): ProjectItem {
  const isSsh = project.endpoint_profile_id !== "local";
  const hostBadge = isSsh ? (hostLabels[project.endpoint_profile_id] ?? project.endpoint_profile_id) : null;
  const active = activeProjectId === project.id;
  const branch = projectBranch?.trim() || project.branch?.trim() || "—";

  // Match agents belonging strictly to this project's workspace (AC-011-01)
  const workspaceId = project.binding?.workspace_id;
  const projectAgents = workspaceId
    ? allAgents.filter((a) => a.workspace_id === workspaceId)
    : [];

  const allDots: AgentDot[] = [];
  for (const agent of projectAgents) {
    const dot = mapAgentStatus(agent.status);
    if (dot) allDots.push(dot);
  }

  const visibleDots = allDots.slice(0, 3);
  const overflowCount = Math.max(0, allDots.length - 3);
  const agentSummary = formatAgentSummary(allDots);

  return {
    id: project.id,
    name: project.label,
    endpoint: project.endpoint_profile_id,
    isSsh,
    hostBadge,
    branch,
    active,
    allDots,
    visibleDots,
    overflowCount,
    agentSummary,
    session: project.session_name,
    root: project.root,
    workspaceId: workspaceId ?? "",
    opening,
  };
}

export function buildTree(
  collections: readonly CollectionDto[],
  projects: readonly ProjectDto[],
  activeProjectId: string | null,
  hostLabels: Record<string, string>,
  agents: readonly AgentDto[],
  branches: Record<string, string> = {},
  opening: Record<string, true> = {},
): readonly GroupItem[] {
  const projectMap = new Map<string, ProjectDto>();
  for (const p of projects) {
    projectMap.set(p.id, p);
  }

  if (collections.length === 0) {
    return [
      {
        id: "meus-projetos",
        name: t("projects.tree.defaultGroup"),
        color: groupColor(0),
        count: 0,
        empty: true,
        projects: [],
      },
    ];
  }
  return collections.map((col, index) => {
    const groupProjects: ProjectItem[] = [];
    for (const id of col.project_ids) {
      const p = projectMap.get(id);
      if (p) {
        groupProjects.push(
          buildProjectItem(
            p,
            activeProjectId,
            hostLabels,
            agents,
            branches[p.id] ?? p.branch,
            opening[p.id] === true,
          ),
        );
      }
    }
    return {
      id: col.id,
      name: col.name,
      color: groupColor(index),
      count: groupProjects.length,
      empty: groupProjects.length === 0,
      projects: groupProjects,
    };
  });
}

export function buildConnectionItems(
  hosts: readonly HostDto[],
  selectedEndpoint: string | null,
): readonly ConnectionItem[] {
  const localHost = hosts.find((h) => h.endpoint === "local");
  const sshHosts = hosts.filter((h) => h.endpoint !== "local");

  const items: ConnectionItem[] = [];

  // The tooltip shows the herdr binary that was chosen for this connection, exactly as the TUI
  // resolves it (spec 029, AC-029-01); without one it stays the state text.
  const tooltipOf = (host: HostDto, statusText: string): string => {
    const binary = host.herdr_binary ?? null;
    if (!binary) return statusText;
    const version = binary.version ?? host.server_version ?? null;
    return version ? `herdr ${version} · ${binary.path}` : `herdr · ${binary.path}`;
  };

  // 1. Local host ("Este computador · Local")
  const localActive = selectedEndpoint === "local" || selectedEndpoint === null;
  const localOnline = localHost?.phase === "online";
  // P3 of the PRD (spec 068): the phase is said in the user's language. `phase_label` is the
  // engine's own sentence — Portuguese today, English after spec 071 — and is never shown here.
  const localStatus = phaseText(localHost?.phase ?? "online");
  items.push({
    endpoint: "local",
    name: t("projects.host.thisComputer"),
    typeBadge: "Local",
    latencyText: localHost?.latency_ms != null ? `${localHost.latency_ms} ms` : "Local",
    statusText: localStatus,
    tone: localOnline ? "ok" : "idle",
    active: localActive,
    online: localOnline,
    retryable: false,
    tooltip: localStatus,
  });

  // 2. SSH hosts
  for (const host of sshHosts) {
    const isOnline = host.phase === "online";
    const isOffline = host.phase === "offline";
    const isReconnecting = host.phase === "reconnecting" || host.phase === "connecting";
    const isAttention = host.phase === "attention";
    // A failed connection (spec 029, AC-029-02): the reason replaces the phase text, the dot goes
    // to --error and Tentar novamente appears. Retrying hosts keep their countdown.
    const reason = failureReason(host);
    const failed = reason !== null && (isOffline || isAttention);

    let latencyText = t("projects.offline");
    // Same rule as Local: the phase in the user's language, never the engine's `phase_label`.
    // A failed connection still replaces it with its reason further down (AC-029-02).
    let statusText = phaseText(host.phase);
    let tone: "ok" | "idle" | "warn" | "attention" = "idle";

    if (isOnline) {
      latencyText = host.latency_ms != null ? `${host.latency_ms} ms` : t("projects.host.online");
      tone = "ok";
    } else if (isOffline) {
      latencyText = failed ? t("projects.host.attention") : t("projects.offline");
      statusText = failed ? reason! : statusText;
      tone = failed ? "attention" : "idle";
    } else if (isReconnecting) {
      latencyText = host.retry_in_ms != null ? `${Math.ceil(host.retry_in_ms / 1000)}s` : t("projects.host.connecting");
      tone = "warn";
    } else if (isAttention) {
      latencyText = t("projects.host.attention");
      statusText = reason ?? statusText;
      tone = "attention";
    }

    items.push({
      endpoint: host.endpoint,
      name: host.label || host.endpoint,
      typeBadge: "SSH",
      latencyText,
      statusText,
      tone,
      active: selectedEndpoint === host.endpoint,
      online: isOnline,
      retryable: failed,
      tooltip: tooltipOf(host, statusText),
    });
  }

  return items;
}

// ---------------------------------------------------------------------------------------
// Spec 025 — a project IS an engine workspace (AC-025-01/02): one tree per host, groups are
// desktop-only folders keyed by the root cwd. Live rows come from the engine's workspace list
// (carried on `host.workspaces`); saved projects without a live workspace are closed rows.
// ---------------------------------------------------------------------------------------

export interface WorkspaceRow {
  readonly id: string;
  readonly kind: "workspace" | "closed";
  readonly endpoint: string;
  readonly name: string;
  /** Branch of the workspace, null without git and `fechado` for a saved project not open. */
  readonly branch: string | null;
  readonly active: boolean;
  readonly focused: boolean;
  readonly cwd: string | null;
  readonly projectId: string | null;
  readonly workspaceId: string | null;
  readonly session: string;
  readonly allDots: readonly AgentDot[];
  readonly visibleDots: readonly AgentDot[];
  readonly overflowCount: number;
  readonly agentSummary: string;
  readonly opening: boolean;
  /** Offline host: the row is shown but clicking it sends nothing. */
  readonly disabled: boolean;
}

export interface WorkspaceGroupNode {
  readonly id: string;
  readonly name: string;
  readonly color: string;
  readonly count: number;
  readonly empty: boolean;
  readonly rows: readonly WorkspaceRow[];
}

export interface HostNode {
  readonly endpoint: string;
  readonly name: string;
  readonly typeBadge: "Local" | "SSH";
  /** `Este computador · Local`, `mac-mini · SSH`. */
  readonly header: string;
  readonly offline: boolean;
  /** Workspaces and closed projects without a group, straight under the host (like the TUI). */
  readonly rows: readonly WorkspaceRow[];
  readonly groups: readonly WorkspaceGroupNode[];
}

export interface WorkspaceTreeInput {
  readonly hosts: readonly HostDto[];
  readonly groups: readonly CollectionDto[];
  readonly projects: readonly ProjectDto[];
  readonly agents?: readonly AgentDto[];
  readonly opening?: Record<string, true>;
  readonly selectedEndpoint?: string | null;
}

function isEndpointSelected(endpoint: string, selectedEndpoint?: string | null): boolean {
  if (endpoint === "local") {
    return selectedEndpoint === "local" || selectedEndpoint == null;
  }
  return selectedEndpoint === endpoint;
}

function normalizeRoot(root: string): string {
  return root.replace(/[\\/]+$/, "") || root;
}

/**
 * A live workspace matches a saved root when the root is its main cwd or any cwd the engine
 * reported for its panes (spec 025 AC-025-05): a `cd` inside one pane must never turn an open
 * workspace into a closed project. Never matches on a missing cwd.
 */
export function workspaceMatchesRoot(
  workspace: { cwd?: string | null; cwds?: readonly string[] },
  root: string,
): boolean {
  const target = normalizeRoot(root);
  if (workspace.cwd && normalizeRoot(workspace.cwd) === target) return true;
  return (workspace.cwds ?? []).some((cwd) => normalizeRoot(cwd) === target);
}

/** Every root cwd the live workspace claims: the main one plus the cwds of its panes. */
function workspaceRoots(workspace: { cwd?: string | null; cwds?: readonly string[] }): string[] {
  return [...(workspace.cwd ? [workspace.cwd] : []), ...(workspace.cwds ?? [])];
}

function workspaceRow(
  host: HostDto,
  workspace: NonNullable<HostDto["workspaces"]>[number],
  project: ProjectDto | null,
  agents: readonly AgentDto[],
  offline: boolean,
  opening: Record<string, true>,
  active: boolean,
): WorkspaceRow {
  const dots: AgentDot[] = [];
  for (const agent of agents) {
    if (agent.workspace_id !== workspace.workspace_id) continue;
    const dot = mapAgentStatus(agent.status);
    if (dot) dots.push(dot);
  }
  const branch = workspace.branch?.trim() || null;
  return {
    id: workspace.workspace_id,
    kind: "workspace",
    endpoint: host.endpoint,
    name: workspace.label.trim() || project?.label || workspace.workspace_id,
    branch,
    active,
    focused: workspace.focused,
    cwd: workspace.cwd,
    projectId: project?.id ?? null,
    workspaceId: workspace.workspace_id,
    session: host.session,
    allDots: dots,
    visibleDots: dots.slice(0, 3),
    overflowCount: Math.max(0, dots.length - 3),
    agentSummary: formatAgentSummary(dots),
    opening: opening[workspace.workspace_id] === true,
    disabled: offline,
  };
}

function closedRow(
  host: HostDto,
  project: ProjectDto,
  agents: readonly AgentDto[],
  opening: Record<string, true>,
): WorkspaceRow {
  const dots: AgentDot[] = [];
  for (const agent of agents) {
    if (agent.workspace_id !== project.id) continue;
    const dot = mapAgentStatus(agent.status);
    if (dot) dots.push(dot);
  }
  return {
    id: `closed:${project.id}`,
    kind: "closed",
    endpoint: host.endpoint,
    name: project.label,
    branch: t("projects.row.closed"),
    active: false,
    focused: false,
    cwd: project.root,
    projectId: project.id,
    workspaceId: null,
    session: project.session_name,
    allDots: dots,
    visibleDots: dots.slice(0, 3),
    overflowCount: Math.max(0, dots.length - 3),
    agentSummary: formatAgentSummary(dots),
    opening: opening[project.id] === true,
    disabled: false,
  };
}

export function buildWorkspaceTree(input: WorkspaceTreeInput): readonly HostNode[] {
  const { projects, agents = [], opening = {}, selectedEndpoint } = input;
  const local = input.hosts.filter((host) => host.kind === "local");
  const ssh = input.hosts.filter((host) => host.kind !== "local");
  const ordered = [...local, ...ssh];

  const groupOfProject = new Map<string, string>();
  for (const group of input.groups) {
    for (const id of group.project_ids) {
      if (!groupOfProject.has(id)) groupOfProject.set(id, group.id);
    }
  }

  const groupsWithMembers = new Set<string>();
  const tree = ordered.map((host) => {
    const offline = host.phase !== "online";
    const isSsh = host.kind !== "local";
    const isHostSelected = isEndpointSelected(host.endpoint, selectedEndpoint);
    const workspaces = [...(host.workspaces ?? [])].sort((a, b) => a.number - b.number);
    const savedByRoot = new Map<string, ProjectDto>();
    for (const project of projects) {
      if (project.endpoint_profile_id !== host.endpoint) continue;
      savedByRoot.set(normalizeRoot(project.root), project);
    }

    const loose: WorkspaceRow[] = [];
    const perGroup = new Map<string, WorkspaceRow[]>();
    const liveRoots = new Set<string>();
    const place = (row: WorkspaceRow) => {
      const groupId = row.projectId ? groupOfProject.get(row.projectId) : undefined;
      if (!groupId) {
        loose.push(row);
        return;
      }
      groupsWithMembers.add(groupId);
      perGroup.set(groupId, [...(perGroup.get(groupId) ?? []), row]);
    };

    for (const workspace of workspaces) {
      // AC-025-05: any cwd of the workspace may identify the saved project; every cwd keeps the
      // root "live", so no closed row is emitted for a workspace that is open.
      let project: ProjectDto | null = null;
      for (const root of workspaceRoots(workspace)) {
        liveRoots.add(normalizeRoot(root));
        if (!project) project = savedByRoot.get(normalizeRoot(root)) ?? null;
      }
      // Spec 036 (AC-036-01): exactly one line has active: the focused workspace of the selected host.
      const active = isHostSelected && workspace.focused;
      place(workspaceRow(host, workspace, project, agents, offline, opening, active));
    }
    for (const project of projects) {
      if (project.endpoint_profile_id !== host.endpoint) continue;
      if (liveRoots.has(normalizeRoot(project.root))) continue;
      place(closedRow(host, project, agents, opening));
    }

    return {
      endpoint: host.endpoint,
      name: host.label || host.endpoint,
      typeBadge: (isSsh ? "SSH" : "Local") as "Local" | "SSH",
      header: isSsh ? `${host.label || host.endpoint} · SSH` : `${t("projects.host.thisComputer")} · Local`,
      offline,
      rows: loose,
      groups: perGroup,
    };
  });

  return tree.map((host, index) => {
    const groups: WorkspaceGroupNode[] = input.groups.flatMap((group, position) => {
      const rows = host.groups.get(group.id) ?? [];
      // A group with members on this host; a group with none anywhere stays visible under the
      // first host so `Novo grupo` has somewhere to live (never fabricated from nothing).
      if (rows.length === 0 && !(index === 0 && !groupsWithMembers.has(group.id))) return [];
      return [
        {
          id: group.id,
          name: group.name,
          color: groupColor(position),
          count: rows.length,
          empty: rows.length === 0,
          rows,
        },
      ];
    });
    return { ...host, groups };
  });
}
