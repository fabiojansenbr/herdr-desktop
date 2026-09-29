// @vitest-environment happy-dom
// Spec 024 AC-024-01 — title bar at fixed logical widths. Would catch: items overlapping
// (design/bug-topbar-narrow.png at 783 px), items leaving the window, or auto-collapse writing
// herdr.sidebar.open.
//
// Spec 047 AC-047-04 reframes the bar: the mark, the 460 px search and the host pill moved into
// the sidebar (Buscar/Ctrl K of 041, host foot of 048).
//
// Spec 049 AC-049-03 takes the panel button out of the bar as well (the sidebar head keeps the
// only one).
//
// Spec 055 AC-055-01/02 replaces the breadcrumb with the workspace tabs: the bar is the tab
// strip, the free drag area that takes the leftover width, the bell and the three controls. So
// what narrows here is the strip (it scrolls inside itself), the drag area keeps a grabbable
// minimum, and the fixed controls never shrink.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { TabDto } from "../../agents/types";
import type { FrameContext } from "../../shell/frame-context";
import Sidebar from "./Sidebar.svelte";
import TitleBar, { type TitleBarHost } from "./TitleBar.svelte";
import type { MenuAction } from "./menus";
import { SIDEBAR_STORAGE_KEY } from "./sidebar";
import type { WindowApi } from "./window-api";

/** Left to right in the bar: the tab strip, the free drag area, the bell, the three controls. */
const ORDER = ["tabs", "drag", "notifications", "window-minimize", "window-maximize", "window-close"] as const;

/** Preferred width of a six-tab strip, its floor, and the smallest grabbable drag area. */
const TABS_PREFERRED = 900;
const TABS_MIN = 120;
const DRAG_MIN = 24;

interface PackedItem {
  id: string;
  left: number;
  width: number;
  right: number;
}

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

function fakeWindowApi(): WindowApi {
  return { minimize: () => {}, toggleMaximize: () => {}, close: () => {} };
}

function host(): TitleBarHost {
  return { label: "dev-box", kind: "SSH", state: "conectado", tone: "live" };
}

function idOf(el: HTMLElement): string {
  if (el.dataset.topbarItem) return el.dataset.topbarItem;
  if (el.hasAttribute("data-topbar-drag")) return "drag";
  return "tabs";
}

/** Top-level children of the bar that take part in its packing. */
function barItems(root: HTMLElement): HTMLElement[] {
  const bar = root.querySelector<HTMLElement>("[data-region='topbar']")!;
  return Array.from(bar.querySelectorAll<HTMLElement>("[data-center-tabs], [data-topbar-drag], [data-topbar-item]")).filter(
    (el) => !el.parentElement?.closest("[data-topbar-item], [data-center-tabs]"),
  );
}

function rect(left: number, width: number, height = 30): DOMRect {
  return {
    x: left,
    y: 7,
    left,
    top: 7,
    width,
    height,
    right: left + width,
    bottom: 37,
    toJSON: () => ({}),
  } as DOMRect;
}

/** Sequential flex pack matching TitleBar gaps (10 between the three groups, 4 inside `.right`)
 * and padding (10 + 12): the strip is the only item that shrinks, the drag area takes the rest. */
function pack(width: number): PackedItem[] {
  const specs: { id: (typeof ORDER)[number]; width: number; min: number; shrink: number; gapAfter: number }[] = [
    { id: "tabs", width: TABS_PREFERRED, min: TABS_MIN, shrink: 1, gapAfter: 10 },
    { id: "drag", width: DRAG_MIN, min: DRAG_MIN, shrink: 0, gapAfter: 10 },
    { id: "notifications", width: 30, min: 30, shrink: 0, gapAfter: 4 },
    { id: "window-minimize", width: 30, min: 30, shrink: 0, gapAfter: 4 },
    { id: "window-maximize", width: 30, min: 30, shrink: 0, gapAfter: 4 },
    { id: "window-close", width: 30, min: 30, shrink: 0, gapAfter: 0 },
  ];
  const pad = 22;
  const gaps = specs.reduce((sum, item) => sum + item.gapAfter, 0);
  let used = pad + gaps + specs.reduce((sum, item) => sum + item.width, 0);
  if (used > width) {
    let rest = used - width;
    for (const item of specs) {
      if (item.shrink <= 0 || rest <= 0) continue;
      const cut = Math.min(rest, item.width - item.min);
      item.width -= cut;
      rest -= cut;
    }
    used = width + rest;
  } else if (used < width) {
    // The free area is what grows: the strip never stretches past its content.
    specs.find((item) => item.id === "drag")!.width += width - used;
    used = width;
  }
  let x = 10;
  return specs.map((item) => {
    const packed = { id: item.id, left: x, width: item.width, right: x + item.width };
    x += item.width + item.gapAfter;
    return packed;
  });
}

