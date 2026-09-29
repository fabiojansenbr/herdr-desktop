// @vitest-environment happy-dom
// Spec 041 — the new sidebar mounted with a fake ctx: structure and actions (AC-041-01),
// collections across hosts with collapse (AC-041-02) and the workspace row (AC-041-03).
// Would catch the pre-041 window: PROJETOS split per host, no Novo agente block and no
// mount points for the parallel specs.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentDto, AgentStatus } from "../../agents/types";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import { createProjectsController } from "../../projects/controller";
import type { CollectionDto, ProjectDto } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import { resolveShortcut, shortcutLabel } from "../../shell/shortcuts";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder, type ReactiveHolder } from "../center/reactive-test-state.svelte";
import SidebarV2 from "./SidebarV2.svelte";

const sources = import.meta.glob(["../../App.svelte", "./*.svelte"], { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
});

function workspace(overrides: Partial<HostWorkspaceDto> & { workspace_id: string }): HostWorkspaceDto {
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

function agentDto(workspaceId: string, status: AgentStatus, paneId = `p-${workspaceId}-${status}`): AgentDto {
  return { pane_id: paneId, workspace_id: workspaceId, tab_id: "t1", name: paneId, kind: "codex", status, launch_pending: false, ready: true, focused: false };
}

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd041", online: true, identity: null },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  };
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

interface RenderOptions {
  hosts?: HostDto[];
  projects?: ProjectDto[];
  collections?: CollectionDto[];
  agents?: AgentDto[];
  selected?: string | ReactiveHolder<string | null>;
}

async function render(options: RenderOptions = {}) {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-041",
    seed: { version: 1, projects: options.projects ?? [], collections: options.collections ?? [] },
  });
  const controller = createProjectsController(bridge);
  await controller.load();
  const calls: string[] = [];
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [
      name,
      () => {
        calls.push(name);
      },
    ]),
  ) as Record<MenuAction, () => void>;
  const holder = typeof options.selected === "string" || options.selected === undefined ? reactiveHolder<string | null>(options.selected ?? "local") : options.selected;
  const hosts = options.hosts ?? [
    hostFixture({ endpoint: "local", label: "Este computador", kind: "local", phase: "online", phase_label: "Online", session: "hd041", target: null, workspaces: [] }),
  ];
  const createTab = vi.fn(async () => {});
  const focusWorkspace = vi.spyOn(controller, "focusWorkspace");
  const openClosedProject = vi.spyOn(controller, "openClosedProject");
  const select = vi.fn(async () => ({}) as never);
  const ctx: FrameContext = {
    surface: surface(),
    agents: { agents: options.agents ?? [], kinds: [] } as unknown as FrameContext["agents"],
    connections: connectionsState(hosts),
    get navigator() {
      return controller.state;
    },
    get selectedEndpoint() {
      return holder.value;
    },
    set selectedEndpoint(next: string | null) {
      holder.value = next;
    },
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: true,
    controllers: {
      surface: { select } as unknown as FrameContext["controllers"]["surface"],
      agents: { createTab, state: { topology: null } } as unknown as FrameContext["controllers"]["agents"],
      projects: controller,
      connections: {
        openDialog: () => {},
        disconnect: vi.fn(async () => {}),
        reconnect: vi.fn(async () => {}),
        removeProfile: vi.fn(async () => {}),
        editProfile: vi.fn(async () => {}),
      } as unknown as FrameContext["controllers"]["connections"],
    },
    actions,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  el.style.height = "800px";
  document.body.appendChild(el);
  const app = mount(SidebarV2, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, app: app as unknown as { openNewAgent: () => void }, calls, controller, focusWorkspace, openClosedProject, createTab, holder };
}

