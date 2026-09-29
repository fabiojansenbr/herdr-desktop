// @vitest-environment happy-dom
// Spec 060 — the remote hosts go back where they were and the reconnection rides on the row. The
// 041 organization is the one again: `Sem coleção` holds every loose workspace, of Local and of the
// SSH hosts, and a remote workspace put in a collection shows there with its host tag (AC-060-01).
// What the 059 section made visible stays, now on the line itself: the host tag carries the phase
// of the connection (AC-060-02), and a host still opening its connection with no snapshot at all
// shows a single waiting line under `Sem coleção` (AC-060-03).
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentDto } from "../../agents/types";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto, SshProfile } from "../../connections/types";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import { createProjectsController } from "../../projects/controller";
import type { CollectionDto, ProjectDto } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder, type ReactiveHolder } from "../center/reactive-test-state.svelte";
import { buildSidebarSections, UNGROUPED_ID } from "./sidebar-model";
import SidebarV2 from "./SidebarV2.svelte";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 041/048). */
function rule(source: string, selector: string): string {
  const style = source.includes("<style>") ? source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>")) : source;
  for (const m of style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1]!.split(",").map((s) => s.trim()).includes(selector)) return m[2]!;
  }
  return "";
}

const order = (el: HTMLElement, selector: string) => [...el.querySelectorAll<HTMLElement>(selector)];

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

afterEach(() => {
  for (const host of mounted.splice(0)) {
    unmount(host.app as never);
    host.el.remove();
  }
});

beforeEach(() => {
  localStorage.clear();
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
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd060", online: true, identity: null },
    status: null,
    phase: "live",
    reason: null,
    error: null,
    identity: null,
    surfaceKey: 1,
    busy: false,
  };
}

