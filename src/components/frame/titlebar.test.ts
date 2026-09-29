// @vitest-environment happy-dom
// Spec 017 — AC-017-01: the design title bar is the only chrome (no system decorations),
// the three window controls call an injected window API, and the bar is a drag region.
//
// Spec 047 (AC-047-04) reframes the bar: it sits only over the main area, shows the
// `coleção › workspace` breadcrumb with the branch chip and the path, and no longer carries the
// mark, the search or the host pill — those live in the sidebar of the new design (041/048).
//
// Spec 049 (AC-049-03) drops the bar's panel button too: collapsing and expanding the sidebar
// had two controls doing the same thing, so only the one in the sidebar head stays (plus
// Ctrl+B). What 047 asserted here is now asserted as an absence.
//
// Spec 055 (AC-055-01/02) moves the workspace tabs into this bar and drops the breadcrumb: the
// bar is, left to right, the tab strip (the same `WorkspaceTabs`, with `+`, the × of every tab
// and the context menu of 028/040/041), a free drag area, the bell and the three window
// controls. The branch and the path stay in the status bar, so nothing unique is lost. What
// 047 asserted about the breadcrumb is now asserted as an absence.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { mount, unmount, flushSync } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto } from "../../connections/types";
import type { NavigatorState } from "../../projects/reducer";
import type { FrameContext } from "../../shell/frame-context";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { TabDto } from "../../agents/types";
import CenterRegion from "../center/CenterRegion.svelte";
import type { MenuAction } from "./menus";
import { runWindowControl, type WindowApi } from "./window-api";
import TitleBar from "./TitleBar.svelte";

const conf = JSON.parse(readFileSync(resolve("src-tauri/tauri.conf.json"), "utf8")) as {
  app: { windows: { decorations?: boolean }[] };
};
const titleBarSource = readFileSync(resolve("src/components/frame/TitleBar.svelte"), "utf8");
const centerSource = readFileSync(resolve("src/components/center/CenterRegion.svelte"), "utf8");

/** Any node still carrying a breadcrumb hook (`data-crumb-*`), which spec 055 removes. */
const crumbNodes = (root: HTMLElement) =>
  Array.from(root.querySelectorAll<HTMLElement>("*")).filter((el) =>
    Array.from(el.attributes).some((attr) => attr.name.startsWith("data-crumb")),
  );

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

function fakeApi(): WindowApi & { calls: string[] } {
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

function renderBar(windowApi: WindowApi) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TitleBar, {
    target: el,
    props: {
      host: { label: "Este computador", kind: "Local", state: "conectado", tone: "live" as const },
      searchHint: "Ctrl K",
      onOpenPalette: () => {},
      onHost: () => {},
      windowApi,
    },
  });
  flushSync();
  hosts.push({ el, app });
  return el;
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

describe("AC-017-01 title bar chrome", () => {
  // Would catch: the system title bar left on (two bars: "Herdr Desktop" plus the design bar).
  it("declares decorations: false on the product window", () => {
    expect(conf.app.windows[0]?.decorations).toBe(false);
  });

  // Would catch: a control that only looks like a button (disabled, or a no-op) while the
  // system decorations still own minimize/maximize/close.
  it("the three controls call the injected window API once each", () => {
    const api = fakeApi();
    const root = renderBar(api);
    const click = (item: string) => {
      const button = root.querySelector<HTMLButtonElement>(`[data-topbar-item="${item}"]`);
      expect(button, item).toBeTruthy();
      expect(button!.disabled).toBe(false);
      button!.click();
      flushSync();
    };
    click("window-minimize");
    click("window-maximize");
    click("window-close");
    expect(api.calls).toEqual(["minimize", "maximize", "close"]);
  });

  // Would catch: a bar the compositor cannot drag, or a double-click that does not maximize.
  it("exposes a drag region and maximises on double-click of the bar", () => {
    const api = fakeApi();
    const root = renderBar(api);
    const bar = root.querySelector<HTMLElement>("[data-tauri-drag-region]");
    expect(bar).toBeTruthy();
    expect(bar!.getAttribute("data-region")).toBe("topbar");
    bar!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    flushSync();
    expect(api.calls).toEqual(["maximize"]);
  });

  it("runWindowControl maps the three actions without inventing a fourth", () => {
    const api = fakeApi();
    runWindowControl(api, "minimize");
    runWindowControl(api, "maximize");
    runWindowControl(api, "close");
    expect(api.calls).toEqual(["minimize", "maximize", "close"]);
  });
});

