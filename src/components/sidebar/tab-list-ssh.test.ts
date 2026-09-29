// @vitest-environment happy-dom
// Spec 057 — the sidebar's nested tabs on a host without the agents API lane (039,
// `HostDto.api === false`): the list is read from the hub snapshot, as the top bar already does
// (`WorkspaceTabs.svelte` `isSnapshotOnly`), and the focused workspace of that host keeps both the
// highlight and the single `tab.focus` a click sends.
// Would catch: the measured bug (the list attributed to `ctx.agents`, which carries no tab of that
// host, leaving the row with no tabs while the bar shows two), a title that drifts from the bar's
// for the same tab, a highlight on the tab that is not the snapshot's focused one, a click routed
// through the 043 chain (host switch + `workspace_focus`) instead of the bar's `focusTab`, and the
// snapshot decision leaking into a host that does answer the API lane (042/052 unchanged).
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, TabDto } from "../../agents/types";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto } from "../../connections/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import WorkspaceTabs from "../center/WorkspaceTabs.svelte";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import type { MenuAction } from "../frame/menus";
import type { SidebarRow } from "./sidebar-model";
import WorkspaceTabList from "./WorkspaceTabList.svelte";

const ENDPOINT = "mac-mini";
const WS = "w1";

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
  vi.useRealTimers();
});

const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

/**
 * The host of the report (2026-09-24): an SSH Herdr whose API lane is absent, so `tab.list` is
 * never answered and the hub snapshot is the only source of its tabs — `w1` focused with `codex`
 * (its active tab) and `claude`, plus a tab of `w2` that must never be listed under `w1`.
 */
function sshHost(overrides: Partial<HostDto> = {}): HostDto {
  return hostFixture({
    endpoint: ENDPOINT,
    label: "mac-mini",
    kind: "ssh",
    session: "hd057-remote",
    target: "user@mac-mini",
    phase: "online",
    phase_label: "Online",
    generation: 1,
    boot_id: "boot-057",
    api: false,
    workspaces: [
      {
        workspace_id: WS,
        number: 1,
        label: "acme-web-works",
        focused: true,
        tab_count: 2,
        pane_count: 2,
        active_tab_id: "w1:t1",
        agent_status: "working",
        cwd: "/srv/acme-web-works",
        branch: "main",
      },
      {
        workspace_id: "w2",
        number: 2,
        label: "outro",
        focused: false,
        tab_count: 1,
        pane_count: 1,
        active_tab_id: "w2:t1",
        agent_status: "idle",
        cwd: "/srv/outro",
        branch: null,
      },
    ],
    // Unnamed tabs (the engine sends the position as `label`): the title is the app each pane runs,
    // which is exactly what the user saw in the bar — `codex` and `claude`.
    tabs: [
      { tab_id: "w1:t2", workspace_id: WS, number: 2, label: "2", custom_label: false, focused: false, zoomed: false, pane_count: 1, agent_status: "working" },
      { tab_id: "w1:t1", workspace_id: WS, number: 1, label: "1", custom_label: false, focused: true, zoomed: false, pane_count: 1, agent_status: "idle" },
      { tab_id: "w2:t1", workspace_id: "w2", number: 1, label: "1", custom_label: false, focused: false, zoomed: false, pane_count: 1, agent_status: "idle" },
    ],
    panes: [
      { pane_id: "w1:p1", workspace_id: WS, tab_id: "w1:t1", focused: true, input_enabled: true, input_block: null, target: null, cwd: "/srv/acme-web-works", foreground_cwd: "/srv/acme-web-works", title: "codex", terminal_title: "codex", agent: "codex", agent_status: "idle" },
      { pane_id: "w1:p2", workspace_id: WS, tab_id: "w1:t2", focused: false, input_enabled: true, input_block: null, target: null, cwd: "/srv/acme-web-works", foreground_cwd: "/srv/acme-web-works", title: "claude", terminal_title: "claude", agent: "claude", agent_status: "working" },
      { pane_id: "w2:p1", workspace_id: "w2", tab_id: "w2:t1", focused: false, input_enabled: true, input_block: null, target: null, cwd: "/srv/outro", foreground_cwd: "/srv/outro", title: "Shell", terminal_title: "Shell", agent: null, agent_status: "idle" },
    ],
    agents: [
      { pane_id: "w1:p1", workspace_id: WS, tab_id: "w1:t1", name: "codex", display_agent: "codex", agent: "codex", title: "codex", terminal_title: "codex", terminal_title_stripped: "codex", agent_status: "idle", focused: true },
      { pane_id: "w1:p2", workspace_id: WS, tab_id: "w1:t2", name: "claude", display_agent: "claude", agent: "claude", title: "claude", terminal_title: "claude", terminal_title_stripped: "claude", agent_status: "working", focused: false },
    ],
    ...overrides,
  });
}

