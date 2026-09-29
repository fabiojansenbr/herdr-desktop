// @vitest-environment happy-dom
// Spec 043 — "Precisa de você": the box that gathers, across every host, the agents that are
// waiting for the user or have finished (AC-043-01), the navigation one click makes (AC-043-02,
// with fake bridges and the order of the calls) and what makes an item leave (AC-043-03).
// Would catch: `done` folded into idle (the 041 split lost), items of a host the window is not
// attached to being read from the live list, a `tab.focus` sent to the host the user just left
// (before its metadata confirmed), two `workspace_focus` per click, and a finished item that
// keeps nagging after the user visited its tab — or that never comes back afterwards.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto, AgentStatus, TabDto } from "../../agents/types";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostAgentDto, HostDto, HostWorkspaceDto } from "../../connections/types";
import { createProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import InboxSection from "./InboxSection.svelte";
import { createInboxSeen, inboxAgents, inboxItems, inboxTitle } from "./inbox-model";
import { createActivityLedger } from "../agents/panel";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010/041). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const NOW = 1_700_000_000_000;

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

function workspace(overrides: Partial<HostWorkspaceDto> & { workspace_id: string; label: string }): HostWorkspaceDto {
  return {
    number: 1,
    focused: true,
    tab_count: 1,
    pane_count: 1,
    active_tab_id: `${overrides.workspace_id}:t1`,
    agent_status: "unknown",
    cwd: null,
    branch: null,
    ...overrides,
  };
}

function hostAgent(overrides: Partial<HostAgentDto> & { pane_id: string; agent: string; agent_status: string }): HostAgentDto {
  const workspaceId = overrides.pane_id.split(":")[0]!;
  return {
    workspace_id: workspaceId,
    tab_id: `${workspaceId}:t1`,
    name: overrides.agent,
    display_agent: overrides.agent,
    title: null,
    terminal_title: null,
    terminal_title_stripped: null,
    focused: false,
    ...overrides,
  };
}

function liveAgent(overrides: Partial<AgentDto> & { pane_id: string; status: AgentStatus }): AgentDto {
  const workspaceId = overrides.pane_id.split(":")[0]!;
  return {
    workspace_id: workspaceId,
    tab_id: `${workspaceId}:t1`,
    name: overrides.kind ?? null,
    kind: null,
    launch_pending: false,
    ready: true,
    focused: false,
    state_change_seq: 1,
    ...overrides,
  };
}

/** Local with `api` (host selecionado) and SSH `dev-box` with `lib`, both online. */
function hostsFixture(overrides: { localAgents?: HostAgentDto[]; remoteAgents?: HostAgentDto[] } = {}): HostDto[] {
  return [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd043",
      target: null,
      workspaces: [workspace({ workspace_id: "w-api", label: "api" })],
      agents: overrides.localAgents ?? [hostAgent({ pane_id: "w-api:p1", agent: "codex", agent_status: "blocked" })],
    }),
    hostFixture({
      endpoint: "ssh-dev",
      label: "dev-box",
      kind: "ssh",
      phase: "online",
      phase_label: "Online",
      session: "hd043",
      workspaces: [workspace({ workspace_id: "w-lib", label: "lib" })],
      agents: overrides.remoteAgents ?? [hostAgent({ pane_id: "w-lib:p1", agent: "claude", agent_status: "done" })],
    }),
  ];
}

/** Live state of the attached host: only it carries `detection_last_line` (spec 014). */
function agentsState(overrides: { agents?: AgentDto[]; tabs?: TabDto[]; focusedTabId?: string | null; endpoint?: string }): AgentsState {
  const endpoint = overrides.endpoint ?? "local";
  return {
    phase: "connected",
    identity: { endpoint, session: "hd043", connection_generation: 1, boot_id: "boot-043" },
    agents: overrides.agents ?? [],
    tabs: overrides.tabs ?? [],
    tabFocus:
      overrides.focusedTabId === undefined
        ? null
        : { tab_id: overrides.focusedTabId, endpoint, session: "hd043", connection_generation: 1, boot_id: "boot-043", revision: 1 },
    topology: null,
    capabilities: null,
    kinds: [],
  } as unknown as AgentsState;
}

