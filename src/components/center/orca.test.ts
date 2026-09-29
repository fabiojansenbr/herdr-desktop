// @vitest-environment happy-dom
// Spec 019 — Orca tabs (AC-019-01) and frameless panes (AC-019-02). Would catch: a tab without
// the status/glyph/title/× order, + that does not open Novo agente, Comando that does not fire,
// a wrap border around the TUI, or split buttons missing from the content.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { AgentsState } from "../../agents/reducer";
import { hostFixture } from "../../connections/fake-bridge";
import { reactiveHolder } from "./reactive-test-state.svelte";
import type { MenuAction } from "../frame/menus";
import type { FrameContext } from "../../shell/frame-context";
import type { SurfaceState } from "../../shell/controller";
import WorkspaceTabs from "./WorkspaceTabs.svelte";
import PaneFrames from "./PaneFrames.svelte";
import NewAgentPopover from "./NewAgentPopover.svelte";

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];
const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

function liveSurface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "default", online: true, identity: null },
    status: { session: "default", session_available: true, connected: true, state: "live", reason: null, generation: null, connection_generation: 1, boot_id: "boot-019", server_version: "0.9.0", pane_id: "w1:p1", last_error: null },
    phase: "live",
    reason: null,
    error: null,
    identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-019", pane_id: "w1:p1" },
    surfaceKey: 1,
    busy: false,
  };
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

async function tabCtx(
  opts: {
    agents?: [string, "idle" | "working" | "blocked"][];
    kinds?: string[];
    agentKind?: string;
    tabLabel?: string;
    paneCount?: number;
    tabs?: import("../../agents/types").TabDto[];
    connections?: unknown;
    missing?: ("rename_tab" | "close_tab" | "focus_tab" | "create_tab")[];
    /** Wires onChange to a rune so bridge events re-render the component (App's own wiring). */
    reactive?: boolean;
  } = {},
) {
  const bridge = createFakeAgentsBridge({
    bootId: "boot-019",
    generation: 1,
    session: "default",
    kinds: opts.kinds ?? ["claude", "codex"],
    agents: opts.agents ?? [["w1:p1", "idle"]],
    agentKind: opts.agentKind,
    tabLabel: opts.tabLabel,
    paneCount: opts.paneCount,
    tabs: opts.tabs,
    missing: opts.missing,
  });
  const holder = reactiveHolder<AgentsState | null>(null);
  const controller = createAgentsController(
    bridge,
    opts.reactive ? (next) => (holder.value = next) : undefined,
  );
  await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  bridge.calls.length = 0;
  const openPalette = vi.fn();
  const ctx: FrameContext = {
    surface: liveSurface(),
    get agents() {
      return opts.reactive ? holder.value : controller.state;
    },
    connections: (opts.connections ?? null) as FrameContext["connections"],
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
    actions: { ...ACTIONS, openPalette, newTab: () => void controller.createTab() },
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  return { ctx, bridge, controller, openPalette, holder };
}

describe("Orca workspace tabs (AC-019-01)", () => {
  it("each tab shows status icon, glyph, title and × on the active tab; + creates a tab and Comando is absent", async () => {
    const { ctx, bridge } = await tabCtx({ agents: [["w1:p1", "idle"]] });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });

    const tab = el.querySelector<HTMLElement>("[data-tab]");
    expect(tab).toBeTruthy();
    expect(tab!.querySelector("[data-tab-icon]")?.getAttribute("data-tab-icon")).toBe("idle");
    expect(tab!.querySelector("[data-glyph]")?.textContent?.trim()).toBe("p");
    expect(tab!.querySelector("[data-label]")?.textContent?.trim()).toBe("principal");
    expect(tab!.getAttribute("title")).toBe("principal · pi · Ocioso");
    const close = tab!.querySelector<HTMLButtonElement>("[data-close-tab]");
    expect(close, "× on the active tab").toBeTruthy();
    close!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_close")).toHaveLength(1);
    });

    // Spec 028 AC-028-01: + creates one tab in the focused workspace; Novo agente stays in the
    // top bar and nothing is split.
    bridge.calls.length = 0;
    el.querySelector<HTMLButtonElement>("[data-new-tab]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1);
    });
    expect(bridge.calls.filter((c) => c.command === "pane_split")).toHaveLength(0);
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();

    expect(el.querySelector("[data-command]")).toBeNull();
  });

  it("× is absent from an inactive tab until hover; title truncates", async () => {
    const { ctx } = await tabCtx();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    const style = el.querySelector("style") ? "" : document.documentElement.innerHTML;
    expect(document.querySelector("[data-center-tabs]") || el.querySelector("[data-center-tabs]")).toBeTruthy();
    const source = el.innerHTML;
    expect(source).toMatch(/data-close-tab/);
  });
});

