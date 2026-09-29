// @vitest-environment happy-dom
// Spec 049 — the two sidebar menus on a window layer (AC-049-01) and closing them by a
// pointerdown anywhere outside or by Esc (AC-049-02).
//
// Would catch the window the user tested: `WorkspaceMenu`/`CollectionMenu` positioned
// `absolute` inside `WorkspacesSection` (`overflow-y: auto`) and `Sidebar.svelte`
// (`overflow: hidden`), so "Mover para coleção" opened a submenu cut at the 288 px edge; and a
// menu that only closed by picking an item, never by clicking away from it.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import { createFakeProjectsBridge, type RecordedCall } from "../../projects/fake-bridge";
import { createProjectsController } from "../../projects/controller";
import type { NavigatorState } from "../../projects/reducer";
import type { CollectionDto, ProjectDto, WorkspacePrefDto } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import { dropdownStyle, flyoutStyle, MENU_MARGIN, SUBMENU_Z_INDEX } from "./menu-layer";
import SidebarV2 from "./SidebarV2.svelte";

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

beforeEach(() => {
  try {
    localStorage.clear();
  } catch {
    /* the suite must not depend on storage being available */
  }
});

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

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd049", online: true, identity: null },
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

/** Two local workspaces of `g1` and a second collection, so the submenu has somewhere to move to. */
function fixture() {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd049",
      target: null,
      workspaces: [
        workspace({ workspace_id: "w-a", number: 1, label: "a", cwd: "/w/a", branch: "main", focused: true }),
        workspace({ workspace_id: "w-b", number: 2, label: "b", cwd: "/w/b", branch: null }),
      ],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-a", label: "a", endpoint_profile_id: "local", session_name: "hd049", root: "/w/a", binding: null },
    { id: "p-b", label: "b", endpoint_profile_id: "local", session_name: "hd049", root: "/w/b", binding: null },
  ];
  const collections: CollectionDto[] = [
    { id: "g1", name: "Acme · Clientes", project_ids: ["p-a", "p-b"] },
    { id: "g2", name: "Open source", project_ids: [] },
  ];
  const prefs: WorkspacePrefDto[] = [];
  return { hosts, projects, collections, prefs };
}