function tabDto(overrides: Partial<TabDto> & { tab_id: string; number: number; workspace_id: string }): TabDto {
  return { label: String(overrides.number), focused: false, pane_count: 1, agent_status: "idle", ...overrides };
}

function agentDto(overrides: Partial<AgentDto> & { pane_id: string; tab_id: string; workspace_id: string; status: AgentStatus }): AgentDto {
  return { name: overrides.kind ?? null, kind: null, launch_pending: false, ready: true, focused: false, state_change_seq: 1, ...overrides };
}

/**
 * The connection this window holds. On the host of the report it answers no `tab.list`, so it
 * carries nothing of `w1` — the bug was reading the list from here anyway.
 */
function agentsState(
  overrides: { tabs?: TabDto[]; agents?: AgentDto[]; focusedTabId?: string | null; phase?: AgentsState["phase"] } = {},
): AgentsState {
  return {
    phase: overrides.phase ?? "connected",
    identity: null,
    agents: overrides.agents ?? [],
    tabs: overrides.tabs ?? [],
    tabFocus:
      overrides.focusedTabId === undefined || overrides.focusedTabId === null
        ? null
        : { tab_id: overrides.focusedTabId, endpoint: ENDPOINT, session: "hd057-remote", connection_generation: 1, boot_id: "boot-057", revision: 1 },
    topology: null,
    capabilities: null,
    kinds: [],
  } as unknown as AgentsState;
}

function surface(): SurfaceState {
  return {
    selection: { endpoint: ENDPOINT, kind: "ssh", label: "mac-mini", session: "hd057-remote", online: true, identity: null },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  };
}

function sidebarRow(overrides: Partial<SidebarRow> = {}): SidebarRow {
  return {
    id: WS,
    kind: "workspace",
    endpoint: ENDPOINT,
    name: "acme-web-works",
    branch: "main",
    // Spec 036: the focused workspace of the selected host is the one with `active`.
    active: true,
    focused: true,
    cwd: "/srv/acme-web-works",
    projectId: "p-acme-web",
    workspaceId: WS,
    session: "hd057-remote",
    allDots: [],
    visibleDots: [],
    overflowCount: 0,
    agentSummary: "",
    opening: false,
    disabled: false,
    hostBadge: null,
    offline: false,
    status: { working: 0, waiting: 0, done: 0, idle: 0, kind: "idle", label: "" },
    prefRoot: "/srv/acme-web-works",
    color: null,
    tileColor: "#8FA8FF",
    pinned: false,
    hidden: false,
    ...overrides,
  } as SidebarRow;
}

interface RenderOptions {
  host?: HostDto;
  state?: AgentsState;
  row?: SidebarRow;
}