function blockedLocal(): AgentsState {
  return agentsState({
    agents: [liveAgent({ pane_id: "w-api:p1", kind: "codex", status: "blocked", detection_last_line: "Permitir sqlx migrate run?" })],
    tabs: [{ tab_id: "w-api:t1", workspace_id: "w-api", label: "migração", number: 1, focused: true, pane_count: 1, agent_status: "blocked" }],
  });
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
  } as ConnectionsState;
}

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd043", online: true, identity: null },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  };
}

interface RenderOptions {
  hosts?: HostDto[];
  agents?: AgentsState | null;
  selected?: string;
  clock?: () => number;
  controllers?: Partial<FrameContext["controllers"]>;
}

function render(options: RenderOptions = {}) {
  const agentsHolder = reactiveHolder<AgentsState | null>(options.agents ?? null);
  const hostsHolder = reactiveHolder<readonly HostDto[]>(options.hosts ?? hostsFixture());
  const selectedHolder = reactiveHolder<string | null>(options.selected ?? "local");
  const focusWorkspace = vi.fn(async () => {});
  const focusTab = vi.fn(async () => {});
  const select = vi.fn(async () => ({}) as never);
  const ctx: FrameContext = {
    surface: surface(),
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
      projects: { focusWorkspace } as unknown as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
      ...options.controllers,
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(InboxSection, { target: el, props: { ctx, clock: options.clock ?? (() => NOW) } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, ctx, agentsHolder, hostsHolder, selectedHolder, focusWorkspace, focusTab, select };
}

const items = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("[data-inbox-item]")];
const text = (node: Element | null) => node?.textContent?.trim() ?? null;

describe("inbox model (puro)", () => {
  // Would catch: `mapAgentStatus` folding done into idle (the item would never exist), a working
  // agent listed as needing the user, or the live list of the attached host used for every host.
  it("lists only the blocked and done agents of every host, blocked first", () => {
    const agents = inboxAgents({
      hosts: hostsFixture({
        localAgents: [
          hostAgent({ pane_id: "w-api:p1", agent: "codex", agent_status: "blocked" }),
          hostAgent({ pane_id: "w-api:p2", agent: "pi", agent_status: "working" }),
          hostAgent({ pane_id: "w-api:p3", agent: "pi", agent_status: "idle" }),
        ],
      }),
      selectedEndpoint: "local",
      live: blockedLocal().agents,
    });
    const ledger = createActivityLedger();
    const list = inboxItems(agents, { ledger, now: NOW });
    expect(list.map((item) => item.kind)).toEqual(["blocked", "done"]);
    expect(list.map((item) => item.text)).toEqual(["codex · api", "claude · lib"]);
    expect(list.map((item) => item.endpoint)).toEqual(["local", "ssh-dev"]);
  });

  // Would catch: the detection line of the attached host lost, or invented for a host the window
  // is not attached to (only the selected host reports `detection_last_line`, spec 014).
  it("details a blocked item with the detection line only for the attached host", () => {
    const ledger = createActivityLedger();
    const attached = inboxItems(
      inboxAgents({ hosts: hostsFixture(), selectedEndpoint: "local", live: blockedLocal().agents }),
      { ledger, now: NOW },
    );
    expect(attached[0]!.detail).toBe("Permitir sqlx migrate run?");
    expect(attached[1]!.detail).toBe("Concluído");

    const elsewhere = inboxItems(
      inboxAgents({
        hosts: hostsFixture({ remoteAgents: [hostAgent({ pane_id: "w-lib:p1", agent: "claude", agent_status: "blocked" })] }),
        selectedEndpoint: "local",
        live: blockedLocal().agents,
      }),
      { ledger, now: NOW },
    );
    expect(elsewhere.map((item) => item.detail)).toEqual(["Permitir sqlx migrate run?", "Aguardando você"]);
  });

  // AC-043-03 in the model: the key is `state_change_seq` + status, so the same finished state is
  // dismissed for good and the next transition brings the item back.
  it("dismisses a visited done item until the agent changes state again, never a blocked one", () => {
    const ledger = createActivityLedger();
    const build = (status: AgentStatus, seq: number) =>
      inboxItems(
        inboxAgents({
          hosts: hostsFixture({ localAgents: [hostAgent({ pane_id: "w-api:p1", agent: "claude", agent_status: status })] }),
          selectedEndpoint: "local",
          live: [liveAgent({ pane_id: "w-api:p1", kind: "claude", status, state_change_seq: seq })],
        }),
        { ledger, now: NOW },
      ).filter((item) => item.endpoint === "local");

    const seen = createInboxSeen();
    const done = build("done", 7);
    expect(seen.dismissed(done[0]!)).toBe(false);
    seen.visit({ endpoint: "local", tabId: "w-api:t1", items: done });
    expect(seen.dismissed(done[0]!), "visited: it leaves").toBe(true);
    expect(seen.dismissed(build("done", 7)[0]!), "same state: it stays away").toBe(true);
    expect(seen.dismissed(build("done", 8)[0]!), "the agent changed state again").toBe(false);

    const blocked = build("blocked", 9);
    seen.visit({ endpoint: "local", tabId: "w-api:t1", items: blocked });
    expect(seen.dismissed(blocked[0]!), "a blocked item is not dismissed by a visit").toBe(false);
  });

  // Would catch: a visit on another host, or on another tab, dismissing the item.
  it("only dismisses items of the tab the user is actually in, on the selected host", () => {
    const ledger = createActivityLedger();
    const list = inboxItems(inboxAgents({ hosts: hostsFixture(), selectedEndpoint: "local", live: [] }), { ledger, now: NOW });
    const remote = list.find((item) => item.endpoint === "ssh-dev")!;
    const seen = createInboxSeen();
    seen.visit({ endpoint: "local", tabId: "w-lib:t1", items: list });
    expect(seen.dismissed(remote), "same tab id, other host").toBe(false);
    seen.visit({ endpoint: "ssh-dev", tabId: "w-api:t1", items: list });
    expect(seen.dismissed(remote), "right host, other tab").toBe(false);
    seen.visit({ endpoint: "ssh-dev", tabId: "w-lib:t1", items: list });
    expect(seen.dismissed(remote)).toBe(true);
  });
});

describe("AC-043-01 seção Precisa de você", () => {
  // Would catch: the counter counting hosts instead of items, the done item ranked first, the
  // workspace label replaced by its id, or the detection line dropped.
  it("shows the header with the counter and the two items, blocked first", () => {
    const { el } = render({ agents: blockedLocal() });
    expect(text(el.querySelector("[data-inbox-title]"))).toBe(inboxTitle());
    expect(text(el.querySelector("[data-inbox-title]"))).toBe("Precisa de você");
    expect(text(el.querySelector("[data-inbox-count]"))).toBe("2");

    const list = items(el);
    expect(list).toHaveLength(2);
    expect(list.map((item) => item.dataset.kind)).toEqual(["blocked", "done"]);
    // 051: the line carries the workspace; the agent stays in the avatar and in the label.
    expect(list.map((item) => text(item.querySelector("[data-inbox-text]")))).toEqual(["api", "lib"]);
    expect(list.map((item) => item.getAttribute("aria-label"))).toEqual([
      "codex · api, Este computador, Aguardando você, Permitir sqlx migrate run?",
      "claude · lib, dev-box, Concluído",
    ]);
    expect(text(list[0]!.querySelector("[data-inbox-detail]"))).toBe("Permitir sqlx migrate run?");
  });

  // Would catch: a generic text for the attached host, or the detection line invented elsewhere.
  it("says Aguardando você for a blocked agent on a host that is not selected", () => {
    const { el } = render({
      hosts: hostsFixture({
        localAgents: [],
        remoteAgents: [hostAgent({ pane_id: "w-lib:p1", agent: "claude", agent_status: "blocked" })],
      }),
      agents: agentsState({}),
    });
    expect(items(el)).toHaveLength(1);
    // 051: with no detection line the state lives in the icon and in the label, never in a second line.
    expect(items(el)[0]!.getAttribute("aria-label")).toBe("claude · lib, dev-box, Aguardando você");
    expect(items(el)[0]!.querySelector("[data-inbox-detail]"), "sem detecção, sem segunda linha").toBeNull();
  });

  // Would catch: an empty box (header and border) drawn when nothing needs the user.
  it("renders nothing at all when no agent needs the user", () => {
    const { el } = render({
      hosts: hostsFixture({
        localAgents: [hostAgent({ pane_id: "w-api:p1", agent: "codex", agent_status: "working" })],
        remoteAgents: [hostAgent({ pane_id: "w-lib:p1", agent: "claude", agent_status: "idle" })],
      }),
      agents: agentsState({}),
    });
    expect(el.textContent!.trim()).toBe("");
    expect(el.querySelector("[data-sidebar-inbox-section]")).toBeNull();
  });
});

/** Declarations of one rule as a comparable set, so two headers are judged by what they declare. */
function decls(css: string): string[] {
  return css
    .split(";")
    .map((decl) => decl.trim().replace(/\s+/g, " "))
    .filter(Boolean)
    .sort();
}

describe("AC-051-01 cabeçalho na linguagem de WORKSPACES", () => {
  // Would catch: the amber dot or the amber title coming back, and the counter drawn as a pill.
  it("renders the WORKSPACES header: title, plain count, no dot and no pill", () => {
    const { el } = render({ agents: blockedLocal() });
    const head = el.querySelector<HTMLElement>("[data-sidebar-inbox-section] > header")!;
    expect(head.classList.contains("section-head")).toBe(true);

    const title = el.querySelector<HTMLElement>("[data-inbox-title]")!;
    expect(text(title)).toBe("Precisa de você");
    expect(title.classList.contains("section-title")).toBe(true);
    expect(head.firstElementChild, "nada antes do título").toBe(title);
    expect(head.querySelector("[aria-hidden]"), "nenhum ponto decorativo").toBeNull();
    expect(el.querySelector(".dot")).toBeNull();

    const count = el.querySelector<HTMLElement>("[data-inbox-count]")!;
    expect(text(count)).toBe("2");
    expect(head.lastElementChild, "a contagem fecha o cabeçalho, à direita").toBe(count);
  });

  // Spec 063: cabeçalho em destaque âmbar (--attention-soft, --attention) e time em --text-dim.
  it("declares the amber header styles in InboxSection", () => {
    const source = sources["./InboxSection.svelte"]!;
    expect(rule(source, ".section-head")).toMatch(/var\(--attention-soft/);
    expect(rule(source, ".section-head")).toMatch(/border-radius:\s*8px/);
    expect(rule(source, ".section-title")).toMatch(/var\(--attention/);
    expect(rule(source, ".section-count")).toMatch(/var\(--attention/);
    expect(rule(source, ".section-count"), "número simples, sem pílula").not.toMatch(/background|border-radius/);
    expect(rule(source, ".dot"), "nenhum ponto decorativo").toBe("");
  });
});

describe("AC-051-02 item na linguagem da linha de aba", () => {
  // Would catch: the two-line card kept, the agent name in the line instead of the avatar, or a
  // status drawn by colour alone instead of the tab row's icon.
  it("draws each item as a tab-row line: status icon, 16 px avatar, workspace and time", () => {
    const { el } = render({ agents: blockedLocal() });
    const list = items(el);
    expect(list.map((item) => item.querySelector<HTMLElement>("[data-status]")!.dataset.status)).toEqual(["waiting", "done"]);
    expect(list.map((item) => text(item.querySelector("[data-inbox-avatar]")))).toEqual(["X", "C"]);
    expect(list.map((item) => text(item.querySelector("[data-inbox-text]")))).toEqual(["api", "lib"]);
    expect(list.map((item) => item.querySelector("[data-inbox-time]") !== null)).toEqual([true, true]);
    expect(list.map((item) => item.getAttribute("title"))).toEqual(list.map((item) => item.getAttribute("aria-label")));

    const source = sources["./InboxSection.svelte"]!;
    expect(rule(source, ".avatar")).toMatch(/width:\s*16px/);
    expect(rule(source, ".time")).toMatch(/var\(--text-dim/);
  });

  // Would catch: a second drawing of the status icon in the box, drifting from the tab row's.
  it("reuses the very status icon of the tab row", () => {
    const inbox = sources["./InboxSection.svelte"]!;
    const tabs = sources["./WorkspaceTabList.svelte"]!;
    expect(inbox).toMatch(/import StatusIcon from "\.\/StatusIcon\.svelte"/);
    expect(tabs).toMatch(/import StatusIcon from "\.\/StatusIcon\.svelte"/);
    expect(inbox, "nenhum desenho próprio").not.toMatch(/<svg/);
    expect(tabs, "o desenho vive num lugar só").not.toMatch(/<svg/);

    const icon = sources["./StatusIcon.svelte"]!;
    expect(rule(icon, '.status[data-status="waiting"]')).toMatch(/var\(--attention/);
    expect(rule(icon, '.status[data-status="done"]')).toMatch(/var\(--working/);
  });

  // Would catch: the card coming back (background, radius, coloured `border-left`).
  it("is a 30 px line with no background and no border, --surface-2 only on hover/focus", () => {
    const source = sources["./InboxSection.svelte"]!;
    const item = rule(source, ".item");
    expect(item).toMatch(/background:\s*transparent/);
    expect(item).not.toMatch(/--surface-2/);
    expect(item).toMatch(/border:\s*none/);
    expect(item).not.toMatch(/border-left|border-color|border-width/);
    expect(source, "nenhuma borda lateral").not.toMatch(/border-left/);
    expect(rule(source, '[data-kind="blocked"]'), "nenhuma cor própria do cartão").toBe("");
    expect(rule(source, '[data-kind="done"]')).toBe("");
    expect(rule(source, ".line")).toMatch(/min-height:\s*30px/);
    expect(rule(source, ".item:hover")).toMatch(/background-color:\s*var\(--surface-2/);
    expect(rule(source, ".item:focus-visible")).toMatch(/background-color:\s*var\(--surface-2/);
  });

  // Would catch: `Concluído` printed under a finished item again, or a detection line that wraps.
  it("gives a second line only to a blocked item with a detection line", () => {
    const { el } = render({ agents: blockedLocal() });
    const list = items(el);
    expect(text(list[0]!.querySelector("[data-inbox-detail]"))).toBe("Permitir sqlx migrate run?");
    expect(list[1]!.querySelector("[data-inbox-detail]"), "o done tem uma linha só").toBeNull();
    expect(el.textContent).not.toMatch(/Concluído/);

    const detail = rule(sources["./InboxSection.svelte"]!, ".detail");
    expect(detail).toMatch(/font-size:\s*12px/);
    expect(detail).toMatch(/var\(--text-muted/);
    expect(detail).toMatch(/white-space:\s*nowrap/);
    expect(detail).toMatch(/text-overflow:\s*ellipsis/);
  });
});

/** Real controllers over fake bridges: the click is judged by the commands that reach them. */
async function renderWired() {
  const order: string[] = [];
  const projectsBridge = createFakeProjectsBridge({ bootId: "boot-043" });
  const projects = createProjectsController({
    ...projectsBridge,
    async workspaceFocus(endpoint: string, workspaceId: string) {
      order.push(`workspace_focus:${endpoint}:${workspaceId}`);
      return projectsBridge.workspaceFocus(endpoint, workspaceId);
    },
  });

  const attach = async (endpoint: string, paneId: string, status: AgentStatus, kind: string) => {
    const workspaceId = paneId.split(":")[0]!;
    const bridge = createFakeAgentsBridge({
      bootId: "boot-043",
      generation: 1,
      session: "hd043",
      endpoint,
      agentKind: kind,
      agents: [[paneId, status]],
      tabs: [{ tab_id: `${workspaceId}:t1`, workspace_id: workspaceId, label: "aba", number: 1, focused: true, pane_count: 1, agent_status: status }],
    });
    const recording = {
      ...bridge,
      async focusTab(target: Parameters<typeof bridge.focusTab>[0], tabId: string) {
        order.push(`tab_focus:${endpoint}:${tabId}`);
        return bridge.focusTab(target, tabId);
      },
    };
    const holder = reactiveHolder<AgentsState | null>(null);
    const controller = createAgentsController(recording, (next) => (holder.value = next));
    await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
    bridge.calls.length = 0;
    return { bridge, controller, holder };
  };

  const local = await attach("local", "w-api:p1", "blocked", "codex");
  const remote = await attach("ssh-dev", "w-lib:p1", "done", "claude");

  const attached = reactiveHolder<"local" | "ssh-dev">("local");
  const selectedHolder = reactiveHolder<string | null>("local");
  const current = () => (attached.value === "local" ? local : remote);
  const select = vi.fn(async (endpoint: string) => {
    order.push(`select:${endpoint}`);
    // The 035/037 switch: the selection moves at once; the new host's metadata lands later.
    selectedHolder.value = endpoint;
    return {} as never;
  });
  /** The agents state of the newly selected host reaching the window (what the effect waits for). */
  const landMetadata = () => {
    attached.value = selectedHolder.value === "local" ? "local" : "ssh-dev";
    flushSync();
  };

  const ctx: FrameContext = {
    surface: surface(),
    get agents() {
      return current().holder.value;
    },
    get connections() {
      return connectionsState(hostsFixture());
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
    get controllers() {
      return {
        surface: { select } as unknown as FrameContext["controllers"]["surface"],
        agents: current().controller,
        projects,
        connections: {} as FrameContext["controllers"]["connections"],
      };
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  } as FrameContext;

  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(InboxSection, { target: el, props: { ctx, clock: () => NOW } });
  flushSync();
  mounted.push({ el, app: app as never });
  const settle = async () => {
    for (let i = 0; i < 6; i += 1) await Promise.resolve();
    flushSync();
  };
  return { el, order, local, remote, select, projectsBridge, landMetadata, settle };
}

describe("AC-043-02 um clique leva ao agente", () => {
  // Would catch: `tab.focus` sent to the host the user just left (before the new metadata), the
  // workspace focused twice, or a reconnect instead of the 035/037 switch.
  it("switches host, focuses the workspace and only then the tab, once each", async () => {
    const { el, order, remote, landMetadata, settle } = await renderWired();
    const item = items(el).find((node) => node.dataset.inboxItem!.startsWith("ssh-dev"))!;
    item.click();
    await settle();

    expect(order, "no tab.focus before the metadata of the new host").toEqual([
      "select:ssh-dev",
      "workspace_focus:ssh-dev:w-lib",
    ]);

    landMetadata();
    await settle();
    expect(order).toEqual(["select:ssh-dev", "workspace_focus:ssh-dev:w-lib", "tab_focus:ssh-dev:w-lib:t1"]);
    expect(remote.bridge.calls.filter((call) => call.command === "tab_focus")).toHaveLength(1);

    // A later engine list must not replay the focus.
    remote.bridge.emit({ type: "agents", agents: [remote.bridge.agent("w-lib:p1", "done")] });
    await settle();
    expect(order.filter((entry) => entry.startsWith("tab_focus:"))).toHaveLength(1);
  });

  // Would catch: an item of the attached host switching host anyway (a detach/attach round trip
  // the 035 path exists to avoid).
  it("does not switch host for an item of the selected host, and focuses its tab once", async () => {
    const { el, order, select, local, settle } = await renderWired();
    const item = items(el).find((node) => node.dataset.inboxItem!.startsWith("local"))!;
    item.click();
    await settle();
    expect(select).not.toHaveBeenCalled();
    expect(order).toEqual(["workspace_focus:local:w-api", "tab_focus:local:w-api:t1"]);
    expect(local.bridge.calls.filter((call) => call.command === "tab_focus")).toHaveLength(1);
  });
});

describe("AC-043-03 visitar a aba tira o item", () => {
  // Would catch: a finished item nagging after the user looked at it, or one that never returns
  // when the agent finishes again.
  it("drops a done item when its tab becomes focused and brings it back on the next change", () => {
    const hosts = hostsFixture({
      localAgents: [hostAgent({ pane_id: "w-api:p1", agent: "claude", agent_status: "done" })],
      remoteAgents: [],
    });
    const tabs: TabDto[] = [
      { tab_id: "w-api:t1", workspace_id: "w-api", label: "aba", number: 1, focused: false, pane_count: 1, agent_status: "done" },
      { tab_id: "w-api:t2", workspace_id: "w-api", label: "outra", number: 2, focused: true, pane_count: 1, agent_status: "idle" },
    ];
    const done = (seq: number) => [liveAgent({ pane_id: "w-api:p1", kind: "claude", status: "done", state_change_seq: seq })];
    const { el, agentsHolder } = render({
      hosts,
      agents: agentsState({ agents: done(1), tabs, focusedTabId: "w-api:t2" }),
    });
    expect(items(el)).toHaveLength(1);

    agentsHolder.value = agentsState({ agents: done(1), tabs, focusedTabId: "w-api:t1" });
    flushSync();
    expect(items(el), "the user is in the tab: the item leaves").toHaveLength(0);

    agentsHolder.value = agentsState({ agents: done(1), tabs, focusedTabId: "w-api:t2" });
    flushSync();
    expect(items(el), "same state: it does not come back").toHaveLength(0);

    agentsHolder.value = agentsState({ agents: done(2), tabs, focusedTabId: "w-api:t2" });
    flushSync();
    expect(items(el), "the agent finished again").toHaveLength(1);
  });

  // Would catch: a blocked agent dismissed by a visit — it still needs an answer in the terminal.
  it("keeps a blocked item through the visit and drops it only when it stops being blocked", () => {
    const hosts = hostsFixture({
      localAgents: [hostAgent({ pane_id: "w-api:p1", agent: "codex", agent_status: "blocked" })],
      remoteAgents: [],
    });
    const tabs: TabDto[] = [{ tab_id: "w-api:t1", workspace_id: "w-api", label: "aba", number: 1, focused: true, pane_count: 1, agent_status: "blocked" }];
    const { el, agentsHolder, hostsHolder } = render({
      hosts,
      agents: agentsState({
        agents: [liveAgent({ pane_id: "w-api:p1", kind: "codex", status: "blocked", detection_last_line: "Permitir sqlx migrate run?" })],
        tabs,
        focusedTabId: "w-api:t1",
      }),
    });
    expect(items(el), "visiting the tab does not answer the agent").toHaveLength(1);

    hostsHolder.value = hostsFixture({
      localAgents: [hostAgent({ pane_id: "w-api:p1", agent: "codex", agent_status: "working" })],
      remoteAgents: [],
    });
    agentsHolder.value = agentsState({
      agents: [liveAgent({ pane_id: "w-api:p1", kind: "codex", status: "working", state_change_seq: 2 })],
      tabs,
      focusedTabId: "w-api:t1",
    });
    flushSync();
    expect(items(el)).toHaveLength(0);
  });
});
