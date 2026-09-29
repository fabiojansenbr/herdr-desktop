// @vitest-environment happy-dom
// Spec 018 — clean casca: only the four regions of frame Z1lHQt, collapsible sidebar, and
// open → choose → terminals. Would catch: the 017 window (activity/agents/files/home still
// mounted), a title bar that keeps the menus instead of the project switcher / Novo agente /
// waiting bell, a sidebar that ignores Ctrl+B or localStorage, or a frame menu that drops
// Fechar pane / fires two bridge calls for one click.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../agents/controller";
import { createFakeAgentsBridge } from "../agents/fake-bridge";
import NewAgentPopover from "../components/center/NewAgentPopover.svelte";
import { PANE_MENU_ITEMS } from "../components/center/pane-menu";
import PaneFrames from "../components/center/PaneFrames.svelte";
import WorkspaceTabs from "../components/center/WorkspaceTabs.svelte";
import ProjectSwitcher from "../components/frame/ProjectSwitcher.svelte";
import { openProjectItem, projectSwitcherLabel, projectSwitcherMenu } from "../components/frame/project-switcher";
import Sidebar from "../components/frame/Sidebar.svelte";
import SidebarHeader from "../components/sidebar/SidebarHeader.svelte";
import { readSidebarOpen, SIDEBAR_STORAGE_KEY, writeSidebarOpen } from "../components/frame/sidebar";
import StatusBar from "../components/frame/StatusBar.svelte";
import { statusBarModel } from "../components/frame/status";
import TitleBar from "../components/frame/TitleBar.svelte";
import type { MenuAction } from "../components/frame/menus";
import type { WindowApi } from "../components/frame/window-api";
import { createProjectsController } from "../projects/controller";
import { createFakeProjectsBridge } from "../projects/fake-bridge";
import type { FrameContext } from "./frame-context";
import { resolveShortcut } from "./shortcuts";
import type { SurfaceState } from "./controller";
import Workbench from "./Workbench.svelte";

const appSource = readFileSync(resolve("src/App.svelte"), "utf8");
const workbenchSource = readFileSync(resolve("src/shell/Workbench.svelte"), "utf8");
const titleBarSource = readFileSync(resolve("src/components/frame/TitleBar.svelte"), "utf8");
const tabsSource = readFileSync(resolve("src/components/center/WorkspaceTabs.svelte"), "utf8");
const centerSource = readFileSync(resolve("src/components/center/CenterRegion.svelte"), "utf8");
const framesSource = readFileSync(resolve("src/components/center/PaneFrames.svelte"), "utf8");

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

function fakeWindowApi(): WindowApi & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    minimize: () => {
      calls.push("minimize");
    },
    toggleMaximize: () => {
      calls.push("maximize");
    },
    close: () => {
      calls.push("close");
    },
  };
}

function liveSurface(): SurfaceState {
  return {
    selection: {
      endpoint: "local",
      kind: "local",
      label: "Este computador",
      session: "default",
      online: true,
      identity: null,
    },
    status: { session: "default", session_available: true, connected: true, state: "live", reason: null, generation: null, connection_generation: 1, boot_id: "boot-018", server_version: "0.9.0", pane_id: "w1:p1", last_error: null },
    phase: "live",
    reason: null,
    error: null,
    identity: {
      endpoint: "local",
      session: "default",
      connection_generation: 1,
      boot_id: "boot-018",
      pane_id: "w1:p1",
    },
    surfaceKey: 1,
    busy: false,
  };
}

function topLevelItems(root: HTMLElement): string[] {
  return Array.from(root.querySelectorAll<HTMLElement>("[data-topbar-item]"))
    .filter((el) => !el.parentElement?.closest("[data-topbar-item]"))
    .map((el) => el.dataset.topbarItem ?? "");
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
  localStorage.removeItem(SIDEBAR_STORAGE_KEY);
});