/** AC-041-02 fixture: Local (`api` in G1, `web` loose), online SSH `dev-box` (`lib` in G1), empty `G2`. */
function collectionsFixture(): RenderOptions {
  return {
    hosts: [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        phase_label: "Online",
        session: "hd041",
        target: null,
        workspaces: [
          workspace({ workspace_id: "w-web", number: 2, label: "web", cwd: "/w/web", branch: "feat/login" }),
          workspace({ workspace_id: "w-api", number: 1, label: "api", cwd: "/w/api", branch: "main", focused: true }),
        ],
      }),
      hostFixture({
        endpoint: "ssh-dev",
        label: "dev-box",
        kind: "ssh",
        phase: "online",
        phase_label: "Online",
        session: "hd041",
        workspaces: [workspace({ workspace_id: "w-lib", number: 1, label: "lib", cwd: "/w/lib", branch: "master" })],
      }),
    ],
    projects: [
      { id: "p-api", label: "api", endpoint_profile_id: "local", session_name: "hd041", root: "/w/api", binding: null },
      { id: "p-web", label: "web", endpoint_profile_id: "local", session_name: "hd041", root: "/w/web", binding: null },
      { id: "p-lib", label: "lib", endpoint_profile_id: "ssh-dev", session_name: "hd041", root: "/w/lib", binding: null },
    ],
    collections: [
      { id: "g1", name: "G1", project_ids: ["p-api", "p-lib"] },
      { id: "g2", name: "G2", project_ids: [] },
    ],
  };
}

/**
 * AC-041-03 fixture: three distinct statuses in different workspaces (`api` working ×2,
 * `web` waiting, `docs` done), a workspace without branch (`web`) and an offline SSH host.
 */
function rowsFixture(): RenderOptions {
  return {
    hosts: [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        phase_label: "Online",
        session: "hd041",
        target: null,
        workspaces: [
          workspace({ workspace_id: "w-api", number: 1, label: "api", cwd: "/w/api", branch: "main", focused: true }),
          workspace({ workspace_id: "w-web", number: 2, label: "web", cwd: "/w/web", branch: null }),
          workspace({ workspace_id: "w-docs", number: 3, label: "docs", cwd: "/w/docs", branch: "docs" }),
          workspace({ workspace_id: "w-idle", number: 4, label: "idle-ws", cwd: "/w/idle", branch: "main" }),
        ],
      }),
      hostFixture({
        endpoint: "ssh-dev",
        label: "dev-box",
        kind: "ssh",
        phase: "offline",
        phase_label: "Offline",
        session: "hd041",
        workspaces: [workspace({ workspace_id: "w-lib", number: 1, label: "lib", cwd: "/w/lib", branch: "master" })],
      }),
    ],
    projects: [
      { id: "p-closed", label: "closed-app", endpoint_profile_id: "local", session_name: "hd041", root: "/w/closed", binding: null },
    ],
    agents: [
      agentDto("w-api", "working"),
      agentDto("w-api", "working", "p-api-2"),
      agentDto("w-web", "blocked"),
      agentDto("w-docs", "done"),
      agentDto("w-idle", "idle"),
    ],
  };
}

const order = (el: HTMLElement, selector: string) => [...el.querySelectorAll<HTMLElement>(selector)];

