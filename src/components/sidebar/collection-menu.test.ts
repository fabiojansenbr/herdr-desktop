// @vitest-environment happy-dom
// Spec 045 — the collection menu (AC-045-02) and the collapse that survives a reload
// (AC-045-03). A stored collection keeps its state in the store (`Group.collapsed`); the two
// synthetic buckets — `Sem coleção` and `Ocultos` — have no row in the store, so they keep it in
// `localStorage`, read and written inside try/catch.
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
import { GROUP_PALETTE } from "../projects/tree-model";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import { collapseKey, HIDDEN_ID, readCollapsed, UNGROUPED_ID, writeCollapsed } from "./sidebar-model";
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
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd045", online: true, identity: null },
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

/** `G1 = [a, b]`, the loose `d` (so `Sem coleção` exists) and the hidden `c` (so `Ocultos` does). */
function fixture(collections?: CollectionDto[]) {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd045",
      target: null,
      workspaces: [
        workspace({ workspace_id: "w-a", number: 1, label: "a", cwd: "/w/a", focused: true }),
        workspace({ workspace_id: "w-b", number: 2, label: "b", cwd: "/w/b" }),
        workspace({ workspace_id: "w-c", number: 3, label: "c", cwd: "/w/c" }),
        workspace({ workspace_id: "w-d", number: 4, label: "d", cwd: "/w/d" }),
      ],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-a", label: "a", endpoint_profile_id: "local", session_name: "hd045", root: "/w/a", binding: null },
    { id: "p-b", label: "b", endpoint_profile_id: "local", session_name: "hd045", root: "/w/b", binding: null },
    { id: "p-c", label: "c", endpoint_profile_id: "local", session_name: "hd045", root: "/w/c", binding: null },
  ];
  const prefs: WorkspacePrefDto[] = [
    { endpoint_profile_id: "local", root: "/w/c", color: null, pinned: false, hidden: true },
  ];
  return {
    hosts,
    projects,
    prefs,
    collections: collections ?? [{ id: "g1", name: "G1", project_ids: ["p-a", "p-b", "p-c"] }],
  };
}

async function render(options = fixture()) {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-045",
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
  return { el, calls: bridge.calls, controller, nav, bridge };
}

async function settle() {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
  flushSync();
}