describe("AC-018-01 four regions and title bar order", () => {
  // Would catch: App still mounting the 017 activity/agents/files/home regions, or Workbench
  // keeping data-region="projects"|"center" as the window regions.
  it("the window source mounts only topbar, sidebar, main and status", () => {
    const regions = [...appSource.matchAll(/data-region="([^"]+)"/g)].map((m) => m[1]);
    const wb = [...workbenchSource.matchAll(/data-region="([^"]+)"/g)].map((m) => m[1]);
    expect(regions).toEqual([]);
    expect(wb).toEqual(["main"]);
    expect(titleBarSource).toContain('data-region="topbar"');
    expect(readFileSync(resolve("src/components/frame/Sidebar.svelte"), "utf8")).toContain('data-region="sidebar"');
    expect(readFileSync(resolve("src/components/frame/StatusBar.svelte"), "utf8")).toContain('data-region="status"');
    expect(appSource).not.toMatch(/<ActivityBar\b/);
    expect(appSource).not.toMatch(/<AgentsRegion\b/);
    expect(appSource).not.toMatch(/<FilesRegion\b/);
    expect(appSource).not.toMatch(/<HomeRegion\b/);
    expect(appSource).not.toMatch(/homeShown = true/);
    expect(workbenchSource).not.toMatch(/data-region="activity"/);
    expect(workbenchSource).not.toMatch(/data-region="agents"/);
    expect(workbenchSource).not.toMatch(/data-region="files"/);
    expect(workbenchSource).not.toMatch(/data-region="home"/);
  });

  it("Workbench paints exactly the four regions and none of the removed ones", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(Workbench, { target: el, props: { sidebarOpen: true } });
    flushSync();
    hosts.push({ el, app });
    const title = document.createElement("header");
    title.setAttribute("data-region", "topbar");
    const status = document.createElement("footer");
    status.setAttribute("data-region", "status");
    el.querySelector(".wb-shell")?.prepend(title);
    el.querySelector(".wb-shell")?.append(status);
    const names = Array.from(el.querySelectorAll<HTMLElement>("[data-region]")).map((n) => n.dataset.region);
    expect(names.sort()).toEqual(["main", "sidebar", "status", "topbar"]);
    for (const gone of ["activity", "agents", "files", "home", "projects", "center"]) {
      expect(el.querySelector(`[data-region="${gone}"]`)).toBeNull();
    }
  });

  // Spec 047 AC-047-04 (moldura do desenho). Would catch: the top bar back above the whole
  // window, which cuts the sidebar 44 px below the top and puts the bar over it.
  it("the sidebar starts at the top of the window and the top bar sits only over the main area", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(Workbench, { target: el, props: { sidebarOpen: true } });
    flushSync();
    hosts.push({ el, app });
    const shell = el.querySelector<HTMLElement>(".wb-shell")!;
    const body = el.querySelector<HTMLElement>(".wb-body")!;
    const aside = el.querySelector<HTMLElement>("[data-region='sidebar']")!;
    const column = el.querySelector<HTMLElement>(".wb-column")!;
    const main = el.querySelector<HTMLElement>("[data-region='main']")!;
    expect(shell.firstElementChild).toBe(body);
    expect(aside.parentElement).toBe(body);
    expect(column.parentElement).toBe(body);
    expect(column.contains(main)).toBe(true);
    expect(aside.contains(column)).toBe(false);
    expect(workbenchSource).toMatch(/<div class="wb-column">\s*\{@render topBar\?\.\(\)\}/);
    // The rail of 047 is the sidebar's own slot, so the collapsed sidebar still paints.
    expect(workbenchSource).toMatch(/\{@render railRegion\?\.\(\)\}/);
  });

  // Spec 047 (AC-047-04): the bar became the frame over the main area — the mark, the search and
  // the host pill moved into the sidebar (Buscar/Ctrl K of 041, host foot of 048).
  // Spec 055 (AC-055-01): the breadcrumb left it too; the bar opens with the workspace tabs.
  it("the title bar items are the bell and the window controls, with no breadcrumb", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(TitleBar, {
      target: el,
      props: {
        host: { label: "dev-box", kind: "SSH", state: "conectado", tone: "live" as const },
        searchHint: "Ctrl K",
        onOpenPalette: () => {},
        onHost: () => {},
        windowApi: fakeWindowApi(),
        sidebarOpen: true,
        onToggleSidebar: () => {},
        waitingCount: 2,
        projectLabel: "Acme › erp-api ⎇ main",
      },
    });
    flushSync();
    hosts.push({ el, app });
    // AC-049-03: the bar lost its panel button; the sidebar head keeps the only one.
    // AC-055-01: and the breadcrumb, whose branch and path stay in the status bar.
    expect(topLevelItems(el)).toEqual([
      "notifications",
      "window-minimize",
      "window-maximize",
      "window-close",
    ]);
    expect(el.querySelector("[data-topbar-item='project']")).toBeNull();
    expect(el.textContent).not.toContain("Acme › erp-api ⎇ main");
    expect(el.querySelector("[data-topbar-item='sidebar']")).toBeNull();
    expect(el.querySelector("[data-panel-icon]")).toBeNull();
    expect(el.textContent).not.toContain("herdr");
    expect(el.textContent).not.toContain("Novo agente");
    expect(el.querySelector("[data-topbar-item='new-agent']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='logo']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='search']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='host']")).toBeNull();
    expect(el.textContent).not.toMatch(/Ctrl K/);
    expect(el.querySelector("[data-waiting-count]")?.textContent?.trim()).toBe("2");
  });

  // Spec 055 AC-055-01/02: the tabs are the item that yields; the free drag area takes what is
  // left and the bell and the controls keep their size.
  it("the tab strip is the only item that yields; the controls keep their size", () => {
    expect(titleBarSource).toMatch(/:global\(\[data-center-tabs\]\)[^}]*min-width:\s*0/);
    expect(titleBarSource).toMatch(/\.drag \{[^}]*flex:\s*1/);
    expect(titleBarSource).toMatch(/\.right \{[^}]*flex:\s*none/);
    expect(titleBarSource).not.toMatch(/\.crumb \{/);
  });

  it("the tab bar keeps the status icon, the title and +; the grid is 8 px", () => {
    expect(tabsSource).toMatch(/data-tab-icon=\{tab\.icon\}/);
    expect(tabsSource).toMatch(/data-label/);
    expect(tabsSource).toMatch(/data-more/);
    expect(tabsSource).toMatch(/data-new-tab/);
    expect(tabsSource).toMatch(/height:\s*48px/);
    // Spec 055: the strip is drawn by the top bar, not by a row of the center region.
    expect(titleBarSource).toMatch(/<WorkspaceTabs\b/);
    expect(centerSource).not.toMatch(/WorkspaceTabs/);
    expect(centerSource).toMatch(/padding:\s*8px/);
    expect(centerSource).toMatch(/gap:\s*8px/);
    expect(centerSource).not.toMatch(/<WorkspaceHeader/);
    expect(framesSource).toMatch(/border-radius:\s*0/);
    expect(framesSource).toMatch(/background:\s*transparent/);
    expect(framesSource).toMatch(/border:\s*none/);
  });

  it("the status line shows the server, pane/tab counts and active agents", () => {
    expect(
      statusBarModel({
        phase: "live",
        hasEndpoint: true,
        serverVersion: "0.9.0",
        branch: "main",
        panes: 3,
        tabs: 3,
        agents: 4,
      }),
    ).toMatchObject({
      server: "herdr server 0.9.0 · conectado",
      counts: "3 panes · 3 abas",
      agents: "4 agentes ativos",
    });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(StatusBar, {
      target: el,
      props: {
        phase: "live",
        diagnostics: "ok",
        model: statusBarModel({
          phase: "live",
          hasEndpoint: true,
          serverVersion: "0.9.0",
          branch: "main",
          panes: 3,
          tabs: 3,
          agents: 4,
        }),
        host: "dev-box · SSH",
        session: "default",
        blocked: null,
        readOnly: false,
      },
    });
    flushSync();
    hosts.push({ el, app });
    expect(el.textContent).toMatch(/herdr server 0\.9\.0 · conectado/);
    expect(el.textContent).toMatch(/3 panes · 3 abas/);
    expect(el.textContent).toMatch(/4 agentes ativos/);
  });

  // AC-027-02. Would catch: the closed-tab warning existing only in state, never on screen.
  it("shows a temporary notice on the status line and nothing when there is none", () => {
    const model = statusBarModel({ phase: "live", hasEndpoint: true, serverVersion: "0.9.0", branch: "main", panes: 1, tabs: 1 });
    const mountStatus = (notice: string | null) => {
      const el = document.createElement("div");
      document.body.appendChild(el);
      const app = mount(StatusBar, {
        target: el,
        props: { phase: "live", diagnostics: "ok", model, host: "Local · Local", session: "default", blocked: null, readOnly: false, notice },
      });
      flushSync();
      hosts.push({ el, app });
      return el;
    };
    expect(mountStatus("Aba fechada em outro cliente").querySelector("[data-item='notice']")?.textContent).toContain("Aba fechada em outro cliente");
    expect(mountStatus(null).querySelector("[data-item='notice']")).toBeNull();
  });
});