describe("engine tab labels (AC-023-01)", () => {
  // Would catch: "Grok" shown as the tab title instead of TabDto.label "workers".
  it("a tab labelled workers with a grok agent shows workers and a Grok tooltip", async () => {
    const { ctx } = await tabCtx({ agentKind: "grok", tabLabel: "workers", paneCount: 2 });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    const tab = el.querySelector<HTMLElement>("[data-tab]");
    expect(tab!.querySelector("[data-label]")?.textContent?.trim()).toBe("workers");
    expect(tab!.querySelector("[data-glyph]")?.textContent?.trim()).toBe("g");
    expect(tab!.getAttribute("title")).toBe("workers · grok · Ocioso · shell · Ocioso");
  });
});

describe("tab title from the open app (AC-026-01, AC-026-02, AC-026-03)", () => {
  // Would catch: an unnamed tab still showing the engine number, the numeric pane count still on
  // screen, the title not following an agent detected/renamed live (24/7 `pane.read` polling), or
  // the +N of a multi-pane title being swallowed by the ellipsis.
  it("an unnamed tab shows the pane's app and follows a live agent detect without pane.read", async () => {
    const { ctx, bridge } = await tabCtx({ agents: [], tabLabel: "1", paneCount: 1, reactive: true });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    const label = () => el.querySelector<HTMLElement>("[data-label]")!.textContent?.trim();
    // A pane the engine listed only with its own title (no detected agent yet): `zsh`.
    const shell = { ...bridge.agent("w1:p1", "idle"), kind: null, terminal_title: "zsh" };
    bridge.emit({ type: "agents", agents: [shell] });
    flushSync();
    expect(label()).toBe("zsh");
    bridge.emit({ type: "agents", agents: [{ ...shell, kind: "claude" }] });
    flushSync();
    expect(label()).toBe("claude");
    // The agent ended (the engine drops it from `agent.list`): the title follows the metadata.
    bridge.emit({ type: "agents", agents: [] });
    flushSync();
    expect(label()).toBe("shell");
    expect(bridge.calls.map((c) => c.command)).not.toContain("pane_read");
  });

  it("a renamed tab takes over immediately, before the panes", async () => {
    const { ctx, bridge } = await tabCtx({ agents: [], tabLabel: "1", paneCount: 1, reactive: true });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    const label = () => el.querySelector<HTMLElement>("[data-label] .names")?.textContent?.trim();
    bridge.emit({ type: "agents", agents: [{ ...bridge.agent("w1:p1", "idle"), kind: "claude" }] });
    flushSync();
    expect(label()).toBe("claude");
    bridge.emit({
      type: "tabs",
      tabs: [{ tab_id: "w1:t1", workspace_id: "w1", label: "squad", number: 1, focused: true, pane_count: 1, agent_status: "idle" }],
    });
    flushSync();
    expect(label()).toBe("squad");
    expect(el.querySelector<HTMLElement>("[data-tab]")!.getAttribute("title")).toContain("claude");
  });

  it("lists two layout names and keeps +N visible; a named tab keeps its label with panes in the tooltip", async () => {
    const { ctx, bridge } = await tabCtx({ agents: [], tabLabel: "1", paneCount: 4, reactive: true });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    const kinds = ["claude", "codex", "grok", "pi"];
    bridge.emit({ type: "agents", agents: kinds.map((kind, i) => ({ ...bridge.agent(`w1:p${i + 1}`, "idle"), kind })) });
    flushSync();
    const tab = el.querySelector<HTMLElement>("[data-tab]")!;
    expect(tab.querySelector("[data-label] .names")?.textContent?.trim()).toBe("claude · codex");
    expect(tab.querySelector("[data-more]")?.textContent?.trim()).toBe("+2");
    expect(tab.querySelector("[data-panes]"), "the numeric count left the title").toBeNull();
    expect(tab.getAttribute("title")).toBe("claude · Ocioso · codex · Ocioso · grok · Ocioso · pi · Ocioso");
  });

  it("a named tab keeps the 023 label and shows every pane in the tooltip", async () => {
    const { ctx, bridge } = await tabCtx({ agents: [], tabLabel: "workers", paneCount: 2, reactive: true });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    bridge.emit({
      type: "agents",
      agents: [
        { ...bridge.agent("w1:p1", "blocked"), kind: "opencode" },
        { ...bridge.agent("w1:p2", "idle"), kind: "claude" },
      ],
    });
    flushSync();
    const tab = el.querySelector<HTMLElement>("[data-tab]")!;
    expect(tab.querySelector("[data-label] .names")?.textContent?.trim()).toBe("workers");
    expect(tab.querySelector("[data-more]")).toBeNull();
    expect(tab.getAttribute("title")).toBe("workers · opencode · Aguardando você · claude · Ocioso");
  });
});