function render(options: RenderOptions = {}) {
  const holder = reactiveHolder<AgentsState | null>(options.state ?? agentsState());
  const focusTab = vi.fn(async () => {});
  const focusWorkspace = vi.fn(async () => {});
  const select = vi.fn(async () => ({}) as never);
  const ctx: FrameContext = {
    surface: surface(),
    get agents() {
      return holder.value;
    },
    connections: {
      view: { hub: { revision: 1, hosts: [options.host ?? sshHost()] }, profiles: [], store_error: null },
    } as unknown as FrameContext["connections"],
    navigator: null,
    selectedEndpoint: ENDPOINT,
    activeProject: null,
    hostLabels: { [ENDPOINT]: "mac-mini" },
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: false,
    controllers: {
      surface: { select } as unknown as FrameContext["controllers"]["surface"],
      agents: { focusTab } as unknown as FrameContext["controllers"]["agents"],
      projects: { focusWorkspace } as unknown as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspaceTabList, { target: el, props: { ctx, row: options.row ?? sidebarRow(), clock: () => 1_700_000_000_000 } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, ctx, holder, focusTab, focusWorkspace, select };
}

/** The same tabs rendered by the top bar, on the same ctx: the title oracle of AC-057-01. */
function renderBar(ctx: FrameContext) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspaceTabs, { target: el, props: { ctx, layoutWidth: 1440 } });
  flushSync();
  mounted.push({ el, app: app as never });
  return el;
}

const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("[data-tab-row]")];

describe("AC-057-01 tabs of an SSH host without the agents lane", () => {
  // Would catch the measured bug: the list read from `ctx.agents` (which holds no tab of this
  // host) leaves the row empty while the bar shows the two snapshot tabs.
  it("lists the two snapshot tabs of the focused workspace, in number order and without w2's", () => {
    const { el } = render();
    expect(rows(el).map((r) => r.dataset.tabRow)).toEqual(["w1:t1", "w1:t2"]);
  });

  // Would catch a title recomputed here instead of the one the bar renders for the same tab.
  it("shows for each tab the same title the top bar shows", () => {
    const { el, ctx } = render();
    const bar = renderBar(ctx);
    const barTitle = (id: string) => bar.querySelector<HTMLElement>(`[data-tab="${id}"] [data-label]`)!.textContent;
    expect(bar.querySelectorAll("[data-tab]")).toHaveLength(2);
    expect(barTitle("w1:t1")).toBe("codex");
    expect(barTitle("w1:t2")).toBe("claude");
    for (const id of ["w1:t1", "w1:t2"]) {
      expect(el.querySelector<HTMLElement>(`[data-tab-row="${id}"] [data-tab-title]`)!.textContent, id).toBe(barTitle(id));
    }
  });

  // Would catch a list with no highlight at all (the snapshot's focused tab ignored) or a second
  // selected row.
  it("marks only the snapshot's focused tab as selected", () => {
    const { el } = render();
    expect(rows(el).filter((r) => r.dataset.selected === "true").map((r) => r.dataset.tabRow)).toEqual(["w1:t1"]);
    expect(el.querySelector<HTMLElement>('[data-tab-row="w1:t1"]')!.getAttribute("aria-current")).toBe("true");
  });

  // Would catch a highlight taken from `focused` on the host's tab list instead of the workspace's
  // `active_tab_id`, which is what the bar reads first.
  it("follows the workspace's active_tab_id when it disagrees with the tab's focused flag", () => {
    const host = sshHost();
    const moved: HostDto = {
      ...host,
      workspaces: host.workspaces!.map((w) => (w.workspace_id === WS ? { ...w, active_tab_id: "w1:t2" } : w)),
    };
    const { el, ctx } = render({ host: moved });
    const bar = renderBar(ctx);
    expect(bar.querySelector<HTMLElement>('[data-tab="w1:t2"]')!.dataset.active).toBe("true");
    expect(rows(el).filter((r) => r.dataset.selected === "true").map((r) => r.dataset.tabRow)).toEqual(["w1:t2"]);
  });
});

