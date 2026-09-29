import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentDto, AgentStatus } from "../../agents/types";
import { hostFixture } from "../../connections/fake-bridge";
import type { ConnectionsState } from "../../connections/controller";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import { createProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import type { CollectionDto, ProjectDto, WorkspacePrefDto } from "../../projects/types";
import type { FrameContext } from "../../shell/frame-context";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import CollectionGroup from "./CollectionGroup.svelte";
import HostSwitcher from "./HostSwitcher.svelte";
import InboxSection from "./InboxSection.svelte";
import { inboxTitle } from "./inbox-model";
import SidebarHeader from "./SidebarHeader.svelte";
import SidebarRail from "./SidebarRail.svelte";
import SidebarV2 from "./SidebarV2.svelte";
import WorkspacesSection from "./WorkspacesSection.svelte";

function sourceStyle(path: string): HTMLStyleElement {
  const source = readFileSync(resolve(path), "utf8");
  const style = document.createElement("style");
  style.textContent = source.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? "";
  return style;
}

function rootStyle(): HTMLStyleElement {
  const source = readFileSync(resolve("src/app.css"), "utf8");
  const style = document.createElement("style");
  style.textContent = source;
  return style;
}

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];
let styles: HTMLStyleElement[] = [];

beforeEach(() => {
  const root = rootStyle();
  document.head.appendChild(root);
  styles.push(root);
});

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
  for (const s of styles.splice(0)) {
    s.remove();
  }
});

function injectStyles(...paths: string[]) {
  for (const path of paths) {
    const s = sourceStyle(path);
    document.head.appendChild(s);
    styles.push(s);
  }
}

function normalizeColor(color: string): string {
  const c = color.trim().toLowerCase();
  if (c === "rgb(237, 237, 237)" || c === "#ededed") return "var(--text)";
  if (c === "rgb(10, 10, 10)" || c === "#0a0a0a") return "var(--surface)";
  if (c === "rgb(23, 23, 23)" || c === "#171717") return "var(--surface-2)";
  if (c === "rgb(36, 36, 36)" || c === "#242424") return "var(--surface-3)";
  if (c === "rgb(107, 107, 107)" || c === "#6b6b6b") return "var(--text-dim)";
  if (c === "rgb(163, 163, 163)" || c === "#a3a3a3") return "var(--text-muted)";
  if (c === "rgb(233, 162, 59)" || c === "#e9a23b") return "var(--attention)";
  if (c === "rgb(43, 33, 18)" || c === "#2b2112") return "var(--attention-soft)";
  return c;
}

function isColor(actual: string, expectedVar: string, expectedHex: string, expectedRgb: string): boolean {
  const a = actual.trim().toLowerCase();
  return (
    a === expectedVar.toLowerCase() ||
    a.startsWith(expectedVar.toLowerCase()) ||
    a === expectedHex.toLowerCase() ||
    a === expectedRgb.toLowerCase()
  );
}

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

interface FakeContextOptions {
  hosts?: HostDto[];
  projects?: ProjectDto[];
  collections?: CollectionDto[];
  agents?: AgentDto[];
  selectedEndpoint?: string | null;
  prefs?: WorkspacePrefDto[];
}

async function createTestContext(options: FakeContextOptions = {}): Promise<FrameContext> {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-063",
    seed: {
      version: 1,
      projects: options.projects ?? [],
      collections: options.collections ?? [],
      workspace_prefs: options.prefs ?? [],
    },
  });
  const controller = createProjectsController(bridge);
  await controller.load();
  const hosts = options.hosts ?? [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd063",
      target: null,
      workspaces: [],
    }),
  ];

  return {
    surface: {
      selection: { endpoint: "local", kind: "local", label: "Local", session: "hd063", online: true, identity: null },
      status: null,
      phase: "live",
      reason: null,
      error: null,
      identity: null,
      surfaceKey: 1,
      busy: false,
    },
    agents: {
      agents: options.agents ?? [],
      kinds: [],
      tabs: [],
    } as unknown as FrameContext["agents"],
    connections: connectionsState(hosts),
    get navigator() {
      return controller.state;
    },
    selectedEndpoint: options.selectedEndpoint ?? "local",
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: true,
    controllers: {
      surface: { select: vi.fn(async () => ({} as never)) } as unknown as FrameContext["controllers"]["surface"],
      agents: { createTab: vi.fn(async () => {}), focusTab: vi.fn(async () => {}) } as unknown as FrameContext["controllers"]["agents"],
      projects: controller,
      connections: {
        openDialog: () => {},
        disconnect: vi.fn(async () => {}),
        reconnect: vi.fn(async () => {}),
        removeProfile: vi.fn(async () => {}),
        editProfile: vi.fn(async () => {}),
        setConnectOnOpen: vi.fn(async () => {}),
      } as unknown as FrameContext["controllers"]["connections"],
    },
    actions: {
      toggleProjects: vi.fn(),
      showProjects: vi.fn(),
      openConnections: vi.fn(),
      paste: vi.fn(),
      toggleAgents: vi.fn(),
      toggleFiles: vi.fn(),
      openPalette: vi.fn(),
      newAgent: vi.fn(),
      split: vi.fn(),
      newTab: vi.fn(),
      reconnect: vi.fn(),
    },
    unavailable: { split: null, newTab: null, newAgent: null },
  };
}