describe("AC-041-01 structure and actions of the new sidebar", () => {
  // Would catch: the region losing the `data-slot="projects"` contract, or the blocks shuffled
  // (the actions above the brand, CONEXÕES floating in the middle, no mount point for 043).
  it("renders header, actions, InboxSection, Workspaces and HostSwitcher in this order", async () => {
    const { el } = await render();
    const root = el.querySelector<HTMLElement>('[data-slot="projects"]')!;
    expect(root).toBeTruthy();
    const slots = order(root, "[data-sidebar-header],[data-sidebar-actions],[data-sidebar-inbox],[data-sidebar-workspaces],[data-sidebar-hosts]");
    expect(slots.map((n) => n.dataset.sidebarHeader !== undefined ? "header" : n.dataset.sidebarActions !== undefined ? "actions" : n.dataset.sidebarInbox !== undefined ? "inbox" : n.dataset.sidebarWorkspaces !== undefined ? "workspaces" : "hosts")).toEqual([
      "header",
      "actions",
      "inbox",
      "workspaces",
      "hosts",
    ]);
    expect(root.querySelector("[data-sidebar-header]")!.textContent).toContain("herdr");
    expect(root.querySelector("[data-sidebar-workspaces]")!.textContent).toContain("Workspaces");
    // The 043 mount point exists and renders nothing in this spec.
    expect(root.querySelector("[data-sidebar-inbox]")!.textContent!.trim()).toBe("");
  });

  // Would catch: the collapse button toggling a local flag instead of the window's action, or
  // firing it twice (one click collapsing and immediately reopening the sidebar).
  it("calls ctx.actions.toggleProjects exactly once from Recolher lateral", async () => {
    const { el, calls } = await render();
    const button = el.querySelector<HTMLButtonElement>("[data-collapse-sidebar]")!;
    expect(button.getAttribute("aria-label")).toBe("Recolher lateral");
    button.click();
    flushSync();
    expect(calls.filter((c) => c === "toggleProjects")).toEqual(["toggleProjects"]);
  });

  // Spec 056 (AC-056-01/02): Buscar left the sidebar, so the action block is Novo agente alone
  // and the palette is reached by the chord the window claims. Would catch: the search button
  // back in the block, its `Ctrl K` hint left behind, or Ctrl K no longer opening the palette.
  it("drops Buscar from the action block and leaves the palette on the Ctrl K chord", async () => {
    const { el, calls } = await render();
    const actions = el.querySelector<HTMLElement>("[data-sidebar-actions]")!;
    expect([...actions.querySelectorAll<HTMLElement>("[data-sidebar-action]")].map((node) => node.dataset.sidebarAction)).toEqual(["newAgent"]);
    expect(actions.querySelector('[data-sidebar-action="search"]')).toBeNull();
    expect(actions.textContent).not.toContain("Buscar");
    expect(actions.textContent).not.toContain(shortcutLabel("palette", "linux"));
    expect(calls).not.toContain("openPalette");

    const key = { key: "k", ctrlKey: true, metaKey: false, shiftKey: false, altKey: false, isComposing: false, repeat: false, target: null };
    expect(resolveShortcut(key, "linux")).toBe("palette");
    expect(resolveShortcut({ ...key, ctrlKey: false, metaKey: true }, "macos")).toBe("palette");
    const app = sources["../../App.svelte"]!;
    expect(app).toMatch(/id === "palette"/);
    expect(app).toMatch(/openPalette\(\)/);
  });

  // Would catch (P8, reverting part of 040): Novo agente missing from the sidebar, or opening
  // something other than the 017 popover.
  it("mounts NewAgentPopover from Novo agente and unmounts it on onclose", async () => {
    const { el, createTab } = await render();
    const button = el.querySelector<HTMLButtonElement>('[data-sidebar-action="newAgent"]')!;
    expect(button.textContent).toContain("Novo agente");
    expect(button.textContent).toContain(shortcutLabel("new-agent", "linux"));
    expect(button.textContent).toContain("Ctrl+Shift+N");
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();

    button.click();
    flushSync();
    const popover = el.querySelector<HTMLFormElement>("[data-new-agent-popover]")!;
    expect(popover, "popover of spec 017").toBeTruthy();

    // `Shell` is the only option with no engine kinds: Iniciar creates the tab and calls onclose.
    popover.querySelector<HTMLButtonElement>("[data-start-agent]")!.click();
    flushSync();
    await Promise.resolve();
    await Promise.resolve();
    flushSync();
    expect(createTab).toHaveBeenCalledTimes(1);
    expect(el.querySelector("[data-new-agent-popover]"), "onclose unmounts it").toBeNull();
  });

  // Would catch: Ctrl+Shift+N unclaimed (reaching the terminal), bound to Ctrl+N (readline/vim),
  // or the macOS build left on the Ctrl chord.
  it("declares new-agent as Ctrl+Shift+N (⌘⇧N on macOS) and resolves it only with both modifiers", () => {
    const key = { key: "N", ctrlKey: true, metaKey: false, shiftKey: true, altKey: false, isComposing: false, repeat: false, target: null };
    expect(resolveShortcut(key, "linux")).toBe("new-agent");
    expect(resolveShortcut(key, "windows")).toBe("new-agent");
    expect(resolveShortcut({ ...key, ctrlKey: true, shiftKey: false }, "linux")).toBeNull();
    expect(resolveShortcut({ ...key, ctrlKey: false, metaKey: true }, "macos")).toBe("new-agent");
    expect(shortcutLabel("new-agent", "linux")).toBe("Ctrl+Shift+N");
    expect(shortcutLabel("new-agent", "macos")).toBe("⌘⇧N");
  });

  // Would catch: the chord opening a second popover of its own instead of the sidebar's one.
  it("opens the same popover through the chord entry point the window calls", async () => {
    const { el, app } = await render();
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();
    app.openNewAgent();
    flushSync();
    expect(el.querySelector("[data-new-agent-popover]")).toBeTruthy();
    const app2 = sources["../../App.svelte"]!;
    expect(app2).toMatch(/id === "new-agent"/);
    expect(app2).toMatch(/sidebar\?\.openNewAgent\(\)/);
    expect(app2).toMatch(/<SidebarV2\b[^>]*bind:this=\{sidebar\}/);
  });

  // Would catch: the foot losing the host selector (spec 048 replaced the 041 slot content, the
  // CONEXÕES list, with the compact line of the design; the host block itself must stay).
  it("keeps the host selector inside HostSwitcher", async () => {
    const { el } = await render();
    const hosts = el.querySelector<HTMLElement>("[data-sidebar-hosts]")!;
    const trigger = hosts.querySelector<HTMLElement>("[data-host-trigger]")!;
    expect(trigger).toBeTruthy();
    expect(trigger.textContent).toMatch(/Este computador/);
  });
});

