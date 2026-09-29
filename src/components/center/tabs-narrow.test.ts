// @vitest-environment happy-dom
// Spec 028 AC-028-01/02/03 — workspace tabs: width follows the content (64..280 px, 10 px
// padding), the `…` button is gone, `+` creates one tab in the focused workspace (never a
// split or the Novo agente popover), and right click opens the tab menu (`Nova aba`,
// `Renomear`, `Fechar`; only `Nova aba` on the bar outside a tab).
// Would catch: a fixed min-width coming back (labels ellipsized while there is room), the title
// growing past 280 px, `+` opening the popover or splitting, a menu item firing two actions, or
// the wheel/scrollIntoView behavior of the strip regressing.
//
// Spec 077 AC-077-02/03 — the × (and Fechar) of a tab the bar lists from the host snapshot goes
// through `host_tab_close` on that host, not through `agents.closeTab` (which acts only on the
// live list and returned in silence, the reported bug); a backend refusal is shown translated in
// the bar, and an offline host keeps the × disabled with `center.reason.hostOffline`.
import { flushSync, mount, unmount } from "svelte";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { AgentsState } from "../../agents/reducer";
import type { TabDto } from "../../agents/types";
import type { MenuAction } from "../frame/menus";
import { errorText, t } from "../../i18n/index.svelte";
import type { FrameContext } from "../../shell/frame-context";
import type { SurfaceState } from "../../shell/controller";
import { reactiveHolder } from "./reactive-test-state.svelte";
import {
  estimatedTextWidth,
  TAB_GAP_PX,
  TAB_MAX_PX,
  TAB_MIN_PX,
  TAB_PADDING_PX,
  TAB_STATUS_PX,
  tabText,
  tabWidth,
} from "./tabs-narrow";
import WorkspaceTabs from "./WorkspaceTabs.svelte";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto } from "../../connections/types";
import { hostTabClose } from "../../projects/workspace-bridge";

vi.mock("../../projects/workspace-bridge", () => ({ hostTabClose: vi.fn(async () => {}) }));

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];
const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

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

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

function tabsOf(labels: string[]): TabDto[] {
  return labels.map((label, index) => ({
    tab_id: `w1:t${index + 1}`,
    workspace_id: "w1",
    label,
    number: index + 1,
    pane_count: 1,
    focused: index === labels.length - 1,
  }));
}

async function tabsCtx(options: { tabs?: TabDto[]; reactive?: boolean } = {}) {
  const bridge = createFakeAgentsBridge({
    bootId: "boot-028",
    generation: 1,
    session: "default",
    kinds: ["grok", "claude"],
    agents: [["w1:p1", "idle"]],
    agentKind: "grok",
    tabLabel: "1",
    paneCount: 1,
    tabs: options.tabs,
  });
  const holder = reactiveHolder<AgentsState | null>(null);
  const controller = createAgentsController(
    bridge,
    options.reactive ? (next) => (holder.value = next) : undefined,
  );
  await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  bridge.calls.length = 0;
  const ctx: FrameContext = {
    surface: liveSurface(),
    get agents() {
      return options.reactive ? holder.value : controller.state;
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
      surface: {} as FrameContext["controllers"]["surface"],
      agents: controller,
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  return { ctx, bridge };
}

function mountTabs(ctx: FrameContext, width = 1440) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspaceTabs, { target: el, props: { ctx, layoutWidth: width } });
  flushSync();
  hosts.push({ el, app });
  return el;
}

/** The component's authored <style>, as a <style> node: vitest stubs CSS imports, so this is how
 * the test document gets the real rules for `getComputedStyle`. */
function sourceStyle(path: string): HTMLStyleElement {
  const source = readFileSync(resolve(path), "utf8");
  const style = document.createElement("style");
  style.textContent = source.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? "";
  return style;
}

function contextMenu(el: HTMLElement, target: Element, clientX = 40, clientY = 10) {
  target.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX, clientY }));
  flushSync();
  return Array.from(el.querySelectorAll<HTMLElement>("[data-menu-item]")).map((item) => ({
    id: item.dataset.menuItem ?? "",
    label: item.textContent?.trim() ?? "",
  }));
}

