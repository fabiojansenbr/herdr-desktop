// @vitest-environment happy-dom
// Spec 068 (PRD i18n) — the sidebar and the projects tree in the three languages. The 067 base
// gives `t()`, the plurals, `phaseText` and the `untranslated()` detector; this spec moves every
// visible string of the area into `src/i18n/areas/sidebar.ts` and `projects.ts`.
//
// Would catch: a label left in Portuguese in English (the detector reports it with its own
// words), a Spanish dictionary missing a key (it would fall back to English), an engine
// `phase_label` leaking into a tag title instead of `phaseText`, a count printed with the
// English plural rule in Spanish, and the Portuguese wording drifting — the whole suite is fixed
// on `pt` (AC-067-04) and the existing assertions of the area must keep passing untouched.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentsState } from "../../agents/reducer";
import type { AgentDto } from "../../agents/types";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostAgentDto, HostDto, HostTabDto, HostWorkspaceDto, SshProfile } from "../../connections/types";
import { setLocalePreference, type Locale } from "../../i18n/index.svelte";
import { untranslated } from "../../i18n/testing";
import { createProjectsController, type ProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import type { CollectionDto, ProjectDto } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import type { MenuAction } from "../frame/menus";
import ConnectionsFooter from "../projects/ConnectionsFooter.svelte";
import ProjectTree from "../projects/ProjectTree.svelte";
import { buildConnectionItems, type ConnectionItem } from "../projects/tree-model";
import { expandedKey } from "./expanded";
import NewWorkspaceDialog from "./NewWorkspaceDialog.svelte";
import SidebarRail from "./SidebarRail.svelte";
import SidebarV2 from "./SidebarV2.svelte";

const NOW = 1_700_000_000_000;

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
  // Every portalled menu lives on `document.body`; nothing may survive into the next language.
  for (const layer of Array.from(document.body.querySelectorAll("[data-workspace-menu],[data-collection-menu]"))) layer.remove();
  setLocalePreference("pt");
  vi.useRealTimers();
});

beforeEach(() => {
  localStorage.clear();
});

const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

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

