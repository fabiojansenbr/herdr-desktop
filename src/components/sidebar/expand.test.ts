// @vitest-environment happy-dom
// Spec 052 — expandir/recolher é um estado do cliente por workspace, independente do foco: o
// workspace que perde o foco continua listando as suas abas (AC-052-01), um workspace expandido
// que não é o focado lista as abas no formato da 042 sem seleção, inclusive a partir do snapshot
// do hub de outro host, e uma aba dele navega pelo caminho da 043 (AC-052-02), e o estado
// sobrevive a uma remontagem, com o foco de fora expandindo quem o recebe (AC-052-03).
//
// Would catch: the pre-052 window (`{#if row.active}`), where focusing another workspace collapses
// the previous one; a click on the focused row that re-sends `workspace_focus` instead of
// toggling; tabs of a non-focused workspace carrying the selection highlight; a tab of another
// host focused before its metadata arrives (or without switching the host first); an expansion
// that is forgotten when the sidebar is reopened; and a `localStorage` that throws taking the
// sidebar with it.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, TabDto } from "../../agents/types";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostAgentDto, HostDto, HostTabDto, HostWorkspaceDto } from "../../connections/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import WorkspacesSection from "./WorkspacesSection.svelte";

const LOCAL = "local";
const SSH = "ssh-dev";
const HUB = "w-hub";
const DESK = "w-desk";
const LIB = "w-lib";

const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

beforeEach(() => {
  try {
    globalThis.localStorage?.clear();
  } catch {
    /* no storage in this environment: the defaults are what the tests assert */
  }
});

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function hostWorkspace(overrides: Partial<HostWorkspaceDto> & { workspace_id: string }): HostWorkspaceDto {
  return {
    number: 1,
    label: "workspace",
    focused: false,
    tab_count: 1,
    pane_count: 1,
    active_tab_id: "t1",
    agent_status: "unknown",
    cwd: null,
    branch: null,
    ...overrides,
  };
}

/** Local, online and selected: `acme-web` (number 1) and `herdr-desktop` (number 2). */
function localHost(focused: string): HostDto {
  return hostFixture({
    endpoint: LOCAL,
    label: "Este computador",
    kind: "local",
    phase: "online",
    phase_label: "Online",
    session: "hd052",
    target: null,
    workspaces: [
      hostWorkspace({ workspace_id: HUB, number: 1, label: "acme-web", cwd: "/w/acme-web", branch: "main", focused: focused === HUB }),
      hostWorkspace({ workspace_id: DESK, number: 2, label: "herdr-desktop", cwd: "/w/herdr-desktop", branch: "spec/052", focused: focused === DESK }),
    ],
  });
}

function hostTab(overrides: Partial<HostTabDto> & { tab_id: string; workspace_id: string; number: number }): HostTabDto {
  return {
    label: String(overrides.number),
    custom_label: false,
    focused: false,
    zoomed: false,
    pane_count: 1,
    agent_status: "idle",
    ...overrides,
  };
}

function hostAgent(overrides: Partial<HostAgentDto> & { pane_id: string; workspace_id: string; tab_id: string }): HostAgentDto {
  return {
    name: null,
    display_agent: null,
    agent: null,
    title: null,
    terminal_title: null,
    terminal_title_stripped: null,
    agent_status: "idle",
    focused: false,
    ...overrides,
  };
}

/**
 * The other host, online but not attached: its tabs, agents and workspaces come from the hub
 * snapshot (039), which is the only source the sidebar has for it.
 */
function sshHost(): HostDto {
  return hostFixture({
    endpoint: SSH,
    label: "dev-box",
    kind: "ssh",
    phase: "online",
    phase_label: "Online",
    session: "hd052",
    workspaces: [hostWorkspace({ workspace_id: LIB, number: 1, label: "lib", cwd: "/w/lib", branch: "master", focused: true })],
    tabs: [
      hostTab({ tab_id: "t-lib-2", workspace_id: LIB, number: 2, label: "2" }),
      hostTab({ tab_id: "t-lib-1", workspace_id: LIB, number: 1, label: "migrar índices", focused: true, custom_label: true, agent_status: "blocked" }),
    ],
    agents: [hostAgent({ pane_id: `${LIB}:p1`, workspace_id: LIB, tab_id: "t-lib-1", agent: "codex", name: "codex", agent_status: "blocked" })],
  });
}