describe("tab width follows the content (AC-028-02)", () => {
  // Would catch: a fixed inline width coming back, an inline estimate of a pane-derived title
  // disagreeing with the rendered label, the `…` button or a title growing past 280 px.
  it("lets the CSS measure each tab (max-content, 64..280 px) and drops the `…` menu", async () => {
    const long = "x".repeat(60);
    const { ctx } = await tabsCtx({ tabs: tabsOf(["novo", long]) });
    const el = mountTabs(ctx);
    const bar = el.querySelector<HTMLElement>("[data-center-tabs]")!;
    expect(bar.dataset.tabMin).toBe(String(TAB_MIN_PX));
    expect(bar.dataset.tabMax).toBe(String(TAB_MAX_PX));

    const tabs = Array.from(el.querySelectorAll<HTMLElement>("[data-tab]"));
    expect(tabs).toHaveLength(2);
    const short = tabs[0]!;
    const wide = tabs[1]!;

    // Spec 028 r2 (bug: the renamed tab "vim" collapsed to the ellipsis): no inline width from
    // an estimate of `tab.display`; the CSS measures the same text the tab renders (`tabText`).
    expect(short.getAttribute("style"), "no inline width").toBeNull();
    expect(wide.getAttribute("style"), "no inline width").toBeNull();
    expect(short.querySelector("[data-label] .names")?.textContent?.trim()).toBe("novo");

    // Numeric oracle for the 64..280 px limits: "novo" stays under 136 px (110 px of content plus
    // the × and its gap, which every tab lays out), 60 chars hit the cap.
    const shortWidth = tabWidth("novo", { glyph: true, more: 0 });
    expect(shortWidth).toBeLessThanOrEqual(136);
    expect(shortWidth).toBeGreaterThanOrEqual(TAB_MIN_PX);
    expect(tabWidth(long, { glyph: true, more: 0 })).toBe(TAB_MAX_PX);

    expect(el.querySelector("[data-tab-menu]"), "the … button is gone").toBeNull();
    expect(el.textContent).not.toContain("…");

    // The authored CSS applies the real rules (vitest stubs style imports): the tab measures its
    // content, the label contributes to that measurement and only the 280 px cap shrinks it.
    const styleNode = sourceStyle("src/components/center/WorkspaceTabs.svelte");
    document.head.appendChild(styleNode);
    const tabStyle = getComputedStyle(short);
    expect(tabStyle.width, "the tab is as wide as its content").toBe("max-content");
    expect(tabStyle.minWidth).toBe("64px");
    expect(tabStyle.maxWidth).toBe("280px");
    expect(tabStyle.flexGrow, "a tab never stretches").toBe("0");
    expect(tabStyle.flexShrink, "a tab never shrinks").toBe("0");
    const labelStyle = getComputedStyle(short.querySelector<HTMLElement>("[data-label]")!);
    expect(labelStyle.flexBasis, "the title is not flex-basis 0").not.toBe("0%");
    expect(labelStyle.flexGrow, "the label does not claim the leftover space").toBe("0");
    expect(Number.parseFloat(labelStyle.minWidth), "the label can still shrink at the cap").toBe(0);
    const namesStyle = getComputedStyle(short.querySelector<HTMLElement>(".names")!);
    expect(Number.parseFloat(namesStyle.minWidth)).toBe(0);
    expect(namesStyle.overflow).toBe("hidden");
    expect(namesStyle.textOverflow).toBe("ellipsis");
    expect(namesStyle.whiteSpace).toBe("nowrap");
    const closeStyle = getComputedStyle(short.querySelector<HTMLElement>("[data-close-tab]")!);
    expect(closeStyle.flexShrink, "the × keeps its size when laid out").toBe("0");
    expect(short.classList.contains("active"), "the measured tab is inactive").toBe(false);
    expect(closeStyle.display, "an inactive tab still shows its × without hover").toBe("inline-flex");
    styleNode.remove();

    // Renaming is a double click (or the context menu), never a dedicated button.
    expect(readFileSync(resolve("src/components/center/WorkspaceTabs.svelte"), "utf8")).not.toMatch(/data-tab-menu/);
  });

  // Would catch: measuring a named tab by its pane-derived display (empty or stale) instead of
  // the label it renders — the case that clamped the "vim" tab to the floor and cut the title.
  it("measures a named tab by its label even when the pane-derived display is empty (spec 028 r2)", () => {
    const named = { label: "vim", display: "", named: true };
    expect(tabText(named)).toBe("vim");
    expect(tabText({ label: "1", display: "claude", named: false })).toBe("claude");
    expect(tabWidth(tabText(named), { glyph: false, more: 0 })).toBeGreaterThanOrEqual(TAB_MIN_PX);
    // A title that does not fit the 64 px floor keeps room for the whole text.
    const title = "sessão vim";
    const room =
      tabWidth(tabText({ label: title, display: "", named: true }), { glyph: false, more: 0 }) -
      (TAB_PADDING_PX * 2 + TAB_GAP_PX + TAB_STATUS_PX);
    expect(room).toBeGreaterThanOrEqual(estimatedTextWidth(title));
    // The component renders (and therefore measures) that same text.
    expect(readFileSync(resolve("src/components/center/WorkspaceTabs.svelte"), "utf8")).toContain("tabText(tab)");
  });
});

