// @vitest-environment happy-dom
// Spec 028 AC-028-03 — context menus: the tab bar menu is covered by tabs-narrow.test.ts; here
// the pane menu of the TUI (../herdr/src/client/shell/context_menu.rs:55-75) and the generic
// ContextMenu component: 8 items in order with the conditional ones, exactly one engine call per
// item, keyboard navigation, Escape and click outside closing.
// Would catch: an item missing/mislabeled, an item firing two actions, `Cleared`/`Swap` shown
// without their conditions, the passthrough toggle not persisted in the window session, the
// Zoom label not following the engine's answer, or a menu that cannot be closed by keyboard.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { FrameContext } from "../../shell/frame-context";
import type { SurfaceState } from "../../shell/controller";
import TerminalView from "../../terminal/TerminalView.svelte";
import type { FrameEvent, PaneMeta } from "../../terminal/types";
import CenterRegion from "./CenterRegion.svelte";
import ContextMenu from "./ContextMenu.svelte";
import PaneFrames from "./PaneFrames.svelte";
import { paneContextMenuItems, RIGHT_CLICK_STORAGE_KEY } from "./pane-menu";

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

function liveSurface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "default", online: true, identity: null },
    status: {
      session: "default",
      session_available: true,
      connected: true,
      state: "live",
      reason: null,
      generation: null,
      connection_generation: 1,
      boot_id: "boot-028",
      server_version: "0.9.0",
      pane_id: "w1:p1",
      last_error: null,
    },
    phase: "live",
    reason: null,
    error: null,
    identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-028", pane_id: "w1:p1" },
    surfaceKey: 1,
    busy: false,
  };
}

const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as FrameContext["actions"];

function paneMeta(paneId: string, x: number, focused: boolean): PaneMeta {
  return {
    pane_id: paneId,
    content_revision: 1,
    rect: { x, y: 0, width: 50, height: 40 },
    inner_rect: { x: x + 1, y: 1, width: 48, height: 38 },
    scroll: null,
    focused,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: 0,
    pixel_height: 0,
  };
}

