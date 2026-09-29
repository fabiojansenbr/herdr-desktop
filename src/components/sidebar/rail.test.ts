// @vitest-environment happy-dom
// Spec 047 — the collapsed sidebar as a 56 px rail (`design/v2-04-lateral-recolhida.png`).
// Would catch the pre-047 window: a collapsed sidebar of 0 px that renders no child at all, so
// the status of every workspace and the "Precisa de você" count disappear with one Ctrl+B; an
// avatar that spells the first two characters instead of the initials of the design (`erp-api`
// → `EA`), loses the collection colour, the active outline or the status dot; a rail that does
// not focus the workspace through the same controller path as the open row; and the footer host
// drawn with an emoji instead of the `monitor`/`server` line icon (AC-047-05).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentsState } from "../../agents/reducer";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostAgentDto, HostDto, HostWorkspaceDto } from "../../connections/types";
import { createProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import type { CollectionDto, ProjectDto, WorkspacePrefDto } from "../../projects/types";
import type { NavigatorState } from "../../projects/reducer";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import { resolveShortcut } from "../../shell/shortcuts";
import { SIDEBAR_OPEN_PX, SIDEBAR_RAIL_PX } from "../frame/sidebar";
import Sidebar from "../frame/Sidebar.svelte";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import HostSwitcher from "./HostSwitcher.svelte";
import SidebarRail from "./SidebarRail.svelte";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const sidebarSource = readFileSync(resolve("src/components/frame/Sidebar.svelte"), "utf8");
const appSource = readFileSync(resolve("src/App.svelte"), "utf8");

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010/041). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
});

function workspace(overrides: Partial<HostWorkspaceDto> & { workspace_id: string; label: string }): HostWorkspaceDto {
  return {
    number: 1,
    focused: false,
    tab_count: 1,
    pane_count: 1,
    active_tab_id: `${overrides.workspace_id}:t1`,
    agent_status: "unknown",
    cwd: null,
    branch: null,
    ...overrides,
  };
}

function hostAgent(overrides: Partial<HostAgentDto> & { pane_id: string; agent: string; agent_status: string }): HostAgentDto {
  const workspaceId = overrides.pane_id.split(":")[0]!;
  return {
    workspace_id: workspaceId,
    tab_id: `${workspaceId}:t1`,
    name: overrides.agent,
    display_agent: overrides.agent,
    title: null,
    terminal_title: null,
    terminal_title_stripped: null,
    focused: false,
    ...overrides,
  };
}

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd047", online: true, identity: null },
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
 * The window of the design: Local with four workspaces of the collection `Acme · Clientes`
 * (`erp-api` focused and working, `portal-web` finished, `mobile-app` idle, `oculto` hidden by
 * preference) plus the SSH host `dev-box` with `herdr` blocked. Two agents need the user.
 */
function fixture() {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      target: null,
      phase: "online",
      phase_label: "Online",
      session: "hd047",
      server_version: "0.9.0",
      workspaces: [
        workspace({ workspace_id: "w-erp", label: "erp-api", number: 1, focused: true, cwd: "/work/acme/erp-api", branch: "main" }),
        workspace({ workspace_id: "w-web", label: "portal-web", number: 2, cwd: "/work/acme/portal-web", branch: "feat/login" }),
        workspace({ workspace_id: "w-mob", label: "mobile-app", number: 3, cwd: "/work/acme/mobile-app", branch: "dev" }),
        workspace({ workspace_id: "w-hid", label: "oculto", number: 4, cwd: "/work/acme/oculto", branch: "main" }),
      ],
      agents: [
        hostAgent({ pane_id: "w-erp:p1", agent: "claude", agent_status: "working" }),
        hostAgent({ pane_id: "w-web:p1", agent: "claude", agent_status: "done" }),
        hostAgent({ pane_id: "w-mob:p1", agent: "codex", agent_status: "idle" }),
      ],
    }),
    hostFixture({
      endpoint: "ssh-dev",
      label: "dev-box",
      kind: "ssh",
      phase: "online",
      phase_label: "Online",
      session: "hd047",
      workspaces: [workspace({ workspace_id: "w-herdr", label: "herdr", number: 1, cwd: "/srv/herdr", branch: "master" })],
      agents: [hostAgent({ pane_id: "w-herdr:p1", agent: "codex", agent_status: "blocked" })],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-erp", label: "erp-api", endpoint_profile_id: "local", session_name: "hd047", root: "/work/acme/erp-api", binding: null },
    { id: "p-web", label: "portal-web", endpoint_profile_id: "local", session_name: "hd047", root: "/work/acme/portal-web", binding: null },
    { id: "p-mob", label: "mobile-app", endpoint_profile_id: "local", session_name: "hd047", root: "/work/acme/mobile-app", binding: null },
    { id: "p-hid", label: "oculto", endpoint_profile_id: "local", session_name: "hd047", root: "/work/acme/oculto", binding: null },
  ];
  const collections: CollectionDto[] = [{ id: "g1", name: "Acme · Clientes", project_ids: ["p-erp", "p-web", "p-mob", "p-hid"] }];
  const prefs: WorkspacePrefDto[] = [
    { endpoint_profile_id: "local", root: "/work/acme/portal-web", color: "#B18CFF", pinned: false, hidden: false },
    { endpoint_profile_id: "local", root: "/work/acme/oculto", color: null, pinned: false, hidden: true },
  ];
  return { hosts, projects, collections, prefs };
}