/** The window of `design/v2-01-workspace.png`: `erp-api` focused inside `Acme · Clientes`. */
function frameContext(): FrameContext {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      target: null,
      phase: "online",
      phase_label: "Online",
      session: "hd047",
      workspaces: [
        {
          workspace_id: "w-erp",
          label: "erp-api",
          number: 1,
          focused: true,
          tab_count: 1,
          pane_count: 1,
          active_tab_id: "w-erp:t1",
          agent_status: "unknown",
          cwd: "~/work/acme/erp-api",
          branch: "main",
        },
      ],
    }),
  ];
  const navigator = {
    snapshot: {
      version: 3,
      projects: [
        { id: "p-erp", label: "erp-api", endpoint_profile_id: "local", session_name: "hd047", root: "~/work/acme/erp-api", binding: null },
      ],
      collections: [{ id: "g1", name: "Acme · Clientes", project_ids: ["p-erp"] }],
      workspace_prefs: [],
    },
    opening: {},
  } as unknown as NavigatorState;
  return {
    surface: { selection: { endpoint: "local", session: "hd047" } },
    agents: null,
    connections: { view: { hub: { revision: 1, hosts }, profiles: [], store_error: null } } as unknown as ConnectionsState,
    navigator,
    selectedEndpoint: "local",
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: false,
    agentsOpen: false,
    controllers: {} as FrameContext["controllers"],
    actions: {} as FrameContext["actions"],
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;
}

function renderFrame(props: Record<string, unknown> = {}) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TitleBar, {
    target: el,
    props: {
      host: { label: "Este computador", kind: "Local", state: "conectado", tone: "live" as const },
      searchHint: "Ctrl K",
      onOpenPalette: () => {},
      onHost: () => {},
      windowApi: fakeApi(),
      sidebarOpen: false,
      projectLabel: "sessão hd047",
      ctx: frameContext(),
      layoutWidth: 1440,
      ...props,
    },
  });
  flushSync();
  hosts.push({ el, app });
  return el;
}

const items = (root: HTMLElement) =>
  Array.from(root.querySelectorAll<HTMLElement>("[data-topbar-item]"))
    .filter((el) => !el.parentElement?.closest("[data-topbar-item]"))
    .map((el) => el.dataset.topbarItem);

describe("AC-049-03 the bar carries no collapse control at all", () => {
  // Would catch the window the user tested: two controls doing the same thing — the bar's panel
  // button and `Recolher lateral` in the sidebar head. Only the sidebar's own stays.
  it("has no sidebar button, with the sidebar open or collapsed", () => {
    for (const sidebarOpen of [false, true]) {
      const root = renderFrame({ sidebarOpen, onToggleSidebar: vi.fn() });
      expect(root.querySelector("[data-topbar-item='sidebar']"), `sidebarOpen=${sidebarOpen}`).toBeNull();
      expect(root.querySelector("[data-panel-icon]")).toBeNull();
      expect(root.textContent ?? "").not.toContain("Recolher lateral");
      expect(root.textContent ?? "").not.toContain("Expandir lateral");
      expect(items(root)).not.toContain("sidebar");
      const labels = [...root.querySelectorAll<HTMLElement>("[aria-label],[title]")].flatMap((node) => [
        node.getAttribute("aria-label") ?? "",
        node.getAttribute("title") ?? "",
      ]);
      expect(labels.filter((label) => /lateral/i.test(label))).toEqual([]);
    }
  });
});