async function paneCtx() {
  const bridge = createFakeAgentsBridge({
    bootId: "boot-028",
    generation: 1,
    session: "default",
    kinds: ["pi"],
    agents: [
      ["w1:p1", "idle"],
      ["w1:p2", "idle"],
    ],
    labels: { "w1:p2": "build" },
  });
  const controller = createAgentsController(bridge);
  await controller.connect({ cols: 100, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  bridge.calls.length = 0;
  const ctx: FrameContext = {
    surface: liveSurface(),
    get agents() {
      return controller.state;
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
      surface: {
        subscribe: (handler: (event: unknown) => void) => {
          handler({ type: "full", revision: 1, width: 100, height: 40, cells: [{ s: " ", fg: 0, bg: 0, m: 0 }], cursor: null, panes: [] });
          handler({
            type: "metadata",
            revision: 1,
            panes: [paneMeta("w1:p1", 0, true), paneMeta("w1:p2", 50, false)],
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
  return { ctx, bridge, controller };
}

function mountPanes(ctx: FrameContext) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const stage = document.createElement("div");
  stage.className = "stage";
  stage.setAttribute("data-center-stage", "");
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
  return { el, stage };
}

function openPaneMenu(el: HTMLElement, stage: HTMLElement, paneId = "w1:p2") {
  stage.dispatchEvent(
    new CustomEvent("herdr-pane-menu", { detail: { pane_id: paneId, client_x: 120, client_y: 40 } }),
  );
  flushSync();
  return Array.from(el.querySelectorAll<HTMLElement>("[data-menu-item]")).map((item) => ({
    id: item.dataset.menuItem ?? "",
    label: item.textContent?.trim() ?? "",
  }));
}

function clickItem(el: HTMLElement, id: string) {
  el.querySelector<HTMLButtonElement>(`[data-menu-item='${id}']`)!.click();
  flushSync();
}

function count(bridge: { calls: { command: string }[] }, command: string) {
  return bridge.calls.filter((c) => c.command === command).length;
}

beforeEach(() => {
  sessionStorage.clear();
});

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

describe("paneContextMenuItems (TUI order and labels)", () => {
  // Would catch: a missing/misordered item, a translated label drifting from the TUI, or the
  // conditional items appearing without their preconditions.
  it("lists the eight items in order and hides the conditional ones", () => {
    const full = paneContextMenuItems({
      paneId: "w1:p2",
      hasManualLabel: true,
      swapSourcePaneId: "w1:p1",
      rightClickPassthrough: false,
      zoomed: false,
    });
    expect(full.map((i) => i.label)).toEqual([
      "Renomear pane",
      "Limpar nome do pane",
      "Trocar com o pane focado",
      "Dividir à direita",
      "Dividir abaixo",
      "Zoom",
      "Enviar cliques direitos ao pane",
      "Fechar pane",
    ]);
    expect(full.map((i) => i.id)).toEqual([
      "rename",
      "clear-name",
      "swap",
      "split-right",
      "split-down",
      "zoom",
      "toggle-right-click",
      "close",
    ]);

    const minimal = paneContextMenuItems({
      paneId: "w1:p1",
      hasManualLabel: false,
      swapSourcePaneId: null,
      rightClickPassthrough: true,
      zoomed: true,
    });
    expect(minimal.map((i) => i.label)).toEqual([
      "Renomear pane",
      "Dividir à direita",
      "Dividir abaixo",
      "Desfazer zoom",
      "Usar menu do Herdr",
      "Fechar pane",
    ]);
  });
});

describe("pane context menu (AC-028-03)", () => {
  // Would catch: each item not calling exactly one engine action.
  it("shows the eight items and each runs exactly one action", async () => {
    const { ctx, bridge } = await paneCtx();
    const { el, stage } = mountPanes(ctx);

    expect(openPaneMenu(el, stage).map((i) => i.label)).toEqual([
      "Renomear pane",
      "Limpar nome do pane",
      "Trocar com o pane focado",
      "Dividir à direita",
      "Dividir abaixo",
      "Zoom",
      "Enviar cliques direitos ao pane",
      "Fechar pane",
    ]);

    clickItem(el, "clear-name");
    await vi.waitFor(() => expect(count(bridge, "pane_rename")).toBe(1));
    expect((bridge.calls.at(-1)!.args as { paneId: string; label: string | null })).toMatchObject({ paneId: "w1:p2", label: null });

    openPaneMenu(el, stage);
    clickItem(el, "swap");
    await vi.waitFor(() => expect(count(bridge, "pane_swap")).toBe(1));

    openPaneMenu(el, stage);
    clickItem(el, "split-right");
    await vi.waitFor(() => expect(count(bridge, "pane_split")).toBe(1));
    expect((bridge.calls.at(-1)!.args as { direction: string }).direction).toBe("right");

    openPaneMenu(el, stage);
    clickItem(el, "split-down");
    await vi.waitFor(() => expect(count(bridge, "pane_split")).toBe(2));
    expect((bridge.calls.at(-1)!.args as { direction: string }).direction).toBe("down");

    openPaneMenu(el, stage);
    clickItem(el, "zoom");
    await vi.waitFor(() => expect(count(bridge, "pane_zoom")).toBe(1));
    expect((bridge.calls.at(-1)!.args as { mode: string }).mode).toBe("toggle");

    openPaneMenu(el, stage);
    clickItem(el, "close");
    await vi.waitFor(() => expect(count(bridge, "pane_close")).toBe(1));
    // One call per item: no other action was sent alongside.
    expect(bridge.calls.filter((c) => c.command === "pane_rename")).toHaveLength(1);
    expect(bridge.calls.filter((c) => c.command === "pane_swap")).toHaveLength(1);
    expect(bridge.calls.filter((c) => c.command === "pane_close")).toHaveLength(1);
  });

  // Would catch: a rename that bypasses pane.rename, or the inline field sending without Enter.
  it("Renomear opens the inline field and sends one pane.rename with the trimmed name", async () => {
    const { ctx, bridge } = await paneCtx();
    const { el, stage } = mountPanes(ctx);
    openPaneMenu(el, stage);
    clickItem(el, "rename");
    const input = el.querySelector<HTMLInputElement>("[data-pane-rename='w1:p2']");
    expect(input, "inline rename field").toBeTruthy();
    expect(input!.value).toBe("build");
    input!.value = "  build nova  ";
    input!.dispatchEvent(new InputEvent("input", { bubbles: true }));
    input!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await vi.waitFor(() => expect(count(bridge, "pane_rename")).toBe(1));
    expect(bridge.calls.at(-1)!.args).toMatchObject({ paneId: "w1:p2", label: "build nova" });
    expect(el.querySelector("[data-pane-rename]")).toBeNull();
  });

  // Would catch: the toggle not persisting in the window session, its label not flipping, or the
  // engine call not carrying the new target.
  it("alterna a passagem de cliques direitos por pane e persiste na sessão da janela", async () => {
    const { ctx, bridge } = await paneCtx();
    const { el, stage } = mountPanes(ctx);
    expect(openPaneMenu(el, stage).at(-2)!.label).toBe("Enviar cliques direitos ao pane");
    clickItem(el, "toggle-right-click");
    await vi.waitFor(() => expect(count(bridge, "pane_input_set")).toBe(1));
    expect(bridge.calls.at(-1)!.args).toMatchObject({ paneId: "w1:p2", passthrough: true });
    expect(JSON.parse(sessionStorage.getItem(RIGHT_CLICK_STORAGE_KEY)!)).toEqual({ "w1:p2": true });
    expect(openPaneMenu(el, stage).at(-2)!.label).toBe("Usar menu do Herdr");

    clickItem(el, "toggle-right-click");
    await vi.waitFor(() => expect(count(bridge, "pane_input_set")).toBe(2));
    expect(bridge.calls.at(-1)!.args).toMatchObject({ paneId: "w1:p2", passthrough: false });
    expect(openPaneMenu(el, stage).at(-2)!.label).toBe("Enviar cliques direitos ao pane");
  });

  // Would catch: the Zoom label not following the engine's own `zoomed` answer.
  it("mostra Desfazer zoom depois de um zoom confirmado pela engine", async () => {
    const { ctx, bridge } = await paneCtx();
    const { el, stage } = mountPanes(ctx);
    openPaneMenu(el, stage);
    clickItem(el, "zoom");
    await vi.waitFor(() => expect(count(bridge, "pane_zoom")).toBe(1));
    expect(openPaneMenu(el, stage).find((item) => item.id === "zoom")!.label).toBe("Desfazer zoom");
  });
});

// ---------------------------------------------------------------------------------------------
// Spec 028 AC-028-03 (r2c) — the browser `contextmenu` reaches TerminalView's surface container,
// the same place the spec 022 wheel moved to, and the center region renders the menu at the click.
// Would catch: the listener back on the IME textarea (the user report: a right click over the pane
// opens nothing), the container route losing its `preventDefault`, the menu only opening when the
// test dispatches `herdr-pane-menu` itself, or the menu ignoring the click coordinates.
// ---------------------------------------------------------------------------------------------

class FakeMenuCtx {
  fillStyle: string | CanvasGradient | CanvasPattern = "";
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  globalAlpha = 1;
  fillRect() {}
  fillText() {}
  setTransform() {}
  measureText() {
    // fontSize 20 → cellWidth 10, cellHeight ceil(20 × 1.3) = 26.
    return { width: 10 };
  }
}

const rect = (left: number, top: number, width: number, height: number) =>
  ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;

const MENU_LABELS = [
  "Renomear pane",
  "Limpar nome do pane",
  "Trocar com o pane focado",
  "Dividir à direita",
  "Dividir abaixo",
  "Zoom",
  "Enviar cliques direitos ao pane",
  "Fechar pane",
];

describe("right click on the surface container (AC-028-03 r2c)", () => {
  it("opens the pane menu with the eight items at the click and runs the chosen action", async () => {
    const { ctx, bridge } = await paneCtx();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const stage = document.createElement("div");
    stage.setAttribute("data-center-stage", "");
    stage.getBoundingClientRect = () => rect(0, 0, 1000, 1040);
    el.appendChild(stage);

    const originalGetContext = HTMLCanvasElement.prototype.getContext;
    const OriginalResizeObserver = globalThis.ResizeObserver;
    HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, type: string, attrs?: unknown) {
      void type;
      void attrs;
      return new FakeMenuCtx() as unknown as CanvasRenderingContext2D;
    } as typeof HTMLCanvasElement.prototype.getContext;
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as typeof ResizeObserver;
    try {
      let push: (event: FrameEvent) => void = () => {};
      const view = mount(TerminalView, {
        target: stage,
        props: {
          subscribe: (handler: (event: FrameEvent) => void) => {
            push = handler;
            return () => {
              push = () => {};
            };
          },
          onInput: () => {},
          onResize: () => {},
          fontSize: 20,
        },
      });
      hosts.push({ el, app: view });
      flushSync();
      const container = stage.querySelector<HTMLElement>(".terminal")!;
      expect(container, "the surface container of TerminalView").toBeTruthy();
      stage.querySelector("canvas")!.getBoundingClientRect = () => rect(0, 0, 1000, 1040);
      push({ type: "identity", boot_id: "boot-028", generation: 1, connection_generation: 1, server_version: "0.9.0", pane_id: "w1:p1" });
      push({
        type: "full",
        revision: 1,
        width: 100,
        height: 40,
        cells: Array.from({ length: 100 * 40 }, () => ({ s: " ", fg: 0, bg: 0, m: 0 })),
        cursor: null,
        panes: [],
      });
      push({ type: "metadata", revision: 1, panes: [paneMeta("w1:p1", 0, true), paneMeta("w1:p2", 50, false)], hyperlinks: [] });
      flushSync();

      // The real center region: PaneFrames over the stage, ContextMenu rendered by it.
      const frames = mount(PaneFrames, { target: el, props: { ctx, stage } });
      hosts.push({ el, app: frames });
      flushSync();

      // Cell (51, 1) is inside pane w1:p2 (inner_rect starts at column 51); cell 10×26 px.
      const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 515, clientY: 31 });
      expect(container.dispatchEvent(event), "the browser menu is suppressed").toBe(false);
      flushSync();

      const menu = el.querySelector<HTMLElement>("[data-context-menu]");
      expect(menu, "the right click on the container opened the pane menu").toBeTruthy();
      expect(menu!.style.left).toBe("515px");
      expect(menu!.style.top).toBe("31px");
      expect(Array.from(el.querySelectorAll<HTMLElement>("[data-menu-item]")).map((item) => item.textContent?.trim())).toEqual(MENU_LABELS);

      el.querySelector<HTMLButtonElement>("[data-menu-item='close']")!.click();
      await vi.waitFor(() => expect(count(bridge, "pane_close")).toBe(1));
      expect(bridge.calls.at(-1)!.args).toMatchObject({ target: expect.objectContaining({ pane_id: "w1:p2" }) });
    } finally {
      HTMLCanvasElement.prototype.getContext = originalGetContext;
      globalThis.ResizeObserver = OriginalResizeObserver;
    }
  });

  // Would catch: the menu existing only in the test composition; the window mounts it through
  // CenterRegion (stage over the surface) and PaneFrames, never a parallel path.
  it("the real window composition mounts the menu (App → CenterRegion → PaneFrames → ContextMenu)", () => {
    const sources = import.meta.glob(["../../App.svelte", "./CenterRegion.svelte", "./PaneFrames.svelte", "./ContextMenu.svelte"], {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    expect(sources["../../App.svelte"]).toContain("<CenterRegion");
    expect(sources["./CenterRegion.svelte"]).toContain("<PaneFrames");
    expect(sources["./PaneFrames.svelte"]).toContain("<ContextMenu");
    expect(sources["./PaneFrames.svelte"]).toContain("herdr-pane-menu");
  });

  // Spec 028 r3 — the debug trace of the menu state and of the rendered menu (the window test
  // reads the host's stderr to tell "never opened" from "open but invisible"). Would catch: the
  // open/ignored/closed state not traced, or the menu rect/z-index trace removed.
  it("traces the pane menu state and the rendered menu rect with its stacking", () => {
    const sources = import.meta.glob(["./PaneFrames.svelte", "./ContextMenu.svelte"], {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    expect(sources["./PaneFrames.svelte"]).toContain('menuStateLine({ state: "open"');
    expect(sources["./PaneFrames.svelte"]).toContain('menuStateLine({ state: "ignored"');
    expect(sources["./PaneFrames.svelte"]).toContain('menuStateLine({ state: "closed" })');
    expect(sources["./ContextMenu.svelte"]).toContain("menuRectLine({ label, rect, zIndex, items: items.length })");
    expect(sources["./ContextMenu.svelte"]).toContain("getComputedStyle(menuEl).zIndex");
  });
});

describe("ContextMenu component", () => {
  // Would catch: keyboard navigation not moving the highlight, Enter not choosing, or Escape and
  // a click outside not closing.
  it("navigates with the keyboard and closes on Escape or a click outside", () => {
    const chosen: string[] = [];
    const closed = vi.fn();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(ContextMenu, {
      target: el,
      props: {
        items: [
          { id: "um", label: "Um" },
          { id: "dois", label: "Dois" },
          { id: "tres", label: "Três", disabled: true, reason: "indisponível" },
        ],
        x: 10,
        y: 20,
        onselect: (id) => chosen.push(id),
        onclose: closed,
      },
    });
    flushSync();
    hosts.push({ el, app });
    const menu = el.querySelector<HTMLElement>("[data-context-menu]")!;
    expect(menu.style.left).toBe("10px");
    expect(menu.style.top).toBe("20px");
    const highlighted = () => menu.querySelector("[data-highlighted='true']")?.getAttribute("data-menu-item");
    expect(highlighted()).toBe("um");

    menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    flushSync();
    expect(highlighted()).toBe("dois");
    menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(chosen).toEqual(["dois"]);

    const disabled = el.querySelector<HTMLElement>("[data-menu-item='tres']")!;
    expect(disabled.getAttribute("aria-disabled")).toBe("true");
    disabled.click();
    expect(chosen).toEqual(["dois"]);

    menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(closed).toHaveBeenCalledTimes(1);

    window.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(closed).toHaveBeenCalledTimes(2);
  });
});

describe("pane menu through the real center composition (AC-028-03 r3)", () => {
  // Would catch: the menu listener attached once in onMount with the stage still `undefined`
  // (the parent binds `bind:this` after PaneFrames mounts; the user's right click was routed to
  // the menu and nothing appeared), or the menu not rendered through CenterRegion → PaneFrames.
  it("opens the pane menu when the stage arrives through the parent binding", async () => {
    const { ctx } = await paneCtx();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const Original = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as typeof ResizeObserver;
    const app = mount(CenterRegion, { target: el, props: { ctx } });
    hosts.push({ el, app });
    flushSync();
    globalThis.ResizeObserver = Original;
    const stage = el.querySelector<HTMLElement>("[data-center-stage]");
    expect(stage, "the stage bound by CenterRegion").toBeTruthy();
    stage!.dispatchEvent(new CustomEvent("herdr-pane-menu", { detail: { pane_id: "w1:p2", client_x: 10, client_y: 10 } }));
    flushSync();
    const menu = el.querySelector<HTMLElement>("[data-context-menu]");
    expect(menu, "the menu reached through herdr-pane-menu").toBeTruthy();
    expect(Array.from(el.querySelectorAll<HTMLElement>("[data-menu-item]")).map((item) => item.textContent?.trim())).toEqual(MENU_LABELS);
  });
});