function tabDto(overrides: Partial<TabDto> & { tab_id: string; workspace_id: string; number: number }): TabDto {
  return {
    label: String(overrides.number),
    focused: false,
    pane_count: 1,
    agent_status: "idle",
    ...overrides,
  };
}

function agentDto(overrides: Partial<AgentDto> & { pane_id: string; workspace_id: string; tab_id: string; status: AgentStatus }): AgentDto {
  return {
    name: overrides.kind ?? null,
    kind: null,
    launch_pending: false,
    ready: true,
    focused: false,
    state_change_seq: 1,
    ...overrides,
  };
}

const IDENTITY = (endpoint: string) => ({ endpoint, session: "hd052", connection_generation: 1, boot_id: "boot-052" });

/** Live list of the attached host: Local, with the tabs of both workspaces. */
function localAgents(focusedTabId: string): AgentsState {
  return {
    phase: "connected",
    identity: IDENTITY(LOCAL),
    agents: [
      agentDto({ pane_id: `${HUB}:p1`, workspace_id: HUB, tab_id: "t-hub-1", kind: "claude", status: "working", focused: true }),
      agentDto({ pane_id: `${DESK}:p1`, workspace_id: DESK, tab_id: "t-desk-1", kind: "codex", status: "blocked" }),
    ],
    tabs: [
      tabDto({ tab_id: "t-hub-1", workspace_id: HUB, number: 1, label: "revisar plano", agent_status: "working", focused: focusedTabId === "t-hub-1" }),
      tabDto({ tab_id: "t-hub-2", workspace_id: HUB, number: 2, label: "2" }),
      tabDto({ tab_id: "t-desk-1", workspace_id: DESK, number: 1, label: "spec 052", agent_status: "blocked", focused: focusedTabId === "t-desk-1" }),
    ],
    tabFocus: { tab_id: focusedTabId, ...IDENTITY(LOCAL), revision: 1 },
    topology: null,
    capabilities: null,
    kinds: [],
  } as unknown as AgentsState;
}

/** Live list after the host switch of 037: the window is attached to `dev-box` now. */
function sshAgents(): AgentsState {
  return {
    phase: "connected",
    identity: IDENTITY(SSH),
    agents: [agentDto({ pane_id: `${LIB}:p1`, workspace_id: LIB, tab_id: "t-lib-1", kind: "codex", status: "blocked" })],
    tabs: [
      tabDto({ tab_id: "t-lib-1", workspace_id: LIB, number: 1, label: "migrar índices", agent_status: "blocked", focused: true }),
      tabDto({ tab_id: "t-lib-2", workspace_id: LIB, number: 2, label: "2" }),
    ],
    tabFocus: { tab_id: "t-lib-1", ...IDENTITY(SSH), revision: 1 },
    topology: null,
    capabilities: null,
    kinds: [],
  } as unknown as AgentsState;
}

function surface(endpoint: string): SurfaceState {
  return {
    selection: { endpoint, kind: endpoint === LOCAL ? "local" : "ssh", label: endpoint, session: "hd052", online: true, identity: null },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  } as unknown as SurfaceState;
}

function connectionsState(hosts: readonly HostDto[]): ConnectionsState {
  return {
    view: { hub: { revision: 1, hosts: [...hosts] }, profiles: [], store_error: null },
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: { open: false, draft: { id: null, label: "", target: "", port: "", session: "", auth: "key" }, errors: {}, submitting: false, submitError: null, connecting: null },
    importReport: null,
    workspaces: {},
  } as unknown as ConnectionsState;
}

interface RenderOptions {
  hosts?: HostDto[];
  state?: AgentsState | null;
  selected?: string;
}