describe("AC-018-02 collapsible sidebar", () => {
  it("click and Ctrl+B toggle aria-expanded; remount reads herdr.sidebar.open", () => {
    localStorage.removeItem(SIDEBAR_STORAGE_KEY);
    expect(readSidebarOpen(localStorage)).toBe(true);
    const el = document.createElement("div");
    document.body.appendChild(el);
    let open = readSidebarOpen(localStorage);
    const app = mount(Sidebar, {
      target: el,
      props: {
        get open() {
          return open;
        },
        set open(value: boolean) {
          open = value;
          writeSidebarOpen(value, localStorage);
        },
      },
    });
    flushSync();
    hosts.push({ el, app });
    const aside = el.querySelector<HTMLElement>("[data-region='sidebar']")!;
    expect(aside.getAttribute("aria-expanded")).toBe("true");
    expect(aside.style.width === "288px" || getComputedStyle(aside).width !== "0px").toBe(true);

    // AC-049-03: the only collapse control is the one in the sidebar head.
    const head = document.createElement("div");
    document.body.appendChild(head);
    const header = mount(SidebarHeader, {
      target: head,
      props: {
        onCollapse: () => {
          open = !open;
          writeSidebarOpen(open, localStorage);
        },
      },
    });
    flushSync();
    hosts.push({ el: head, app: header });
    head.querySelector<HTMLButtonElement>("[data-collapse-sidebar]")!.click();
    flushSync();
    expect(open).toBe(false);
    expect(localStorage.getItem(SIDEBAR_STORAGE_KEY)).toBe("false");

    unmount(app);
    el.remove();
    hosts.shift();
    const remounted = document.createElement("div");
    document.body.appendChild(remounted);
    const again = mount(Sidebar, {
      target: remounted,
      props: { open: readSidebarOpen(localStorage) },
    });
    flushSync();
    hosts.push({ el: remounted, app: again });
    expect(remounted.querySelector("[data-region='sidebar']")!.getAttribute("aria-expanded")).toBe("false");
  });

  it("Ctrl+B is a window shortcut that does not claim Ctrl+C or terminal Ctrl+B", () => {
    const outside = { closest: () => null };
    const inside = { closest: (selector: string) => (selector.includes(".ime-target") ? {} : null) };
    const key = (k: string, mods: Record<string, boolean> = {}, target: { closest: (s: string) => unknown } | null = outside) => ({
      key: k,
      ctrlKey: false,
      metaKey: false,
      shiftKey: false,
      altKey: false,
      isComposing: false,
      repeat: false,
      target,
      ...mods,
    });
    expect(resolveShortcut(key("b", { ctrlKey: true }), "linux")).toBe("sidebar");
    expect(resolveShortcut(key("b", { ctrlKey: true }, inside), "linux")).toBeNull();
    expect(resolveShortcut(key("c", { ctrlKey: true }), "linux")).toBeNull();
  });
});