describe("rename tab like the TUI (AC-023-02)", () => {
  // Would catch: two IPC calls, Escape still sending, or a rename that bypasses tab.rename.
  it("Renomear then Enter calls tab_rename once; Escape does not call", async () => {
    const { ctx, bridge } = await tabCtx({ tabLabel: "1" });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });

    // Spec 028: the `…` button is gone; Renomear comes from the tab context menu.
    const tab = el.querySelector<HTMLElement>("[data-tab]")!;
    tab.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 10 }));
    flushSync();
    el.querySelector<HTMLButtonElement>("[data-menu-item='rename']")!.click();
    flushSync();
    const input = el.querySelector<HTMLInputElement>("[data-tab-rename]");
    expect(input, "inline rename field").toBeTruthy();
    input!.value = "squad";
    input!.dispatchEvent(new InputEvent("input", { bubbles: true }));
    input!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_rename")).toEqual([
        {
          command: "tab_rename",
          args: {
            target: {
              endpoint: "local",
              session: "default",
              connection_generation: 1,
              boot_id: "boot-019",
              workspace_id: "w1",
              pane_id: "w1:p1",
            },
            tabId: "w1:t1",
            label: "squad",
          },
        },
      ]);
    });

    bridge.calls.length = 0;
    tab.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    flushSync();
    const again = el.querySelector<HTMLInputElement>("[data-tab-rename]")!;
    again.value = "nope";
    again.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();
    expect(bridge.calls.filter((c) => c.command === "tab_rename")).toEqual([]);
    expect(el.querySelector("[data-tab-rename]")).toBeNull();
  });

  it("without tab.rename the Renomear item is disabled and explains why", async () => {
    const { ctx, bridge } = await tabCtx({ missing: ["rename_tab"] });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    el.querySelector<HTMLElement>("[data-tab]")!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 10 }),
    );
    flushSync();
    const item = el.querySelector<HTMLButtonElement>("[data-menu-item='rename']")!;
    expect(item.getAttribute("aria-disabled")).toBe("true");
    expect(item.getAttribute("title")).toMatch(/renomear/i);
    item.click();
    flushSync();
    expect(el.querySelector("[data-tab-rename]")).toBeNull();
    expect(bridge.calls.filter((c) => c.command === "tab_rename")).toEqual([]);
  });
});

