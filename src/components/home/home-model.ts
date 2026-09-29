// Spec 012 — what the home screen of the design shows. Every number (projects, groups, agents
// waiting), name, branch, path, host state and last activity comes from the reducers/engine and
// is derived here from the 011 tree and the 014 agent states; nothing is simulated.
//
// Thumbnails are *static* `pane.read` snapshots: one read per card when the screen mounts and
// one when it is revisited (never more than THUMBNAIL_READS_PER_VISIT per visit), never a frame,
// a timer or a poll — the store below does not even know about requestAnimationFrame.

import { normalizeStatus } from "../../agents/status";
import { phaseText, t } from "../../i18n/index.svelte";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus } from "../../agents/types";
import type { HostDto } from "../../connections/types";
import {
  counters,
  relativeTime,
  runningRows,
  UNOBSERVED_TIME,
  type ActivityLedger,
  type PanelAgent,
} from "../agents/panel";
import type { NavigatorState } from "../../projects/reducer";
import { buildTree, formatAgentSummary } from "../projects/tree-model";

/** Cards requested from `pane.read` per thumbnail; the box clips them. */
export const THUMBNAIL_LINES = 8;
/** Up to three thumbnails per card, as in the design. */
export const THUMBNAIL_LIMIT = 3;
/** Reads allowed per card inside one visit (mount + return); the schedule never uses more. */
export const THUMBNAIL_READS_PER_VISIT = 2;
/** A group with more projects than this starts collapsed. */
export const GROUP_COLLAPSE_LIMIT = 12;
/** Shown while the window knows no collection at all. */
export function emptyText(): string {
  return t("home.empty");
}
/** Stands in for a user the backend could not name. */
export function emptyUser(): string {
  return t("home.user");
}

// ----------------------------------------------------------------------------------- greeting

export function greetingFor(hour: number, user: string): string {
  const period = t(hour < 12 ? "home.greeting.morning" : hour < 18 ? "home.greeting.afternoon" : "home.greeting.evening");
  const name = user.trim() || emptyUser();
  return t("home.greeting", { period, name });
}

export interface HomeSummary {
  projects: number;
  groups: number;
  waiting: number;
  text: string;
}

/** The summary of AC-012-01: the same counts the 011 tree and the 014 panel publish. */
export function homeSummary(navigator: NavigatorState | null, agents: AgentsState | null): HomeSummary {
  const projects = navigator?.snapshot?.projects.length ?? 0;
  const groups = navigator?.snapshot?.collections.length ?? 0;
  const waiting = counters(agents?.agents ?? []).waiting;
  return {
    projects,
    groups,
    waiting,
    text: t("home.summary", { projects, groups, waiting }),
  };
}

// ---------------------------------------------------------------------------------- thumbnails

export interface ThumbnailDot {
  status: "working" | "blocked" | "idle" | "unknown";
  color: string;
  label: string;
  /** Amber highlight of the agent waiting for the user (AC-012-02). */
  waiting: boolean;
}

/** Same mapping as the 011 tree, with unknown kept visible and explicitly labeled. */
export function thumbnailDot(status: AgentStatus): ThumbnailDot {
  switch (normalizeStatus(status)) {
    case "working":
      return { status: "working", color: "var(--working, #5BD68A)", label: t("home.dot.working"), waiting: false };
    case "blocked":
      return { status: "blocked", color: "var(--attention, #F4B454)", label: t("home.dot.blocked"), waiting: true };
    case "idle":
    case "done":
      return { status: "idle", color: "var(--idle, #6B7280)", label: t("home.dot.idle"), waiting: false };
    default:
      return { status: "unknown", color: "var(--idle, #6B7280)", label: t("home.dot.unknown"), waiting: false };
  }
}

export interface HomeThumbnail {
  paneId: string;
  name: string;
  kind: string;
  dot: ThumbnailDot;
  /** Static `pane.read` snapshot of this visit; null while unread or unavailable. */
  text: string | null;
}