function paint(bar: HTMLElement, root: HTMLElement, width: number): PackedItem[] {
  const packed = pack(width);
  bar.getBoundingClientRect = () => rect(0, width, 44);
  Object.defineProperty(bar, "scrollWidth", { configurable: true, value: packed.at(-1)!.right + 12 });
  Object.defineProperty(bar, "clientWidth", { configurable: true, value: width });
  for (const el of barItems(root)) {
    const slot = packed.find((item) => item.id === idOf(el));
    if (!slot) continue;
    el.getBoundingClientRect = () => rect(slot.left, slot.width);
  }
  return packed;
}

/** Live context with six tabs, so the bar draws the strip it now owns. */
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
    session: "default",
    kinds: ["claude"],
    agents: [["w1:p1", "idle"]],
    tabs,
  });
  const controller = createAgentsController(bridge);
  await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  return {
    surface: {
      selection: { endpoint: "local", kind: "local", label: "Local", session: "default", online: true, identity: null },
      status: null,
      phase: "live",
      reason: null,
      error: null,
      identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-055", pane_id: "w1:p1" },
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
      surface: {} as FrameContext["controllers"]["surface"],
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
}

async function mountBar(width: number) {
  const ctx = await tabsContext(["workers", "api", "principal", "deploy", "docs", "site"]);
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TitleBar, {
    target: el,
    props: {
      host: host(),
      searchHint: "Ctrl K",
      onOpenPalette: () => {},
      onHost: () => {},
      windowApi: fakeWindowApi(),
      sidebarOpen: true,
      onToggleSidebar: () => {},
      waitingCount: 0,
      ctx,
      layoutWidth: width,
    },
  });
  flushSync();
  hosts.push({ el, app });
  const bar = el.querySelector<HTMLElement>("[data-region='topbar']")!;
  return { el, bar, packed: paint(bar, el, width) };
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
  localStorage.removeItem(SIDEBAR_STORAGE_KEY);
});

