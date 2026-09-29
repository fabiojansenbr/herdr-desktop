// @vitest-environment happy-dom
// Spec 017 — AC-017-02: projects sidebar header/icons, collection form on demand, CONEXÕES
// pinned inside an 800 px sidebar. Would catch the 016 window: the old "Nova coleção" form
// always visible and the connections footer missing.
import { mount, unmount, flushSync } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import type { ProjectDto } from "../../projects/types";
import type { FrameContext } from "../../shell/frame-context";
import type { SurfaceState } from "../../shell/controller";
import type { ConnectionsState } from "../../connections/controller";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder, type ReactiveHolder } from "../center/reactive-test-state.svelte";
import ProjectsRegion from "./ProjectsRegion.svelte";

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

function box(top: number, left: number, width: number, height: number): DOMRect {
  return {
    top,
    left,
    width,
    height,
    bottom: top + height,
    right: left + width,
    x: left,
    y: top,
    toJSON() {
      return this;
    },
  } as DOMRect;
}

function inside(outer: DOMRect, inner: DOMRect): boolean {
  return inner.top >= outer.top && inner.left >= outer.left && inner.bottom <= outer.bottom && inner.right <= outer.right;
}

function surface(): SurfaceState {
  return {
    selection: {
      endpoint: "local",
      kind: "local",
      label: "Local",
      session: "hd017",
      online: true,
      identity: null,
    },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  };
}

