// @vitest-environment happy-dom
// Spec 044 — hover actions and the workspace menu (AC-044-02), the effect of the preferences
// on the list (AC-044-03) and the two icons the 041 capture got wrong (AC-044-04).
// Every action goes through the real controller over the fake bridges, so each case asserts the
// command that actually left the WebView, once, with the arguments the Tauri bridge would send.
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
import SidebarV2 from "./SidebarV2.svelte";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];
const clipboard: string[] = [];

beforeEach(() => {
  clipboard.length = 0;
  Object.defineProperty(globalThis.navigator, "clipboard", {
    configurable: true,
    value: { writeText: async (text: string) => void clipboard.push(text) },
  });
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
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd044", online: true, identity: null },
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

/**
 * Three local workspaces of one collection (`a`, `b`, `c`), each a saved project on its own
 * root, plus a second collection so "Mover para coleção" has somewhere to go.
 */
function fixture(prefs: WorkspacePrefDto[] = []) {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd044",
      target: null,
      workspaces: [
        workspace({ workspace_id: "w-a", number: 1, label: "a", cwd: "/w/a", branch: "main", focused: true }),
        workspace({ workspace_id: "w-b", number: 2, label: "b", cwd: "/w/b", branch: "feat/login" }),
        workspace({ workspace_id: "w-c", number: 3, label: "c", cwd: "/w/c", branch: null }),
      ],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-a", label: "a", endpoint_profile_id: "local", session_name: "hd044", root: "/w/a", binding: null },
    { id: "p-b", label: "b", endpoint_profile_id: "local", session_name: "hd044", root: "/w/b", binding: null },
    { id: "p-c", label: "c", endpoint_profile_id: "local", session_name: "hd044", root: "/w/c", binding: null },
  ];
  const collections: CollectionDto[] = [
    { id: "g1", name: "Acme · Clientes", project_ids: ["p-a", "p-b", "p-c"] },
    { id: "g2", name: "Open source", project_ids: [] },
  ];
  return { hosts, projects, collections, prefs };
}

async function render(options: ReturnType<typeof fixture> = fixture()) {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-044",
    seed: { version: 3, projects: options.projects, collections: options.collections, workspace_prefs: options.prefs },
  });
  // Same wiring as App.svelte: the controller's onChange feeds a `$state`, so a store command
  // re-renders the section (a preference only shows once the snapshot came back).
  const nav = reactiveHolder<NavigatorState | null>(null);
  const controller = createProjectsController(bridge, (state) => (nav.value = state));
  await controller.load();
  const calls = bridge.calls;
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [name, () => {}]),
  ) as Record<MenuAction, () => void>;
  const holder = reactiveHolder<string | null>("local");
  const createTab = vi.fn(async () => {});
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
  document.body.appendChild(el);
  const app = mount(SidebarV2, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, calls, controller, createTab };
}

/** Lets the controller's promise chain settle and re-renders. */
async function settle() {
  for (let i = 0; i < 6; i += 1) await Promise.resolve();
  flushSync();
}