export interface HomeActivity {
  avatars: string[];
  summary: string | null;
  time: string;
}

export interface HomeCard {
  id: string;
  name: string;
  branch: string;
  /** `host:/root` for SSH projects; the root itself for Local ones. */
  path: string;
  hostBadge: string | null;
  endpoint: string;
  isSsh: boolean;
  thumbnails: HomeThumbnail[];
  overflow: number;
  activity: HomeActivity;
}

export interface HomeGroup {
  id: string;
  name: string;
  color: string;
  count: number;
  /** `local`, `SSH` or `local + SSH`, as the group's projects are (design's group subtitle). */
  hostSummary: string;
  agentSummary: string;
  cards: HomeCard[];
}

export interface AttentionChip {
  paneId: string;
  projectId: string;
  projectName: string;
  name: string;
  kind: string;
  /** Project and the engine's own line/title, as received. */
  label: string;
  time: string;
}

export interface HomeServer {
  endpoint: string;
  name: string;
  typeBadge: "Local" | "SSH";
  detail: string;
  state: string;
  tone: "ok" | "offline" | "warn" | "attention";
  active: boolean;
}

export interface HomeView {
  greeting: string;
  summary: HomeSummary;
  empty: boolean;
  emptyText: string;
  cache: boolean;
  attention: AttentionChip[];
  groups: HomeGroup[];
  servers: HomeServer[];
}

export interface HomeInput {
  hour: number;
  user: string;
  navigator: NavigatorState | null;
  agents: AgentsState | null;
  hosts: readonly HostDto[];
  selectedEndpoint: string | null;
  hostLabels: Record<string, string>;
  snapshots: ThumbnailSnapshots;
  ledger: ActivityLedger;
  now: number;
  /** The selected host is live: snapshots may be read and servers are current. */
  connected: boolean;
}

function activityOf(agents: AgentDto[], ledger: ActivityLedger, now: number, snapshot: NavigatorState | null, tabs: AgentsState["tabs"]): HomeActivity {
  const rows: PanelAgent[] = runningRows(agents, { ledger, now, snapshot: snapshot?.snapshot ?? null, tabs });
  const latest = rows[0];
  return {
    avatars: rows.slice(0, THUMBNAIL_LIMIT).map((row) => row.avatar),
    summary: latest?.summary ?? null,
    time: latest ? relativeTime(latest.since, now) : UNOBSERVED_TIME,
  };
}

function hostSummaryOf(projects: readonly { isSsh: boolean }[]): string {
  if (projects.length === 0) return "local";
  const kinds = new Set<string>(projects.map((project) => (project.isSsh ? "SSH" : "local")));
  return ["local", "SSH"].filter((kind) => kinds.has(kind)).join(" + ");
}