interface RenderOptions {
  hosts?: HostDto[];
  projects?: ProjectDto[];
  collections?: CollectionDto[];
  prefs?: WorkspacePrefDto[];
  agents?: AgentsState | null;
  selected?: string;
}

async function render(options: RenderOptions = {}) {
  const base = fixture();
  const bridge = createFakeProjectsBridge({
    bootId: "boot-047",
    seed: {
      version: 3,
      projects: options.projects ?? base.projects,
      collections: options.collections ?? base.collections,
      workspace_prefs: options.prefs ?? base.prefs,
    },
  });
  const nav = reactiveHolder<NavigatorState | null>(null);
  const controller = createProjectsController(bridge, (state) => (nav.value = state));
  await controller.load();
  const calls: string[] = [];
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [
      name,
      () => {
        calls.push(name);
      },
    ]),
  ) as Record<MenuAction, () => void>;
  const hostsHolder = reactiveHolder<readonly HostDto[]>(options.hosts ?? base.hosts);
  const focusWorkspace = vi.spyOn(controller, "focusWorkspace").mockResolvedValue(undefined as never);
  const openClosedProject = vi.spyOn(controller, "openClosedProject").mockResolvedValue(undefined as never);
  const select = vi.fn(async () => ({}) as never);
  const ctx: FrameContext = {
    surface: surface(),
    get agents() {
      return options.agents ?? ({ agents: [], kinds: [], tabs: [] } as unknown as AgentsState);
    },
    get connections() {
      return connectionsState(hostsHolder.value);
    },
    get navigator() {
      return nav.value;
    },
    selectedEndpoint: options.selected ?? "local",
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: false,
    agentsOpen: false,
    controllers: {
      surface: { select } as unknown as FrameContext["controllers"]["surface"],
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
  el.style.height = "800px";
  document.body.appendChild(el);
  const app = mount(SidebarRail, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, ctx, calls, controller, focusWorkspace, openClosedProject };
}

const avatars = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>("[data-rail-workspace]")];
const text = (node: Element | null | undefined) => node?.textContent?.replace(/\s+/g, " ").trim() ?? null;