function connectionsState(hosts: readonly HostDto[], profiles: readonly SshProfile[]): ConnectionsState {
  return {
    view: { hub: { revision: 1, hosts: [...hosts] }, profiles: [...profiles], store_error: null },
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

const PROFILE = (id: string, over: Partial<SshProfile> = {}): SshProfile => ({
  id,
  label: id,
  target: `user@${id}`,
  port: null,
  session: "hd060",
  connect_on_open: true,
  resume_on_open: false,
  ...over,
});

const LOCAL = (over: Partial<HostDto> = {}) =>
  hostFixture({
    endpoint: "local",
    label: "Este computador",
    kind: "local",
    target: null,
    phase: "online",
    phase_label: "Online",
    session: "hd060",
    workspaces: [workspace({ workspace_id: "w-web", number: 1, label: "web", cwd: "/w/web", branch: "main", focused: true })],
    ...over,
  });

const MAC = (over: Partial<HostDto> = {}) =>
  hostFixture({
    endpoint: "ssh-mac",
    label: "mac-mini",
    kind: "ssh",
    phase: "online",
    phase_label: "Online",
    session: "hd060",
    latency_ms: 42,
    workspaces: [
      workspace({ workspace_id: "w-lib", number: 1, label: "lib", cwd: "/w/lib", branch: "master" }),
      workspace({ workspace_id: "w-api", number: 2, label: "api", cwd: "/w/api", branch: "main" }),
    ],
    ...over,
  });

interface RenderOptions {
  hosts?: readonly HostDto[] | ReactiveHolder<readonly HostDto[]>;
  profiles?: readonly SshProfile[];
  projects?: readonly ProjectDto[];
  collections?: readonly CollectionDto[];
  agents?: readonly AgentDto[];
  selected?: string | ReactiveHolder<string | null>;
}

/** AC-060-01 data: Local with the loose `web`, `mac-mini` with `lib` in `G1` and the loose `api`. */
interface Fixture extends RenderOptions {
  hosts: readonly HostDto[];
  profiles: readonly SshProfile[];
  projects: readonly ProjectDto[];
  collections: readonly CollectionDto[];
}

function fixture(): Fixture {
  return {
    hosts: [LOCAL(), MAC()],
    profiles: [PROFILE("ssh-mac", { label: "mac-mini" })],
    projects: [
      { id: "p-web", label: "web", endpoint_profile_id: "local", session_name: "hd060", root: "/w/web", binding: null },
      { id: "p-lib", label: "lib", endpoint_profile_id: "ssh-mac", session_name: "hd060", root: "/w/lib", binding: null },
      { id: "p-api", label: "api", endpoint_profile_id: "ssh-mac", session_name: "hd060", root: "/w/api", binding: null },
    ],
    collections: [{ id: "g1", name: "G1", project_ids: ["p-lib"] }],
  };
}

/**
 * AC-060-03 data: `mac-mini` is still dialling and carries no snapshot at all (no live workspace
 * and no saved project of its own), and Local's only workspace is in `G1` — so `Sem coleção` has
 * no row of its own and the waiting line is the only thing it can show.
 */
function waitingFixture(over: Partial<HostDto> = {}): Fixture {
  return {
    hosts: [LOCAL(), MAC({ phase: "connecting", phase_label: "Conectando", workspaces: [], latency_ms: null, ...over })],
    profiles: [PROFILE("ssh-mac", { label: "mac-mini" })],
    projects: [{ id: "p-web", label: "web", endpoint_profile_id: "local", session_name: "hd060", root: "/w/web", binding: null }],
    collections: [{ id: "g1", name: "G1", project_ids: ["p-web"] }],
  };
}

async function render(options: RenderOptions = {}) {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-060",
    seed: { version: 1, projects: [...(options.projects ?? [])], collections: [...(options.collections ?? [])] },
  });
  const controller = createProjectsController(bridge);
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
  const holder = typeof options.selected === "string" || options.selected === undefined ? reactiveHolder<string | null>(options.selected ?? "local") : options.selected;
  const focusWorkspace = vi.spyOn(controller, "focusWorkspace");
  const connections = {
    openDialog: vi.fn(() => {}),
    connect: vi.fn(async () => {}),
    disconnect: vi.fn(async () => {}),
    reconnect: vi.fn(async () => {}),
    removeProfile: vi.fn(async () => {}),
    editProfile: vi.fn(async () => {}),
    setConnectOnOpen: vi.fn(async () => {}),
  };
  const hostsHolder =
    options.hosts === undefined || Array.isArray(options.hosts)
      ? reactiveHolder<readonly HostDto[]>((options.hosts as readonly HostDto[] | undefined) ?? [LOCAL(), MAC()])
      : (options.hosts as ReactiveHolder<readonly HostDto[]>);
  const profileList = options.profiles ?? [PROFILE("ssh-mac", { label: "mac-mini" })];
  const ctx: FrameContext = {
    surface: surface(),
    agents: { agents: [...(options.agents ?? [])], kinds: [] } as unknown as FrameContext["agents"],
    get connections() {
      return connectionsState(hostsHolder.value, profileList);
    },
    get navigator() {
      return controller.state;
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
      connections: connections as unknown as FrameContext["controllers"]["connections"],
    },
    actions,
    unavailable: { split: null, newTab: null, newAgent: null },
  } as unknown as FrameContext;

  const el = document.createElement("div");
  el.style.height = "800px";
  document.body.appendChild(el);
  const app = mount(SidebarV2, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, calls, connections, controller, focusWorkspace, holder, hostsHolder };
}