describe("the + creates a tab (AC-028-01)", () => {
  // Would catch: + opening Novo agente, splitting a pane, or the new tab never becoming active.
  it("sends one tab.create, splits nothing and activates the tab the engine confirms", async () => {
    const { ctx, bridge } = await tabsCtx({ tabs: tabsOf(["principal"]), reactive: true });
    const el = mountTabs(ctx);
    el.querySelector<HTMLButtonElement>("[data-new-tab]")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1);
    });
    expect(bridge.calls.filter((c) => c.command === "pane_split")).toHaveLength(0);
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();

    // Spec 027: the new tab appears through the engine event and becomes active.
    bridge.emit({
      type: "tabs",
      tabs: [
        { tab_id: "w1:t1", workspace_id: "w1", label: "principal", number: 1, pane_count: 1, focused: false },
        { tab_id: "w1:t2", workspace_id: "w1", label: "2", number: 2, pane_count: 1, focused: false },
      ],
    });
    bridge.emit({
      type: "tab_focus",
      focus: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-028", revision: 2, tab_id: "w1:t2" },
    });
    flushSync();
    expect(el.querySelector<HTMLElement>("[data-tab][data-active='true']")?.getAttribute("data-tab")).toBe("w1:t2");
  });
});

describe("tab context menu (AC-028-03)", () => {
  // Would catch: the menu missing an item, an item firing two actions, or the bar menu offering
  // Renomear/Fechar outside a tab.
  it("right click on a tab offers Nova aba, Renomear and Fechar; each runs once", async () => {
    const { ctx, bridge } = await tabsCtx({ tabs: tabsOf(["principal", "workers"]) });
    const el = mountTabs(ctx);
    const tab = el.querySelector<HTMLElement>("[data-tab='w1:t1']")!;
    const items = contextMenu(el, tab);
    expect(items.map((i) => i.label)).toEqual(["Nova aba", "Renomear", "Fechar"]);

    el.querySelector<HTMLButtonElement>("[data-menu-item='new-tab']")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_create")).toHaveLength(1);
    });

    contextMenu(el, tab, 60, 12);
    el.querySelector<HTMLButtonElement>("[data-menu-item='rename']")!.click();
    flushSync();
    const input = el.querySelector<HTMLInputElement>("[data-tab-rename]");
    expect(input, "Renomear opens the inline field").toBeTruthy();
    input!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();

    contextMenu(el, tab, 80, 14);
    el.querySelector<HTMLButtonElement>("[data-menu-item='close']")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_close" && (c.args as { tabId?: string }).tabId === "w1:t1")).toHaveLength(1);
    });
    expect(bridge.calls.filter((c) => c.command === "tab_close")).toHaveLength(1);
  });

  it("right click on the bar outside a tab offers only Nova aba", async () => {
    const { ctx } = await tabsCtx({ tabs: tabsOf(["principal"]) });
    const el = mountTabs(ctx);
    const bar = el.querySelector<HTMLElement>("[data-center-tabs]")!;
    const items = contextMenu(el, bar, 300, 8);
    expect(items.map((i) => i.label)).toEqual(["Nova aba"]);
  });
});