async function render() {
  const options = fixture();
  const bridge = createFakeProjectsBridge({
    bootId: "boot-049",
    seed: { version: 3, projects: options.projects, collections: options.collections, workspace_prefs: options.prefs },
  });
  const nav = reactiveHolder<NavigatorState | null>(null);
  const controller = createProjectsController(bridge, (state) => (nav.value = state));
  await controller.load();
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [name, () => {}]),
  ) as Record<MenuAction, () => void>;
  const holder = reactiveHolder<string | null>("local");
  const ctx: FrameContext = {
    surface: surface(),
    agents: { agents: [], kinds: [] } as unknown as FrameContext["agents"],
    connections: connectionsState(options.hosts),
    get navigator() {
      return nav.value ?? controller.state;
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
      surface: { select: vi.fn(async () => ({}) as never) } as unknown as FrameContext["controllers"]["surface"],
      agents: { createTab: vi.fn(async () => {}), state: { topology: null } } as unknown as FrameContext["controllers"]["agents"],
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
  document.body.appendChild(el);
  const app = mount(SidebarV2, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, calls: bridge.calls, controller };
}

async function settle() {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
  flushSync();
}

const named = (calls: RecordedCall[], command: string) => calls.filter((call) => call.command === command);
const row = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-workspace-row="${id}"]`)!;
const workspaceMenu = () => document.querySelector<HTMLElement>("[data-workspace-menu]");
const submenu = () => document.querySelector<HTMLElement>("[data-collection-submenu]");
const collectionMenu = () => document.querySelector<HTMLElement>("[data-collection-menu]");

/** Rectangle a happy-dom node never measures on its own. */
function rect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    toJSON: () => ({}),
  } as DOMRect;
}

function stub(node: Element, box: DOMRect) {
  node.getBoundingClientRect = () => box;
}

const styleOf = (node: Element) => node.getAttribute("style") ?? "";
const coord = (node: Element, name: "left" | "top") =>
  Number.parseFloat(new RegExp(`${name}:\\s*(-?[\\d.]+)px`).exec(styleOf(node))?.[1] ?? "NaN");

async function openWorkspaceMenu(el: HTMLElement, id: string) {
  row(el, id).querySelector<HTMLButtonElement>("[data-workspace-menu-button]")!.click();
  await settle();
  return workspaceMenu()!;
}

async function openCollectionMenu(el: HTMLElement, id: string) {
  el.querySelector<HTMLButtonElement>(`[data-collection="${id}"] [data-collection-menu-button]`)!.click();
  await settle();
  return collectionMenu()!;
}

const pointerdown = (node: EventTarget) => {
  node.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true }));
  flushSync();
};

const escape = () => {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  flushSync();
};

describe("AC-049-01 the menus live on a window layer, not inside the scrolling list", () => {
  // Would catch the measured cause: the menu drawn `position: absolute` inside
  // `[data-slot="projects"]`, where `WorkspacesSection` (overflow-y: auto) and `Sidebar.svelte`
  // (overflow: hidden) clip everything past 288 px.
  it("mounts the workspace menu outside [data-slot=projects] as a fixed layer", async () => {
    const { el } = await render();
    const menu = await openWorkspaceMenu(el, "w-a");
    expect(menu).toBeTruthy();
    expect(menu.closest('[data-slot="projects"]')).toBeNull();
    expect(menu.closest("[data-sidebar-workspaces]")).toBeNull();
    expect(el.contains(menu)).toBe(false);
    expect(styleOf(menu)).toMatch(/position:\s*fixed/);
  });

  it("mounts the collection menu outside [data-slot=projects] as a fixed layer", async () => {
    const { el } = await render();
    const menu = await openCollectionMenu(el, "g1");
    expect(menu).toBeTruthy();
    expect(menu.closest('[data-slot="projects"]')).toBeNull();
    expect(el.contains(menu)).toBe(false);
    expect(styleOf(menu)).toMatch(/position:\s*fixed/);
  });

  // Would catch: the submenu re-parented but still placed by CSS (`left: calc(100% + 6px)`) on an
  // anchor whose own box is clipped, instead of from the rectangle of "Mover para coleção".
  it("opens the submenu outside the sidebar, to the right of the Mover para coleção item", async () => {
    const { el } = await render();
    const menu = await openWorkspaceMenu(el, "w-a");
    const move = menu.querySelector<HTMLButtonElement>('[data-menu-item="move"]')!;
    const anchor = rect(96, 220, 208, 26);
    stub(move, anchor);
    move.click();
    await settle();
    const flyout = submenu()!;
    expect(flyout).toBeTruthy();
    expect(flyout.closest('[data-slot="projects"]')).toBeNull();
    expect(el.contains(flyout)).toBe(false);
    expect(styleOf(flyout)).toMatch(/position:\s*fixed/);
    expect(coord(flyout, "left")).toBeGreaterThanOrEqual(anchor.right);
  });

  // Would catch: the layer moving the submenu but losing the 044 command behind it.
  it("still moves the row by a single group_assign from the submenu", async () => {
    const { el, calls } = await render();
    const menu = await openWorkspaceMenu(el, "w-a");
    menu.querySelector<HTMLButtonElement>('[data-menu-item="move"]')!.click();
    await settle();
    const entries = [...submenu()!.querySelectorAll<HTMLElement>("[data-move-collection]")];
    expect(entries.map((node) => node.dataset.moveCollection)).toEqual(["g1", "g2"]);
    entries[1]!.click();
    await settle();
    expect(named(calls, "group_assign")).toEqual([
      { command: "group_assign", args: { groupId: "g2", endpoint_profile_id: "local", session_name: "hd049", cwd: "/w/a", label: "a" } },
    ]);
    expect(submenu(), "picking a collection closes the menu").toBeNull();
    expect(workspaceMenu()).toBeNull();
  });

  // Would catch: a flyout that always opens to the right and leaves the window on a narrow one,
  // or a dropdown that never flips up at the foot of the list.
  it("flips the flyout to the left and the dropdown upwards when the window has no room", () => {
    const wide = flyoutStyle(rect(96, 100, 208, 26), 232, 180, SUBMENU_Z_INDEX);
    expect(wide).toMatch(new RegExp(`z-index:${SUBMENU_Z_INDEX}`));
    expect(Number.parseFloat(/left:\s*(-?[\d.]+)px/.exec(wide)![1]!)).toBeGreaterThanOrEqual(304);

    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const tight = rect(vw - 240, 100, 208, 26);
    const left = Number.parseFloat(/left:\s*(-?[\d.]+)px/.exec(flyoutStyle(tight, 232, 180, SUBMENU_Z_INDEX))![1]!);
    expect(left).toBeLessThan(tight.right);
    expect(left).toBeGreaterThanOrEqual(MENU_MARGIN);

    const foot = rect(96, vh - 40, 208, 26);
    const top = Number.parseFloat(/top:\s*(-?[\d.]+)px/.exec(dropdownStyle(foot, 232, 320, SUBMENU_Z_INDEX))![1]!);
    expect(top).toBeLessThan(foot.top);
    expect(top).toBeGreaterThanOrEqual(MENU_MARGIN);
  });
});

describe("AC-049-02 a pointerdown anywhere outside, or Esc, closes the menu", () => {
  // Would catch the window the user tested: the menu staying open after clicking away from it,
  // because nothing listened outside the sidebar.
  it("closes the workspace menu on a pointerdown on the window, running no item", async () => {
    const { el, calls } = await render();
    await openWorkspaceMenu(el, "w-a");
    pointerdown(document.body);
    await settle();
    expect(workspaceMenu()).toBeNull();
    expect(named(calls, "workspace_pref_set")).toEqual([]);
    expect(named(calls, "group_assign")).toEqual([]);
    expect(named(calls, "workspace_close")).toEqual([]);
  });

  it("keeps the workspace menu open on a pointerdown inside it or inside the submenu", async () => {
    const { el } = await render();
    const menu = await openWorkspaceMenu(el, "w-a");
    pointerdown(menu);
    await settle();
    expect(workspaceMenu()).toBeTruthy();

    menu.querySelector<HTMLButtonElement>('[data-menu-item="move"]')!.click();
    await settle();
    pointerdown(submenu()!.querySelector("[data-move-collection]")!);
    await settle();
    expect(workspaceMenu(), "the submenu is part of the same menu").toBeTruthy();
    expect(submenu()).toBeTruthy();
  });

  it("closes the workspace menu on Esc without running an item", async () => {
    const { el, calls } = await render();
    await openWorkspaceMenu(el, "w-a");
    escape();
    await settle();
    expect(workspaceMenu()).toBeNull();
    for (const command of ["workspace_pref_set", "group_assign", "workspace_close", "workspace_rename"]) {
      expect(named(calls, command), command).toEqual([]);
    }
  });

  it("closes the collection menu on a pointerdown outside and on Esc, running no item", async () => {
    const { el, calls } = await render();
    const menu = await openCollectionMenu(el, "g1");
    pointerdown(menu);
    await settle();
    expect(collectionMenu(), "inside the menu keeps it open").toBeTruthy();

    pointerdown(document.body);
    await settle();
    expect(collectionMenu()).toBeNull();
    expect(named(calls, "group_delete")).toEqual([]);
    expect(named(calls, "group_set_color")).toEqual([]);

    await openCollectionMenu(el, "g1");
    escape();
    await settle();
    expect(collectionMenu()).toBeNull();
    expect(named(calls, "group_delete")).toEqual([]);
  });
});