const group = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-collection="${id}"]`)!;
const named = (calls: RecordedCall[], command: string) => calls.filter((call) => call.command === command);

/** Opens the `…` menu of a collection header and returns it. */
async function openMenu(el: HTMLElement, id: string) {
  group(el, id).querySelector<HTMLButtonElement>("[data-collection-menu-button]")!.click();
  await settle();
  // AC-049-01: the menu is mounted on a window layer, not under the collection header.
  return document.querySelector<HTMLElement>("[data-collection-menu]")!;
}

describe("AC-045-02 collection menu", () => {
  // Would catch: the menu missing from the header, offered on the synthetic buckets (which have
  // no store row to edit), or the items in another order.
  it("opens the 3 items from … and from a right click, never on Sem coleção", async () => {
    const { el } = await render();
    const menu = await openMenu(el, "g1");
    expect([...menu.querySelectorAll<HTMLElement>("[data-collection-menu-item]")].map((n) => n.dataset.collectionMenuItem)).toEqual([
      "rename",
      "color",
      "delete",
    ]);
    expect(menu.textContent).toContain("Renomear");
    expect(menu.textContent).toContain("Cor");
    expect(menu.textContent).toContain("Excluir coleção…");
    expect(menu.querySelectorAll("[data-collection-swatch]")).toHaveLength(GROUP_PALETTE.length);

    group(el, "g1").querySelector<HTMLButtonElement>("[data-collection-menu-button]")!.click();
    await settle();
    group(el, UNGROUPED_ID).querySelector<HTMLElement>("[data-collection-head]")!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
    );
    await settle();
    expect(document.querySelector("[data-collection-menu]"), "Sem coleção is not a stored collection").toBeNull();
    expect(group(el, UNGROUPED_ID).querySelector("[data-collection-menu-button]")).toBeNull();

    group(el, "g1").querySelector<HTMLElement>("[data-collection-head]")!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
    );
    await settle();
    expect(document.querySelector("[data-collection-menu]")!.getAttribute("aria-label")).toBe("Ações da coleção G1");
  });

  // Would catch: Renomear sending the raw value, sending an empty name, or firing twice.
  it("renames in line: one group_rename with the trimmed name, nothing when empty", async () => {
    const { el, calls } = await render();
    (await openMenu(el, "g1")).querySelector<HTMLButtonElement>('[data-collection-menu-item="rename"]')!.click();
    await settle();
    const input = group(el, "g1").querySelector<HTMLInputElement>("[data-collection-rename-input]")!;
    expect(input.value).toBe("G1");

    input.value = "   ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();
    expect(named(calls, "group_rename")).toEqual([]);

    input.value = "  Clientes  ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();
    expect(named(calls, "group_rename")).toEqual([{ command: "group_rename", args: { groupId: "g1", name: "Clientes" } }]);
    expect(group(el, "g1").querySelector("[data-collection-rename-input]"), "Enter closes the editor").toBeNull();
    expect(group(el, "g1").querySelector("[data-collection-name]")!.textContent).toContain("Clientes");
  });

  // Would catch: Esc committing the rename anyway.
  it("cancels the inline rename on Esc without sending", async () => {
    const { el, calls } = await render();
    (await openMenu(el, "g1")).querySelector<HTMLButtonElement>('[data-collection-menu-item="rename"]')!.click();
    await settle();
    const input = group(el, "g1").querySelector<HTMLInputElement>("[data-collection-rename-input]")!;
    input.value = "Outro";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(named(calls, "group_rename")).toEqual([]);
    expect(group(el, "g1").querySelector("[data-collection-rename-input]")).toBeNull();
  });

  // Would catch: a swatch outside the palette, or the colour written on a workspace preference
  // (044) instead of on the collection.
  it("writes the chosen palette colour on the collection and paints its swatch", async () => {
    const { el, calls, nav } = await render();
    const menu = await openMenu(el, "g1");
    const swatches = [...menu.querySelectorAll<HTMLButtonElement>("[data-collection-swatch]")];
    expect(swatches.map((s) => s.dataset.collectionSwatch)).toEqual([...GROUP_PALETTE]);
    swatches[2]!.click();
    await settle();

    expect(named(calls, "group_set_color")).toEqual([{ command: "group_set_color", args: { groupId: "g1", color: "#B18CFF" } }]);
    expect(named(calls, "workspace_pref_set")).toEqual([]);
    expect(nav.value!.snapshot!.collections[0]!.color).toBe("#B18CFF");
    expect(group(el, "g1").querySelector<HTMLElement>("[data-collection-color]")!.getAttribute("style")).toMatch(/#B18CFF/i);
  });

  // Would catch: Excluir deleting on the first click, deleting the projects, or closing a
  // workspace on the engine.
  it("asks in line before deleting and then moves the members to Sem coleção", async () => {
    const { el, calls, nav } = await render();
    const menu = await openMenu(el, "g1");
    menu.querySelector<HTMLButtonElement>('[data-collection-menu-item="delete"]')!.click();
    await settle();
    expect(named(calls, "group_delete")).toEqual([]);
    expect(document.querySelector("[data-confirm-delete]")).toBeTruthy();

    document.querySelector<HTMLButtonElement>("[data-cancel-delete]")!.click();
    await settle();
    expect(named(calls, "group_delete")).toEqual([]);

    menu.querySelector<HTMLButtonElement>('[data-collection-menu-item="delete"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>("[data-confirm-delete]")!.click();
    await settle();

    expect(named(calls, "group_delete")).toEqual([{ command: "group_delete", args: { groupId: "g1" } }]);
    expect(named(calls, "workspace_close")).toEqual([]);
    expect(nav.value!.snapshot!.collections).toEqual([]);
    expect(nav.value!.snapshot!.projects.map((p) => p.id)).toEqual(["p-a", "p-b", "p-c"]);
    expect(el.querySelector('[data-collection="g1"]')).toBeNull();
    const loose = group(el, UNGROUPED_ID);
    expect([...loose.querySelectorAll<HTMLElement>("[data-workspace-row]")].map((n) => n.dataset.workspaceRow)).toEqual([
      "w-a",
      "w-b",
      "w-d",
    ]);
  });
});

describe("AC-045-03 collapse that survives a reload", () => {
  // Would catch: the collapse kept only in memory (the 041 behaviour), or written somewhere the
  // next window would not read.
  it("persists a stored collection's collapse with group_set_collapsed and reads it back", async () => {
    const { el, calls } = await render();
    const toggle = () => group(el, "g1").querySelector<HTMLButtonElement>("[data-collection-toggle]")!;
    expect(toggle().getAttribute("aria-expanded")).toBe("true");

    toggle().click();
    await settle();
    expect(named(calls, "group_set_collapsed")).toEqual([
      { command: "group_set_collapsed", args: { groupId: "g1", collapsed: true } },
    ]);
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(group(el, "g1").querySelectorAll("[data-workspace-row]")).toHaveLength(0);

    toggle().click();
    await settle();
    expect(named(calls, "group_set_collapsed").at(-1)!.args).toEqual({ groupId: "g1", collapsed: false });
  });

  // Would catch (verify round 1): the optimistic collapse surviving a refused write, so the
  // header kept claiming a state the store never recorded.
  it("reverts the optimistic collapse when the store refuses the write", async () => {
    const { el, calls, bridge } = await render();
    const toggle = () => group(el, "g1").querySelector<HTMLButtonElement>("[data-collection-toggle]")!;
    bridge.failCommand("group_set_collapsed", { code: "store_write_failed", message: "falha ao gravar projetos", retryable: false });

    toggle().click();
    flushSync();
    expect(toggle().getAttribute("aria-expanded"), "the click is answered at once").toBe("false");

    await settle();
    expect(named(calls, "group_set_collapsed")).toEqual([
      { command: "group_set_collapsed", args: { groupId: "g1", collapsed: true } },
    ]);
    expect(toggle().getAttribute("aria-expanded"), "the refusal falls back to the stored state").toBe("true");
    expect(group(el, "g1").querySelectorAll("[data-workspace-row]").length).toBeGreaterThan(0);

    // With the store accepting again the collapse sticks, so the revert is not a blanket reset.
    bridge.failCommand("group_set_collapsed", null);
    toggle().click();
    await settle();
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
  });

  // A fresh window over a store that already says `collapsed` starts collapsed, with no command.
  it("starts collapsed when the store says so, without writing anything", async () => {
    const base = fixture([{ id: "g1", name: "G1", project_ids: ["p-a", "p-b", "p-c"], collapsed: true }]);
    const { el, calls } = await render(base);
    expect(group(el, "g1").querySelector("[data-collection-toggle]")!.getAttribute("aria-expanded")).toBe("false");
    expect(named(calls, "group_set_collapsed")).toEqual([]);
  });

  // The two synthetic buckets have no store row: their state lives in localStorage under the
  // documented key, and a fresh window reads it back.
  it("keeps Sem coleção and Ocultos collapsed through localStorage", async () => {
    const first = await render();
    expect(collapseKey(UNGROUPED_ID)).toBe("herdr.sidebar.collapsed.__ungrouped__");
    expect(collapseKey(HIDDEN_ID)).toBe("herdr.sidebar.collapsed.__hidden__");

    group(first.el, UNGROUPED_ID).querySelector<HTMLButtonElement>("[data-collection-toggle]")!.click();
    await settle();
    expect(localStorage.getItem(collapseKey(UNGROUPED_ID))).toBe("1");
    expect(named(first.calls, "group_set_collapsed"), "a synthetic bucket never touches the store").toEqual([]);

    first.el.querySelector<HTMLButtonElement>("[data-hidden-toggle]")!.click();
    await settle();
    expect(localStorage.getItem(collapseKey(HIDDEN_ID))).toBe("0");

    // A new window over the same storage reopens in the same state.
    const second = await render();
    expect(group(second.el, UNGROUPED_ID).querySelector("[data-collection-toggle]")!.getAttribute("aria-expanded")).toBe("false");
    expect(second.el.querySelector("[data-hidden-toggle]")!.getAttribute("aria-expanded")).toBe("true");
  });

  // Would catch: a private window (storage throwing) taking the sidebar down with it.
  it("reads and writes the collapse state inside try/catch", () => {
    const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("storage disabled");
      },
    });
    try {
      expect(readCollapsed(UNGROUPED_ID, true)).toBe(true);
      expect(() => writeCollapsed(UNGROUPED_ID, false)).not.toThrow();
    } finally {
      if (original) Object.defineProperty(globalThis, "localStorage", original);
    }
    expect(readCollapsed(UNGROUPED_ID, false)).toBe(false);
  });
});