function hostTab(workspaceId: string, index: number, label: string): HostTabDto {
  return {
    tab_id: `${workspaceId}:t${index}`,
    workspace_id: workspaceId,
    number: index,
    label,
    custom_label: false,
    focused: index === 1,
    zoomed: false,
    pane_count: 1,
    agent_status: "idle",
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

/**
 * The scenario of AC-068-01: Local with the focused `api` (one tab) and the closed project `old`,
 * `mac-mini` reconnecting with the cached `lib` (two tabs) inside the user's collection, and
 * `build-box` still dialling with no snapshot at all — the host `Sem coleção` waits for. Every
 * engine-owned name is neutral on purpose: the detector must only ever report our own labels.
 */
const LOCAL = (): HostDto =>
  hostFixture({
    endpoint: "local",
    label: "workstation",
    kind: "local",
    target: null,
    phase: "online",
    phase_label: "Online",
    session: "hd068",
    workspaces: [workspace({ workspace_id: "w-api", label: "api", number: 1, cwd: "/w/api", branch: "main", focused: true })],
    agents: [hostAgent({ pane_id: "w-api:p1", agent: "codex", agent_status: "blocked" })],
  });

const MAC = (): HostDto =>
  hostFixture({
    endpoint: "ssh-mac",
    label: "mac-mini",
    kind: "ssh",
    phase: "reconnecting",
    phase_label: "Reconectando",
    session: "hd068",
    latency_ms: null,
    workspaces: [workspace({ workspace_id: "w-lib", label: "lib", number: 1, cwd: "/w/lib", branch: "master" })],
    tabs: [hostTab("w-lib", 1, "build"), hostTab("w-lib", 2, "test")],
    agents: [hostAgent({ pane_id: "w-lib:p1", agent: "claude", agent_status: "done" })],
  });

const BUILD = (): HostDto =>
  hostFixture({
    endpoint: "ssh-build",
    label: "build-box",
    kind: "ssh",
    phase: "connecting",
    phase_label: "Conectando",
    session: "hd068",
    latency_ms: null,
    workspaces: [],
  });

const PROJECTS: readonly ProjectDto[] = [
  { id: "p-api", label: "api", endpoint_profile_id: "local", session_name: "hd068", root: "/w/api", binding: null },
  { id: "p-old", label: "old", endpoint_profile_id: "local", session_name: "hd068", root: "/w/old", binding: null },
  { id: "p-lib", label: "lib", endpoint_profile_id: "ssh-mac", session_name: "hd068", root: "/w/lib", binding: null },
];

const COLLECTIONS: readonly CollectionDto[] = [{ id: "g1", name: "G1", project_ids: ["p-lib"] }];

const PROFILES: readonly SshProfile[] = [
  { id: "ssh-mac", label: "mac-mini", target: "user@mac-mini", port: null, session: "hd068", connect_on_open: true, resume_on_open: false },
  { id: "ssh-build", label: "build-box", target: "user@build-box", port: null, session: "hd068", connect_on_open: true, resume_on_open: false },
];

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd068", online: true, identity: null },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  } as SurfaceState;
}

function connectionsState(hosts: readonly HostDto[]): ConnectionsState {
  return {
    view: { hub: { revision: 1, hosts: [...hosts] }, profiles: [...PROFILES], store_error: null },
    loading: false,
    globalError: null,
    hostErrors: {},
    inputs: {},
    sending: {},
    dialog: { open: false, draft: { id: null, label: "", target: "", port: "", session: "", auth: "key" }, errors: {}, submitting: false, submitError: null, connecting: null },
    importReport: null,
    workspaces: {},
  } as unknown as ConnectionsState;
}

/** Live state of the attached host: `api` publishes one tab, so its list counts one. */
function agentsState(): AgentsState {
  const agents: AgentDto[] = [
    { pane_id: "w-api:p1", workspace_id: "w-api", tab_id: "w-api:t1", name: "codex", kind: "codex", status: "blocked", launch_pending: false, ready: true, focused: true, state_change_seq: 1 },
  ];
  return {
    // Spec 074: the live list is read only while the connection is `connected`, as it is here.
    phase: "connected",
    agents,
    tabs: [{ tab_id: "w-api:t1", workspace_id: "w-api", label: "shell", number: 1, focused: true, pane_count: 1, agent_status: "blocked" }],
    tabFocus: { tab_id: "w-api:t1" },
    identity: { endpoint: "local" },
    topology: null,
    kinds: ["claude", "codex"],
    serverVersion: "0.9.0",
    start: { error: null },
    capabilities: {},
  } as unknown as AgentsState;
}

async function makeCtx(
  options: { hosts?: readonly HostDto[]; selected?: string } = {},
): Promise<{ ctx: FrameContext; controller: ProjectsController }> {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-068",
    seed: { version: 2, projects: [...PROJECTS], collections: [...COLLECTIONS] },
  });
  const controller = createProjectsController(bridge);
  await controller.load();
  const hosts = reactiveHolder<readonly HostDto[]>(options.hosts ?? [LOCAL(), MAC(), BUILD()]);
  const selected = reactiveHolder<string | null>(options.selected ?? "local");
  const ctx = {
    surface: surface(),
    agents: agentsState(),
    get connections() {
      return connectionsState(hosts.value);
    },
    get navigator() {
      return controller.state;
    },
    get selectedEndpoint() {
      return selected.value;
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
      agents: {
        createTab: vi.fn(async () => {}),
        focusTab: vi.fn(async () => {}),
        state: { topology: null, start: { error: null } },
        editStart: vi.fn(() => {}),
        startAgent: vi.fn(async () => {}),
      } as unknown as FrameContext["controllers"]["agents"],
      projects: controller,
      connections: {
        openDialog: vi.fn(() => {}),
        connect: vi.fn(async () => {}),
        disconnect: vi.fn(async () => {}),
        reconnect: vi.fn(async () => {}),
        removeProfile: vi.fn(async () => {}),
        editProfile: vi.fn(async () => {}),
        setConnectOnOpen: vi.fn(async () => {}),
      } as unknown as FrameContext["controllers"]["connections"],
    },
    actions: ACTIONS,
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;
  return { ctx, controller };
}

function place(): HTMLElement {
  const el = document.createElement("div");
  el.style.height = "900px";
  document.body.appendChild(el);
  return el;
}

function show(component: unknown, props: Record<string, unknown>): HTMLElement {
  const el = place();
  const app = mount(component as never, { target: el, props });
  mounted.push({ el, app: app as never });
  return el;
}

const CONNECTION_ITEMS: readonly ConnectionItem[] = [
  { endpoint: "local", name: "workstation", typeBadge: "Local", latencyText: "Local", statusText: "Online", tone: "ok", active: true, online: true, retryable: false, tooltip: "Online" },
  { endpoint: "ssh-mac", name: "mac-mini", typeBadge: "SSH", latencyText: "42 ms", statusText: "Online", tone: "ok", active: false, online: true, retryable: false, tooltip: "herdr 0.9.1 · /usr/bin/herdr" },
];