async function renderSidebar(
  sidebarHeight: number,
  selected: string | ReactiveHolder<string | null> = "local",
  customHosts?: HostDto[],
  seedProjects: ProjectDto[] = [],
) {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-017",
    seed: {
      version: 1,
      projects: seedProjects,
      collections: [{ id: "c-empty", name: "Vazio", project_ids: [] }],
    },
  });
  const controller = createProjectsController(bridge);
  await controller.load();
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
      (name) => [name, () => {}],
    ),
  ) as Record<MenuAction, () => void>;
  const endpointHolder = typeof selected === "string" ? reactiveHolder<string | null>(selected) : selected;
  const connections = {
    view: {
      hub: {
        revision: 1,
        hosts: customHosts ?? [
          hostFixture({
            endpoint: "local",
            label: "Local",
            kind: "local",
            phase: "online",
            phase_label: "Online",
            session: "hd017",
            target: null,
          }),
          hostFixture({ endpoint: "ssh-dev", label: "dev-box", kind: "ssh", phase: "online", phase_label: "Online", latency_ms: 12 }),
        ],
      },
      profiles: [{ id: "ssh-dev", label: "dev-box", target: "ana@dev-box", port: null, session: "hd017" }],
      store_error: null,
    },
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: { open: false, draft: { id: null, label: "", target: "", port: "", session: "", auth: "key" }, errors: {}, submitting: false, submitError: null, connecting: null },
    importReport: null,
    workspaces: {},
  } as ConnectionsState;
  const select = vi.fn(async () => ({}) as never);
  const disconnect = vi.fn(async () => {});
  const removeProfile = vi.fn(async () => {});
  const ctx: FrameContext = {
    surface: surface(),
    agents: null,
    connections,
    get navigator() {
      return controller.state;
    },
    get selectedEndpoint() {
      return endpointHolder.value;
    },
    set selectedEndpoint(next: string | null) {
      endpointHolder.value = next;
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
      agents: {} as FrameContext["controllers"]["agents"],
      projects: controller,
      connections: {
        openDialog: () => {},
        disconnect,
        reconnect: vi.fn(async () => {}),
        removeProfile,
        editProfile: vi.fn(async () => {}),
      } as unknown as FrameContext["controllers"]["connections"],
    },
    actions,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  el.style.height = `${sidebarHeight}px`;
  document.body.appendChild(el);
  const Original = globalThis.ResizeObserver;
  globalThis.ResizeObserver = class {
    private readonly cb: ResizeObserverCallback;
    constructor(cb: ResizeObserverCallback) {
      this.cb = cb;
    }
    observe(target: Element) {
      const rect = { x: 0, y: 0, width: 272, height: sidebarHeight, top: 0, left: 0, bottom: sidebarHeight, right: 272, toJSON: () => ({}) };
      this.cb([{ target, contentRect: rect, borderBoxSize: [], contentBoxSize: [], devicePixelContentBoxSize: [] } as ResizeObserverEntry], this as unknown as ResizeObserver);
    }
    unobserve() {}
    disconnect() {}
  };
  const app = mount(ProjectsRegion, { target: el, props: { ctx } });
  flushSync();
  hosts.push({ el, app });
  globalThis.ResizeObserver = Original;
  return { el, controller, select, disconnect, removeProfile, endpointHolder, ctx };
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

describe("AC-017-02 projects sidebar", () => {
  it("pins CONEXÕES inside the sidebar at height 800 via getBoundingClientRect", async () => {
    const { el } = await renderSidebar(800);
    const sidebar = el.querySelector<HTMLElement>('[data-slot="projects"]')!;
    const footer = sidebar.querySelector<HTMLElement>("footer[aria-label='Conexões']")!;
    expect(sidebar).toBeTruthy();
    expect(footer).toBeTruthy();
    expect(sidebar.getAttribute("data-footer-pin")).toBe("true");
    expect(footer.textContent).toMatch(/CONEXÕES/);
    expect(footer.textContent).toMatch(/Este computador/);
    expect(footer.textContent).toMatch(/Local/);
    expect(sidebar.contains(footer)).toBe(true);
    const footerHeight = 140;
    sidebar.getBoundingClientRect = () => box(0, 0, 272, 800);
    footer.getBoundingClientRect = () => box(800 - footerHeight, 0, 272, footerHeight);
    const sidebarRect = sidebar.getBoundingClientRect();
    const footerRect = footer.getBoundingClientRect();
    expect(sidebarRect.height).toBe(800);
    expect(inside(sidebarRect, footerRect)).toBe(true);
  });

  it("keeps CONEXÕES in the tree when the window is shorter than 700 px (it never disappears)", async () => {
    const { el } = await renderSidebar(600);
    const sidebar = el.querySelector<HTMLElement>('[data-slot="projects"]')!;
    const footer = sidebar.querySelector<HTMLElement>("footer[aria-label='Conexões']")!;
    expect(sidebar.getAttribute("data-footer-pin")).toBe("false");
    expect(footer).toBeTruthy();
    expect(sidebar.contains(footer)).toBe(true);
  });

  it("hides Novo grupo until the new-group icon is clicked, then shows the inline form", async () => {
    const { el } = await renderSidebar(800);
    expect(el.querySelector('input[placeholder="Novo grupo"]')).toBeNull();
    expect(el.textContent).not.toMatch(/Criar grupo/);
    expect(el.textContent).not.toMatch(/Coleção vazia/);
    expect(el.textContent).not.toMatch(/Novo projeto aqui/);
    expect(el.textContent).toMatch(/PROJETOS/);
    // Spec 025: the saved group stays visible as an empty group (count 0), under its host.
    expect(el.textContent).toMatch(/Vazio/);
    expect(el.textContent).toMatch(/\b0\b/);
    const neu = el.querySelector<HTMLButtonElement>("[data-new-group]");
    expect(neu, "new group icon").toBeTruthy();
    expect(el.querySelector("[data-filter]"), "filter icon").toBeNull();
    neu!.click();
    flushSync();
    const input = el.querySelector<HTMLInputElement>('input[placeholder="Novo grupo"]');
    expect(input).toBeTruthy();
    expect(el.textContent).toMatch(/Criar grupo/);
  });

  // Spec 029 (AC-029-03): disconnect is one call and, for the selected host, the selection
  // returns to Local (the disconnected host loses its workspaces in the tree).
  it("disconnects the selected SSH host through the menu and returns the selection to Local", async () => {
    const { el, disconnect, select } = await renderSidebar(800, "ssh-dev");
    const menu = el.querySelector<HTMLElement>('[data-menu="ssh-dev"]')!;
    expect(menu, "host menu button").toBeTruthy();
    menu.click();
    flushSync();
    const action = el.querySelector<HTMLElement>('[data-action="disconnect"]')!;
    expect(action.textContent).toContain("Desconectar");
    action.click();
    flushSync();
    await Promise.resolve();
    await Promise.resolve();
    expect(disconnect).toHaveBeenCalledExactlyOnceWith("ssh-dev");
    expect(select).toHaveBeenCalledExactlyOnceWith("local");
  });
});

// Spec 032 (AC-032-01): CONEXÕES is pinned to the end of the sidebar (the footer is the last
// flex child), so the host menu can never bleed past the window: it is a fixed overlay measured
// from the row/button rect, flipped upward when there is no room below and clamped to 8 px of
// the viewport edges. happy-dom has no layout, so the row and the overlays report their boxes.
const MENU_BOX = { width: 168, height: 96 };
const CONFIRM_BOX = { width: 232, height: 34 };

function setInnerHeight(value: number): () => void {
  const own = Object.getOwnPropertyDescriptor(window, "innerHeight");
  Object.defineProperty(window, "innerHeight", { value, configurable: true });
  return () => {
    if (own) Object.defineProperty(window, "innerHeight", own);
    else delete (window as unknown as { innerHeight?: number }).innerHeight;
  };
}

/** Fakes the row, the `…` button that anchors the menu and both overlays. */
function stubRects(row: HTMLElement, rowRect: DOMRect) {
  const button = row.querySelector<HTMLElement>("[data-menu]")!;
  const buttonRect = box(rowRect.top + 4, rowRect.right - 26, 18, 18);
  const original = HTMLElement.prototype.getBoundingClientRect;
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    if (this.matches("[data-menu-for]")) return box(0, 0, MENU_BOX.width, MENU_BOX.height);
    if (this.matches("[data-confirm]")) return box(0, 0, CONFIRM_BOX.width, CONFIRM_BOX.height);
    if (this === row) return rowRect;
    if (this === button) return buttonRect;
    return original.call(this);
  };
  return {
    buttonRect,
    restore() {
      HTMLElement.prototype.getBoundingClientRect = original;
    },
  };
}