describe("frameless pane content (AC-019-02)", () => {
  it("has no wrap radius/border/header; split buttons sit on the content; geometry stays inner_rect", async () => {
    const { ctx, bridge } = await tabCtx();
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
    const mounted: FrameContext = {
      ...ctx,
      controllers: {
        ...ctx.controllers,
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
      },
    };
    const Original = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as typeof ResizeObserver;
    const app = mount(PaneFrames, { target: el, props: { ctx: mounted, stage } });
    flushSync();
    hosts.push({ el, app });
    globalThis.ResizeObserver = Original;

    const frame = el.querySelector<HTMLElement>("[data-pane-frame]");
    expect(frame, "geometry box").toBeTruthy();
    expect(el.querySelector("[data-pane-band]")).toBeNull();
    expect(el.querySelector(".band, .name")).toBeNull();
    expect(el.querySelector("[data-split-right]")).toBeTruthy();
    expect(el.querySelector("[data-split-down]")).toBeTruthy();
    el.querySelector<HTMLButtonElement>("[data-split-right]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "pane_split")).toEqual([
        expect.objectContaining({ command: "pane_split", args: expect.objectContaining({ direction: "right" }) }),
      ]);
    });
    expect(NewAgentPopover).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------------------
// Spec 025 AC-025-04 — the bar belongs to the focused workspace, not the session: 4 tabs of 2
// workspaces must never render together, and a focus change replaces the whole bar.
// ---------------------------------------------------------------------------------------

const SCOPED_TABS: import("../../agents/types").TabDto[] = [
  { tab_id: "w1:t1", workspace_id: "w1", label: "api", number: 1, focused: true, pane_count: 2, agent_status: "idle" },
  { tab_id: "w1:t2", workspace_id: "w1", label: "workers", number: 2, focused: false, pane_count: 1, agent_status: "idle" },
  { tab_id: "w2:t1", workspace_id: "w2", label: "site", number: 1, focused: true, pane_count: 3, agent_status: "idle" },
  { tab_id: "w2:t2", workspace_id: "w2", label: "deploy", number: 2, focused: false, pane_count: 1, agent_status: "idle" },
];

function scopedHosts(focused: "w1" | "w2") {
  return [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "default",
      target: null,
      workspaces: ["w1", "w2"].map((workspace_id, index) => ({
        workspace_id,
        number: index + 1,
        label: workspace_id === "w1" ? "api" : "site",
        focused: workspace_id === focused,
        tab_count: 2,
        pane_count: 1,
        active_tab_id: `${workspace_id}:t1`,
        agent_status: "idle" as const,
        cwd: `/srv/${workspace_id}`,
        branch: null,
      })),
    }),
  ];
}

describe("focused workspace scopes the tab bar (AC-025-04)", () => {
  it("shows only the confirmed workspace's tabs and replaces the bar when the focus moves", async () => {
    const { ctx, bridge } = await tabCtx({ tabs: SCOPED_TABS, agents: [["w1:p1", "idle"]], reactive: true });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    const ids = () => Array.from(el.querySelectorAll<HTMLElement>("[data-tab]")).map((node) => node.getAttribute("data-tab"));

    expect(ids()).toHaveLength(4);
    const focus = (tab_id: string, revision: number) => ({
      endpoint: "local",
      session: "default",
      connection_generation: 1,
      boot_id: "boot-019",
      revision,
      tab_id,
    });
    bridge.emit({ type: "tab_focus", focus: focus("w2:t1", 2) });
    flushSync();
    expect(ids()).toEqual(["w2:t1", "w2:t2"]);
    bridge.emit({ type: "tab_focus", focus: focus("w1:t2", 3) });
    flushSync();
    expect(ids()).toEqual(["w1:t1", "w1:t2"]);
  });

  it("the selected host's snapshot scopes the bar even before a tab focus is confirmed", async () => {
    const { ctx } = await tabCtx({ tabs: SCOPED_TABS, agents: [["w1:p1", "idle"]] });
    (ctx as { connections: unknown }).connections = {
      view: { hub: { revision: 1, hosts: scopedHosts("w2") }, profiles: [], store_error: null },
    };
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });
    expect(Array.from(el.querySelectorAll<HTMLElement>("[data-tab]")).map((node) => node.getAttribute("data-tab"))).toEqual([
      "w2:t1",
      "w2:t2",
    ]);
  });
});