describe("title bar at 640/783/1000/1440 (AC-024-01, AC-055-01)", () => {
  it.each([640, 783, 1000, 1440])(
    "keeps the item order, disjoint rects and no horizontal overflow at %i px",
    async (width) => {
      const { el, bar, packed } = await mountBar(width);
      expect(barItems(el).map(idOf)).toEqual([...ORDER]);
      expect(packed.map((item) => item.id)).toEqual([...ORDER]);
      for (let i = 1; i < packed.length; i++) {
        expect(packed[i]!.left, `${packed[i]!.id} overlaps ${packed[i - 1]!.id}`).toBeGreaterThanOrEqual(
          packed[i - 1]!.right,
        );
      }
      const last = packed.at(-1)!;
      expect(last.right + 12, "items leave the window").toBeLessThanOrEqual(width);
      expect(bar.scrollWidth).toBeLessThanOrEqual(bar.clientWidth);
      for (const item of packed) {
        const node = barItems(el).find((el) => idOf(el) === item.id)!;
        const box = node.getBoundingClientRect();
        expect(box.left).toBeGreaterThanOrEqual(0);
        expect(box.right).toBeLessThanOrEqual(width);
      }
    },
  );

  // AC-047-04: the mark, the search and the host pill are gone from the bar at every width.
  // AC-049-03: so is the panel button. AC-055-01: so is the breadcrumb.
  it.each([500, 640, 783, 1000, 1440])("carries no mark, search, host pill, panel button or breadcrumb at %i px", async (width) => {
    const { el } = await mountBar(width);
    expect(el.querySelector("[data-topbar-item='sidebar']")).toBeNull();
    expect(el.querySelector("[data-panel-icon]")).toBeNull();
    expect(el.querySelector("[data-topbar-item='logo']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='search']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='host']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='new-agent']")).toBeNull();
    expect(el.querySelector("[data-topbar-item='project']")).toBeNull();
    expect(el.querySelector("[data-crumb-workspace]")).toBeNull();
    expect(el.querySelector(".brand-name")).toBeNull();
    expect(el.textContent ?? "").not.toMatch(/Ctrl K/);
    // AC-055-01: the strip is there at every width, with all six tabs (it scrolls, none is cut).
    expect(el.querySelectorAll("[data-center-tabs] [data-tab]")).toHaveLength(6);
  });

  // AC-055-02: whatever the width, there is free bar left to grab, and it is the strip — never
  // the bell or a window control — that gives way.
  it.each([640, 783, 1000, 1440])("the strip yields, the drag area stays grabbable and the controls keep 30 px at %i px", async (width) => {
    const { packed } = await mountBar(width);
    const by = (id: string) => packed.find((item) => item.id === id)!;
    expect(by("drag").width, "no free bar left to drag").toBeGreaterThanOrEqual(DRAG_MIN);
    for (const id of ["notifications", "window-minimize", "window-maximize", "window-close"]) {
      expect(by(id).width, id).toBe(30);
    }
    expect(by("tabs").width).toBeGreaterThanOrEqual(TABS_MIN);
    expect(by("tabs").width).toBeLessThanOrEqual(TABS_PREFERRED);
  });

  it("at 1440 px the strip keeps its preferred width and the drag area takes the rest", async () => {
    const { packed } = await mountBar(1440);
    expect(packed.find((item) => item.id === "tabs")!.width).toBe(TABS_PREFERRED);
    expect(packed.find((item) => item.id === "drag")!.width).toBeGreaterThan(DRAG_MIN);
  });

  it("at 783 px the strip has already given way and nothing leaves the window", async () => {
    const { packed } = await mountBar(783);
    expect(packed.find((item) => item.id === "tabs")!.width).toBeLessThan(TABS_PREFERRED);
    expect(packed.at(-1)!.right + 12).toBeLessThanOrEqual(783);
  });

  it("at 640 px the yielded items stay inside the window", async () => {
    const { packed } = await mountBar(640);
    expect(packed.at(-1)!.right + 12).toBeLessThanOrEqual(640);
  });
});

describe("sidebar auto-collapse keeps 400 px for the center (AC-024-01 edge)", () => {
  it("stays open at 783 px (center 495 >= 400) and does not write the saved preference", () => {
    localStorage.setItem(SIDEBAR_STORAGE_KEY, "true");
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(Sidebar, { target: el, props: { open: true, viewportWidth: 783 } });
    flushSync();
    hosts.push({ el, app });
    const aside = el.querySelector<HTMLElement>("[data-region='sidebar']")!;
    expect(aside.getAttribute("aria-expanded")).toBe("true");
    expect(localStorage.getItem(SIDEBAR_STORAGE_KEY)).toBe("true");
  });

  it("collapses below a 400 px center and restores on widen without changing herdr.sidebar.open", () => {
    localStorage.setItem(SIDEBAR_STORAGE_KEY, "true");
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(Sidebar, { target: el, props: { open: true, viewportWidth: 671 } });
    flushSync();
    hosts.push({ el, app });
    expect(el.querySelector("[data-region='sidebar']")!.getAttribute("aria-expanded")).toBe("false");
    expect(localStorage.getItem(SIDEBAR_STORAGE_KEY)).toBe("true");

    unmount(app);
    el.remove();
    hosts.pop();
    const again = document.createElement("div");
    document.body.appendChild(again);
    const remount = mount(Sidebar, { target: again, props: { open: true, viewportWidth: 783 } });
    flushSync();
    hosts.push({ el: again, app: remount });
    expect(again.querySelector("[data-region='sidebar']")!.getAttribute("aria-expanded")).toBe("true");
    expect(localStorage.getItem(SIDEBAR_STORAGE_KEY)).toBe("true");
  });
});