describe("AC-041-02 WORKSPACES section", () => {
  // Would catch: a section per host, SSH before Local inside a collection, the empty collection
  // dropped, or `Sem coleção` before the named ones.
  it("lists G1 (api, lib), the empty G2 and Sem coleção (web), each with colour and count", async () => {
    const { el } = await render(collectionsFixture());
    const section = el.querySelector<HTMLElement>("[data-sidebar-workspaces]")!;
    const groups = order(section, "[data-collection]");
    expect(groups.map((g) => g.dataset.collection)).toEqual(["g1", "g2", "__ungrouped__"]);
    expect(groups.map((g) => g.querySelector("[data-collection-name]")!.textContent!.trim())).toEqual(["G1", "G2", "Sem coleção"]);
    expect(groups.map((g) => g.querySelector("[data-collection-count]")!.textContent!.trim())).toEqual(["2", "0", "1"]);
    for (const group of groups) {
      const swatch = group.querySelector<HTMLElement>("[data-collection-color]")!;
      expect(swatch.style.backgroundColor, group.dataset.collection).not.toBe("");
    }
    expect(order(groups[0]!, "[data-workspace-row]").map((r) => r.dataset.workspaceRow)).toEqual(["w-api", "w-lib"]);
    expect(order(groups[0]!, "[data-workspace-row]").map((r) => r.dataset.endpoint)).toEqual(["local", "ssh-dev"]);
    expect(order(groups[1]!, "[data-workspace-row]")).toEqual([]);
    expect(order(groups[2]!, "[data-workspace-row]").map((r) => r.dataset.workspaceRow)).toEqual(["w-web"]);
  });

  // Would catch: one collapse flag shared by every collection, or a collapse that hides the header.
  it("collapses and expands only the clicked collection, keeping the state in memory", async () => {
    const { el } = await render(collectionsFixture());
    const g1 = el.querySelector<HTMLElement>('[data-collection="g1"]')!;
    const ungrouped = el.querySelector<HTMLElement>('[data-collection="__ungrouped__"]')!;
    const header = g1.querySelector<HTMLElement>("[data-collection-toggle]")!;
    expect(header.getAttribute("aria-expanded")).toBe("true");

    header.click();
    flushSync();
    expect(header.getAttribute("aria-expanded")).toBe("false");
    expect(order(g1, "[data-workspace-row]")).toEqual([]);
    expect(g1.querySelector("[data-collection-name]")!.textContent).toContain("G1");
    expect(order(ungrouped, "[data-workspace-row]").map((r) => r.dataset.workspaceRow)).toEqual(["w-web"]);

    header.click();
    flushSync();
    expect(header.getAttribute("aria-expanded")).toBe("true");
    expect(order(g1, "[data-workspace-row]").map((r) => r.dataset.workspaceRow)).toEqual(["w-api", "w-lib"]);
  });

  // Edge case of the spec: no host connected keeps the saved collections and shows the empty state.
  it("shows Nenhum workspace with the saved collections when no host is connected", async () => {
    const { el } = await render({ ...collectionsFixture(), hosts: [] });
    const section = el.querySelector<HTMLElement>("[data-sidebar-workspaces]")!;
    expect(section.textContent).toContain("Nenhum workspace");
    expect(order(section, "[data-collection]").map((g) => g.dataset.collection)).toEqual(["g1", "g2"]);
  });

  // Would catch: an empty `Sem coleção` header drawn when nothing is loose.
  it("omits Sem coleção when it is empty", async () => {
    const base = collectionsFixture();
    const { el } = await render({ ...base, collections: [{ id: "g1", name: "G1", project_ids: ["p-api", "p-web", "p-lib"] }] });
    expect(el.querySelector('[data-collection="__ungrouped__"]')).toBeNull();
  });
});