describe("six wide tabs at 783 px (AC-024-02, spec 028 widths)", () => {
  // Spec 040 AC-040-02: + is inside strip immediately following the last tab, Comando is absent.
  it("scrolls the strip, keeps the active tab in view and + follows the last tab", async () => {
    const { ctx, bridge } = await tabsCtx({ tabs: tabsOf(["workers", "api", "principal", "deploy", "docs", "site"]) });
    bridge.emit({
      type: "tab_focus",
      focus: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-028", revision: 2, tab_id: "w1:t6" },
    });
    const seen: HTMLElement[] = [];
    const Original = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = function (this: HTMLElement) {
      seen.push(this);
    };
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx, layoutWidth: 783 } });
    flushSync();
    hosts.push({ el, app });
    HTMLElement.prototype.scrollIntoView = Original;

    const bar = el.querySelector<HTMLElement>("[data-center-tabs]")!;
    const strip = el.querySelector<HTMLElement>("[data-tab-strip]")!;
    const tabs = Array.from(el.querySelectorAll<HTMLElement>("[data-tab]"));
    expect(tabs).toHaveLength(6);
    for (const tab of tabs) {
      // Spec 028 r2: no inline width; the oracle runs on the same text the tab renders.
      expect(tab.getAttribute("style")).toBeNull();
      const width = tabWidth(tab.querySelector<HTMLElement>("[data-label] .names")!.textContent!.trim(), {
        glyph: tab.querySelector("[data-glyph]") !== null,
        more: Number.parseInt(tab.querySelector("[data-more]")?.textContent?.replace("+", "") ?? "0", 10),
      });
      expect(width).toBeGreaterThanOrEqual(TAB_MIN_PX);
      expect(width).toBeLessThanOrEqual(TAB_MAX_PX);
    }
    const active = el.querySelector<HTMLElement>("[data-tab][data-active='true']")!;
    expect(seen.some((node) => node === active || node.getAttribute?.("data-tab") === active.getAttribute("data-tab"))).toBe(true);

    const plus = el.querySelector("[data-new-tab]")!;
    const command = el.querySelector("[data-command]");
    expect(strip.contains(plus)).toBe(true);
    expect(command).toBeNull();
    expect(bar.contains(plus)).toBe(true);
    expect(tabs.at(-1)?.nextElementSibling).toBe(plus);

    let left = 0;
    Object.defineProperty(strip, "scrollLeft", {
      configurable: true,
      get: () => left,
      set: (value: number) => {
        left = value;
      },
    });
    strip.dispatchEvent(new WheelEvent("wheel", { deltaY: 80, deltaX: 0, bubbles: true, cancelable: true }));
    expect(left).toBeGreaterThan(0);
    const style = getComputedStyle(strip);
    expect(style.scrollbarWidth === "none" || strip.getAttribute("data-tab-strip") !== null).toBe(true);
  });

  it("AC-040-02: with 1 tab the + is stuck right next to it, inside strip and no fixed element on the right", async () => {
    const { ctx } = await tabsCtx({ tabs: tabsOf(["única"]) });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx } });
    flushSync();
    hosts.push({ el, app });

    const bar = el.querySelector<HTMLElement>("[data-center-tabs]")!;
    const strip = el.querySelector<HTMLElement>("[data-tab-strip]")!;
    const singleTab = strip.querySelector<HTMLElement>("[data-tab]")!;
    const plus = el.querySelector<HTMLButtonElement>("[data-new-tab]")!;

    expect(strip.contains(plus)).toBe(true);
    expect(singleTab.nextElementSibling).toBe(plus);
    expect(el.querySelector("[data-command]")).toBeNull();
    const outsideStrip = Array.from(bar.children).filter((c) => c !== strip && !c.classList.contains("arrow"));
    expect(outsideStrip).toHaveLength(0);

    const styleNode = sourceStyle("src/components/center/WorkspaceTabs.svelte");
    document.head.appendChild(styleNode);
    const plusStyle = getComputedStyle(plus);
    expect(plusStyle.width).toBe("28px");
    expect(plusStyle.height).toBe("28px");
    expect(plusStyle.marginLeft).toBe("6px");
    expect(plusStyle.borderRadius).toBe("6px");
    expect(plusStyle.fontSize).toBe("16px");
    expect(plusStyle.lineHeight).toBe("1");
    expect(plusStyle.alignSelf).toBe("center");
    expect(plusStyle.borderTopStyle).toBe("none");
    expect(plusStyle.outlineStyle).toBe("none");
    expect(plusStyle.boxShadow).toBe("none");

    plus.setAttribute("aria-disabled", "true");
    const disabledStyle = getComputedStyle(plus);
    expect(disabledStyle.opacity).toBe("0.45");

    styleNode.remove();
  });
});