/**
 * The whole area on screen at once, in `language`: the sidebar with its box, its collections, the
 * closed row, the reconnecting host and the waiting one, both menus open, the modal, the rail,
 * the projects tree and the connections foot.
 */
async function scenario(language: Locale): Promise<HTMLElement> {
  setLocalePreference(language);
  // AC-042/052: the two workspaces start expanded, so both tab lists (and their counts) render.
  localStorage.setItem(expandedKey("local", "/w/api"), "1");
  localStorage.setItem(expandedKey("ssh-mac", "/w/lib"), "1");

  const { ctx, controller } = await makeCtx();
  const sidebar = show(SidebarV2, { ctx });
  show(SidebarRail, { ctx, clock: () => NOW });
  show(NewWorkspaceDialog, { ctx, endpoint: null, onclose: () => {}, pickFolder: async () => null });
  show(ProjectTree, {
    controller,
    state: controller.state,
    agents: [],
    hosts: [LOCAL(), MAC(), BUILD()],
    selectedEndpoint: "local",
  });
  show(ConnectionsFooter, {
    items: CONNECTION_ITEMS,
    onSelectHost: () => {},
    onOpenDialog: () => {},
    onConnect: () => {},
    onDisconnect: () => {},
    onReconnect: () => {},
    onEdit: () => {},
    onRemove: () => {},
  });
  flushSync();

  sidebar.querySelector<HTMLElement>('[data-collection="g1"] [data-collection-menu-button]')!.click();
  sidebar.querySelector<HTMLElement>('[data-workspace-row="w-api"] [data-workspace-menu-button]')!.click();
  flushSync();
  return sidebar;
}

/** Every visible text of the window, menus and modal included. */
const allText = () => document.body.textContent ?? "";

const attributes = () =>
  Array.from(document.body.querySelectorAll("*"))
    .flatMap((node) => ["aria-label", "title", "placeholder"].map((name) => node.getAttribute(name) ?? ""))
    .join("\n");

const everything = () => `${allText()}\n${attributes()}`;

describe("AC-068-01 inglês", () => {
  // Would catch: any label of the sidebar, of the menus, of the modal, of the rail or of the tree
  // still written in Portuguese — the detector names it.
  it("renders the whole area with nothing left in Portuguese", async () => {
    await scenario("en");
    expect(untranslated(document.body)).toEqual([]);
  });

  it("shows the English words of the area", async () => {
    await scenario("en");
    const seen = everything();
    for (const word of ["Workspaces", "Needs you", "New agent", "No collection", "closed", "Loading workspaces…", "reconnecting…", "now"]) {
      expect(seen, word).toContain(word);
    }
  });
});

describe("AC-068-02 espanhol", () => {
  it("shows the Spanish words of the area", async () => {
    await scenario("es");
    const seen = everything();
    for (const word of [
      "Espacios de trabajo",
      "Te necesita",
      "Nuevo agente",
      "Sin colección",
      "cerrado",
      "Cargando espacios de trabajo…",
      "reconectando…",
      "ahora",
    ]) {
      expect(seen, word).toContain(word);
    }
  });

  // Would catch: a count built by string concatenation, which prints `1 pestañas`.
  it("counts tabs with the Spanish plural rule", async () => {
    await scenario("es");
    const labels = Array.from(document.body.querySelectorAll("[data-sidebar-tabs]")).map((list) => list.getAttribute("aria-label"));
    expect(labels.some((label) => label?.includes("1 pestaña") && !label.includes("1 pestañas"))).toBe(true);
    expect(labels.some((label) => label?.includes("2 pestañas"))).toBe(true);
  });
});

describe("AC-068-03 português preservado", () => {
  // Would catch: a key missing from `pt`, which would render the English source instead.
  it("keeps the Portuguese wording the area already had", async () => {
    await scenario("pt");
    const seen = everything();
    for (const word of ["Workspaces", "Precisa de você", "Novo agente", "Sem coleção", "fechado", "Carregando workspaces…", "reconectando…", "agora"]) {
      expect(seen, word).toContain(word);
    }
    const labels = Array.from(document.body.querySelectorAll("[data-sidebar-tabs]")).map((list) => list.getAttribute("aria-label"));
    expect(labels.some((label) => label?.includes("1 aba") && !label.includes("1 abas"))).toBe(true);
    expect(labels.some((label) => label?.includes("2 abas"))).toBe(true);
  });

  // Spec 060 read the tag title from the engine's `phase_label`; the PRD (P3) puts the phase
  // through `phaseText`, so the Spanish window never shows a Portuguese phase from the host.
  it("titles the host tag with phaseText, not with the engine phase_label", async () => {
    const sidebar = await scenario("es");
    const badge = sidebar.querySelector<HTMLElement>('[data-workspace-row="w-lib"] [data-host-badge]')!;
    expect(badge.getAttribute("title")).toBe("Reconectando");
  });
});