describe("AC-047-04/AC-055-01 the bar is the frame of the design", () => {
  // Would catch: the mark, the 460 px search or the host pill left in the bar after the design
  // moved them into the sidebar (041 Buscar/Ctrl K, 048 host foot).
  // Spec 055: the breadcrumb left too — the bar now opens with the tab strip.
  it("keeps only the tabs, the bell and the window controls", () => {
    const root = renderFrame();
    expect(items(root)).toEqual(["notifications", "window-minimize", "window-maximize", "window-close"]);
    expect(root.querySelector("[data-topbar-item='logo']")).toBeNull();
    expect(root.querySelector("[data-topbar-item='search']")).toBeNull();
    expect(root.querySelector("[data-topbar-item='host']")).toBeNull();
    expect(root.querySelector(".brand-name")).toBeNull();
    expect(root.textContent ?? "").not.toContain("herdr");
    expect(root.textContent ?? "").not.toMatch(/Ctrl K/);
    // Spec 017: moving the window still works after the reframe.
    expect(root.querySelector("[data-region='topbar']")?.hasAttribute("data-tauri-drag-region")).toBe(true);
  });

  // Spec 055 AC-055-01. Would catch: the breadcrumb (collection square, `coleção › workspace`,
  // branch chip, path) still drawn in the bar the user asked to give to the tabs.
  it("draws no breadcrumb: no project item and no crumb node at all", () => {
    for (const root of [renderFrame(), renderFrame({ ctx: null, projectLabel: "sessão hd047" })]) {
      expect(root.querySelector("[data-topbar-item='project']")).toBeNull();
      expect(crumbNodes(root)).toEqual([]);
      expect(root.textContent ?? "").not.toContain("Acme · Clientes");
      expect(root.textContent ?? "").not.toContain("~/work/acme/erp-api");
      expect(root.textContent ?? "").not.toContain("sessão hd047");
    }
    expect(titleBarSource).not.toMatch(/data-crumb/);
    expect(titleBarSource).not.toMatch(/data-topbar-item="project"/);
    expect(titleBarSource).not.toMatch(/buildSidebarSections/);
  });
});

/** Live context with real tabs, so the bar mounts the same `WorkspaceTabs` the center had. */
async function tabsContext(labels: string[]) {
  const tabs: TabDto[] = labels.map((label, index) => ({
    tab_id: `w1:t${index + 1}`,
    workspace_id: "w1",
    label,
    number: index + 1,
    pane_count: 1,
    focused: index === 0,
  }));
  const bridge = createFakeAgentsBridge({
    bootId: "boot-055",
    generation: 1,
    session: "hd055",
    kinds: ["claude"],
    agents: [["w1:p1", "idle"]],
    tabs,
  });
  const controller = createAgentsController(bridge);
  await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  bridge.calls.length = 0;
  const ctx = {
    surface: {
      selection: { endpoint: "local", kind: "local", label: "Local", session: "hd055", online: true, identity: null },
      status: null,
      phase: "live",
      reason: null,
      error: null,
      identity: { endpoint: "local", session: "hd055", connection_generation: 1, boot_id: "boot-055", pane_id: "w1:p1" },
      surfaceKey: 1,
      busy: false,
    },
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
      // PaneFrames subscribes on mount: the center region is mounted here to prove it draws no
      // strip of its own, so the surface controller only has to be subscribable.
      surface: { subscribe: () => () => {} } as unknown as FrameContext["controllers"]["surface"],
      agents: controller,
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: Object.fromEntries(
      ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
        (name) => [name, () => {}],
      ),
    ) as Record<MenuAction, () => void>,
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;
  return { ctx, bridge };
}

/** The bar as the window mounts it in `App.svelte`: the context, and nothing of the 047 bar. */
function mountBarWith(ctx: FrameContext) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TitleBar, {
    target: el,
    props: {
      host: null,
      searchHint: "Ctrl K",
      onOpenPalette: () => {},
      onHost: () => {},
      windowApi: fakeApi(),
      ctx,
      layoutWidth: 1440,
    },
  });
  flushSync();
  hosts.push({ el, app });
  return el;
}

function mountCenter(ctx: FrameContext) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(CenterRegion, { target: el, props: { ctx } });
  flushSync();
  hosts.push({ el, app });
  return el;
}

