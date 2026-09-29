// @vitest-environment happy-dom
// Spec 042 — the nested tabs of the open workspace, mounted with a fake ctx: rows, status icon,
// type avatar, title and selection (AC-042-01), the client-owned relative time with an injected
// clock (AC-042-02) and the single `tab.focus` a click sends (AC-042-03).
// Would catch: tabs of another workspace leaking into the row, a title that drifts from the one
// the center bar shows for the same tab, `done` folded into idle, a time that ages without the
// engine changing anything (or that never ages), and a click re-focusing the focused tab.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, TabDto } from "../../agents/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import WorkspaceTabs from "../center/WorkspaceTabs.svelte";
import type { SidebarRow } from "./sidebar-model";
import WorkspaceTabList from "./WorkspaceTabList.svelte";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010/041). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const WS = "w-api";

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

function tabDto(overrides: Partial<TabDto> & { tab_id: string; number: number }): TabDto {
  return {
    workspace_id: WS,
    label: String(overrides.number),
    focused: false,
    pane_count: 1,
    agent_status: "idle",
    ...overrides,
  };
}

function agentDto(overrides: Partial<AgentDto> & { pane_id: string; tab_id: string; status: AgentStatus }): AgentDto {
  return {
    workspace_id: WS,
    name: overrides.kind ?? null,
    kind: null,
    launch_pending: false,
    ready: true,
    focused: false,
    state_change_seq: 1,
    ...overrides,
  };
}

function agentsState(overrides: { tabs?: TabDto[]; agents?: AgentDto[]; focusedTabId?: string | null }): AgentsState {
  return {
    phase: "connected",
    agents: overrides.agents ?? [],
    tabs: overrides.tabs ?? [],
    tabFocus: overrides.focusedTabId === undefined ? null : { tab_id: overrides.focusedTabId, endpoint: "local", session: "hd042", connection_generation: 1, boot_id: "boot-042", revision: 1 },
    topology: null,
    capabilities: null,
    kinds: [],
  } as unknown as AgentsState;
}

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd042", online: true, identity: null },
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
    endpoint: "local",
    name: "erp-api",
    branch: "main",
    active: true,
    focused: true,
    cwd: "/w/api",
    projectId: "p-api",
    workspaceId: WS,
    session: "hd042",
    allDots: [],
    visibleDots: [],
    overflowCount: 0,
    agentSummary: "",
    opening: false,
    disabled: false,
    hostBadge: null,
    offline: false,
    hostPhase: "online",
    hostState: "online",
    hostStateText: null,
    hostStateTitle: null,
    stale: false,
    status: { working: 0, waiting: 0, done: 0, idle: 0, kind: "idle", label: "" },
    prefRoot: "/w/api",
    color: null,
    tileColor: "#8FA8FF",
    pinned: false,
    hidden: false,
    ...overrides,
  };
}

interface RenderOptions {
  state?: AgentsState;
  clock?: () => number;
  row?: SidebarRow;
}

function render(options: RenderOptions = {}) {
  const holder = reactiveHolder<AgentsState | null>(options.state ?? agentsState({}));
  const focusTab = vi.fn(async () => {});
  const createTab = vi.fn(async () => {});
  const closeTab = vi.fn(async () => {});
  const select = vi.fn(async () => ({}) as never);
  const ctx: FrameContext = {
    surface: surface(),
    get agents() {
      return holder.value;
    },
    connections: null,
    navigator: null,
    selectedEndpoint: "local",
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: false,
    controllers: {
      surface: { select } as unknown as FrameContext["controllers"]["surface"],
      agents: { focusTab, createTab, closeTab } as unknown as FrameContext["controllers"]["agents"],
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspaceTabList, { target: el, props: { ctx, row: options.row ?? sidebarRow(), clock: options.clock } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, ctx, holder, focusTab, createTab, closeTab, select };
}

/** The same tabs rendered by the center bar, mounted on the same ctx (title oracle). */
function renderBar(ctx: FrameContext) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspaceTabs, { target: el, props: { ctx, layoutWidth: 1440 } });
  flushSync();
  mounted.push({ el, app: app as never });
  return el;
}

const rows = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("[data-tab-row]")];

/**
 * AC-042-01 fixture, in the engine's arrival order (t2 before t1) so a row list that simply
 * mirrors the list instead of ordering by `number` is caught: `t1` claude working and focused,
 * `t2` codex blocked, `t3` without an agent, plus `t9` in another workspace.
 */
