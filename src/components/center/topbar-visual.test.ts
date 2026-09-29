// @vitest-environment happy-dom
// Spec 064 — Topo e status: aba ativa em pílula, barra superior sem borda, status discreta
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { mount, unmount, flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { FrameContext } from "../../shell/frame-context";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { TabDto } from "../../agents/types";
import type { SurfaceState } from "../../shell/controller";
import WorkspaceTabs from "./WorkspaceTabs.svelte";
import TitleBar from "../frame/TitleBar.svelte";
import StatusBar from "../frame/StatusBar.svelte";
import type { WindowApi } from "../frame/window-api";

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];
let styleNodes: HTMLStyleElement[] = [];

function sourceStyle(path: string): HTMLStyleElement {
  const source = readFileSync(resolve(path), "utf8");
  const style = document.createElement("style");
  if (path.endsWith(".css")) {
    style.textContent = source;
  } else {
    style.textContent = source.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? "";
  }
  return style;
}

function injectStyles(...paths: string[]) {
  for (const path of paths) {
    const node = sourceStyle(path);
    document.head.appendChild(node);
    styleNodes.push(node);
  }
}

function fakeApi(): WindowApi {
  return {
    minimize: () => {},
    toggleMaximize: () => {},
    close: () => {},
  };
}

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
      boot_id: "boot-064",
      server_version: "0.9.0",
      pane_id: "w1:p1",
      last_error: null,
    },
    phase: "live",
    reason: null,
    error: null,
    identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-064", pane_id: "w1:p1" },
    surfaceKey: 1,
    busy: false,
  };
}

function tabsOf(labels: string[]): TabDto[] {
  return labels.map((label, index) => ({
    tab_id: `w1:t${index + 1}`,
    workspace_id: "w1",
    label,
    number: index + 1,
    pane_count: 1,
    focused: index === 0, // First tab focused / active
  }));
}

async function createTestCtx(labels = ["claude", "terminal"]): Promise<FrameContext> {
  const bridge = createFakeAgentsBridge({
    bootId: "boot-064",
    generation: 1,
    session: "default",
    kinds: ["claude"],
    agents: [["w1:p1", "idle"]],
    agentKind: "claude",
    tabLabel: "1",
    paneCount: 1,
    tabs: tabsOf(labels),
  });
  const controller = createAgentsController(bridge);
  await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  bridge.emit({
    type: "tab_focus",
    focus: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-064", revision: 1, tab_id: "w1:t1" },
  });
  return {
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
      surface: {} as FrameContext["controllers"]["surface"],
      agents: controller,
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: Object.fromEntries(
      ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
        (name) => [name, () => {}],
      ),
    ) as FrameContext["actions"],
    unavailable: { split: null, newTab: null, newAgent: null },
  };
}

beforeEach(() => {
  // Always inject app.css for design tokens
  injectStyles("src/app.css");
});

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
  for (const node of styleNodes.splice(0)) {
    node.remove();
  }
});

describe("AC-064-01 (aba ativa em pílula)", () => {
  it("renders tabs as 32px pills with 8px radius, no border, 13.5px font, surface-3 for active, no accent used", async () => {
    injectStyles("src/components/center/WorkspaceTabs.svelte");
    const ctx = await createTestCtx(["claude", "terminal"]);

    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspaceTabs, { target: el, props: { ctx, layoutWidth: 1440 } });
    flushSync();
    hosts.push({ el, app });

    const tabs = Array.from(el.querySelectorAll<HTMLElement>("[data-tab]"));
    expect(tabs).toHaveLength(2);

    const activeTab = tabs.find((t) => t.dataset.active === "true") ?? tabs[0]!;
    const inactiveTab = tabs.find((t) => t.dataset.active !== "true") ?? tabs[1]!;

    expect(activeTab.dataset.active).toBe("true");
    expect(inactiveTab.dataset.active).toBe("false");

    // Dimensions, radius, border and font size for both tabs
    for (const tab of [activeTab, inactiveTab]) {
      const style = getComputedStyle(tab);
      expect(style.height, "tab height must be 32px").toBe("32px");
      expect(style.borderRadius, "border-radius must be 8px").toBe("8px");
      expect(style.borderBottomWidth, "border-bottom-width must be 0px").toBe("0px");
      expect(style.fontSize, "font-size must be 13.5px").toBe("13.5px");
    }

    // Active tab: background --surface-3 (#242424), color --text (#EDEDED), font-weight 500
    const activeStyle = getComputedStyle(activeTab);
    expect(activeStyle.backgroundColor, "active tab background is --surface-3").toBe("#242424");
    expect(activeStyle.color, "active tab title color is --text").toBe("#EDEDED");
    expect(activeStyle.fontWeight, "active tab title weight is 500").toBe("500");

    // Inactive tab: no background, color --text-muted (#A3A3A3)
    const inactiveStyle = getComputedStyle(inactiveTab);
    expect(
      inactiveStyle.backgroundColor === "transparent" ||
        inactiveStyle.backgroundColor === "rgba(0, 0, 0, 0)" ||
        inactiveStyle.backgroundColor === "",
      "inactive tab has no background",
    ).toBe(true);
    expect(inactiveStyle.color, "inactive tab title color is --text-muted").toBe("#A3A3A3");

    // No tab rule uses --accent
    const tabsSource = readFileSync(resolve("src/components/center/WorkspaceTabs.svelte"), "utf8");
    const styleContent = tabsSource.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? "";
    const tabRules = Array.from(styleContent.matchAll(/\.tab\b[^{]*\{([^}]*)\}/g)).map((m) => m[1] ?? "");
    for (const ruleBody of tabRules) {
      expect(ruleBody, "tab rule must not reference --accent").not.toContain("var(--accent)");
    }

    // 040 limits still hold: min 64px, max 280px, and × is visible
    expect(activeStyle.minWidth).toBe("64px");
    expect(activeStyle.maxWidth).toBe("280px");
    const closeBtn = inactiveTab.querySelector<HTMLElement>("[data-close-tab]")!;
    expect(closeBtn).toBeTruthy();
    expect(getComputedStyle(closeBtn).display).toBe("inline-flex");
  });
});