function render(options: RenderOptions = {}) {
  const hostsHolder = reactiveHolder<readonly HostDto[]>(options.hosts ?? [localHost(HUB)]);
  const agentsHolder = reactiveHolder<AgentsState | null>(options.state === undefined ? localAgents("t-hub-1") : options.state);
  const selectedHolder = reactiveHolder<string | null>(options.selected ?? LOCAL);
  const focusWorkspace = vi.fn(async () => {});
  const focusTab = vi.fn(async () => {});
  const select = vi.fn(async (endpoint: string) => {
    selectedHolder.value = endpoint;
    return {} as never;
  });
  const ctx: FrameContext = {
    get surface() {
      return surface(selectedHolder.value ?? LOCAL);
    },
    get agents() {
      return agentsHolder.value;
    },
    get connections() {
      return connectionsState(hostsHolder.value);
    },
    navigator: null,
    get selectedEndpoint() {
      return selectedHolder.value;
    },
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: false,
    controllers: {
      surface: { select } as unknown as FrameContext["controllers"]["surface"],
      agents: { focusTab } as unknown as FrameContext["controllers"]["agents"],
      projects: { focusWorkspace, setWorkspacePref: vi.fn(async () => {}) } as unknown as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspacesSection, { target: el, props: { ctx } });
  flushSync();
  const entry = { el, app: app as never };
  mounted.push(entry);
  return {
    el,
    ctx,
    hostsHolder,
    agentsHolder,
    selectedHolder,
    focusWorkspace,
    focusTab,
    select,
    remount() {
      unmount(entry.app as never);
      const next = mount(WorkspacesSection, { target: el, props: { ctx } });
      flushSync();
      entry.app = next as never;
      return el;
    },
  };
}

const row = (el: HTMLElement, workspaceId: string) => el.querySelector<HTMLElement>(`[data-workspace-row="${workspaceId}"]`)!;
const list = (el: HTMLElement, workspaceId: string) => el.querySelector<HTMLElement>(`[data-workspace-tabs="${workspaceId}"]`);
const tabsOf = (el: HTMLElement, workspaceId: string) => [...(list(el, workspaceId)?.querySelectorAll<HTMLElement>("[data-tab-row]") ?? [])];
const click = (node: HTMLElement) => {
  node.click();
  flushSync();
};
/** The 043 chain is asynchronous (host switch, then `workspace_focus`): let it run, then repaint. */
const settle = async () => {
  for (let i = 0; i < 6; i += 1) await Promise.resolve();
  flushSync();
};

describe("AC-052-01 expandir e recolher independem do foco", () => {
  // Would catch the pre-052 `{#if row.active}`: focusing `herdr-desktop` would take the tabs of
  // `acme-web` off the sidebar.
  it("mantém as abas do workspace que perdeu o foco e alterna o focado sem refocar", () => {
    const { el, hostsHolder, agentsHolder, focusWorkspace } = render();
    expect(list(el, HUB), "o focado começa expandido").toBeTruthy();
    expect(tabsOf(el, HUB).map((node) => node.dataset.tabRow)).toEqual(["t-hub-1", "t-hub-2"]);
    expect(list(el, DESK), "os outros começam recolhidos").toBeNull();

    click(row(el, DESK));
    expect(focusWorkspace).toHaveBeenCalledExactlyOnceWith(LOCAL, DESK);
    expect(el.querySelectorAll("[data-workspace-tabs]"), "duas listas montadas").toHaveLength(2);
    expect(tabsOf(el, DESK).map((node) => node.dataset.tabRow)).toEqual(["t-desk-1"]);
    expect(tabsOf(el, HUB).map((node) => node.dataset.tabRow), "acme-web continua listando").toEqual(["t-hub-1", "t-hub-2"]);

    // A metadata do foco chega: `herdr-desktop` é o workspace focado do host selecionado.
    hostsHolder.value = [localHost(DESK)];
    agentsHolder.value = localAgents("t-desk-1");
    flushSync();
    expect(row(el, DESK).dataset.active).toBe("true");
    expect(el.querySelectorAll("[data-workspace-tabs]"), "o foco não recolhe ninguém").toHaveLength(2);

    click(row(el, DESK));
    expect(focusWorkspace, "clicar no focado não refoca").toHaveBeenCalledTimes(1);
    expect(list(el, DESK), "recolheu só ele").toBeNull();
    expect(list(el, HUB)).toBeTruthy();

    click(row(el, DESK));
    expect(focusWorkspace).toHaveBeenCalledTimes(1);
    expect(list(el, DESK), "o clique seguinte expande de novo").toBeTruthy();
    expect(row(el, DESK).getAttribute("aria-expanded")).toBe("true");
  });
});

describe("AC-052-02 abas de um workspace que não é o focado", () => {
  /** `herdr-desktop` (mesmo host) e `lib` (outro host) expandidos, sem contar o clique da arrumação. */
  function expanded() {
    const harness = render({ hosts: [localHost(HUB), sshHost()] });
    click(row(harness.el, DESK));
    click(row(harness.el, LIB));
    harness.focusWorkspace.mockClear();
    harness.select.mockClear();
    return harness;
  }

  // Would catch: a list that only knows the attached host (the SSH rows would show nothing), or a
  // format rebuilt for the non-focused workspace instead of the 042 rows.
  it("lista as abas no formato da 042, do host selecionado e do snapshot do hub", () => {
    const { el } = expanded();
    const cells = (workspaceId: string, tabId: string) => {
      const node = el.querySelector<HTMLElement>(`[data-workspace-tabs="${workspaceId}"] [data-tab-row="${tabId}"]`)!;
      return {
        status: node.querySelector<HTMLElement>("[data-status]")!.dataset.status,
        avatar: node.querySelector<HTMLElement>("[data-tab-avatar]")!.textContent!.trim(),
        title: node.querySelector<HTMLElement>("[data-tab-title]")!.textContent!.trim(),
        time: node.querySelector<HTMLElement>("[data-tab-time]")!.textContent!.trim(),
      };
    };
    expect(cells(DESK, "t-desk-1")).toEqual({ status: "waiting", avatar: "X", title: "spec 052", time: "agora" });
    expect(tabsOf(el, LIB).map((node) => node.dataset.tabRow), "o outro host vem do snapshot, por number").toEqual(["t-lib-1", "t-lib-2"]);
    expect(cells(LIB, "t-lib-1")).toEqual({ status: "waiting", avatar: "X", title: "migrar índices", time: "agora" });
    expect(cells(LIB, "t-lib-2").status).toBe("idle");
    expect(cells(LIB, "t-lib-2").avatar).toBe("$");
  });

  // Would catch: the highlight of the focused tab leaking into a list that is not the focused
  // workspace of the selected host.
  it("não marca nenhuma aba como selecionada fora do workspace focado", () => {
    const { el } = expanded();
    expect(list(el, DESK)!.querySelectorAll("[data-selected]")).toHaveLength(0);
    expect(list(el, LIB)!.querySelectorAll("[data-selected]")).toHaveLength(0);
    expect([...list(el, HUB)!.querySelectorAll<HTMLElement>('[data-selected="true"]')].map((node) => node.dataset.tabRow)).toEqual(["t-hub-1"]);
  });

  // Would catch: a `tab.focus` sent to the host the user just left, a second `workspace_focus`, or
  // a click that focuses the tab before the new host's metadata confirms (spec 043).
  it("navega pelo caminho da 043: troca de host, um focusWorkspace e um focusTab depois da metadata", async () => {
    const { el, agentsHolder, focusWorkspace, focusTab, select } = expanded();
    click(el.querySelector<HTMLElement>(`[data-workspace-tabs="${LIB}"] [data-tab-row="t-lib-1"]`)!);
    await settle();
    expect(select).toHaveBeenCalledExactlyOnceWith(SSH);
    expect(focusWorkspace).toHaveBeenCalledExactlyOnceWith(SSH, LIB);
    expect(focusTab, "nada antes da metadata do host novo").not.toHaveBeenCalled();

    agentsHolder.value = sshAgents();
    flushSync();
    expect(focusTab).toHaveBeenCalledExactlyOnceWith("t-lib-1");
  });

  // Would catch: a tab of a non-focused workspace of the attached host focused without focusing
  // the workspace first (the engine would move the focus inside another workspace).
  it("foca o workspace antes da aba quando o workspace não é o focado do mesmo host", async () => {
    const { el, focusWorkspace, focusTab, select } = expanded();
    click(el.querySelector<HTMLElement>(`[data-workspace-tabs="${DESK}"] [data-tab-row="t-desk-1"]`)!);
    await settle();
    expect(select, "o host já é o selecionado").not.toHaveBeenCalled();
    expect(focusWorkspace).toHaveBeenCalledExactlyOnceWith(LOCAL, DESK);
    expect(focusTab).toHaveBeenCalledExactlyOnceWith("t-desk-1");
  });
});

describe("AC-052-03 estado guardado e foco de fora", () => {
  // Would catch: an expansion kept only in memory, which a reopened sidebar would forget.
  it("volta ao estado guardado quando a lateral é remontada", () => {
    const harness = render({ hosts: [localHost(HUB), sshHost()] });
    click(row(harness.el, HUB));
    click(row(harness.el, DESK));
    expect(list(harness.el, HUB)).toBeNull();
    expect(list(harness.el, DESK)).toBeTruthy();
    // A chave é a de 044 (endpoint + raiz normalizada), no formato que a spec fixou.
    expect(globalThis.localStorage.getItem("herdr.sidebar.ws-expanded.local:/w/acme-web")).toBe("0");
    expect(globalThis.localStorage.getItem("herdr.sidebar.ws-expanded.local:/w/herdr-desktop")).toBe("1");

    const el = harness.remount();
    expect(list(el, HUB), "o focado recolhido continua recolhido").toBeNull();
    expect(list(el, DESK), "o expandido continua expandido").toBeTruthy();
    expect(list(el, LIB), "o que nunca foi tocado segue recolhido").toBeNull();
  });

  // Would catch: a workspace that gains the focus by another path (metadata, palette, inbox,
  // engine) staying collapsed, which would leave the sidebar showing no tabs at all.
  it("expande quem ganha o foco por fora e não mexe nos outros", () => {
    const { el, hostsHolder, agentsHolder } = render();
    click(row(el, HUB));
    expect(list(el, HUB)).toBeNull();
    expect(list(el, DESK)).toBeNull();

    hostsHolder.value = [localHost(DESK)];
    agentsHolder.value = localAgents("t-desk-1");
    flushSync();
    expect(list(el, DESK), "ganhou o foco: expandido").toBeTruthy();
    expect(list(el, HUB), "quem não ganhou nada mantém o estado").toBeNull();

    hostsHolder.value = [localHost(HUB)];
    agentsHolder.value = localAgents("t-hub-1");
    flushSync();
    expect(list(el, HUB), "o foco volta e expande o recolhido").toBeTruthy();
    expect(list(el, DESK), "perder o foco não recolhe").toBeTruthy();
  });

  // Would catch: an unguarded `localStorage` read/write taking the whole sidebar down in a private
  // window or with site data blocked (the 045 rule).
  it("não quebra quando localStorage lança", () => {
    const getItem = vi.spyOn(globalThis.localStorage, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const setItem = vi.spyOn(globalThis.localStorage, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const { el } = render();
    expect(row(el, HUB)).toBeTruthy();
    expect(list(el, HUB), "sem estado guardado, o focado começa expandido").toBeTruthy();
    expect(getItem).toHaveBeenCalled();

    click(row(el, DESK));
    expect(list(el, DESK), "a expansão fica em memória nesta janela").toBeTruthy();
    click(row(el, HUB));
    expect(list(el, HUB)).toBeNull();
    expect(setItem).toHaveBeenCalled();
  });
});