export function buildHomeView(input: HomeInput): HomeView {
  const snapshot = input.navigator?.snapshot ?? null;
  const agents = input.agents?.agents ?? [];
  const summary = homeSummary(input.navigator, input.agents);
  const empty = summary.groups === 0;

  const tree = buildTree(
    snapshot?.collections ?? [],
    snapshot?.projects ?? [],
    null,
    input.hostLabels,
    agents,
  );

  const groups: HomeGroup[] = tree.map((group) => ({
    id: group.id,
    name: group.name,
    color: group.color,
    count: group.count,
    hostSummary: hostSummaryOf(group.projects),
    agentSummary: formatAgentSummary(group.projects.flatMap((project) => project.allDots)),
    cards: group.projects.map((project) => {
      const workspace = project.workspaceId;
      const projectAgents = workspace ? agents.filter((agent) => agent.workspace_id === workspace) : [];
      const thumbnails: HomeThumbnail[] = projectAgents.slice(0, THUMBNAIL_LIMIT).map((agent) => ({
        paneId: agent.pane_id,
        name: agent.name ?? agent.pane_id,
        kind: agent.kind ?? "",
        dot: thumbnailDot(agent.status),
        text: input.snapshots.text(agent.pane_id),
      }));
      return {
        id: project.id,
        name: project.name,
        branch: project.branch,
        path: project.isSsh && project.hostBadge ? `${project.hostBadge}:${project.root}` : project.root,
        hostBadge: project.hostBadge,
        endpoint: project.endpoint,
        isSsh: project.isSsh,
        thumbnails,
        overflow: Math.max(0, projectAgents.length - THUMBNAIL_LIMIT),
        activity: activityOf(projectAgents, input.ledger, input.now, input.navigator, input.agents?.tabs ?? []),
      };
    }),
  }));

  const chips: AttentionChip[] = agents
    .filter((agent) => normalizeStatus(agent.status) === "blocked")
    .map((agent) => {
      const project = snapshot?.projects.find((candidate) => candidate.binding?.workspace_id === agent.workspace_id);
      const activity = activityOf([agent], input.ledger, input.now, input.navigator, input.agents?.tabs ?? []);
      const line = agent.detection_last_line?.trim() || agent.terminal_title?.trim() || "";
      const name = agent.name ?? agent.pane_id;
      return {
        paneId: agent.pane_id,
        projectId: project?.id ?? "",
        projectName: project?.label ?? agent.workspace_id,
        name,
        kind: agent.kind ?? "",
        label: line && line !== name ? `${project?.label ?? agent.workspace_id} · ${line}` : (project?.label ?? agent.workspace_id),
        time: activity.time,
      };
    })
    .filter((chip) => chip.projectId !== "");

  return {
    greeting: greetingFor(input.hour, input.user),
    summary,
    empty,
    emptyText: emptyText(),
    cache: !input.connected,
    attention: empty ? [] : chips,
    groups: empty ? [] : groups,
    servers: empty ? [] : buildServerCards(input.hosts, input.selectedEndpoint),
  };
}

// ---------------------------------------------------------------------------------- servers

export function buildServerCards(hosts: readonly HostDto[], selectedEndpoint: string | null): HomeServer[] {
  const local = hosts.find((host) => host.endpoint === "local");
  const ssh = hosts.filter((host) => host.endpoint !== "local");
  const cards: HomeServer[] = [];

  if (local) {
    const online = local.phase === "online";
    const version = local.server_version?.trim();
    const panes = local.panes.length;
    cards.push({
      endpoint: local.endpoint,
      // Spec 072 (AC-072-01): the Local host is named by the product. Since 071 its `label` is
      // the host's own English `This computer` and never empty, so the old `label ||` fallback
      // could not fire; the name of a host the user did not name is not the engine's to send.
      name: t("home.server.thisComputer"),
      typeBadge: "Local",
      detail: version
        ? t("home.server.detail", { version, panes: t("home.server.panes", { count: panes }) })
        : t("home.server.detailNoVersion", { panes: t("home.server.panes", { count: panes }) }),
      // The words of a phase come from the one table of 067 (AC-069-01/02); `phase_label` is the
      // engine's own text and is never shown.
      state: phaseText(local.phase),
      tone: online ? "ok" : local.phase === "offline" ? "offline" : "warn",
      active: selectedEndpoint === null || selectedEndpoint === local.endpoint,
    });
  }

  for (const host of ssh) {
    const online = host.phase === "online";
    const reconnecting = host.phase === "connecting" || host.phase === "reconnecting";
    cards.push({
      endpoint: host.endpoint,
      name: host.label || host.endpoint,
      typeBadge: "SSH",
      detail: host.target ?? host.endpoint,
      state: online && host.latency_ms != null ? `${host.latency_ms} ms` : phaseText(host.phase),
      tone: online ? "ok" : host.phase === "attention" ? "attention" : reconnecting ? "warn" : "offline",
      active: selectedEndpoint === host.endpoint,
    });
  }

  return cards;
}

// ----------------------------------------------------------------------------- group collapse

export function startsCollapsed(count: number): boolean {
  return count > GROUP_COLLAPSE_LIMIT;
}