describe("AC-063-01 cabeçalho, ação e títulos", () => {
  it("cabeçalho tem 48 px, marca --text sobre --surface, nome 15 px 600", async () => {
    injectStyles("src/components/sidebar/SidebarHeader.svelte");
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(SidebarHeader, { target: el, props: { onCollapse: () => {} } });
    flushSync();
    mounted.push({ el, app: app as never });

    const head = el.querySelector<HTMLElement>("[data-sidebar-header]")!;
    const logo = el.querySelector<HTMLElement>(".logo")!;
    const brandName = el.querySelector<HTMLElement>(".brand-name")!;

    const headStyle = getComputedStyle(head);
    expect(headStyle.height).toBe("48px");

    const logoStyle = getComputedStyle(logo);
    // The app icon replaced the light tile: the mark is the SVG of the bundle icon, no tile behind it.
    expect(logo.querySelector("[data-brand-mark]")).not.toBeNull();
    expect(isColor(logoStyle.backgroundColor, "var(--text)", "#ededed", "rgb(237, 237, 237)")).toBe(false);
    expect(logoStyle.backgroundColor).not.toContain("accent");
    expect(logoStyle.color).not.toContain("accent");

    const brandStyle = getComputedStyle(brandName);
    expect(brandName.textContent?.trim()).toBe("herdr");
    expect(brandStyle.fontSize).toBe("15px");
    expect(brandStyle.fontWeight).toBe("600");
  });

  it("Novo agente tem fundo --surface-2, border-radius 8px, rótulo 14 px peso 500", async () => {
    injectStyles("src/components/sidebar/SidebarV2.svelte");
    const ctx = await createTestContext();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(SidebarV2, { target: el, props: { ctx } });
    flushSync();
    mounted.push({ el, app: app as never });

    const btn = el.querySelector<HTMLButtonElement>('[data-sidebar-action="newAgent"]')!;
    const label = btn.querySelector<HTMLElement>(".action-label")!;

    const btnStyle = getComputedStyle(btn);
    expect(isColor(btnStyle.backgroundColor, "var(--surface-2)", "#171717", "rgb(23, 23, 23)")).toBe(true);
    expect(btnStyle.borderRadius).toBe("8px");

    const labelStyle = getComputedStyle(label);
    expect(label.textContent?.trim()).toBe("Novo agente");
    expect(labelStyle.fontSize).toBe("14px");
    expect(labelStyle.fontWeight).toBe("500");
  });

  it("título da seção é Workspaces, 13 px peso 500 --text-dim, sem letter-spacing e sem uppercase", async () => {
    injectStyles("src/components/sidebar/WorkspacesSection.svelte");
    const ctx = await createTestContext();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspacesSection, { target: el, props: { ctx } });
    flushSync();
    mounted.push({ el, app: app as never });

    const title = el.querySelector<HTMLElement>(".section-title")!;
    expect(title.textContent?.trim()).toBe("Workspaces");

    const titleStyle = getComputedStyle(title);
    expect(titleStyle.fontSize).toBe("13px");
    expect(titleStyle.fontWeight).toBe("500");
    expect(isColor(titleStyle.color, "var(--text-dim)", "#6b6b6b", "rgb(107, 107, 107)")).toBe(true);
    expect(titleStyle.letterSpacing === "normal" || titleStyle.letterSpacing === "0px" || titleStyle.letterSpacing === "").toBe(true);
    expect(titleStyle.textTransform).not.toBe("uppercase");

    expect(inboxTitle()).toBe("Precisa de você");
  });
});