function tabsFixture(): AgentsState {
  return agentsState({
    focusedTabId: "t1",
    tabs: [
      tabDto({ tab_id: "t2", number: 2, label: "migração 0042", agent_status: "blocked" }),
      tabDto({ tab_id: "t1", number: 1, label: "refatorar faturamento", focused: true, agent_status: "working" }),
      tabDto({ tab_id: "t3", number: 3, label: "3" }),
      tabDto({ tab_id: "t9", number: 9, label: "de outro workspace", workspace_id: "w-other" }),
    ],
    agents: [
      agentDto({ pane_id: `${WS}:p1`, tab_id: "t1", kind: "claude", status: "working", focused: true }),
      agentDto({ pane_id: `${WS}:p2`, tab_id: "t2", kind: "codex", status: "blocked" }),
      agentDto({ pane_id: "w-other:p1", tab_id: "t9", kind: "claude", status: "working" }),
    ],
  });
}

describe("AC-042-01 rows of the open workspace", () => {
  // Would catch: the engine order kept instead of `number`, the tab of another workspace listed,
  // or a row per pane instead of per tab.
  it("renders one row per tab of the active workspace, ordered by number", () => {
    const { el } = render({ state: tabsFixture() });
    expect(rows(el).map((r) => r.dataset.tabRow)).toEqual(["t1", "t2", "t3"]);
  });

  // Would catch: `mapAgentStatus` folding done into idle, or one icon reused for every state.
  it("gives each state its own data-status, done included", () => {
    const { el } = render({ state: tabsFixture() });
    const status = (id: string) => el.querySelector<HTMLElement>(`[data-tab-row="${id}"] [data-status]`)!.dataset.status;
    expect([status("t1"), status("t2"), status("t3")]).toEqual(["working", "waiting", "idle"]);

    const done = render({
      state: agentsState({
        focusedTabId: "t1",
        tabs: [tabDto({ tab_id: "t1", number: 1, label: "concluída", focused: true, agent_status: "done" })],
        agents: [agentDto({ pane_id: `${WS}:p1`, tab_id: "t1", kind: "claude", status: "done" })],
      }),
    });
    const doneStatus = done.el.querySelector<HTMLElement>('[data-tab-row="t1"] [data-status]')!.dataset.status;
    expect(doneStatus).toBe("done");
    expect(new Set([status("t1"), status("t2"), status("t3"), doneStatus]).size, "one icon per state").toBe(4);
  });

  // Would catch: `c` for both claude and codex (the bar's lowercase glyph), or a shell tab with
  // the avatar of the previous agent.
  it("shows the type avatar of each tab: C, X and $ without an agent", () => {
    const { el } = render({ state: tabsFixture() });
    const avatar = (id: string) => el.querySelector<HTMLElement>(`[data-tab-row="${id}"] [data-tab-avatar]`)!.textContent!.trim();
    expect([avatar("t1"), avatar("t2"), avatar("t3")]).toEqual(["C", "X", "$"]);
  });

  // Would catch: a title recomputed here (the raw engine label, the pane name of a named tab)
  // instead of the one the 026 bar renders for the same tab.
  it("shows for each tab the same title the center bar shows", () => {
    const { el, ctx } = render({ state: tabsFixture() });
    const bar = renderBar(ctx);
    const barTitle = (id: string) => bar.querySelector<HTMLElement>(`[data-tab="${id}"] [data-label]`)!.textContent;
    expect(barTitle("t1")).toBe("refatorar faturamento");
    expect(barTitle("t3")).toBe("shell");
    for (const id of ["t1", "t2", "t3"]) {
      expect(el.querySelector<HTMLElement>(`[data-tab-row="${id}"] [data-tab-title]`)!.textContent, id).toBe(barTitle(id));
    }
  });

  // Would catch: the highlight left on the workspace row (P7), two selected rows, or the focus
  // read from the engine's `focused` flag while the connection points elsewhere.
  it("marks only the focused tab as selected, with the accent-soft background", () => {
    const { el } = render({ state: tabsFixture() });
    expect(rows(el).filter((r) => r.dataset.selected === "true").map((r) => r.dataset.tabRow)).toEqual(["t1"]);
    expect(el.querySelector<HTMLElement>('[data-tab-row="t1"]')!.getAttribute("aria-current")).toBe("true");
    expect(rule(sources["./WorkspaceTabList.svelte"]!, '.tab[data-selected="true"]')).toMatch(/background[^;]*var\(--surface-3/);
  });
});

describe("AC-042-02 relative time of the last observed change", () => {
  // Would catch: a time read from the engine (which publishes none), a transition dated at mount
  // instead of when it was observed, or a row that never ages.
  it("shows agora at the observed transition and 3 min later, with the injected clock", () => {
    const t0 = 1_700_000_000_000;
    let at = t0;
    vi.useFakeTimers();
    const working = agentsState({
      focusedTabId: "t1",
      tabs: [tabDto({ tab_id: "t1", number: 1, label: "refatorar", focused: true, agent_status: "working" })],
      agents: [agentDto({ pane_id: `${WS}:p1`, tab_id: "t1", kind: "claude", status: "working", state_change_seq: 1 })],
    });
    const { el, holder } = render({ state: working, clock: () => at });
    const time = () => el.querySelector<HTMLElement>('[data-tab-row="t1"] [data-tab-time]')!.textContent!.trim();
    expect(time()).toBe("agora");

    at = t0 + 180_000;
    holder.value = agentsState({
      focusedTabId: "t1",
      tabs: [tabDto({ tab_id: "t1", number: 1, label: "refatorar", focused: true, agent_status: "done" })],
      agents: [agentDto({ pane_id: `${WS}:p1`, tab_id: "t1", kind: "claude", status: "done", state_change_seq: 2 })],
    });
    flushSync();
    expect(time(), "the transition was observed now").toBe("agora");

    at = t0 + 360_000;
    vi.advanceTimersByTime(30_000);
    flushSync();
    expect(time(), "180 s after the observed transition").toBe("3 min");
  });

  // Would catch: a `—` replaced by `agora` for a tab this client never saw changing (014/P2).
  it("shows — for a tab that was never observed", () => {
    const { el } = render({ state: tabsFixture(), clock: () => 1_700_000_000_000 });
    expect(el.querySelector<HTMLElement>('[data-tab-row="t3"] [data-tab-time]')!.textContent!.trim()).toBe("—");
  });

  // Would catch: a per-row timer, a 1 s tick, or a timer left running after the sidebar closes
  // (a hidden pane must not be repainted by the clock).
  it("ages with one 30 s timer, stopped when the list unmounts, and touches no controller", () => {
    vi.useFakeTimers();
    const interval = vi.spyOn(globalThis, "setInterval");
    const clear = vi.spyOn(globalThis, "clearInterval");
    const { el, focusTab, select, holder } = render({ state: tabsFixture(), clock: () => 1_700_000_000_000 });
    const ticks = interval.mock.calls.filter((call) => call[1] === 30_000);
    expect(ticks, "one 30 s tick for the whole list").toHaveLength(1);
    expect(interval.mock.calls.filter((call) => Number(call[1] ?? 0) < 30_000), "nothing ticks faster than 30 s").toHaveLength(0);
    const tickId = interval.mock.results[interval.mock.calls.indexOf(ticks[0]!)]!.value;

    vi.advanceTimersByTime(120_000);
    flushSync();
    expect(focusTab).not.toHaveBeenCalled();
    expect(select, "the clock never touches the surface: no pane repaint").not.toHaveBeenCalled();

    const host = mounted.pop()!;
    unmount(host.app as never);
    host.el.remove();
    expect(clear.mock.calls.map((call) => call[0])).toContain(tickId);
    holder.value = tabsFixture();
    flushSync();
    expect(el.querySelectorAll("[data-tab-row]")).toHaveLength(0);
  });
});

describe("AC-042-03 focusing a tab", () => {
  // Would catch: a click that sends two `tab.focus`, or one that also focuses the workspace.
  it("calls focusTab once with the tab id", () => {
    const { el, focusTab } = render({ state: tabsFixture() });
    el.querySelector<HTMLElement>('[data-tab-row="t2"]')!.click();
    flushSync();
    expect(focusTab).toHaveBeenCalledExactlyOnceWith("t2");
  });

  // Would catch: the focused tab re-sent on every click (a needless engine round trip).
  it("sends nothing when the already focused tab is clicked", () => {
    const { el, focusTab } = render({ state: tabsFixture() });
    el.querySelector<HTMLElement>('[data-tab-row="t1"]')!.click();
    flushSync();
    expect(focusTab).not.toHaveBeenCalled();
  });
});