describe("AC-057-02 focusing a snapshot tab from the sidebar", () => {
  // Would catch a click routed through the 043 chain (host switch + `workspace_focus`, whose
  // `tab.focus` never settles because this connection lists no such tab) instead of the bar's path.
  it("calls focusTab once with the tab id, and nothing else", () => {
    const { el, focusTab, focusWorkspace, select } = render();
    el.querySelector<HTMLElement>('[data-tab-row="w1:t2"]')!.click();
    flushSync();
    expect(focusTab).toHaveBeenCalledExactlyOnceWith("w1:t2");
    expect(focusWorkspace).not.toHaveBeenCalled();
    expect(select).not.toHaveBeenCalled();
  });

  // Would catch the focused tab re-sent on every click (a needless engine round trip).
  it("sends nothing when the tab already focused in the snapshot is clicked", () => {
    const { el, focusTab, focusWorkspace, select } = render();
    el.querySelector<HTMLElement>('[data-tab-row="w1:t1"]')!.click();
    flushSync();
    expect(focusTab).not.toHaveBeenCalled();
    expect(focusWorkspace).not.toHaveBeenCalled();
    expect(select).not.toHaveBeenCalled();
  });
});

describe("AC-057-03 a host that does answer the agents lane", () => {
  // Would catch the snapshot decision widened to every SSH host: with the API lane the live
  // connection stays the source (042/052), even when the snapshot still carries older tabs.
  it("keeps reading the live connection when api is not false", () => {
    const live = agentsState({
      focusedTabId: "w1:t9",
      tabs: [
        tabDto({ tab_id: "w1:t9", number: 1, workspace_id: WS, label: "ao vivo", focused: true, agent_status: "working" }),
        tabDto({ tab_id: "w1:t8", number: 2, workspace_id: WS, label: "também ao vivo" }),
      ],
      agents: [agentDto({ pane_id: "w1:p9", tab_id: "w1:t9", workspace_id: WS, kind: "claude", status: "working", focused: true })],
    });
    const { el } = render({ host: sshHost({ api: true }), state: live });
    expect(rows(el).map((r) => r.dataset.tabRow)).toEqual(["w1:t9", "w1:t8"]);
    expect(rows(el).filter((r) => r.dataset.selected === "true").map((r) => r.dataset.tabRow)).toEqual(["w1:t9"]);
    expect(el.querySelector<HTMLElement>('[data-tab-row="w1:t9"] [data-tab-title]')!.textContent).toBe("ao vivo");
  });
});

// ---------------------------------------------------------------------------------------
// Spec 074 — the same SSH host, now **with** the agents API lane, while the attach that will fill
// the live list has not answered yet (`tabs: []`, `identity: null`, phase `idle`/`connecting`/
// `connected`): the hub snapshot is the only source the window has of it, and the sidebar must read
// it instead of showing nothing until a tab is created.
// ---------------------------------------------------------------------------------------

/**
 * The host of the report (2026-09-28): `mac-mini` online with the API lane, the workspace `~`
 * focused with a single `shell` tab, plus a tab of another workspace that must never be listed
 * under `~`. No agent runs in it, so the tab is titled `shell` in the bar and in the sidebar.
 */
function pendingSshHost(overrides: Partial<HostDto> = {}): HostDto {
  return sshHost({
    api: true,
    workspaces: [
      {
        workspace_id: WS,
        number: 1,
        label: "~",
        focused: true,
        tab_count: 1,
        pane_count: 1,
        active_tab_id: "w1:t1",
        agent_status: "idle",
        cwd: "/Users/user",
        branch: null,
      },
      {
        workspace_id: "w2",
        number: 2,
        label: "outro",
        focused: false,
        tab_count: 1,
        pane_count: 1,
        active_tab_id: "w2:t1",
        agent_status: "idle",
        cwd: "/Users/user/outro",
        branch: null,
      },
    ],
    tabs: [
      { tab_id: "w1:t1", workspace_id: WS, number: 1, label: "1", custom_label: false, focused: true, zoomed: false, pane_count: 1, agent_status: "idle" },
      { tab_id: "w2:t1", workspace_id: "w2", number: 1, label: "1", custom_label: false, focused: false, zoomed: false, pane_count: 1, agent_status: "idle" },
    ],
    panes: [
      { pane_id: "w1:p1", workspace_id: WS, tab_id: "w1:t1", focused: true, input_enabled: true, input_block: null, target: null, cwd: "/Users/user", foreground_cwd: "/Users/user", title: "shell", terminal_title: "shell", agent: null, agent_status: "idle" },
      { pane_id: "w2:p1", workspace_id: "w2", tab_id: "w2:t1", focused: false, input_enabled: true, input_block: null, target: null, cwd: "/Users/user/outro", foreground_cwd: "/Users/user/outro", title: "shell", terminal_title: "shell", agent: null, agent_status: "idle" },
    ],
    agents: [],
    ...overrides,
  });
}