describe("AC-063-02 caixa de entrada em destaque âmbar", () => {
  it("cabeçalho da caixa tem fundo --attention-soft, border-radius 8px, título 14 px 600 e contagem 2 em --attention; itens 14 px 600 --text e tempo 12 px --text-dim, sem bordas", async () => {
    injectStyles("src/components/sidebar/InboxSection.svelte");
    const hosts: HostDto[] = [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        session: "hd063",
        workspaces: [
          workspace({ workspace_id: "w-api", label: "api" }),
        ],
        agents: [
          {
            workspace_id: "w-api",
            tab_id: "w-api:t1",
            pane_id: "w-api:p1",
            agent: "codex",
            agent_status: "blocked",
            name: "codex",
            display_agent: "codex",
            title: null,
            terminal_title: null,
            terminal_title_stripped: null,
            focused: false,
          },
        ],
      }),
      hostFixture({
        endpoint: "ssh-dev",
        label: "dev-box",
        kind: "ssh",
        phase: "online",
        session: "hd063",
        workspaces: [
          workspace({ workspace_id: "w-lib", label: "lib" }),
        ],
        agents: [
          {
            workspace_id: "w-lib",
            tab_id: "w-lib:t1",
            pane_id: "w-lib:p1",
            agent: "claude",
            agent_status: "done",
            name: "claude",
            display_agent: "claude",
            title: null,
            terminal_title: null,
            terminal_title_stripped: null,
            focused: false,
          },
        ],
      }),
    ];
    const ctx = await createTestContext({ hosts });

    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(InboxSection, { target: el, props: { ctx } });
    flushSync();
    mounted.push({ el, app: app as never });

    const head = el.querySelector<HTMLElement>("[data-sidebar-inbox-section] > header")!;
    const title = el.querySelector<HTMLElement>("[data-inbox-title]")!;
    const count = el.querySelector<HTMLElement>("[data-inbox-count]")!;

    const headStyle = getComputedStyle(head);
    expect(isColor(headStyle.backgroundColor, "var(--attention-soft)", "#2b2112", "rgb(43, 33, 18)")).toBe(true);
    expect(headStyle.borderRadius).toBe("8px");

    const titleStyle = getComputedStyle(title);
    expect(title.textContent?.trim()).toBe("Precisa de você");
    expect(titleStyle.fontSize).toBe("14px");
    expect(titleStyle.fontWeight).toBe("600");
    expect(isColor(titleStyle.color, "var(--attention)", "#e9a23b", "rgb(233, 162, 59)")).toBe(true);

    const countStyle = getComputedStyle(count);
    expect(count.textContent?.trim()).toBe("2");
    expect(isColor(countStyle.color, "var(--attention)", "#e9a23b", "rgb(233, 162, 59)")).toBe(true);

    const items = el.querySelectorAll<HTMLElement>("[data-inbox-item]");
    expect(items.length).toBe(2);

    for (const item of items) {
      const itemStyle = getComputedStyle(item);
      expect(itemStyle.borderTopStyle === "none" || itemStyle.borderWidth === "0px" || itemStyle.borderStyle === "none").toBe(true);

      const wsText = item.querySelector<HTMLElement>("[data-inbox-text]")!;
      const textStyle = getComputedStyle(wsText);
      expect(textStyle.fontSize).toBe("14px");
      expect(textStyle.fontWeight).toBe("600");
      expect(isColor(textStyle.color, "var(--text)", "#ededed", "rgb(237, 237, 237)")).toBe(true);

      const timeText = item.querySelector<HTMLElement>("[data-inbox-time]")!;
      const timeStyle = getComputedStyle(timeText);
      expect(timeStyle.fontSize).toBe("12px");
      expect(isColor(timeStyle.color, "var(--text-dim)", "#6b6b6b", "rgb(107, 107, 107)")).toBe(true);
    }
  });
});