describe("AC-018-03 open → choose → terminals", () => {
  it("the switcher menu lists grouped projects then Abrir projeto…; empty saved shows only that item", () => {
    expect(
      projectSwitcherLabel({ group: "Acme", project: "erp-api", branch: "main", session: "default" }),
    ).toBe("Acme › erp-api ⎇ main");
    expect(projectSwitcherLabel({ group: null, project: null, branch: null, session: "default" })).toBe("sessão default");
    const groups = projectSwitcherMenu(
      [
        { id: "c1", name: "Acme", project_ids: ["p1"] },
        { id: "c2", name: "Vazio", project_ids: [] },
      ],
      [
        {
          id: "p1",
          label: "erp-api",
          endpoint_profile_id: "local",
          session_name: "default",
          root: "/work/erp-api",
          binding: null,
        },
      ],
    );
    expect(groups.map((g) => g.name)).toEqual(["Acme"]);
    expect(groups[0]!.projects.map((p) => p.label)).toEqual(["erp-api"]);
    expect(projectSwitcherMenu([], [])).toEqual([]);
    expect(openProjectItem()).toBe("Abrir projeto…");
  });

  // Spec 025 AC-025-05: the menu never offers a "closed project" whose cwd is an open workspace.
  it("hides saved projects whose root cwd is a live workspace", () => {
    const collections = [{ id: "c1", name: "Acme", project_ids: ["p1", "p2"] }];
    const projects = [
      { id: "p1", label: "erp-api", endpoint_profile_id: "local", session_name: "default", root: "/work/erp-api", binding: null },
      { id: "p2", label: "closed-one", endpoint_profile_id: "local", session_name: "default", root: "/work/closed", binding: null },
    ];
    const live = projectSwitcherMenu(collections, projects, ["/work/erp-api/"]);
    expect(live.map((g) => g.projects.map((p) => p.label))).toEqual([["closed-one"]]);
    // No live roots: the catalog is listed as saved.
    expect(projectSwitcherMenu(collections, projects)[0]!.projects).toHaveLength(2);
  });

  it("choosing a project opens it once; Abrir projeto… calls the 016 folder picker once", async () => {
    const projectsBridge = createFakeProjectsBridge({
      bootId: "boot-018",
      seed: {
        version: 1,
        collections: [{ id: "c1", name: "Acme", project_ids: ["p1"] }],
        projects: [
          {
            id: "p1",
            label: "erp-api",
            endpoint_profile_id: "local",
            session_name: "default",
            root: "/work/erp-api",
            binding: null,
            branch: "main",
          },
        ],
      },
    });
    const projects = createProjectsController(projectsBridge);
    await projects.load();
    projectsBridge.calls.length = 0;
    const pick = vi.fn(async () => "/tmp/picked");
    const el = document.createElement("div");
    document.body.appendChild(el);
    const ctx = {
      surface: liveSurface(),
      agents: null,
      connections: null,
      navigator: projects.state,
      selectedEndpoint: "local",
      activeProject: null,
      hostLabels: {},
      branch: null,
      activity: "projects" as const,
      view: "terminal" as const,
      sidebarOpen: true,
      agentsOpen: false,
      controllers: {
        surface: {} as FrameContext["controllers"]["surface"],
        agents: {} as FrameContext["controllers"]["agents"],
        projects,
        connections: {} as FrameContext["controllers"]["connections"],
      },
      actions: ACTIONS,
      unavailable: { split: null, newTab: null, newAgent: null },
    } satisfies FrameContext;
    const app = mount(ProjectSwitcher, {
      target: el,
      props: { ctx, session: "default", pickFolder: pick },
    });
    flushSync();
    hosts.push({ el, app });
    expect(el.textContent).toMatch(/sessão default/);
    el.querySelector<HTMLButtonElement>("[data-topbar-item='project']")!.click();
    flushSync();
    const items = Array.from(el.querySelectorAll<HTMLElement>("[role='menuitem']")).map((n) => n.textContent?.trim());
    expect(items.at(-1)).toBe("Abrir projeto…");
    expect(items).toContain("erp-api");
    // Spec 025: a saved (closed) project becomes an engine workspace with its cwd, focused.
    el.querySelector<HTMLButtonElement>("[data-project='p1']")!.click();
    await vi.waitFor(() => {
      expect(projectsBridge.calls.filter((c) => c.command === "workspace_create")).toEqual([
        { command: "workspace_create", args: { endpoint: "local", cwd: "/work/erp-api", label: "erp-api", focus: true } },
      ]);
    });
    el.querySelector<HTMLButtonElement>("[data-topbar-item='project']")!.click();
    flushSync();
    el.querySelector<HTMLButtonElement>("[data-open-project]")!.click();
    await vi.waitFor(() => expect(pick).toHaveBeenCalledTimes(1));
  });

  it("Novo agente lists engine kinds plus Shell; Iniciar is one start; Shell is one tab create", async () => {
    const bridge = createFakeAgentsBridge({
      bootId: "boot-018",
      generation: 1,
      session: "default",
      kinds: ["claude", "codex"],
      agents: [],
    });
    const controller = createAgentsController(bridge);
    await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
    bridge.calls.length = 0;
    const el = document.createElement("div");
    document.body.appendChild(el);
    const ctx: FrameContext = {
      surface: liveSurface(),
      agents: controller.state,
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
        surface: {} as FrameContext["controllers"]["surface"],
        agents: controller,
        projects: {} as FrameContext["controllers"]["projects"],
        connections: {} as FrameContext["controllers"]["connections"],
      },
      actions: { ...ACTIONS, newTab: () => void controller.createTab() },
      unavailable: { split: null, newTab: null, newAgent: null },
    };
    const app = mount(NewAgentPopover, { target: el, props: { ctx, onclose: () => {} } });
    flushSync();
    hosts.push({ el, app });
    // Spec 073 replaced the native <select> with the searchable list: the same kinds, now as rows.
    const kinds = Array.from(el.querySelectorAll<HTMLElement>("[data-agent-kind]")).map((row) => row.dataset.agentKind);
    expect(kinds).toEqual(["claude", "codex", "Shell"]);
    el.querySelector<HTMLButtonElement>("[data-start-agent]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "agent_start")).toHaveLength(1);
    });
  });

  it("the hover split buttons are Dividir à direita and Dividir abaixo — one call each", async () => {
    expect(PANE_MENU_ITEMS.map((i) => i.label)).toEqual(["Dividir à direita", "Dividir abaixo", "Fechar pane"]);
    const bridge = createFakeAgentsBridge({
      bootId: "boot-018",
      generation: 1,
      session: "default",
      agents: [],
    });
    const controller = createAgentsController(bridge);
    await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
    bridge.calls.length = 0;
    const ctx: FrameContext = {
      surface: liveSurface(),
      agents: controller.state,
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
        surface: {
          subscribe: (handler: (event: unknown) => void) => {
            handler({
              type: "full",
              revision: 1,
              width: 100,
              height: 40,
              cells: [{ s: " ", fg: 0, bg: 0, m: 0 }],
              cursor: null,
              panes: [],
            });
            handler({
              type: "metadata",
              revision: 1,
              panes: [
                {
                  pane_id: "w1:p1",
                  content_revision: 1,
                  rect: { x: 0, y: 0, width: 100, height: 40 },
                  inner_rect: { x: 1, y: 1, width: 98, height: 38 },
                  scroll: null,
                  focused: true,
                  mouse_reporting: false,
                  sgr_pixel_mouse: false,
                  alternate_screen_active: false,
                  pixel_width: 0,
                  pixel_height: 0,
                },
              ],
              hyperlinks: [],
            });
            return () => {};
          },
        } as FrameContext["controllers"]["surface"],
        agents: controller,
        projects: {} as FrameContext["controllers"]["projects"],
        connections: {} as FrameContext["controllers"]["connections"],
      },
      actions: ACTIONS,
      unavailable: { split: null, newTab: null, newAgent: null },
    };
    const el = document.createElement("div");
    document.body.appendChild(el);
    const stage = document.createElement("div");
    stage.className = "stage";
    const terminal = document.createElement("div");
    terminal.className = "terminal";
    const canvas = document.createElement("canvas");
    canvas.getBoundingClientRect = () =>
      ({ top: 0, left: 0, width: 900, height: 760, bottom: 760, right: 900, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    terminal.appendChild(canvas);
    stage.getBoundingClientRect = () =>
      ({ top: 0, left: 0, width: 900, height: 780, bottom: 780, right: 900, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    stage.appendChild(terminal);
    el.appendChild(stage);
    const Original = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as typeof ResizeObserver;
    const app = mount(PaneFrames, { target: el, props: { ctx, stage } });
    flushSync();
    hosts.push({ el, app });
    globalThis.ResizeObserver = Original;
    expect(el.querySelector("[data-pane-frame]"), "geometry box").toBeTruthy();
    expect(el.querySelector("[data-pane-band]")).toBeNull();
    el.querySelector<HTMLButtonElement>("[data-split-right]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "pane_split")).toEqual([
        expect.objectContaining({ command: "pane_split", args: expect.objectContaining({ direction: "right" }) }),
      ]);
    });
    el.querySelector<HTMLButtonElement>("[data-split-down]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "pane_split" && (c.args as { direction?: string }).direction === "down")).toHaveLength(1);
    });
  });

  it("the bell with waiting agents focuses the first blocked pane once; 0 hides the counter and is a no-op", async () => {
    const bridge = createFakeAgentsBridge({
      bootId: "boot-018",
      generation: 1,
      session: "default",
      agents: [
        ["w1:p1", "blocked"],
        ["w1:p2", "working"],
      ],
    });
    const controller = createAgentsController(bridge);
    await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
    bridge.calls.length = 0;
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(TitleBar, {
      target: el,
      props: {
        host: { label: "Local", kind: "Local", state: "conectado", tone: "live" as const },
        searchHint: "Ctrl K",
        onOpenPalette: () => {},
        onHost: () => {},
        windowApi: fakeWindowApi(),
        sidebarOpen: true,
        onToggleSidebar: () => {},
        waitingCount: 1,
        onBell: () => void controller.openAttention("w1:p1"),
        projectLabel: "sessão default",
      },
    });
    flushSync();
    hosts.push({ el, app });
    expect(el.querySelector("[data-waiting-count]")?.textContent?.trim()).toBe("1");
    el.querySelector<HTMLButtonElement>("[data-topbar-item='notifications']")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "agent_open_attention")).toHaveLength(1);
    });

    const empty = document.createElement("div");
    document.body.appendChild(empty);
    const idle = mount(TitleBar, {
      target: empty,
      props: {
        host: { label: "Local", kind: "Local", state: "conectado", tone: "live" as const },
        searchHint: "Ctrl K",
        onOpenPalette: () => {},
        onHost: () => {},
        windowApi: fakeWindowApi(),
        sidebarOpen: true,
        onToggleSidebar: () => {},
        waitingCount: 0,
        onBell: () => void controller.openAttention("w1:p1"),
        projectLabel: "sessão default",
      },
    });
    flushSync();
    hosts.push({ el: empty, app: idle });
    expect(empty.querySelector("[data-waiting-count]")).toBeNull();
    bridge.calls.length = 0;
    empty.querySelector<HTMLButtonElement>("[data-topbar-item='notifications']")!.click();
    flushSync();
    expect(bridge.calls.filter((c) => c.command === "agent_open_attention")).toHaveLength(0);
  });

  // Spec 028 AC-028-01 — the + of the bar creates one tab (never opens Novo agente, never splits).
  it("the tab bar + creates one tab in the focused workspace and splits nothing", async () => {
    const bridge = createFakeAgentsBridge({
      bootId: "boot-018",
      generation: 1,
      session: "default",
      agents: [],
    });
    const controller = createAgentsController(bridge);
    await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
    bridge.calls.length = 0;
    const el = document.createElement("div");
    document.body.appendChild(el);
    const ctx: FrameContext = {
      surface: liveSurface(),
      agents: controller.state,
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
        surface: {} as FrameContext["controllers"]["surface"],
        agents: controller,
        projects: {} as FrameContext["controllers"]["projects"],
        connections: {} as FrameContext["controllers"]["connections"],
      },
      actions: { ...ACTIONS, newTab: () => void controller.createTab() },
      unavailable: { split: null, newTab: null, newAgent: null },
    };
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    el.querySelector<HTMLButtonElement>("[data-new-tab]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1);
    });
    expect(bridge.calls.filter((c) => c.command === "pane_split")).toHaveLength(0);
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();
  });
});