/** The reported host after `+`: two `shell` tabs in `~`, the first one still the active tab. */
function pendingSshHostWithTwoTabs(): HostDto {
  const host = pendingSshHost();
  return {
    ...host,
    workspaces: (host.workspaces ?? []).map((w) => (w.workspace_id === WS ? { ...w, tab_count: 2, pane_count: 2 } : w)),
    tabs: [
      ...(host.tabs ?? []),
      { tab_id: "w1:t2", workspace_id: WS, number: 2, label: "2", custom_label: false, focused: false, zoomed: false, pane_count: 1, agent_status: "idle" },
    ],
    panes: [
      ...host.panes,
      { pane_id: "w1:p2", workspace_id: WS, tab_id: "w1:t2", focused: false, input_enabled: true, input_block: null, target: null, cwd: "/Users/user", foreground_cwd: "/Users/user", title: "shell", terminal_title: "shell", agent: null, agent_status: "idle" },
    ],
  };
}

describe("AC-074-02 the snapshot fills the sidebar while the live list is still empty", () => {
  // Would catch the reported bug: the empty live list attributed to the just-selected host, which
  // left the expanded row with no tab at all while the bar already showed `shell`.
  for (const phase of ["idle", "connecting", "connected"] as const) {
    it(`lists the snapshot's shell tab of the workspace in the ${phase} phase, with the bar's ids`, () => {
      const { el, ctx } = render({ host: pendingSshHost(), state: agentsState({ phase }) });
      const bar = renderBar(ctx);
      const barIds = [...bar.querySelectorAll<HTMLElement>("[data-tab]")].map((node) => node.dataset.tab);
      expect(rows(el).map((r) => r.dataset.tabRow), phase).toEqual(["w1:t1"]);
      expect(el.querySelector<HTMLElement>('[data-tab-row="w1:t1"] [data-tab-title]')!.textContent, phase).toBe("shell");
      expect(rows(el).map((r) => r.dataset.tabRow), `sidebar and bar agree in ${phase}`).toEqual(barIds);
    });
  }
});

describe("AC-074-03 highlight and focus with the snapshot as the sidebar's fallback", () => {
  // Would catch a fallback list without the highlight of the workspace's active tab (P7), or one
  // that marks every row.
  it("marks only the workspace's active tab as selected", () => {
    const { el } = render({ host: pendingSshHostWithTwoTabs(), state: agentsState() });
    expect(rows(el).map((r) => r.dataset.tabRow)).toEqual(["w1:t1", "w1:t2"]);
    expect(rows(el).filter((r) => r.dataset.selected === "true").map((r) => r.dataset.tabRow)).toEqual(["w1:t1"]);
  });

  // Would catch a click routed through the 043 chain (host switch + `workspace_focus`) because the
  // fallback list is no longer read as the attached host's, and the focused row re-sending focus.
  it("sends one tab.focus and nothing else, and nothing for the tab already active", () => {
    const { el, focusTab, focusWorkspace, select } = render({
      host: pendingSshHostWithTwoTabs(),
      state: agentsState(),
    });
    el.querySelector<HTMLElement>('[data-tab-row="w1:t2"]')!.click();
    flushSync();
    expect(focusTab).toHaveBeenCalledExactlyOnceWith("w1:t2");
    expect(focusWorkspace).not.toHaveBeenCalled();
    expect(select).not.toHaveBeenCalled();

    el.querySelector<HTMLElement>('[data-tab-row="w1:t1"]')!.click();
    flushSync();
    expect(focusTab).toHaveBeenCalledExactlyOnceWith("w1:t2");
  });
});