describe("AC-063-03 coleções, espera, rodapé e trilho", () => {
  it("cabeçalho da coleção tem rótulo 13 px 500 --text-muted, swatch 8x8 border-radius 2px, contagem 12 px --text-dim", async () => {
    injectStyles("src/components/sidebar/CollectionGroup.svelte");
    const ctx = await createTestContext();
    const collection = {
      id: "c1",
      name: "Meus projetos",
      color: "#8fa8ff",
      count: 2,
      empty: false,
      collapsed: false,
      ungrouped: false,
      rows: [],
    };
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(CollectionGroup, {
      target: el,
      props: {
        ctx,
        collection,
        collapsed: false,
        onToggle: () => {},
        onDrop: () => {},
      },
    });
    flushSync();
    mounted.push({ el, app: app as never });

    const name = el.querySelector<HTMLElement>("[data-collection-name]")!;
    const nameStyle = getComputedStyle(name);
    expect(nameStyle.fontSize).toBe("13px");
    expect(nameStyle.fontWeight).toBe("500");
    expect(isColor(nameStyle.color, "var(--text-muted)", "#a3a3a3", "rgb(163, 163, 163)")).toBe(true);

    const swatch = el.querySelector<HTMLElement>("[data-collection-color]")!;
    const swatchStyle = getComputedStyle(swatch);
    expect(swatchStyle.width).toBe("8px");
    expect(swatchStyle.height).toBe("8px");
    expect(swatchStyle.borderRadius).toBe("2px");

    const count = el.querySelector<HTMLElement>("[data-collection-count]")!;
    const countStyle = getComputedStyle(count);
    expect(countStyle.fontSize).toBe("12px");
    expect(isColor(countStyle.color, "var(--text-dim)", "#6b6b6b", "rgb(107, 107, 107)")).toBe(true);
  });

  it("linha de espera de host em reconnecting tem texto 13 px --text-dim", async () => {
    injectStyles("src/components/sidebar/WorkspacesSection.svelte");
    const hosts: HostDto[] = [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        session: "hd063",
        workspaces: [],
      }),
      hostFixture({
        endpoint: "ssh-mac",
        label: "mac-mini",
        kind: "ssh",
        phase: "reconnecting",
        phase_label: "Reconectando…",
        session: "hd063",
        workspaces: [],
      }),
    ];
    const ctx = await createTestContext({ hosts });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(WorkspacesSection, { target: el, props: { ctx } });
    flushSync();
    mounted.push({ el, app: app as never });

    const waitingRow = el.querySelector<HTMLElement>("[data-ungrouped-waiting] .waiting-row")!;
    expect(waitingRow).toBeTruthy();
    const rowStyle = getComputedStyle(waitingRow);
    expect(rowStyle.fontSize).toBe("13px");
    expect(isColor(rowStyle.color, "var(--text-dim)", "#6b6b6b", "rgb(107, 107, 107)")).toBe(true);
  });

  it("rodapé do host é um cartão --surface-2, border-radius 8px, nome 13.5 px 500 --text, detalhe 12 px --text-dim", async () => {
    injectStyles("src/components/sidebar/HostSwitcher.svelte");
    const ctx = await createTestContext();
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(HostSwitcher, { target: el, props: { ctx } });
    flushSync();
    mounted.push({ el, app: app as never });

    const card = el.querySelector<HTMLElement>("[data-host-trigger]")!;
    const cardStyle = getComputedStyle(card);
    expect(isColor(cardStyle.backgroundColor, "var(--surface-2)", "#171717", "rgb(23, 23, 23)")).toBe(true);
    expect(cardStyle.borderRadius).toBe("8px");

    const name = el.querySelector<HTMLElement>("[data-host-name]")!;
    const nameStyle = getComputedStyle(name);
    expect(nameStyle.fontSize).toBe("13.5px");
    expect(nameStyle.fontWeight).toBe("500");
    expect(isColor(nameStyle.color, "var(--text)", "#ededed", "rgb(237, 237, 237)")).toBe(true);

    const summary = el.querySelector<HTMLElement>("[data-host-summary]")!;
    const summaryStyle = getComputedStyle(summary);
    expect(summaryStyle.fontSize).toBe("12px");
    expect(isColor(summaryStyle.color, "var(--text-dim)", "#6b6b6b", "rgb(107, 107, 107)")).toBe(true);
  });

  it("no trilho recolhido o item ativo tem fundo --surface-3 e ícone --text sem --accent, marca segue AC-063-01", async () => {
    injectStyles("src/components/sidebar/SidebarRail.svelte");
    const hosts: HostDto[] = [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        session: "hd063",
        workspaces: [
          workspace({ workspace_id: "w-act", label: "ativo", focused: true }),
        ],
      }),
    ];
    const projects: ProjectDto[] = [
      { id: "p-act", label: "ativo", endpoint_profile_id: "local", session_name: "hd063", root: "/work/act", binding: null },
    ];
    const ctx = await createTestContext({ hosts, projects });
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(SidebarRail, { target: el, props: { ctx } });
    flushSync();
    mounted.push({ el, app: app as never });

    const mark = el.querySelector<HTMLElement>('[data-rail-item="brand"]')!;
    const markStyle = getComputedStyle(mark);
    expect(mark.querySelector("[data-brand-mark]")).not.toBeNull();
    expect(isColor(markStyle.backgroundColor, "var(--text)", "#ededed", "rgb(237, 237, 237)")).toBe(false);
    expect(markStyle.backgroundColor).not.toContain("accent");
    expect(markStyle.color).not.toContain("accent");

    const activeAvatar = el.querySelector<HTMLElement>('.avatar[data-active="true"]')!;
    expect(activeAvatar).toBeTruthy();
    const avatarStyle = getComputedStyle(activeAvatar);
    expect(isColor(avatarStyle.backgroundColor, "var(--surface-3)", "#242424", "rgb(36, 36, 36)")).toBe(true);
    expect(avatarStyle.borderColor === "transparent" || avatarStyle.borderWidth === "0px" || avatarStyle.borderStyle === "none").toBe(true);
    expect(avatarStyle.borderColor).not.toContain("accent");

    const initials = activeAvatar.querySelector<HTMLElement>("[data-rail-initials]")!;
    const initialsStyle = getComputedStyle(initials);
    expect(isColor(initialsStyle.color, "var(--text)", "#ededed", "rgb(237, 237, 237)")).toBe(true);
    expect(initialsStyle.color).not.toContain("accent");
  });
});