export function collapsedGroupText(count: number, agentSummary: string): string {
  return t("home.group.collapsed", { projects: t("home.group.projects", { count }), agents: agentSummary });
}

/** New collapse map with the toggle of `id` applied; the component keeps it as its own state. */
export function toggledCollapse(
  collapsed: Record<string, boolean>,
  id: string,
  collapsedNow: boolean,
): Record<string, boolean> {
  return { ...collapsed, [id]: !collapsedNow };
}

// -------------------------------------------------------------------------------- chip action

export interface ChipActuator {
  /** Existing project open action: reveals the project's workspace on its host. */
  openProject(projectId: string): Promise<void> | void;
  /** Existing attention action: focuses the pane in the engine once, sends no key. */
  focusPane(paneId: string): Promise<void> | void;
}

export type ChipDispatcher = (chip: AttentionChip) => Promise<void>;

/**
 * Opens the project and then focuses its pane once. A chip already in flight is ignored, so a
 * double click never focuses twice; nothing is retried.
 */
export function createChipDispatcher(act: ChipActuator): ChipDispatcher {
  const inFlight = new Set<string>();
  return async (chip) => {
    if (inFlight.has(chip.paneId)) return;
    inFlight.add(chip.paneId);
    try {
      await act.openProject(chip.projectId);
      await act.focusPane(chip.paneId);
    } finally {
      inFlight.delete(chip.paneId);
    }
  };
}

// ----------------------------------------------------------------------------- snapshots

export interface ThumbnailReader {
  /** One `pane.read` of the pane; the bridge validated endpoint/session/generation/boot/pane. */
  read(paneId: string, lines: number): Promise<string>;
}

export interface ThumbnailPane {
  paneId: string;
  endpoint: string;
}

export interface ThumbnailSnapshots {
  /** Starts a visit: the screen mounted or the user came back to it. */
  beginVisit(): void;
  visits(): number;
  reads(paneId: string): number;
  readsThisVisit(paneId: string): number;
  text(paneId: string): string | null;
  /**
   * One read per pane that has no snapshot of this visit, never more than
   * THUMBNAIL_READS_PER_VISIT; panes of a host that cannot be read are skipped (cache) and a
   * pane already read in this visit is never read again (the snapshot is static).
   */
  ensure(panes: readonly ThumbnailPane[], canRead: (endpoint: string) => boolean): Promise<void>;
}

/** A read that failed or is refused leaves the last snapshot (or none) in place, never a poll. */
export function createThumbnailSnapshots(reader: ThumbnailReader): ThumbnailSnapshots {
  const texts = new Map<string, string>();
  const total = new Map<string, number>();
  let visit = 0;
  let thisVisit = new Map<string, number>();

  return {
    beginVisit() {
      visit += 1;
      thisVisit = new Map();
    },
    visits: () => visit,
    reads: (paneId) => total.get(paneId) ?? 0,
    readsThisVisit: (paneId) => thisVisit.get(paneId) ?? 0,
    text: (paneId) => texts.get(paneId) ?? null,
    async ensure(panes, canRead) {
      const wanted = new Set<string>();
      for (const pane of panes) {
        if (wanted.has(pane.paneId) || !canRead(pane.endpoint)) continue;
        // Static snapshot: one read per pane per visit (well inside the per-visit budget).
        if ((thisVisit.get(pane.paneId) ?? 0) > 0) continue;
        wanted.add(pane.paneId);
      }
      await Promise.all(
        Array.from(wanted).map(async (paneId) => {
          thisVisit.set(paneId, (thisVisit.get(paneId) ?? 0) + 1);
          total.set(paneId, (total.get(paneId) ?? 0) + 1);
          try {
            const text = await reader.read(paneId, THUMBNAIL_LINES);
            if (typeof text === "string") texts.set(paneId, text);
          } catch {
            // Host offline or refused: the card keeps its cached snapshot and shows no new one.
          }
        }),
      );
    },
  };
}