// =========================================================================================
// Spec 077 — closing a tab the bar lists from the host snapshot (AC-077-02, AC-077-03).
// =========================================================================================

const SSH = "mac-mini";

/** The reported host: online, answering the API lane, but with no tab in the live list. */
function snapshotHost(overrides: Partial<HostDto> = {}): HostDto {
  return hostFixture({
    endpoint: SSH,
    label: "mac-mini",
    kind: "ssh",
    session: "hd077-remote",
    target: "user@mac-mini",
    phase: "online",
    phase_label: "Online",
    generation: 1,
    boot_id: "boot-077",
    api: true,
    workspaces: [
      {
        workspace_id: "w1",
        number: 1,
        label: "acme-web-works",
        focused: true,
        tab_count: 1,
        pane_count: 1,
        active_tab_id: "w1:t1",
        agent_status: "idle",
        cwd: "/srv/acme-web-works",
        branch: "main",
      },
    ],
    tabs: [
      { tab_id: "w1:t1", workspace_id: "w1", number: 1, label: "ec2-user", custom_label: true, focused: true, zoomed: false, pane_count: 1, agent_status: "idle" },
    ],
    panes: [
      { pane_id: "w1:p1", workspace_id: "w1", tab_id: "w1:t1", focused: true, input_enabled: true, input_block: null, target: null, cwd: "/srv/acme-web-works", foreground_cwd: "/srv/acme-web-works", title: "ec2-user", terminal_title: "ec2-user", agent: null, agent_status: "idle" },
    ],
    agents: [],
    ...overrides,
  }) as HostDto;
}

function sshSurface(phase: SurfaceState["phase"] = "live"): SurfaceState {
  return {
    selection: { endpoint: SSH, kind: "ssh", label: "mac-mini", session: "hd077-remote", online: true, identity: null },
    status: null,
    phase,
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  };
}