describe("snapshot tabs for hosts without API lane (AC-039-01, AC-039-02)", () => {
  it("host sem api: barra com as abas do snapshot, status 2 panes · 1 aba", async () => {
    // Host without API lane has empty bridge tabs (tab.list unsupported),
    // but the host snapshot carries the workspace tabs and panes.
    const { ctx } = await tabCtx({ tabs: [], agents: [] });
    const hostWithoutApi = hostFixture({
      endpoint: "mac-mini",
      label: "mac-mini",
      kind: "ssh",
      api: false,
      workspaces: [
        {
          workspace_id: "w1",
          number: 1,
          label: "herdr",
          focused: true,
          tab_count: 1,
          pane_count: 2,
          active_tab_id: "w1:t1",
          agent_status: "working" as const,
          cwd: "/srv/herdr",
          branch: "main",
        },
      ],
      tabs: [
        {
          tab_id: "w1:t1",
          workspace_id: "w1",
          number: 1,
          label: "main",
          custom_label: false,
          focused: true,
          zoomed: false,
          pane_count: 2,
          agent_status: "working",
        },
      ],
      panes: [
        {
          pane_id: "w1:p1",
          workspace_id: "w1",
          tab_id: "w1:t1",
          focused: true,
          input_enabled: true,
          input_block: null,
          target: null,
          cwd: "/srv/herdr",
          foreground_cwd: "/srv/herdr/src",
          title: "Editor",
          terminal_title: "Editor",
          agent: "claude",
          agent_status: "working",
        },
        {
          pane_id: "w1:p2",
          workspace_id: "w1",
          tab_id: "w1:t1",
          focused: false,
          input_enabled: true,
          input_block: null,
          target: null,
          cwd: "/srv/herdr",
          foreground_cwd: "/srv/herdr",
          title: "Shell",
          terminal_title: "Shell",
          agent: null,
          agent_status: "idle",
        },
      ],
      agents: [
        {
          pane_id: "w1:p1",
          workspace_id: "w1",
          tab_id: "w1:t1",
          name: "claude",
          agent: "claude",
          display_agent: "claude",
          title: "Editor",
          terminal_title: "Editor",
          terminal_title_stripped: "Editor",
          agent_status: "working",
          focused: true,
        },
      ],
    });

    (ctx as { selectedEndpoint: string }).selectedEndpoint = "mac-mini";
    (ctx as { connections: unknown }).connections = {
      view: { hub: { revision: 1, hosts: [hostWithoutApi] }, profiles: [], store_error: null },
    };

    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });

    // AC-039-01: tab bar displays snapshot tab
    const tabEl = el.querySelector<HTMLElement>("[data-tab='w1:t1']");
    expect(tabEl, "snapshot tab w1:t1 is rendered").not.toBeNull();
    expect(tabEl?.getAttribute("data-active")).toBe("true");

    // AC-039-01: status bar counts for the focused workspace: 2 panes · 1 aba
    const { statusBarModel } = await import("../frame/status");
    const status = statusBarModel({
      phase: "live",
      hasEndpoint: true,
      serverVersion: "0.9.0",
      branch: "main",
      panes: hostWithoutApi.panes.filter((p) => p.workspace_id === "w1").length,
      tabs: (hostWithoutApi.tabs ?? []).filter((t) => t.workspace_id === "w1").length,
    });
    expect(status.counts).toBe("2 panes · 1 aba");
  });

  it("AC-039-02: unannounced methods are aria-disabled with title 'indisponível neste host'", async () => {
    const { ctx, bridge } = await tabCtx({ missing: ["rename_tab", "close_tab", "create_tab"] });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });

    // 1. New tab button is aria-disabled and explains why
    const newTabBtn = el.querySelector<HTMLButtonElement>("[data-new-tab]");
    expect(newTabBtn?.getAttribute("aria-disabled")).toBe("true");
    expect(newTabBtn?.getAttribute("title")?.toLowerCase()).toContain("indisponível neste host");

    // 2. Close tab button is aria-disabled and explains why
    const closeTabBtn = el.querySelector<HTMLButtonElement>("[data-close-tab]");
    expect(closeTabBtn?.getAttribute("aria-disabled")).toBe("true");
    expect(closeTabBtn?.getAttribute("title")?.toLowerCase()).toContain("indisponível neste host");

    // 3. Tab context menu: rename is disabled with reason explaining why
    const tabEl = el.querySelector<HTMLElement>("[data-tab]")!;
    tabEl.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 10 }));
    flushSync();

    const renameItem = el.querySelector<HTMLButtonElement>("[data-menu-item='rename']");
    expect(renameItem?.getAttribute("aria-disabled")).toBe("true");
    expect(renameItem?.getAttribute("title")?.toLowerCase()).toContain("indisponível neste host");

    // Clicking disabled item does not trigger rename
    renameItem?.click();
    flushSync();
    expect(el.querySelector("[data-tab-rename]")).toBeNull();
    expect(bridge.calls.filter((c) => c.command === "tab_rename")).toHaveLength(0);
  });
});