/**
 * Spec 068 (PRD i18n, P3) — the connection phase is the product's own word, not the sentence the
 * engine puts in `phase_label`. That field stays a datum (the hub publishes it, spec 071 turns it
 * into English); nothing in the sidebar or in the projects tree renders it.
 */
const ENGINE_PHASE_LABEL = "Reconnecting (attempt 3)";

const PHASE_HOSTS = (): HostDto[] => [
  hostFixture({
    endpoint: "local",
    label: "workstation",
    kind: "local",
    target: null,
    phase: "offline",
    phase_label: ENGINE_PHASE_LABEL,
    session: "hd068",
    workspaces: [],
  }),
  hostFixture({
    endpoint: "ssh-mac",
    label: "mac-mini",
    kind: "ssh",
    phase: "reconnecting",
    phase_label: ENGINE_PHASE_LABEL,
    session: "hd068",
    latency_ms: null,
    workspaces: [],
  }),
  // `attention` without a `connection_error`: no failure reason to replace the phase (AC-029-02).
  hostFixture({
    endpoint: "ssh-build",
    label: "build-box",
    kind: "ssh",
    phase: "attention",
    phase_label: ENGINE_PHASE_LABEL,
    session: "hd068",
    latency_ms: null,
    workspaces: [],
  }),
];

/** Local offline, `mac-mini` reconnecting, `build-box` needing attention, in each language. */
const PHASE_STATUS: Record<Locale, readonly [string, string, string]> = {
  pt: ["Offline", "Reconectando", "Precisa de atenção"],
  en: ["Offline", "Reconnecting", "Needs attention"],
  es: ["Sin conexión", "Reconectando", "Necesita atención"],
};

const FOOTER_HANDLERS = {
  onSelectHost: () => {},
  onOpenDialog: () => {},
  onConnect: () => {},
  onDisconnect: () => {},
  onReconnect: () => {},
  onEdit: () => {},
  onRemove: () => {},
};

describe("AC-068-01/02/03 a fase do host vem de phaseText, nunca de phase_label", () => {
  // Would catch: `buildConnectionItems` reading `host.phase_label` again — the row would then say
  // the engine's sentence, which is Portuguese today and English (in every language) after 071.
  it.each(["pt", "en", "es"] as const)("says the phase of every host in %s", (language) => {
    setLocalePreference(language);
    const items = buildConnectionItems(PHASE_HOSTS(), "local");
    expect(items.map((item) => item.statusText)).toEqual([...PHASE_STATUS[language]]);
    // Without a discovered binary the tooltip is the status text, so it must not leak it either.
    expect(items.map((item) => item.tooltip)).toEqual([...PHASE_STATUS[language]]);
    expect(items.some((item) => item.statusText.includes(ENGINE_PHASE_LABEL))).toBe(false);
  });

  // The two places the area shows a phase: the dot of the connections foot and the host line at
  // the sidebar foot (`HostSwitcher`, whose title used to be `selected.phase_label`).
  it.each(["pt", "en", "es"] as const)("shows it in the connections foot and on the host line in %s", async (language) => {
    setLocalePreference(language);
    const [offline, reconnecting, attention] = PHASE_STATUS[language];
    const { ctx } = await makeCtx({ hosts: PHASE_HOSTS(), selected: "ssh-mac" });
    const footer = show(ConnectionsFooter, { items: buildConnectionItems(PHASE_HOSTS(), "ssh-mac"), ...FOOTER_HANDLERS });
    const sidebar = show(SidebarV2, { ctx });
    flushSync();

    const dots = Array.from(footer.querySelectorAll<HTMLElement>(".state-dot"));
    expect(dots.map((dot) => dot.getAttribute("title"))).toEqual([offline, reconnecting, attention]);
    expect(dots.map((dot) => dot.getAttribute("aria-label"))).toEqual([offline, reconnecting, attention]);
    expect(sidebar.querySelector("[data-host-trigger]")!.getAttribute("title")).toBe(reconnecting);
    expect(everything()).not.toContain(ENGINE_PHASE_LABEL);
  });
});