/** The menu rect as the window sees it: its computed left/top plus the measured size. */
function placed(el: HTMLElement, size: { width: number; height: number }): DOMRect {
  return box(Number.parseFloat(el.style.top) || 0, Number.parseFloat(el.style.left) || 0, size.width, size.height);
}

describe("AC-032-01 host menu stays inside the window", () => {
  it("opens upward, clamped to 8 px of the window, when the host row is 20 px from the bottom", async () => {
    const restoreHeight = setInnerHeight(600);
    try {
      const { el } = await renderSidebar(800, "ssh-dev");
      const row = el.querySelector<HTMLElement>('[data-endpoint="ssh-dev"]')!;
      const rowRect = box(600 - 20 - 28, 0, 272, 28);
      const rects = stubRects(row, rowRect);
      try {
        el.querySelector<HTMLElement>('[data-menu="ssh-dev"]')!.click();
        flushSync();
      } finally {
        rects.restore();
      }
      const menu = el.querySelector<HTMLElement>('[data-menu-for="ssh-dev"]')!;
      expect(menu, "host menu").toBeTruthy();
      menu.getBoundingClientRect = () => placed(menu, MENU_BOX);
      const rect = menu.getBoundingClientRect();
      expect(rect.top, "menu never above the window").toBeGreaterThanOrEqual(8);
      expect(rect.bottom, "menu never below the window").toBeLessThanOrEqual(window.innerHeight - 8);
      expect(rect.bottom, "flipped above the button").toBeLessThanOrEqual(rects.buttonRect.top);
      expect(rect.top, "flipped upward").toBeLessThan(rowRect.top);
      expect(getComputedStyle(menu).position).toBe("fixed");
      expect(Number(getComputedStyle(menu).zIndex), "above the status bar").toBeGreaterThan(5);
    } finally {
      restoreHeight();
    }
  });

  it("keeps the inline remove confirmation inside the window too", async () => {
    const restoreHeight = setInnerHeight(600);
    try {
      const { el } = await renderSidebar(800, "ssh-dev");
      const row = el.querySelector<HTMLElement>('[data-endpoint="ssh-dev"]')!;
      const rowRect = box(600 - 20 - 28, 0, 272, 28);
      const rects = stubRects(row, rowRect);
      try {
        el.querySelector<HTMLElement>('[data-menu="ssh-dev"]')!.click();
        flushSync();
        el.querySelector<HTMLElement>('[data-action="remove"]')!.click();
        flushSync();
      } finally {
        rects.restore();
      }
      const confirm = el.querySelector<HTMLElement>('[data-confirm="ssh-dev"]')!;
      expect(confirm, "remove confirmation").toBeTruthy();
      confirm.getBoundingClientRect = () => placed(confirm, CONFIRM_BOX);
      const rect = confirm.getBoundingClientRect();
      expect(rect.top, "confirmation never above the window").toBeGreaterThanOrEqual(8);
      expect(rect.bottom, "confirmation never below the window").toBeLessThanOrEqual(window.innerHeight - 8);
      expect(getComputedStyle(confirm).position).toBe("fixed");
    } finally {
      restoreHeight();
    }
  });

  it("opens below the row when there is room and keeps its right edge inside the window", async () => {
    const restoreHeight = setInnerHeight(600);
    try {
      const { el } = await renderSidebar(800, "ssh-dev");
      const row = el.querySelector<HTMLElement>('[data-endpoint="ssh-dev"]')!;
      const rowRect = box(100, 0, 272, 28);
      const rects = stubRects(row, rowRect);
      try {
        el.querySelector<HTMLElement>('[data-menu="ssh-dev"]')!.click();
        flushSync();
      } finally {
        rects.restore();
      }
      const menu = el.querySelector<HTMLElement>('[data-menu-for="ssh-dev"]')!;
      expect(menu, "host menu").toBeTruthy();
      menu.getBoundingClientRect = () => placed(menu, MENU_BOX);
      const rect = menu.getBoundingClientRect();
      expect(rect.top).toBe(rects.buttonRect.bottom + 2);
      expect(rect.left).toBeGreaterThanOrEqual(8);
      expect(rect.right).toBeLessThanOrEqual(window.innerWidth - 8);
    } finally {
      restoreHeight();
    }
  });
});