describe("AC-047-01 the collapsed sidebar is a 56 px rail, not a 0 px void", () => {
  // Would catch: `.sidebar.closed { width: 0 }` left in place, or the collapsed sidebar still
  // refusing to render any child (spec 018's rule, which 047 replaces).
  it("Sidebar renders the rail slot at 56 px instead of collapsing to 0", () => {
    expect(SIDEBAR_RAIL_PX).toBe(56);
    expect(SIDEBAR_OPEN_PX).toBe(288);
    expect(rule(sidebarSource, ".sidebar")).toMatch(/width:\s*288px/);
    expect(rule(sidebarSource, ".sidebar.rail")).toMatch(/width:\s*56px/);
    // The 0 px collapse of spec 018 is gone, rule and all.
    expect(rule(sidebarSource, ".sidebar.closed")).toBe("");
    expect(sidebarSource).not.toMatch(/\bclosed\b/);
  });

  it("the collapsed region marks itself as a rail and keeps rendering its children", () => {
    const el = document.createElement("div");
    document.body.appendChild(el);
    const app = mount(Sidebar, { target: el, props: { open: false, viewportWidth: 1440 } });
    flushSync();
    mounted.push({ el, app: app as never });
    const region = el.querySelector<HTMLElement>("[data-region='sidebar']")!;
    expect(region.getAttribute("aria-expanded")).toBe("false");
    expect(region.dataset.rail).toBe("true");
    // The rail is real content: hiding it from the a11y tree would hide the whole status column.
    expect(region.getAttribute("aria-hidden")).toBeNull();
  });

  // Spec 056 (AC-056-01): Buscar left the rail, so the stack is mark, Novo agente, Precisa de
  // você, separator, avatars, host. Would catch: an item of the design missing (no mark, no Novo
  // agente), Buscar back in the stack, the inbox above Novo agente, the separator gone, or the
  // host floating in the middle instead of closing the rail.
  it("stacks mark, Novo agente, Precisa de você, separator, avatars and the host", async () => {
    const { el } = await render();
    const root = el.querySelector<HTMLElement>("[data-slot='rail']")!;
    expect(root).toBeTruthy();
    const slots = [...root.querySelectorAll<HTMLElement>("[data-rail-item],[data-rail-separator],[data-rail-workspace]")];
    expect(
      slots.map((node) =>
        node.dataset.railSeparator !== undefined ? "separator" : (node.dataset.railItem ?? `avatar:${node.dataset.railWorkspace}`),
      ),
    ).toEqual([
      "brand",
      "newAgent",
      "inbox",
      "separator",
      "avatar:local:w-erp",
      "avatar:local:w-web",
      "avatar:local:w-mob",
      "avatar:ssh-dev:w-herdr",
      "host",
    ]);
  });

  // Would catch: a rail that shows every workspace, including the ones the user hid (spec 044/P5).
  it("skips the hidden workspaces and keeps the order of the open list", async () => {
    const { el } = await render();
    expect(avatars(el).map((node) => node.dataset.railWorkspace)).not.toContain("local:w-hid");
    expect(avatars(el)).toHaveLength(4);
  });

  // Would catch: the badge counting something other than the 043 box, or a "0" badge drawn when
  // nothing needs the user.
  it("badges Precisa de você with the 043 count and drops the badge at zero", async () => {
    const { el } = await render();
    const inbox = el.querySelector<HTMLElement>("[data-rail-item='inbox']")!;
    expect(text(inbox.querySelector("[data-rail-inbox-count]"))).toBe("2");
    expect(inbox.getAttribute("aria-label")).toMatch(/precisa de você/i);

    const quiet = fixture();
    const calm = await render({
      hosts: quiet.hosts.map((host) => ({ ...host, agents: (host.agents ?? []).filter((agent) => agent.agent_status === "working") })),
    });
    expect(calm.el.querySelector("[data-rail-inbox-count]")).toBeNull();
  });

  // Spec 056 (AC-056-01): no Buscar on the rail, and nothing on it reaches the palette.
  // Would catch: the search item back, under its data attribute or only by its label.
  it("has no Buscar item and the host still opens the 048 popover", async () => {
    const { el, calls } = await render();
    expect(el.querySelector("[data-rail-item='search']")).toBeNull();
    const rail = el.querySelector<HTMLElement>("[data-slot='rail']")!;
    expect([...rail.querySelectorAll("[data-rail-item]")].some((node) => /buscar/i.test(node.getAttribute("aria-label") ?? ""))).toBe(false);
    expect(rail.textContent ?? "").not.toMatch(/Buscar/i);
    expect(calls).not.toContain("openPalette");

    const trigger = el.querySelector<HTMLButtonElement>("[data-rail-item='host'] [data-host-trigger]")!;
    expect(trigger).toBeTruthy();
    trigger.click();
    flushSync();
    expect(el.querySelector("[data-host-popover]")).toBeTruthy();
  });
});