/** A ctx whose bar lists the host snapshot: the agents connection carries no tab of this host. */
function snapshotCtx(options: { host?: HostDto; phase?: SurfaceState["phase"] } = {}) {
  const closeTab = vi.fn(async () => {});
  const ctx: FrameContext = {
    surface: sshSurface(options.phase ?? "live"),
    agents: {
      phase: "connected",
      identity: null,
      agents: [],
      tabs: [],
      tabFocus: null,
      topology: null,
      capabilities: null,
      kinds: [],
    } as unknown as AgentsState,
    connections: {
      view: { hub: { revision: 1, hosts: [options.host ?? snapshotHost()] }, profiles: [], store_error: null },
    } as unknown as FrameContext["connections"],
    navigator: null,
    selectedEndpoint: SSH,
    activeProject: null,
    hostLabels: { [SSH]: "mac-mini" },
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: false,
    controllers: {
      surface: {} as FrameContext["controllers"]["surface"],
      agents: { closeTab } as unknown as FrameContext["controllers"]["agents"],
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  return { ctx, closeTab };
}

const closeCall = vi.mocked(hostTabClose);

describe("closing a tab listed from the host snapshot (AC-077-02)", () => {
  beforeEach(() => {
    closeCall.mockReset();
    closeCall.mockResolvedValue(undefined);
  });

  // Would catch the reported bug: the × routed to `agents.closeTab`, which only acts on the live
  // list and returns in silence for this host, so nothing happened at all.
  it("the × sends host_tab_close on that host and never agents.closeTab", async () => {
    const { ctx, closeTab } = snapshotCtx();
    const el = mountTabs(ctx);
    expect(el.querySelector("[data-tab='w1:t1']"), "the bar lists the snapshot tab").toBeTruthy();
    el.querySelector<HTMLButtonElement>("[data-close-tab='w1:t1']")!.click();
    await vi.waitFor(() => expect(closeCall).toHaveBeenCalledTimes(1));
    expect(closeCall).toHaveBeenCalledWith(SSH, "w1:t1");
    expect(closeTab).not.toHaveBeenCalled();
  });

  // Would catch the context menu keeping the old path while the × was fixed.
  it("Fechar in the tab context menu takes the same path", async () => {
    const { ctx, closeTab } = snapshotCtx();
    const el = mountTabs(ctx);
    contextMenu(el, el.querySelector<HTMLElement>("[data-tab='w1:t1']")!);
    el.querySelector<HTMLButtonElement>("[data-menu-item='close']")!.click();
    await vi.waitFor(() => expect(closeCall).toHaveBeenCalledTimes(1));
    expect(closeCall).toHaveBeenCalledWith(SSH, "w1:t1");
    expect(closeTab).not.toHaveBeenCalled();
  });

  // Would catch the fix inverting the rule: a host whose live list is the bar's source must keep
  // the existing path (the agents connection owns that tab).
  it("a tab of the live list keeps agents.closeTab and sends no host_tab_close", async () => {
    const { ctx, bridge } = await tabsCtx({ tabs: tabsOf(["principal", "workers"]) });
    const el = mountTabs(ctx);
    el.querySelector<HTMLButtonElement>("[data-close-tab='w1:t1']")!.click();
    await vi.waitFor(() => {
      expect(bridge.calls.filter((c) => c.command === "tab_close" && (c.args as { tabId?: string }).tabId === "w1:t1")).toHaveLength(1);
    });
    expect(closeCall).not.toHaveBeenCalled();
  });
});

describe("a close that fails is never silent (AC-077-03)", () => {
  beforeEach(() => {
    closeCall.mockReset();
    closeCall.mockResolvedValue(undefined);
  });

  // Would catch a refusal swallowed by the void call, leaving the × as mute as the bug.
  it("shows the backend error translated in the bar", async () => {
    closeCall.mockRejectedValue({ code: "tab_not_in_snapshot", message: "the tab no longer exists", retryable: false });
    const { ctx } = snapshotCtx();
    const el = mountTabs(ctx);
    el.querySelector<HTMLButtonElement>("[data-close-tab='w1:t1']")!.click();
    await vi.waitFor(() => expect(el.querySelector("[data-tabs-error]")).toBeTruthy());
    expect(el.querySelector<HTMLElement>("[data-tabs-error]")!.textContent).toBe(
      errorText({ code: "tab_not_in_snapshot", message: "the tab no longer exists" }),
    );
  });

  // Would catch a × still clickable (and a command sent) while the host has no live connection.
  it("an offline host keeps the × disabled with the host reason and sends nothing", async () => {
    const { ctx, closeTab } = snapshotCtx({ phase: "disconnected" });
    const el = mountTabs(ctx);
    const close = el.querySelector<HTMLButtonElement>("[data-close-tab='w1:t1']")!;
    expect(close.getAttribute("aria-disabled")).toBe("true");
    expect(close.title).toBe(t("center.reason.hostOffline"));
    close.click();
    flushSync();
    expect(closeCall).not.toHaveBeenCalled();
    expect(closeTab).not.toHaveBeenCalled();
  });
});