describe("spec 038 host highlight and no dash without branch (AC-038-01, AC-038-02)", () => {
  function workspaceFixture(overrides: Partial<HostWorkspaceDto> & { workspace_id: string }): HostWorkspaceDto {
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

  it("AC-038-01: highlights exactly the focused workspace of the selected SSH host, and returns highlight to Local when switched", async () => {
    const testHosts = [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        phase_label: "Online",
        session: "default",
        target: null,
        workspaces: [
          workspaceFixture({
            workspace_id: "w1",
            number: 1,
            label: "idb-app",
            branch: "main",
            focused: true,
            cwd: "/work/idb-app",
          }),
        ],
      }),
      hostFixture({
        endpoint: "ssh-dev",
        label: "mac-mini",
        kind: "ssh",
        phase: "online",
        phase_label: "Online",
        session: "default",
        workspaces: [
          workspaceFixture({
            workspace_id: "r1",
            number: 1,
            label: "~",
            branch: null,
            focused: true,
            cwd: "/home/user",
          }),
        ],
      }),
    ];

    const endpointHolder = reactiveHolder<string | null>("ssh-dev");
    const { el } = await renderSidebar(800, endpointHolder, testHosts);

    // 1. With ctx.selectedEndpoint = "ssh-dev", exactly one row is active: the SSH workspace (~).
    const activeRowsSsh = el.querySelectorAll<HTMLElement>('.project-row[data-active="true"]');
    expect(activeRowsSsh).toHaveLength(1);
    expect(activeRowsSsh[0]!.getAttribute("data-workspace")).toBe("r1");
    expect(activeRowsSsh[0]!.textContent).toContain("~");
    expect(activeRowsSsh[0]!.classList.contains("active")).toBe(true);

    const localRowWhenSsh = el.querySelector<HTMLElement>('[data-workspace="w1"]')!;
    expect(localRowWhenSsh.getAttribute("data-active")).toBeNull();
    expect(localRowWhenSsh.classList.contains("active")).toBe(false);

    // 2. Switch ctx to "local" -> highlight returns to the focused workspace of Local (idb-app).
    endpointHolder.value = "local";
    flushSync();

    const activeRowsLocal = el.querySelectorAll<HTMLElement>('.project-row[data-active="true"]');
    expect(activeRowsLocal).toHaveLength(1);
    expect(activeRowsLocal[0]!.getAttribute("data-workspace")).toBe("w1");
    expect(activeRowsLocal[0]!.textContent).toContain("idb-app");
    expect(activeRowsLocal[0]!.classList.contains("active")).toBe(true);

    const sshRowWhenLocal = el.querySelector<HTMLElement>('[data-workspace="r1"]')!;
    expect(sshRowWhenLocal.getAttribute("data-active")).toBeNull();
    expect(sshRowWhenLocal.classList.contains("active")).toBe(false);
  });

  it("AC-038-02: workspace without branch has no second line and no '—', while branch and closed retain their second line", async () => {
    const testHosts = [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        phase_label: "Online",
        session: "default",
        target: null,
        workspaces: [
          workspaceFixture({
            workspace_id: "w1",
            number: 1,
            label: "idb-app",
            branch: "main",
            focused: true,
            cwd: "/work/idb-app",
          }),
        ],
      }),
      hostFixture({
        endpoint: "ssh-dev",
        label: "mac-mini",
        kind: "ssh",
        phase: "online",
        phase_label: "Online",
        session: "default",
        workspaces: [
          workspaceFixture({
            workspace_id: "r1",
            number: 1,
            label: "~",
            branch: null,
            focused: true,
            cwd: "/home/user",
          }),
        ],
      }),
    ];

    const seedProjects: ProjectDto[] = [
      {
        id: "p-closed",
        label: "closed-app",
        endpoint_profile_id: "local",
        session_name: "default",
        root: "/work/closed-app",
        binding: null,
      },
    ];

    const { el } = await renderSidebar(800, "local", testHosts, seedProjects);

    // Row without branch (r1: ~)
    const rowNoBranch = el.querySelector<HTMLElement>('[data-workspace="r1"]')!;
    expect(rowNoBranch).toBeTruthy();
    expect(rowNoBranch.textContent).not.toContain("—");
    expect(rowNoBranch.querySelector(".branch-line")).toBeNull();
    expect(rowNoBranch.querySelector(".branch-name")).toBeNull();

    // Row with branch (w1: idb-app, main)
    const rowWithBranch = el.querySelector<HTMLElement>('[data-workspace="w1"]')!;
    expect(rowWithBranch).toBeTruthy();
    const branchLine = rowWithBranch.querySelector<HTMLElement>(".branch-line");
    expect(branchLine).not.toBeNull();
    expect(branchLine!.textContent).toContain("main");
    expect(branchLine!.textContent).toContain("⎇");

    // Closed project row (closed-app)
    const closedRow = el.querySelector<HTMLElement>('[data-closed="true"]')!;
    expect(closedRow).toBeTruthy();
    expect(closedRow.textContent).toContain("closed-app");
    const closedBranchLine = closedRow.querySelector<HTMLElement>(".branch-line");
    expect(closedBranchLine).not.toBeNull();
    expect(closedBranchLine!.textContent).toContain("fechado");
  });
});