describe("AC-047-02 the workspace avatars", () => {
  // Would catch: `erp-api` spelled `ER` (the first two characters) instead of the design's `EA`,
  // or a single-word name losing its second letter.
  it("spells the initials of the design (erp-api → EA, portal-web → PW, herdr → HE)", async () => {
    const { el } = await render();
    expect(avatars(el).map((node) => text(node.querySelector("[data-rail-initials]")))).toEqual(["EA", "PW", "MA", "HE"]);
  });

  // Would catch: the preference colour of 044 ignored, or every avatar painted with the accent.
  it("paints the collection colour, the preference colour and the active selection", async () => {
    const { el } = await render();
    const [erp, web] = avatars(el);
    expect(erp!.dataset.railColor).toBe("#8FA8FF");
    expect(web!.dataset.railColor).toBe("#B18CFF");
    expect(erp!.dataset.active).toBe("true");
    expect(web!.dataset.active).toBeUndefined();
    expect(rule(sources["./SidebarRail.svelte"]!, '.avatar[data-active="true"]')).toMatch(/var\(--surface-3/);
  });

  // Would catch: `done` folded into idle again (no dot at all) or an idle workspace wearing one.
  it("shows the status dot of sidebarStatus and nothing when the row is idle", async () => {
    const { el } = await render();
    const dot = (node: HTMLElement) => node.querySelector<HTMLElement>("[data-rail-dot]")?.dataset.tone ?? null;
    const [erp, web, mob, herdr] = avatars(el);
    expect(dot(erp!)).toBe("working");
    expect(dot(web!)).toBe("done");
    expect(dot(mob!)).toBeNull();
    expect(dot(herdr!)).toBe("waiting");
  });

  // Would catch: a tooltip that only repeats the name, so the rail loses the branch and the state.
  it("hover and focus show name, coleção · branch and the status line", async () => {
    const { el } = await render();
    const [erp, , , herdr] = avatars(el);
    expect(el.querySelector("[data-rail-tooltip]")).toBeNull();
    erp!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
    flushSync();
    const tip = el.querySelector<HTMLElement>("[data-rail-tooltip]")!;
    expect(text(tip.querySelector("[data-rail-tip-name]"))).toBe("erp-api");
    expect(text(tip.querySelector("[data-rail-tip-meta]"))).toBe("Acme · Clientes · main");
    expect(text(tip.querySelector("[data-rail-tip-status]"))).toBe("1 trabalhando");

    erp!.dispatchEvent(new MouseEvent("mouseleave", { bubbles: false }));
    flushSync();
    expect(el.querySelector("[data-rail-tooltip]")).toBeNull();

    herdr!.dispatchEvent(new FocusEvent("focus", { bubbles: false }));
    flushSync();
    expect(text(el.querySelector("[data-rail-tip-name]"))).toBe("herdr");
    expect(text(el.querySelector("[data-rail-tip-status]"))).toBe("1 aguardando");
  });

  // Would catch: the rail inventing its own navigation (a second workspace_focus, an endpoint
  // switch that skips the controller) instead of the path the open row uses.
  it("clicking an avatar focuses the workspace through the row's own controller path", async () => {
    const { el, focusWorkspace } = await render();
    avatars(el)[1]!.click();
    flushSync();
    expect(focusWorkspace).toHaveBeenCalledTimes(1);
    expect(focusWorkspace).toHaveBeenCalledWith("local", "w-web");
  });

  // Would catch: the inbox button toggling the sidebar (closing it when it was auto-collapsed)
  // instead of opening it.
  it("clicking Precisa de você opens the sidebar", async () => {
    const { el, calls } = await render();
    el.querySelector<HTMLButtonElement>("[data-rail-item='inbox']")!.click();
    flushSync();
    expect(calls).toEqual(["showProjects"]);
  });
});

describe("AC-047-05 the host icon is a line icon, never an emoji", () => {
  const EMOJI = /\p{Extended_Pictographic}/u;

  // Would catch: the 🖥️/🖧 of the 042 capture left in the footer (evidencias/042/...png).
  it("the sidebar foot draws monitor/server as a 16 px svg in --text-muted", async () => {
    const { ctx } = await render();
    for (const [endpoint, icon] of [
      ["local", "monitor"],
      ["ssh-dev", "server"],
    ] as const) {
      const el = document.createElement("div");
      document.body.appendChild(el);
      const app = mount(HostSwitcher, { target: el, props: { ctx: { ...ctx, selectedEndpoint: endpoint } as FrameContext } });
      flushSync();
      mounted.push({ el, app: app as never });
      const node = el.querySelector<HTMLElement>("[data-host-icon]")!;
      expect(node.dataset.hostIcon).toBe(icon);
      expect(node.querySelector("svg")?.getAttribute("width")).toBe("16");
      expect(el.textContent ?? "").not.toMatch(EMOJI);
    }
    expect(sources["./HostSwitcher.svelte"]!).not.toMatch(EMOJI);
    expect(rule(sources["./HostSwitcher.svelte"]!, ".icon")).toMatch(/var\(--text-muted/);
  });

  it("the rail host uses the same icon and renders no emoji", async () => {
    const { el } = await render();
    const host = el.querySelector<HTMLElement>("[data-rail-item='host']")!;
    expect(host.querySelector("[data-host-icon]")?.getAttribute("data-host-icon")).toBe("monitor");
    expect(el.textContent ?? "").not.toMatch(EMOJI);
  });
});

// Spec 049 (AC-049-04): the rail is also the way back. The user testing the real build reached
// for the herdr mark and for the items themselves to reopen the sidebar, and nothing happened —
// only "Precisa de você" expanded it. Every item now expands and still does its own job; the
// host at the foot keeps the 048 popover, which is the whole point of having it on the rail.
describe("AC-049-04 every item of the rail expands the sidebar", () => {
  // Would catch: the mark left as an inert `<span>` (the 047 rail), or a mark that expands twice.
  it("opens the sidebar from the herdr mark, once", async () => {
    const { el, calls } = await render();
    const brand = el.querySelector<HTMLButtonElement>("[data-rail-item='brand']")!;
    expect(brand.tagName).toBe("BUTTON");
    expect(brand.getAttribute("aria-label")).toBe("Expandir lateral");
    brand.click();
    flushSync();
    expect(calls).toEqual(["showProjects"]);
  });

  // Would catch: an item that expands and swallows its own action, or one that acts without
  // expanding (the 047 behaviour the user reported).
  it("opens the sidebar from Novo agente and still opens the 017 popover", async () => {
    const { el, calls } = await render();
    expect(el.querySelector("[data-new-agent-popover]")).toBeNull();
    el.querySelector<HTMLButtonElement>("[data-rail-item='newAgent']")!.click();
    flushSync();
    expect(calls).toEqual(["showProjects"]);
    expect(el.querySelector("[data-new-agent-popover]")).toBeTruthy();
  });

  // Spec 056 (AC-056-02): the rail no longer carries Buscar, so from the collapsed window the
  // palette is the Ctrl K chord (⌘K on macOS) the window itself claims. Would catch: Ctrl K
  // unclaimed, bound to the wrong modifier on macOS, or the window dropping the palette branch.
  it("leaves the palette to the Ctrl K chord (⌘K on macOS)", () => {
    const key = { key: "k", ctrlKey: true, metaKey: false, shiftKey: false, altKey: false, isComposing: false, repeat: false, target: null };
    expect(resolveShortcut(key, "linux")).toBe("palette");
    expect(resolveShortcut(key, "windows")).toBe("palette");
    expect(resolveShortcut({ ...key, ctrlKey: false, metaKey: true }, "macos")).toBe("palette");
    expect(appSource).toMatch(/id === "palette"/);
    expect(appSource).toMatch(/openPalette\(\)/);
  });

  it("opens the sidebar from Precisa de você", async () => {
    const { el, calls } = await render();
    el.querySelector<HTMLButtonElement>("[data-rail-item='inbox']")!.click();
    flushSync();
    expect(calls).toEqual(["showProjects"]);
  });

  // Would catch: an avatar that expands but stops focusing, or focuses without expanding.
  it("opens the sidebar from an avatar and still focuses the workspace by the row's path", async () => {
    const { el, calls, focusWorkspace } = await render();
    avatars(el)[1]!.click();
    flushSync();
    expect(calls).toEqual(["showProjects"]);
    expect(focusWorkspace).toHaveBeenCalledTimes(1);
    expect(focusWorkspace).toHaveBeenCalledWith("local", "w-web");
  });

  // Would catch: the foot host expanding the sidebar, which would close the very popover it
  // opens (048) before the user can pick a host.
  it("keeps the foot host on its popover without expanding", async () => {
    const { el, calls } = await render();
    el.querySelector<HTMLButtonElement>("[data-rail-item='host'] [data-host-trigger]")!.click();
    flushSync();
    expect(el.querySelector("[data-host-popover]")).toBeTruthy();
    expect(calls).toEqual([]);
  });
});