const workspaces = (el: HTMLElement) => el.querySelector<HTMLElement>("[data-sidebar-workspaces]")!;
const group = (el: HTMLElement, id: string) => workspaces(el).querySelector<HTMLElement>(`[data-collection="${id}"]`);
const rowsOf = (scope: HTMLElement) => order(scope, "[data-workspace-row]").map((r) => r.dataset.workspaceRow);
const row = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-workspace-row="${id}"]`)!;
const badge = (el: HTMLElement, id: string) => row(el, id).querySelector<HTMLElement>("[data-host-badge]");
const waitingRows = (el: HTMLElement) => order(workspaces(el), "[data-host-waiting]");

describe("AC-060-01 the 041 organization is back", () => {
  // Would catch: the 059 section still mounted, or the loose remote row kept out of `Sem coleção`.
  it("keeps lib in G1 and puts web and api back in Sem coleção, with no HOSTS REMOTOS section", async () => {
    const { el } = await render(fixture());
    expect(el.querySelector("[data-sidebar-remote-hosts]")).toBeNull();
    expect(document.querySelector("[data-remote-host]")).toBeNull();
    // The component itself is gone, and nothing composes it any more.
    expect(sources["./RemoteHostsSection.svelte"]).toBeUndefined();
    expect(sources["./SidebarV2.svelte"]!).not.toContain("RemoteHostsSection");

    const g1 = group(el, "g1")!;
    expect(rowsOf(g1)).toEqual(["w-lib"]);
    expect(g1.querySelector<HTMLElement>('[data-workspace-row="w-lib"] [data-host-badge]')!.textContent).toContain("mac-mini");

    const loose = group(el, UNGROUPED_ID)!;
    // The 041 order inside a bucket: Local before SSH.
    expect(rowsOf(loose)).toEqual(["w-web", "w-api"]);
    expect(loose.querySelector<HTMLElement>('[data-workspace-row="w-api"] [data-host-badge]')!.textContent).toContain("mac-mini");
    expect(loose.querySelector('[data-workspace-row="w-web"] [data-host-badge]')).toBeNull();
    expect(loose.querySelector<HTMLElement>("[data-collection-count]")!.textContent!.trim()).toBe("2");
  });

  // Would catch: the sidebar body keeping a block between Workspaces and the host selector.
  it("composes Workspaces straight into the host selector", async () => {
    const { el } = await render(fixture());
    const blocks = [...el.querySelectorAll<HTMLElement>("[data-sidebar-workspaces], [data-sidebar-remote-hosts], [data-sidebar-hosts]")];
    expect(blocks.map((b) => (b.dataset.sidebarWorkspaces === "" ? "workspaces" : b.dataset.sidebarRemoteHosts === "" ? "remote" : "hosts"))).toEqual([
      "workspaces",
      "hosts",
    ]);
  });

  // The model itself: no loose row leaves `Sem coleção` any more, and there is no block list.
  it("keeps every loose row in Sem coleção in buildSidebarSections", async () => {
    const options = fixture();
    const sections = buildSidebarSections({
      hosts: options.hosts,
      groups: options.collections,
      projects: options.projects,
      selectedEndpoint: "local",
    });
    expect(sections.collections.map((s) => s.id)).toEqual(["g1", UNGROUPED_ID]);
    expect(sections.collections[0]!.rows.map((r) => [r.name, r.hostBadge])).toEqual([["lib", "mac-mini"]]);
    expect(sections.collections[1]!.rows.map((r) => [r.name, r.hostBadge])).toEqual([
      ["web", null],
      ["api", "mac-mini"],
    ]);
    expect((sections as unknown as Record<string, unknown>).remote).toBeUndefined();
  });
});

describe("AC-060-02 the connection state on the row", () => {
  // Would catch: a reconnecting tag left on a live host, or the row refusing the focus click.
  it("shows only the host name while the host is online and takes the focus click", async () => {
    const { el, focusWorkspace } = await render(fixture());
    const api = row(el, "w-api");
    expect(badge(el, "w-api")!.textContent!.trim()).toBe("mac-mini");
    expect(badge(el, "w-api")!.dataset.hostState).toBe("online");
    expect(api.querySelector("[data-host-spinner]")).toBeNull();
    expect(api.textContent).not.toContain("offline");
    expect(api.textContent).not.toContain("reconectando");
    expect(api.getAttribute("data-offline")).toBeNull();
    expect(api.getAttribute("data-stale")).toBeNull();
    api.click();
    flushSync();
    expect(focusWorkspace).toHaveBeenCalledWith("ssh-mac", "w-api");
  });

  // Would catch: the cached line shown as if it were live, or a still tag while the host dials.
  it.each([["connecting"], ["reconnecting"]] as const)("shows a spinner and reconectando… on the tag while %s", async (phase) => {
    const { el, focusWorkspace } = await render({ ...fixture(), hosts: [LOCAL(), MAC({ phase, phase_label: "Reconectando" })] });
    const api = row(el, "w-api");
    const tag = badge(el, "w-api")!;
    expect(tag.dataset.hostState).toBe("reconnecting");
    expect(tag.textContent).toContain("mac-mini");
    expect(tag.textContent).toContain("reconectando…");
    // Spec 068 (PRD i18n, P3): the title is the phase in the user's language (`phaseText`), not
    // the engine's `phase_label` — so `connecting` says Conectando even when the host labelled it
    // otherwise. Only the expected value changed here; the assertion is the same one.
    expect(tag.getAttribute("title")).toBe(phase === "connecting" ? "Conectando" : "Reconectando");
    expect(tag.querySelector("[data-host-spinner]")).toBeTruthy();
    expect(api.dataset.stale).toBe("true");
    expect(api.textContent).not.toContain("offline");
    expect(api.getAttribute("aria-label")).toContain("reconectando…");
    // The cached line takes no focus click while the connection is not live.
    api.click();
    flushSync();
    expect(focusWorkspace).not.toHaveBeenCalled();
    expect(rule(sources["./WorkspaceItem.svelte"]!, '.row[data-stale="true"]')).toMatch(/opacity:/);
    expect(rule(sources["./WorkspaceItem.svelte"]!, ".badge-spinner")).toMatch(/animation:\s*spin/);
  });

  // Would catch: the 041 offline line changed by this spec (it stays exactly as it was).
  it("keeps offline as it was: the offline tag and the dimmed line", async () => {
    const { el, focusWorkspace } = await render({
      ...fixture(),
      hosts: [LOCAL(), MAC({ phase: "offline", phase_label: "Offline", latency_ms: null })],
    });
    const api = row(el, "w-api");
    expect(api.dataset.offline).toBe("true");
    expect(api.getAttribute("data-stale")).toBeNull();
    expect(api.textContent).toContain("offline");
    expect(badge(el, "w-api")!.dataset.hostState).toBe("offline");
    expect(api.querySelector("[data-host-spinner]")).toBeNull();
    expect(api.getAttribute("aria-label")).toContain("offline");
    api.click();
    flushSync();
    expect(focusWorkspace).not.toHaveBeenCalled();
    expect(rule(sources["./WorkspaceItem.svelte"]!, '.row[data-offline="true"]')).toMatch(/opacity:/);
  });

  // Would catch: a host that needs attention shown as a plain offline one, with no reason at hand.
  it("shows an amber atenção tag with the phase_label as its title", async () => {
    const { el } = await render({
      ...fixture(),
      hosts: [LOCAL(), MAC({ phase: "attention", phase_label: "Precisa de atenção", attention: "host_key_unknown", latency_ms: null })],
    });
    const tag = badge(el, "w-api")!;
    expect(tag.dataset.hostState).toBe("attention");
    expect(tag.textContent).toContain("mac-mini");
    expect(tag.textContent).toContain("atenção");
    expect(tag.getAttribute("title")).toBe("Precisa de atenção");
    expect(row(el, "w-api").textContent).not.toContain("offline");
    expect(row(el, "w-api").querySelector("[data-host-spinner]")).toBeNull();
    expect(row(el, "w-api").getAttribute("aria-label")).toContain("atenção");
    expect(rule(sources["./WorkspaceItem.svelte"]!, '[data-host-badge][data-host-state="attention"]')).toMatch(/var\(--attention/);
  });

  // Would catch: the line left dimmed (or the tag left reconnecting) after the host comes back.
  it("returns the line to normal when the host is online again", async () => {
    const hosts = reactiveHolder<readonly HostDto[]>([LOCAL(), MAC({ phase: "reconnecting", phase_label: "Reconectando", cached: true })]);
    const { el, hostsHolder, focusWorkspace } = await render({ ...fixture(), hosts });
    expect(rowsOf(group(el, UNGROUPED_ID)!)).toEqual(["w-web", "w-api"]);
    expect(row(el, "w-api").dataset.stale).toBe("true");

    hostsHolder.value = [LOCAL(), MAC()];
    flushSync();
    const api = row(el, "w-api");
    expect(api.getAttribute("data-stale")).toBeNull();
    expect(api.getAttribute("data-offline")).toBeNull();
    expect(badge(el, "w-api")!.textContent!.trim()).toBe("mac-mini");
    expect(api.querySelector("[data-host-spinner]")).toBeNull();
    api.click();
    flushSync();
    expect(focusWorkspace).toHaveBeenCalledWith("ssh-mac", "w-api");
  });
});

describe("AC-060-03 the waiting line without a snapshot", () => {
  // Would catch: nothing at all while the first connection opens, or a line with an action on it.
  it("shows a single mac-mini · Carregando workspaces… line under Sem coleção", async () => {
    const { el } = await render(waitingFixture());
    const loose = group(el, UNGROUPED_ID)!;
    expect(loose).toBeTruthy();
    expect(rowsOf(loose)).toEqual([]);
    const lines = waitingRows(el);
    expect(lines).toHaveLength(1);
    expect(lines[0]!.dataset.hostWaiting).toBe("ssh-mac");
    expect(lines[0]!.querySelector(".waiting-label")!.textContent!.trim()).toBe("mac-mini · Carregando workspaces…");
    expect(lines[0]!.querySelector("[data-host-waiting-spinner]")).toBeTruthy();
    expect(lines[0]!.querySelector("button")).toBeNull();
    expect(lines[0]!.querySelector("[data-workspace-row]")).toBeNull();
    // It belongs to `Sem coleção`: the list sits right under that bucket, nowhere else.
    expect(el.querySelector<HTMLElement>("[data-ungrouped-waiting]")!.previousElementSibling).toBe(loose);
    expect(rule(sources["./WorkspacesSection.svelte"]!, ".waiting-spinner")).toMatch(/animation:\s*spin/);
  });

  // Would catch: `Nenhum workspace` claimed over the very line that says the host is coming.
  it("does not call the section empty while a host is still opening its connection", async () => {
    const { el } = await render({
      hosts: [LOCAL({ workspaces: [] }), MAC({ phase: "connecting", phase_label: "Conectando", workspaces: [], latency_ms: null })],
      profiles: [PROFILE("ssh-mac", { label: "mac-mini" })],
    });
    expect(waitingRows(el)).toHaveLength(1);
    expect(workspaces(el).textContent).not.toContain("Nenhum workspace");
  });

  // Would catch: the waiting line left behind once the first snapshot arrives.
  it("drops the waiting line and shows the rows as soon as the host is online", async () => {
    const hosts = reactiveHolder<readonly HostDto[]>([LOCAL(), MAC({ phase: "connecting", phase_label: "Conectando", workspaces: [], latency_ms: null })]);
    const { el, hostsHolder } = await render({ ...waitingFixture(), hosts });
    expect(waitingRows(el)).toHaveLength(1);
    expect(rowsOf(group(el, UNGROUPED_ID)!)).toEqual([]);

    hostsHolder.value = [LOCAL(), MAC()];
    flushSync();
    expect(waitingRows(el)).toHaveLength(0);
    expect(rowsOf(group(el, UNGROUPED_ID)!)).toEqual(["w-lib", "w-api"]);
  });

  // Would catch: a waiting line that outlives the attempt, claiming a connection that failed.
  it.each([
    ["offline", "Offline"],
    ["attention", "Precisa de atenção"],
  ] as const)("drops the waiting line when the host ends up %s", async (phase, phase_label) => {
    const hosts = reactiveHolder<readonly HostDto[]>([LOCAL(), MAC({ phase: "connecting", phase_label: "Conectando", workspaces: [], latency_ms: null })]);
    const { el, hostsHolder } = await render({ ...waitingFixture(), hosts });
    expect(waitingRows(el)).toHaveLength(1);
    hostsHolder.value = [LOCAL(), MAC({ phase, phase_label, workspaces: [], latency_ms: null })];
    flushSync();
    expect(waitingRows(el)).toHaveLength(0);
  });

  // Would catch: a waiting line drawn for every saved host that simply has no snapshot yet.
  it("draws no waiting line for a saved host that is not (re)connecting", async () => {
    const { el } = await render({ ...waitingFixture({ phase: "offline", phase_label: "Offline" }) });
    expect(waitingRows(el)).toHaveLength(0);
    expect(el.querySelector("[data-ungrouped-waiting]")).toBeNull();
  });

  // The model itself: the bucket shows up for the waiting line alone, and does not count it.
  it("reports the waiting host in buildSidebarSections", async () => {
    const options = waitingFixture();
    const sections = buildSidebarSections({
      hosts: options.hosts,
      groups: options.collections,
      projects: options.projects,
      selectedEndpoint: "local",
    });
    expect(sections.waiting).toEqual([{ endpoint: "ssh-mac", name: "mac-mini", label: "mac-mini · Carregando workspaces…" }]);
    const loose = sections.collections.find((s) => s.id === UNGROUPED_ID)!;
    expect(loose).toBeTruthy();
    expect([loose.rows.length, loose.count]).toEqual([0, 0]);

    const online = buildSidebarSections({
      hosts: [LOCAL(), MAC()],
      groups: options.collections,
      projects: options.projects,
      selectedEndpoint: "local",
    });
    expect(online.waiting).toEqual([]);
  });
});