describe("AC-064-02 (barra superior sem borda)", () => {
  it("renders TitleBar and WorkspaceTabs at 48px height, 0px border-bottom, and surface background", async () => {
    injectStyles("src/components/frame/TitleBar.svelte", "src/components/center/WorkspaceTabs.svelte");
    const ctx = await createTestCtx(["claude", "terminal"]);

    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(TitleBar, {
      target: el,
      props: {
        host: { label: "Este computador", kind: "Local", state: "conectado", tone: "live" },
        searchHint: "Ctrl K",
        onOpenPalette: () => {},
        onHost: () => {},
        windowApi: fakeApi(),
        ctx,
      },
    });
    flushSync();
    hosts.push({ el, app });

    const topbar = el.querySelector<HTMLElement>(".topbar")!;
    expect(topbar).toBeTruthy();
    const topbarStyle = getComputedStyle(topbar);
    expect(topbarStyle.height, "TitleBar height is 48px").toBe("48px");
    expect(topbarStyle.borderBottomWidth, "TitleBar border-bottom-width is 0px").toBe("0px");
    expect(topbarStyle.backgroundColor, "TitleBar background is --surface").toBe("#0A0A0A");

    const tabsBar = el.querySelector<HTMLElement>("[data-center-tabs]")!;
    expect(tabsBar).toBeTruthy();
    const tabsBarStyle = getComputedStyle(tabsBar);
    expect(tabsBarStyle.height, "WorkspaceTabs height is 48px").toBe("48px");
    expect(tabsBarStyle.borderBottomWidth, "WorkspaceTabs border-bottom-width is 0px").toBe("0px");
    expect(tabsBarStyle.backgroundColor, "WorkspaceTabs background is --surface").toBe("#0A0A0A");
  });
});

describe("AC-064-03 (status discreta)", () => {
  it("renders StatusBar at 28px height, 0px border-top, bg background, 12px text-dim text, attention notice", () => {
    injectStyles("src/components/frame/StatusBar.svelte");

    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(StatusBar, {
      target: el,
      props: {
        phase: "live",
        diagnostics: "ok",
        model: { server: "0.9.0", branch: "main", counts: "1 pane · 1 aba", channel: "canal stable", agents: null, cwd: "/home/user/repo" },
        host: "Local",
        session: "default",
        blocked: null,
        readOnly: false,
        notice: "aviso temporário",
      },
    });
    flushSync();
    hosts.push({ el, app });

    const bar = el.querySelector<HTMLElement>(".status-bar")!;
    expect(bar).toBeTruthy();
    const barStyle = getComputedStyle(bar);

    expect(barStyle.height, "StatusBar height is 28px").toBe("28px");
    expect(barStyle.borderTopWidth, "StatusBar border-top-width is 0px").toBe("0px");
    expect(barStyle.backgroundColor, "StatusBar background is --bg").toBe("#0F0F0F");
    expect(barStyle.fontSize, "StatusBar text size is 12px").toBe("12px");
    expect(barStyle.color, "StatusBar text color is --text-dim").toBe("#6B6B6B");

    // Notice retains --attention
    const notice = el.querySelector<HTMLElement>(".notice")!;
    expect(notice).toBeTruthy();
    expect(getComputedStyle(notice).color, "notice color is --attention").toBe("#E9A23B");

    // Phase dot in live phase retains --working
    const dot = el.querySelector<HTMLElement>("[data-item='server'] .dot")!;
    expect(dot).toBeTruthy();
    expect(getComputedStyle(dot).backgroundColor, "phase live dot is --working").toBe("#4ADE80");
  });
});