describe("AC-041-03 workspace row", () => {
  // Would catch: the 038 regression (a `—` branch line) or the mono font lost on the branch.
  it("shows the name, the mono branch line only when there is a branch, and no dash", async () => {
    const { el } = await render(rowsFixture());
    const api = el.querySelector<HTMLElement>('[data-workspace-row="w-api"]')!;
    expect(api.textContent).toContain("api");
    expect(api.querySelector("[data-branch]")!.textContent).toContain("main");
    const web = el.querySelector<HTMLElement>('[data-workspace-row="w-web"]')!;
    expect(web.querySelector("[data-branch]")).toBeNull();
    expect(web.textContent).not.toContain("—");
    expect(rule(sources["./WorkspaceItem.svelte"]!, ".branch")).toMatch(/font-family:\s*var\(--font-mono\)/);
  });

  // Would catch: the host tag on local rows, or an offline host rendered as if it were live.
  it("tags SSH rows with the host name and dims the rows of a host that is not online", async () => {
    const { el } = await render(rowsFixture());
    const api = el.querySelector<HTMLElement>('[data-workspace-row="w-api"]')!;
    const lib = el.querySelector<HTMLElement>('[data-workspace-row="w-lib"]')!;
    expect(api.querySelector("[data-host-badge]")).toBeNull();
    expect(lib.querySelector("[data-host-badge]")!.textContent).toContain("dev-box");
    expect(api.getAttribute("data-offline")).toBeNull();
    expect(lib.getAttribute("data-offline")).toBe("true");
    expect(lib.textContent).toContain("offline");
    expect(rule(sources["./WorkspaceItem.svelte"]!, '.row[data-offline="true"]')).toMatch(/opacity:/);
  });

  // Would catch: `done` folded into idle (mapAgentStatus), the badge counting every agent, or a
  // marker drawn on a fully idle row.
  it("summarises status on the right: green count, amber waiting, green done, nothing when idle", async () => {
    const { el } = await render(rowsFixture());
    const status = (id: string) => el.querySelector<HTMLElement>(`[data-workspace-row="${id}"] [data-row-status]`);
    expect(status("w-api")!.dataset.rowStatus).toBe("working");
    expect(status("w-api")!.textContent).toContain("2");
    expect(status("w-api")!.getAttribute("aria-label")).toBe("2 trabalhando");
    expect(status("w-web")!.dataset.rowStatus).toBe("waiting");
    expect(status("w-web")!.getAttribute("aria-label")).toBe("1 aguardando");
    expect(status("w-docs")!.dataset.rowStatus).toBe("done");
    expect(status("w-docs")!.getAttribute("aria-label")).toBe("1 concluído");
    expect(status("w-idle"), "a fully idle row shows nothing").toBeNull();
    expect(status("w-lib"), "a row with no agents shows nothing").toBeNull();
    const css = sources["./WorkspaceItem.svelte"]!;
    expect(rule(css, '[data-row-status="working"]')).toMatch(/var\(--working/);
    expect(rule(css, '[data-row-status="done"]')).toMatch(/var\(--working/);
    expect(rule(css, '[data-row-status="waiting"]')).toMatch(/var\(--attention/);
  });

  // Spec 036/041 P7. Would catch: two bold rows, the highlight stuck on Local when SSH is
  // selected, or the 042 mount point missing/duplicated.
  it("keeps exactly one active row, bold, carrying the WorkspaceTabList mount point", async () => {
    const holder = reactiveHolder<string | null>("local");
    const { el } = await render({ ...rowsFixture(), selected: holder });
    const active = el.querySelectorAll<HTMLElement>('[data-workspace-row][data-active="true"]');
    expect(active).toHaveLength(1);
    expect(active[0]!.dataset.workspaceRow).toBe("w-api");
    expect(el.querySelectorAll("[data-workspace-tabs]")).toHaveLength(1);
    expect(active[0]!.parentElement!.querySelector("[data-workspace-tabs]")).toBeTruthy();
    expect(rule(sources["./WorkspaceItem.svelte"]!, '.row[data-active="true"] .name')).toMatch(/font-weight:\s*500/);
    // The 044 mount point is on every row, not only the active one.
    expect(el.querySelectorAll("[data-workspace-actions]").length).toBe(el.querySelectorAll("[data-workspace-row]").length);
  });

  // Would catch: the focus path rebuilt here (a second workspace_focus, a host reconnect) instead
  // of the controller call the 025/035 row already makes.
  it("focuses a live row through the controller and opens a closed one, once each", async () => {
    const { el, focusWorkspace, openClosedProject } = await render(rowsFixture());
    el.querySelector<HTMLElement>('[data-workspace-row="w-web"]')!.click();
    flushSync();
    expect(focusWorkspace).toHaveBeenCalledExactlyOnceWith("local", "w-web");
    expect(openClosedProject).not.toHaveBeenCalled();

    const closed = el.querySelector<HTMLElement>('[data-workspace-row][data-closed="true"]')!;
    expect(closed.textContent).toContain("closed-app");
    closed.click();
    flushSync();
    expect(openClosedProject).toHaveBeenCalledExactlyOnceWith("p-closed");
    expect(focusWorkspace).toHaveBeenCalledTimes(1);
  });

  // Would catch: an offline row still sending workspace_focus to a host that is not connected.
  it("sends nothing when a row of an offline host is clicked", async () => {
    const { el, focusWorkspace } = await render(rowsFixture());
    el.querySelector<HTMLElement>('[data-workspace-row="w-lib"]')!.click();
    flushSync();
    expect(focusWorkspace).not.toHaveBeenCalled();
  });
});