describe("AC-055-01 the workspace tabs live in the top bar", () => {
  // Would catch the composition the user asked for being half done: tabs added to the bar while
  // the center keeps its own row (two strips), or the bar mounting something other than the
  // 028/040/041 `WorkspaceTabs`.
  it("the bar carries the only tab strip, before the bell and the window controls", async () => {
    const { ctx } = await tabsContext(["principal", "deploy"]);
    const barRoot = mountBarWith(ctx);
    const centerRoot = mountCenter(ctx);

    const bar = barRoot.querySelector<HTMLElement>("[data-region='topbar']")!;
    const strip = barRoot.querySelector<HTMLElement>("[data-center-tabs]")!;
    expect(strip, "the tab strip is in the bar").toBeTruthy();
    expect(bar.contains(strip)).toBe(true);
    expect(Array.from(strip.querySelectorAll("[data-tab]")).length).toBe(2);

    // Order: the strip, then the bell, then the three controls.
    const bell = barRoot.querySelector<HTMLElement>("[data-topbar-item='notifications']")!;
    expect(strip.compareDocumentPosition(bell) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    for (const control of ["window-minimize", "window-maximize", "window-close"]) {
      const node = barRoot.querySelector<HTMLElement>(`[data-topbar-item='${control}']`)!;
      expect(strip.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING, control).toBeTruthy();
    }

    // The center no longer draws a strip: exactly one in the whole document.
    expect(centerRoot.querySelector("[data-center-tabs]")).toBeNull();
    expect(centerRoot.querySelector("[data-center-stage]"), "the stage stays").toBeTruthy();
    expect(document.querySelectorAll("[data-center-tabs]")).toHaveLength(1);
    expect(titleBarSource).toMatch(/<WorkspaceTabs\b/);
    expect(centerSource).not.toMatch(/WorkspaceTabs/);
  });

  // Would catch: the strip mounted in the bar but wired to nothing — a click, the `+`, the × or
  // the right click no longer reaching the engine through the same controller.
  it("clicking a tab, +, × and the right click still act on the engine from the bar", async () => {
    const { ctx, bridge } = await tabsContext(["principal", "deploy"]);
    const root = mountBarWith(ctx);
    const strip = root.querySelector<HTMLElement>("[data-center-tabs]")!;

    strip.querySelector<HTMLElement>("[data-tab='w1:t2']")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_focus")).toHaveLength(1);
    });

    strip.querySelector<HTMLButtonElement>("[data-new-tab]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1);
    });

    strip.querySelector<HTMLButtonElement>("[data-close-tab='w1:t1']")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_close")).toHaveLength(1);
    });
    expect(bridge.calls.filter((c) => c.command === "pane_split")).toHaveLength(0);

    strip
      .querySelector<HTMLElement>("[data-tab='w1:t2']")!
      .dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 10 }));
    flushSync();
    expect(Array.from(root.querySelectorAll<HTMLElement>("[data-menu-item]")).map((i) => i.textContent?.trim())).toEqual([
      "Nova aba",
      "Renomear",
      "Fechar",
    ]);
    // The menu is absolutely positioned inside the strip and drops below the 44 px bar: a bar
    // that clipped its overflow would swallow it (the 028 menu worked because the center row
    // clipped nothing).
    const barStyle = titleBarSource.slice(titleBarSource.indexOf(".topbar {"), titleBarSource.indexOf("}", titleBarSource.indexOf(".topbar {")));
    expect(barStyle).not.toMatch(/overflow[^:]*:\s*(hidden|clip|auto|scroll)/);
  });
});

describe("AC-055-02 the free space of the bar drags the window", () => {
  // Would catch: the bar with no room left to grab (the tabs or the controls stretched over the
  // whole width), or a drag region laid over the tabs, the `+`, the × or the window controls —
  // pressing them would start a window drag instead of acting.
  it("a drag element fills the gap between the strip and the bell, and no control carries one", async () => {
    const { ctx } = await tabsContext(["principal", "deploy"]);
    const root = mountBarWith(ctx);
    const bar = root.querySelector<HTMLElement>("[data-region='topbar']")!;
    const strip = root.querySelector<HTMLElement>("[data-center-tabs]")!;
    const spacer = bar.querySelector<HTMLElement>("[data-topbar-drag]")!;
    expect(spacer, "the free drag area exists").toBeTruthy();
    expect(spacer.hasAttribute("data-tauri-drag-region")).toBe(true);
    // It sits between the strip and the bell, and it is what takes the leftover width.
    expect(strip.nextElementSibling).toBe(spacer);
    expect(spacer.nextElementSibling?.querySelector("[data-topbar-item='notifications']") ?? spacer.nextElementSibling)
      .toBeTruthy();
    expect(strip.compareDocumentPosition(spacer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const style = titleBarSource.slice(titleBarSource.indexOf("<style>"), titleBarSource.indexOf("</style>"));
    expect(style).toMatch(/\.drag \{[^}]*flex:\s*1/);

    // Nothing the user acts on is a drag region.
    const acting = [
      strip,
      ...Array.from(strip.querySelectorAll<HTMLElement>("[data-tab], [data-new-tab], [data-close-tab]")),
      ...Array.from(bar.querySelectorAll<HTMLElement>("[data-topbar-item]")),
    ];
    for (const node of acting) {
      expect(node.hasAttribute("data-tauri-drag-region"), node.outerHTML.slice(0, 60)).toBe(false);
    }
    expect(strip.querySelector("[data-tauri-drag-region]")).toBeNull();
    // The window controls keep the 017 guard that a press on them never reaches the drag region.
    expect(titleBarSource.match(/onmousedown=\{stopDrag\}/g) ?? []).toHaveLength(4);
  });
});