const row = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-workspace-row="${id}"]`)!;
const named = (calls: RecordedCall[], command: string) => calls.filter((call) => call.command === command);

/**
 * Opens the `…` menu of a row and returns it. Spec 049 (AC-049-01) mounts the menu on a window
 * layer instead of inside the scrolling list, so it is looked up on the document, not under the
 * sidebar root.
 */
async function openMenu(el: HTMLElement, id: string) {
  row(el, id).querySelector<HTMLButtonElement>("[data-workspace-menu-button]")!.click();
  await settle();
  return document.querySelector<HTMLElement>("[data-workspace-menu]")!;
}

describe("AC-044-02 hover actions and the workspace menu", () => {
  // Would catch: the hover cluster missing from a row, or drawn permanently (the 041 row had no
  // actions at all), and a handle that is not inert in this spec.
  it("mounts the handle, + and … on every row, revealed by hover or keyboard focus", async () => {
    const { el } = await render();
    for (const id of ["w-a", "w-b", "w-c"]) {
      const line = row(el, id);
      expect(line.querySelector("[data-workspace-handle]"), id).toBeTruthy();
      expect(line.querySelector("[data-workspace-add]"), id).toBeTruthy();
      expect(line.querySelector("[data-workspace-menu-button]"), id).toBeTruthy();
    }
    // The alça does nothing in this spec (dragging is 045).
    expect(row(el, "w-a").querySelector("[data-workspace-handle]")!.tagName).not.toBe("BUTTON");
    const css = sources["./WorkspaceActions.svelte"]!;
    expect(rule(css, ".cluster")).toMatch(/opacity:\s*0/);
    expect(rule(css, ":global(.row:hover) .cluster")).toMatch(/opacity:\s*1/);
    expect(rule(css, ":global(.row:focus-visible) .cluster")).toMatch(/opacity:\s*1/);
    expect(rule(css, ":global(.row:focus-within) .cluster")).toMatch(/opacity:\s*1/);
  });

  // Would catch: `+` opening a popover on the wrong workspace (no focus first), focusing twice,
  // or rebuilding the 017 popover instead of mounting it.
  it("focuses the workspace once and opens NewAgentPopover from +", async () => {
    const { el, calls } = await render();
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();
    row(el, "w-b").querySelector<HTMLButtonElement>("[data-workspace-add]")!.click();
    await settle();
    expect(named(calls, "workspace_focus")).toEqual([{ command: "workspace_focus", args: { endpoint: "local", workspaceId: "w-b" } }]);
    expect(row(el, "w-b").querySelector("[data-new-agent-popover]")).toBeTruthy();
  });

  // Would catch: the menu in another order than the design, an item missing, or the destructive
  // item not marked apart.
  it("opens the 7 items in the order of the design from … and from a right click", async () => {
    const { el } = await render();
    const menu = await openMenu(el, "w-a");
    expect([...menu.querySelectorAll<HTMLElement>("[data-menu-item]")].map((n) => n.dataset.menuItem)).toEqual([
      "rename",
      "color",
      "pin",
      "move",
      "copy-path",
      "hide",
      "close",
    ]);
    expect(menu.textContent).toContain("Renomear");
    expect(menu.textContent).toContain("F2");
    expect(menu.textContent).toContain("Cor");
    expect(menu.textContent).toContain("Fixar no topo");
    expect(menu.textContent).toContain("Mover para coleção");
    expect(menu.textContent).toContain("Copiar caminho");
    expect(menu.textContent).toContain("Ocultar da lateral");
    expect(menu.textContent).toContain("Fechar workspace…");
    expect(rule(sources["./WorkspaceMenu.svelte"]!, '[data-menu-item="close"]')).toMatch(/var\(--error/);
    expect(menu.querySelectorAll("[data-color-swatch]")).toHaveLength(GROUP_PALETTE.length);

    // Right click on another row moves the menu there instead of opening a second one.
    row(el, "w-a").querySelector<HTMLButtonElement>("[data-workspace-menu-button]")!.click();
    await settle();
    row(el, "w-b").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    await settle();
    // AC-049-01: one menu, on the window layer, anchored to the row that opened it.
    expect(document.querySelectorAll("[data-workspace-menu]")).toHaveLength(1);
    expect(document.querySelector("[data-workspace-menu]")!.getAttribute("aria-label")).toBe("Ações de b");
  });

  // Would catch: Renomear sending the raw value, sending twice, sending an empty name, or Esc
  // committing the edit anyway.
  it("renames in line from the menu and from F2: one workspace_rename with the trimmed name", async () => {
    const { el, calls } = await render();
    const menu = await openMenu(el, "w-b");
    menu.querySelector<HTMLButtonElement>('[data-menu-item="rename"]')!.click();
    await settle();
    const input = row(el, "w-b").querySelector<HTMLInputElement>("[data-rename-input]")!;
    expect(input.value).toBe("b");

    // Empty sends nothing and keeps the editor open.
    input.value = "   ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();
    expect(named(calls, "workspace_rename")).toEqual([]);
    expect(row(el, "w-b").querySelector("[data-rename-input]")).toBeTruthy();

    input.value = "  portal-web  ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();
    expect(named(calls, "workspace_rename")).toEqual([
      { command: "workspace_rename", args: { endpoint: "local", workspaceId: "w-b", label: "portal-web" } },
    ]);
    expect(row(el, "w-b").querySelector("[data-rename-input]"), "Enter closes the editor").toBeNull();

    // F2 on the focused row opens the same editor; Esc cancels without sending.
    row(el, "w-c").dispatchEvent(new KeyboardEvent("keydown", { key: "F2", bubbles: true, cancelable: true }));
    await settle();
    const second = row(el, "w-c").querySelector<HTMLInputElement>("[data-rename-input]")!;
    second.value = "outro";
    second.dispatchEvent(new Event("input", { bubbles: true }));
    second.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(row(el, "w-c").querySelector("[data-rename-input]")).toBeNull();
    expect(named(calls, "workspace_rename")).toHaveLength(1);
  });

  // Would catch: a swatch outside the palette, or the colour sent to the engine instead of the
  // client store.
  it("writes the chosen palette colour with workspace_pref_set", async () => {
    const { el, calls } = await render();
    const menu = await openMenu(el, "w-a");
    const swatches = [...menu.querySelectorAll<HTMLButtonElement>("[data-color-swatch]")];
    expect(swatches.map((s) => s.dataset.colorSwatch)).toEqual([...GROUP_PALETTE]);
    swatches[4]!.click();
    await settle();
    expect(named(calls, "workspace_pref_set")).toEqual([
      { command: "workspace_pref_set", args: { endpointProfileId: "local", root: "/w/a", color: "#F2777A", pinned: undefined, hidden: undefined } },
    ]);
  });

  // Would catch: Fixar always sending `true` (never unpinning), or the item keeping its label
  // when the row is already pinned.
  it("toggles pinned from Fixar no topo and offers Desafixar when it is pinned", async () => {
    const { el, calls } = await render();
    (await openMenu(el, "w-a")).querySelector<HTMLButtonElement>('[data-menu-item="pin"]')!.click();
    await settle();
    expect(named(calls, "workspace_pref_set")).toEqual([
      { command: "workspace_pref_set", args: { endpointProfileId: "local", root: "/w/a", color: undefined, pinned: true, hidden: undefined } },
    ]);

    const again = await openMenu(el, "w-a");
    expect(again.querySelector('[data-menu-item="pin"]')!.textContent).toContain("Desafixar");
    again.querySelector<HTMLButtonElement>('[data-menu-item="pin"]')!.click();
    await settle();
    expect(named(calls, "workspace_pref_set").at(-1)!.args).toEqual({ endpointProfileId: "local", root: "/w/a", color: undefined, pinned: false, hidden: undefined });
  });

  // Would catch: the submenu listing something other than the store collections, no ✓ on the
  // current one, or a move that creates a project instead of assigning the root cwd.
  it("lists the store collections with ✓ on the current one and moves by group_assign", async () => {
    const { el, calls } = await render();
    const menu = await openMenu(el, "w-a");
    menu.querySelector<HTMLButtonElement>('[data-menu-item="move"]')!.click();
    await settle();
    const entries = [...document.querySelectorAll<HTMLElement>("[data-move-collection]")];
    expect(entries.map((n) => n.dataset.moveCollection)).toEqual(["g1", "g2"]);
    expect(entries.map((n) => n.dataset.current)).toEqual(["true", undefined]);
    entries[1]!.click();
    await settle();
    expect(named(calls, "group_assign")).toEqual([
      { command: "group_assign", args: { groupId: "g2", endpoint_profile_id: "local", session_name: "hd044", cwd: "/w/a", label: "a" } },
    ]);
  });

  // Would catch: Nova coleção… creating the collection and stopping there, or moving before the
  // collection exists.
  it("creates a collection in line from Nova coleção… and moves the row into it", async () => {
    const { el, calls } = await render();
    const menu = await openMenu(el, "w-b");
    menu.querySelector<HTMLButtonElement>('[data-menu-item="move"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>("[data-new-collection]")!.click();
    await settle();
    const input = document.querySelector<HTMLInputElement>("[data-new-collection-input]")!;
    input.value = "  Pessoal  ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await settle();
    expect(named(calls, "group_create")).toEqual([{ command: "group_create", args: { name: "Pessoal" } }]);
    const created = named(calls, "group_assign");
    expect(created).toHaveLength(1);
    expect(created[0]!.args).toMatchObject({ endpoint_profile_id: "local", session_name: "hd044", cwd: "/w/b", label: "b" });
    expect(created[0]!.args.groupId).not.toBe("g1");
  });

  // Would catch: Copiar caminho copying the label, or reaching for a shell command.
  it("writes the root of the row on the clipboard from Copiar caminho", async () => {
    const { el, calls } = await render();
    (await openMenu(el, "w-c")).querySelector<HTMLButtonElement>('[data-menu-item="copy-path"]')!.click();
    await settle();
    expect(clipboard).toEqual(["/w/c"]);
    expect(named(calls, "workspace_pref_set")).toEqual([]);
  });

  // Would catch: Ocultar closing the workspace on the engine instead of writing a preference.
  it("writes hidden: true from Ocultar da lateral", async () => {
    const { el, calls } = await render();
    (await openMenu(el, "w-c")).querySelector<HTMLButtonElement>('[data-menu-item="hide"]')!.click();
    await settle();
    expect(named(calls, "workspace_pref_set")).toEqual([
      { command: "workspace_pref_set", args: { endpointProfileId: "local", root: "/w/c", color: undefined, pinned: undefined, hidden: true } },
    ]);
    expect(named(calls, "workspace_close")).toEqual([]);
  });

  // Would catch: Fechar workspace… closing on the first click (no confirmation), or sending the
  // close twice.
  it("asks for confirmation in line and then closes the workspace once", async () => {
    const { el, calls } = await render();
    const menu = await openMenu(el, "w-b");
    menu.querySelector<HTMLButtonElement>('[data-menu-item="close"]')!.click();
    await settle();
    expect(named(calls, "workspace_close")).toEqual([]);
    expect(document.querySelector("[data-confirm-close]")).toBeTruthy();

    document.querySelector<HTMLButtonElement>("[data-cancel-close]")!.click();
    await settle();
    expect(named(calls, "workspace_close")).toEqual([]);
    expect(document.querySelector("[data-confirm-close]"), "cancel goes back to the menu").toBeNull();

    // The menu stayed open: asking again and confirming closes the workspace exactly once.
    menu.querySelector<HTMLButtonElement>('[data-menu-item="close"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>("[data-confirm-close]")!.click();
    await settle();
    expect(named(calls, "workspace_close")).toEqual([{ command: "workspace_close", args: { endpoint: "local", workspaceId: "w-b" } }]);
  });
});

describe("AC-044-03 effect of the preferences on the list", () => {
  const prefs: WorkspacePrefDto[] = [
    { endpoint_profile_id: "local", root: "/w/b", color: null, pinned: true, hidden: false },
    { endpoint_profile_id: "local", root: "/w/c", color: null, pinned: false, hidden: true },
    { endpoint_profile_id: "local", root: "/w/a", color: "#F2777A", pinned: false, hidden: false },
  ];

  // Would catch: the pinned row left in its engine order, the pin marker missing, the hidden row
  // still in its collection, or the colour applied to the name instead of the folder icon.
  it("puts b (pinned) before a, colours a's folder and keeps c out of the collection", async () => {
    const { el } = await render(fixture(prefs));
    const g1 = el.querySelector<HTMLElement>('[data-collection="g1"]')!;
    expect([...g1.querySelectorAll<HTMLElement>("[data-workspace-row]")].map((n) => n.dataset.workspaceRow)).toEqual(["w-b", "w-a"]);
    expect(row(el, "w-b").querySelector("[data-pinned]"), "pin marker on the pinned row").toBeTruthy();
    expect(row(el, "w-a").querySelector("[data-pinned]")).toBeNull();
    const icon = row(el, "w-a").querySelector<HTMLElement>("[data-row-icon]")!;
    expect(icon.dataset.rowColor).toBe("#F2777A");
    expect(icon.getAttribute("style")).toMatch(/color:\s*#F2777A/i);
    expect(row(el, "w-b").querySelector<HTMLElement>("[data-row-icon]")!.dataset.rowColor).toBeUndefined();
  });

  // Would catch (P5): the hidden row simply gone, the counter wrong, the group not collapsible,
  // or Reexibir writing something other than `hidden: false`.
  it("collects c in a collapsible Ocultos (1) at the end of the section, with Reexibir", async () => {
    const { el, calls } = await render(fixture(prefs));
    const section = el.querySelector<HTMLElement>("[data-sidebar-workspaces]")!;
    const blocks = [...section.querySelectorAll<HTMLElement>("[data-collection],[data-hidden-group]")];
    expect(blocks.at(-1)!.dataset.hiddenGroup, "Ocultos closes the section").toBe("1");
    const toggle = section.querySelector<HTMLButtonElement>("[data-hidden-toggle]")!;
    expect(toggle.textContent).toContain("Ocultos (1)");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(section.querySelector("[data-hidden-row]")).toBeNull();

    toggle.click();
    await settle();
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const hidden = section.querySelector<HTMLElement>('[data-hidden-row="w-c"]')!;
    expect(hidden.textContent).toContain("c");
    const unhide = hidden.querySelector<HTMLButtonElement>("[data-unhide]")!;
    expect(unhide.textContent).toContain("Reexibir");
    unhide.click();
    await settle();
    expect(named(calls, "workspace_pref_set")).toEqual([
      { command: "workspace_pref_set", args: { endpointProfileId: "local", root: "/w/c", color: undefined, pinned: undefined, hidden: false } },
    ]);
    expect(el.querySelector('[data-workspace-row="w-c"]'), "the row is back in its collection").toBeTruthy();
  });

  // Would catch: an empty Ocultos header drawn when nothing is hidden.
  it("omits Ocultos when no workspace is hidden", async () => {
    const { el } = await render();
    expect(el.querySelector("[data-hidden-group]")).toBeNull();
  });
});

describe("AC-044-04 icons of the list", () => {
  // Would catch the 041 capture (`evidencias/041/janela-real.png`): a text caret instead of the
  // 12 px chevron, pointing the same way in both states.
  it("draws a 12 px chevron-down when the collection is open and chevron-right when collapsed", async () => {
    const { el } = await render();
    const head = el.querySelector<HTMLButtonElement>('[data-collection="g1"] [data-collection-toggle]')!;
    const caret = () => el.querySelector<HTMLElement>('[data-collection="g1"] [data-collection-caret]')!;
    expect(caret().dataset.collectionCaret).toBe("chevron-down");
    const svg = caret().querySelector("svg")!;
    expect(svg.getAttribute("width")).toBe("12");
    expect(svg.getAttribute("height")).toBe("12");
    expect(caret().textContent!.trim()).toBe("");

    head.click();
    flushSync();
    expect(caret().dataset.collectionCaret).toBe("chevron-right");
    expect(caret().querySelector("svg")!.getAttribute("width")).toBe("12");
  });

  // Would catch the 041 capture: the `⌇` character in front of the branch instead of the 11 px
  // git-branch icon.
  it("draws the 11 px git-branch icon on the branch line and no ⌇", async () => {
    const { el } = await render();
    const branch = row(el, "w-a").querySelector<HTMLElement>("[data-branch]")!;
    const icon = branch.querySelector<HTMLElement>('[data-branch-icon="git-branch"]')!;
    expect(icon).toBeTruthy();
    const svg = icon.querySelector("svg")!;
    expect(svg.getAttribute("width")).toBe("11");
    expect(svg.getAttribute("height")).toBe("11");
    expect(branch.textContent).toContain("main");
    expect(branch.textContent).not.toContain("⌇");
    expect(branch.textContent).not.toContain("⎇");
    // A row without a branch draws neither the icon nor an empty line (038).
    expect(row(el, "w-c").querySelector("[data-branch]")).toBeNull();
  });
});
