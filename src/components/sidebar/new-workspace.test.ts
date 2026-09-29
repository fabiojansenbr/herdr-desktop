// @vitest-environment happy-dom
// Spec 046 — "Novo workspace": the `+` of the WORKSPACES header and the palette command open one
// modal where the host is chosen (P3: a remote folder is typed, never browsed), the recents of
// that host are offered (store v4) and the creation runs the existing chain in order (P4):
// the 037 switch, `workspace_create`, `agent_start` on the pane the host confirmed and
// `group_assign`. AC-046-04 closes the 016 defect: the native picker never opens for an SSH host.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({ invoked: [] as string[], pick: null as string | null }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    tauri.invoked.push(command);
    return Promise.resolve(command === "project_pick_folder" ? tauri.pick : null);
  },
  Channel: class {
    onmessage: unknown = null;
  },
}));

import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto, PaneDto } from "../../connections/types";
import { createProjectsController } from "../../projects/controller";
import { createFakeProjectsBridge, type RecordedCall } from "../../projects/fake-bridge";
import { createWorkspaceFlow, hostChoices, recentFoldersFor, requestNewWorkspace, SHELL_START, workspaceLabel, type NewWorkspaceDeps } from "../../projects/new-workspace";
import type { NavigatorState } from "../../projects/reducer";
import type { CollectionDto, ProjectDto, RecentFolderDto, RuntimeError } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import { paletteSections } from "../frame/palette";
import ProjectSwitcher from "../frame/ProjectSwitcher.svelte";
import type { MenuAction } from "../frame/menus";
import { sidebarVisuallyOpen, sidebarWidthPx, SIDEBAR_RAIL_PX } from "../frame/sidebar";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import NewWorkspaceHost from "./NewWorkspaceHost.svelte";
import SidebarRail from "./SidebarRail.svelte";
import SidebarV2 from "./SidebarV2.svelte";

const appSource = readFileSync(resolve("src/App.svelte"), "utf8");
const sidebarSources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

beforeEach(() => {
  tauri.invoked.length = 0;
  tauri.pick = null;
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

function pane(paneId: string, workspaceId: string): PaneDto {
  return {
    pane_id: paneId,
    workspace_id: workspaceId,
    focused: false,
    input_enabled: true,
    input_block: null,
    target: null,
  };
}

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd046", online: true, identity: null },
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
 * Local (online, one workspace), dev-box (online) and build-box, offline. `confirmOn` is the
 * host already publishing `w1` — the id the fake engine answers for the next `workspace_create`
 * — so the confirmation of the new workspace's metadata is observable on that host.
 */
function fixture(confirmOn: string | null = "local") {
  const confirmed = (endpoint: string, cwd: string) =>
    confirmOn === endpoint ? { workspaces: [workspace({ workspace_id: "w1", number: 9, label: "fiscal", cwd })], panes: [pane("w1:p1", "w1")] } : { workspaces: [], panes: [] };
  const localExtra = confirmed("local", "/w/acme/fiscal");
  const remoteExtra = confirmed("ssh-dev", "~/w/fiscal");
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd046",
      target: null,
      workspaces: [workspace({ workspace_id: "w-a", label: "a", cwd: "/w/a", focused: true }), ...localExtra.workspaces],
      panes: [pane("w-a:p1", "w-a"), ...localExtra.panes],
    }),
    hostFixture({
      endpoint: "ssh-dev",
      label: "dev-box",
      kind: "ssh",
      phase: "online",
      phase_label: "Online",
      session: "hd046-remote",
      workspaces: remoteExtra.workspaces,
      panes: remoteExtra.panes,
    }),
    hostFixture({ endpoint: "ssh-build", label: "build-box", kind: "ssh", phase: "offline", phase_label: "Offline" }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-a", label: "a", endpoint_profile_id: "local", session_name: "hd046", root: "/w/a", binding: null },
  ];
  const collections: CollectionDto[] = [{ id: "g1", name: "G1", project_ids: ["p-a"] }];
  const recents: RecentFolderDto[] = [
    { endpoint_profile_id: "ssh-dev", path: "~/w/billing-worker" },
    { endpoint_profile_id: "local", path: "/w/dotfiles" },
    { endpoint_profile_id: "ssh-dev", path: "~/w/fiscal-service" },
  ];
  return { hosts, projects, collections, recents };
}

async function render(
  options: {
    startFails?: RuntimeError;
    canStart?: boolean;
    confirmOn?: string | null;
    /** Which sidebar the window is painting: the open one, the 047 rail, or none. */
    sidebar?: "open" | "rail" | "none";
    gate?: boolean;
  } = {},
) {
  const seed = fixture(options.confirmOn === undefined ? "local" : options.confirmOn);
  const bridge = createFakeProjectsBridge({
    bootId: "boot-046",
    seed: { version: 4, projects: seed.projects, collections: seed.collections, workspace_prefs: [], recent_folders: seed.recents },
  });
  const nav = reactiveHolder<NavigatorState | null>(null);
  const controller = createProjectsController(bridge, (state) => (nav.value = state));
  await controller.load();
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [name, () => {}]),
  ) as Record<MenuAction, () => void>;
  const holder = reactiveHolder<string | null>("local");

  // The agents controller of the window, recording `agent_start` in the same list as the store
  // commands so the order of the whole chain is one assertion.
  const start = { paneId: "", kind: "", name: "", busy: false, error: null as RuntimeError | null };
  const agentsController = {
    state: { start, topology: null },
    editStart: (field: "paneId" | "kind" | "name", value: string) => {
      start[field] = value;
    },
    startAgent: async () => {
      bridge.calls.push({ command: "agent_start", args: { paneId: start.paneId, kind: start.kind, name: start.name } });
      start.error = options.startFails ?? null;
    },
    createTab: async () => {},
  };

  const ctx: FrameContext = {
    surface: surface(),
    agents: {
      agents: [],
      kinds: ["claude", "codex"],
      capabilities: { start_agent: options.canStart ?? true },
      start,
    } as unknown as FrameContext["agents"],
    connections: connectionsState(seed.hosts),
    get navigator() {
      return nav.value ?? controller.state;
    },
    get selectedEndpoint() {
      return holder.value;
    },
    activeProject: null,
    hostLabels: {},
    branch: null,
    activity: "projects",
    view: "terminal",
    sidebarOpen: true,
    agentsOpen: true,
    controllers: {
      surface: {
        select: async (endpoint: string) => {
          bridge.calls.push({ command: "select_host", args: { endpoint } });
          holder.value = endpoint;
          return {} as never;
        },
      } as unknown as FrameContext["controllers"]["surface"],
      agents: agentsController as unknown as FrameContext["controllers"]["agents"],
      projects: controller,
      connections: {
        openDialog: () => {},
        disconnect: async () => {},
        reconnect: async () => {},
        removeProfile: async () => {},
        editProfile: async () => {},
      } as unknown as FrameContext["controllers"]["connections"],
    },
    actions,
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const el = document.createElement("div");
  document.body.appendChild(el);
  // The window mounts the modal beside the frame (round 2): the sidebar is replaced by the 047
  // rail whenever it is collapsed, so whoever owns the dialog cannot live inside it.
  const attach = (component: typeof SidebarV2 | typeof SidebarRail | typeof NewWorkspaceHost) => {
    const app = mount(component as typeof NewWorkspaceHost, { target: el, props: { ctx } });
    mounted.push({ el, app: app as never });
    flushSync();
  };
  if (options.sidebar !== "none") attach(options.sidebar === "rail" ? SidebarRail : SidebarV2);
  if (options.gate !== false) attach(NewWorkspaceHost);
  bridge.calls.length = 0;
  return { el, ctx, bridge, controller, nav, selected: holder };
}

async function settle() {
  for (let i = 0; i < 40; i += 1) {
    await Promise.resolve();
    flushSync();
  }
}

const dialog = (el: HTMLElement) => el.querySelector<HTMLElement>("[data-new-workspace-dialog]");
const commands = (calls: RecordedCall[]) => calls.map((call) => call.command);

/** Runs the palette's own "Novo workspace" command, as the window would. */
function runPaletteCommand() {
  const sections = paletteSections(
    { hasSession: true, projects: [], panes: [], agents: [], commands: [], openProject: () => {}, focusPane: () => {}, openAgent: () => {} },
    "novo works",
  );
  const entry = sections.flatMap((section) => section.entries).find((item) => item.id === "command:new-workspace");
  expect(entry?.label).toBe("Novo workspace");
  entry!.run();
}

async function openDialog(el: HTMLElement) {
  el.querySelector<HTMLButtonElement>("[data-new-workspace]")!.click();
  await settle();
  return dialog(el)!;
}

function chooseCollection(box: HTMLElement, id: string) {
  const select = box.querySelector<HTMLSelectElement>("[data-collection]")!;
  const option = [...select.options].find((item) => item.value === id)!;
  option.selected = true;
  select.value = id;
  select.dispatchEvent(new Event("change", { bubbles: true }));
  flushSync();
}

function typeFolder(box: HTMLElement, value: string) {
  const input = box.querySelector<HTMLInputElement>("[data-folder-input]")!;
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

async function pickHost(box: HTMLElement, endpoint: string) {
  box.querySelector<HTMLButtonElement>(`[data-host-option="${endpoint}"]`)!.click();
  await settle();
}

describe("AC-046-01 opening and the host", () => {
  // Would catch: the `+` missing from the header, another icon, the dialog opening on a host
  // that is not the selected one, or Esc/Cancelar leaving a command behind.
  it("the + of WORKSPACES opens the modal with the selected host marked; Esc and Cancelar close without a call", async () => {
    const { el, bridge } = await render();
    const trigger = el.querySelector<HTMLButtonElement>("[data-new-workspace]")!;
    expect(trigger.dataset.icon).toBe("folder-plus");
    expect(el.querySelector("[data-sidebar-workspaces]")!.contains(trigger)).toBe(true);
    expect(dialog(el)).toBeNull();

    const box = await openDialog(el);
    expect(box.querySelector<HTMLElement>('[data-host-option="local"]')!.getAttribute("aria-pressed")).toBe("true");
    expect(box.querySelector<HTMLElement>('[data-host-option="ssh-dev"]')!.getAttribute("aria-pressed")).toBe("false");

    // The backdrop never holds the focus, so Esc is answered on the window (as HostPopover does).
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await settle();
    expect(dialog(el), "Esc closes wherever the focus is").toBeNull();

    (await openDialog(el)).querySelector<HTMLButtonElement>("[data-cancel]")!.click();
    await settle();
    expect(dialog(el), "Cancelar closes").toBeNull();
    expect(bridge.calls, "neither Esc nor Cancelar sends anything").toEqual([]);
    expect(tauri.invoked).toEqual([]);
  });

  // Would catch: an offline host hidden, selectable, or shown without its motive.
  it("lists every host: online selectable, offline with aria-disabled and the motive offline", async () => {
    const { el } = await render();
    const box = await openDialog(el);
    const options = [...box.querySelectorAll<HTMLElement>("[data-host-option]")];
    expect(options.map((node) => node.dataset.hostOption)).toEqual(["local", "ssh-dev", "ssh-build"]);
    expect(options[0]!.textContent).toContain("Este computador");
    expect(options[1]!.textContent).toContain("dev-box");

    const offline = box.querySelector<HTMLElement>('[data-host-option="ssh-build"]')!;
    expect(offline.getAttribute("aria-disabled")).toBe("true");
    expect(offline.dataset.reason).toBe("offline");
    offline.click();
    await settle();
    expect(box.querySelector<HTMLElement>('[data-host-option="local"]')!.getAttribute("aria-pressed")).toBe("true");
    expect(box.querySelector<HTMLElement>('[data-host-option="ssh-build"]')!.getAttribute("aria-pressed")).toBe("false");
  });

  // Would catch: the native picker offered (or called) on an SSH host, or Local losing it.
  it("Escolher… exists only for Local, calls project_pick_folder once and fills the field", async () => {
    const { el } = await render();
    const box = await openDialog(el);
    expect(box.querySelector("[data-pick-folder]"), "Local browses this computer").toBeTruthy();

    await pickHost(box, "ssh-dev");
    expect(box.querySelector("[data-pick-folder]"), "no remote folder browser (P3)").toBeNull();
    expect(box.querySelector("[data-folder-input]"), "the path is typed instead").toBeTruthy();
    expect(tauri.invoked).toEqual([]);

    await pickHost(box, "local");
    tauri.pick = "/w/acme/fiscal";
    box.querySelector<HTMLButtonElement>("[data-pick-folder]")!.click();
    await settle();
    expect(tauri.invoked).toEqual(["project_pick_folder"]);
    expect(box.querySelector<HTMLInputElement>("[data-folder-input]")!.value).toBe("/w/acme/fiscal");
  });

  // Would catch: the palette command missing, or opening something other than this dialog.
  it("the palette command Novo workspace opens the same modal", async () => {
    const { el } = await render();
    runPaletteCommand();
    await settle();
    expect(dialog(el)).toBeTruthy();
  });

  // Would catch round 1: the only subscriber lived in the WORKSPACES section, which the 047 rail
  // replaces, so with the sidebar collapsed the palette command and "Abrir projeto…" asked for a
  // modal nobody was listening for — the request was lost in silence.
  it("with the sidebar collapsed (the 047 rail) the request still opens the modal", async () => {
    // Round 1's composition: the rail alone, with no owner of the modal outside the sidebar.
    const alone = await render({ sidebar: "rail", gate: false });
    expect(alone.el.querySelector("[data-slot='rail']"), "the window is painting the rail").toBeTruthy();
    expect(alone.el.querySelector("[data-sidebar-workspaces]"), "the WORKSPACES section is not mounted").toBeNull();
    expect(alone.el.querySelector("[data-new-workspace]"), "the `+` belongs to the open sidebar").toBeNull();
    runPaletteCommand();
    await settle();
    expect(dialog(alone.el), "nothing inside the rail answers the request").toBeNull();

    const { el } = await render({ sidebar: "rail" });
    runPaletteCommand();
    await settle();
    const box = dialog(el)!;
    expect(box, "the palette command reaches the modal anyway").toBeTruthy();
    expect(box.querySelector<HTMLElement>('[data-host-option="local"]')!.getAttribute("aria-pressed")).toBe("true");
  });

  // Would catch: the modal tied to the user's collapse preference instead of to what is painted,
  // so a narrow window (auto-collapsed, the same rail) would lose the request again.
  it("auto-collapsed by a narrow window it is the same rail, and the request opens the modal there", async () => {
    // A 600 px window leaves the center under 400 px: the sidebar paints the rail although the
    // preference is open (047).
    expect(sidebarVisuallyOpen(true, 600)).toBe(false);
    expect(sidebarWidthPx(true, 600)).toBe(SIDEBAR_RAIL_PX);

    const { el } = await render({ sidebar: "rail" });
    requestNewWorkspace("ssh-dev");
    await settle();
    const box = dialog(el)!;
    expect(box).toBeTruthy();
    expect(box.querySelector<HTMLElement>('[data-host-option="ssh-dev"]')!.getAttribute("aria-pressed")).toBe("true");
  });

  // Would catch: the gate back inside the sidebar (or missing from the window), which is exactly
  // how the request got lost while the sidebar was collapsed.
  it("the window mounts the modal beside the frame, never inside the sidebar regions", () => {
    const region = (name: string) => {
      const at = appSource.indexOf(`{#snippet ${name}()}`);
      return at < 0 ? "" : appSource.slice(at, appSource.indexOf("{/snippet}", at));
    };
    expect(appSource).toContain("<NewWorkspaceHost");
    expect(region("overlay")).toContain("<NewWorkspaceHost");
    expect(region("projectsRegion")).not.toContain("NewWorkspaceHost");
    expect(region("railRegion")).not.toContain("NewWorkspaceHost");
    // The sidebar no longer owns the dialog: it only asks for it.
    expect(sidebarSources["./WorkspacesSection.svelte"]).not.toContain("NewWorkspaceDialog");
    expect(sidebarSources["./WorkspacesSection.svelte"]).toContain("requestNewWorkspace");
  });

  // Would catch: Esc answered only by a backdrop that never holds the focus, or a modal that
  // opens without a field ready to receive the path.
  it("the folder field takes the focus and Esc closes from anywhere", async () => {
    const { el } = await render();
    const box = await openDialog(el);
    expect(document.activeElement).toBe(box.querySelector("[data-folder-input]"));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await settle();
    expect(dialog(el), "Esc closes the modal opened by the +").toBeNull();

    runPaletteCommand();
    await settle();
    expect(dialog(el)).toBeTruthy();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await settle();
    expect(dialog(el), "and the one opened by the palette").toBeNull();
  });
});

describe("AC-046-02 recents of the host", () => {
  // Would catch: the recents of every host mixed, the order lost, or a click that does not fill
  // the field.
  it("lists the recents of the marked host, newest first, and a click fills the field", async () => {
    const { el } = await render();
    const box = await openDialog(el);
    expect([...box.querySelectorAll<HTMLElement>("[data-recent]")].map((node) => node.dataset.recent)).toEqual(["/w/dotfiles"]);

    await pickHost(box, "ssh-dev");
    const remote = [...box.querySelectorAll<HTMLElement>("[data-recent]")];
    expect(remote.map((node) => node.dataset.recent)).toEqual(["~/w/billing-worker", "~/w/fiscal-service"]);
    expect(box.textContent).toContain("Recentes neste host");

    remote[1]!.click();
    await settle();
    expect(box.querySelector<HTMLInputElement>("[data-folder-input]")!.value).toBe("~/w/fiscal-service");
  });

  // Would catch: a creation that never records the folder, or records it on another host.
  it("a successful creation records the folder as the newest recent of its host", async () => {
    const { el, bridge, controller } = await render();
    const box = await openDialog(el);
    await pickHost(box, "ssh-dev");
    typeFolder(box, "~/w/novo");
    box.querySelector<HTMLButtonElement>(`[data-start="${SHELL_START}"]`)!.click();
    box.querySelector<HTMLButtonElement>("[data-submit]")!.click();
    await settle();

    expect(bridge.calls.filter((call) => call.command === "recent_folder_add")).toEqual([
      { command: "recent_folder_add", args: { endpointProfileId: "ssh-dev", path: "~/w/novo" } },
    ]);
    expect(recentFoldersFor(controller.state.snapshot?.recent_folders, "ssh-dev")).toEqual([
      "~/w/novo",
      "~/w/billing-worker",
      "~/w/fiscal-service",
    ]);
    expect(recentFoldersFor(controller.state.snapshot?.recent_folders, "local"), "the other host is untouched").toEqual(["/w/dotfiles"]);
  });
});

describe("AC-046-03 creating", () => {
  // Would catch: a create sent to the selected host instead of the marked one, a second
  // `workspace_create`, the label taken from the whole path, `agent_start` before the host
  // confirmed the workspace, or the collection assigned first.
  it("switches host, creates once, starts the agent in its pane and assigns the collection, in order", async () => {
    const { el, bridge, selected } = await render({ confirmOn: "ssh-dev" });
    const box = await openDialog(el);
    await pickHost(box, "ssh-dev");
    typeFolder(box, "~/w/fiscal");
    chooseCollection(box, "g1");
    box.querySelector<HTMLButtonElement>('[data-start="claude"]')!.click();
    await settle();
    expect(box.querySelector<HTMLSelectElement>("[data-collection]")!.value).toBe("g1");

    box.querySelector<HTMLButtonElement>("[data-submit]")!.click();
    await settle();

    expect(commands(bridge.calls)).toEqual(["select_host", "workspace_create", "agent_start", "group_assign", "recent_folder_add"]);
    expect(bridge.calls[0]).toEqual({ command: "select_host", args: { endpoint: "ssh-dev" } });
    expect(bridge.calls[1]).toEqual({
      command: "workspace_create",
      args: { endpoint: "ssh-dev", cwd: "~/w/fiscal", label: "fiscal", focus: true },
    });
    expect(bridge.calls[2]!.args).toMatchObject({ paneId: "w1:p1", kind: "claude" });
    expect(bridge.calls[3]).toEqual({
      command: "group_assign",
      args: { groupId: "g1", endpoint_profile_id: "ssh-dev", session_name: "hd046-remote", cwd: "~/w/fiscal", label: "fiscal" },
    });
    expect(selected.value).toBe("ssh-dev");
    expect(el.querySelector("[data-new-workspace-error]")?.textContent ?? null).toBeNull();
    expect(dialog(el), "the modal closes when everything succeeded").toBeNull();
  });

  // Would catch: Shell starting an agent anyway, or a create that needs a collection.
  it("Shell creates the workspace and starts no agent; without a collection nothing is assigned", async () => {
    const { el, bridge } = await render();
    const box = await openDialog(el);
    typeFolder(box, "/w/acme/fiscal");
    box.querySelector<HTMLButtonElement>(`[data-start="${SHELL_START}"]`)!.click();
    await settle();
    box.querySelector<HTMLButtonElement>("[data-submit]")!.click();
    await settle();

    expect(commands(bridge.calls)).toEqual(["workspace_create", "recent_folder_add"]);
    expect(bridge.calls[0]).toEqual({
      command: "workspace_create",
      args: { endpoint: "local", cwd: "/w/acme/fiscal", label: "fiscal", focus: true },
    });
    expect(dialog(el)).toBeNull();
  });

  // Would catch: an error swallowed, the modal closed on a refusal, a retry on another host, or
  // a `group_assign` that runs although the agent failed.
  it("an error of any step stays in line in the open modal and nothing is tried on another host", async () => {
    const { el, bridge } = await render();
    const box = await openDialog(el);
    typeFolder(box, "/w/acme/fiscal");
    chooseCollection(box, "g1");
    await settle();
    bridge.failCommand("workspace_create", { code: "workspace_create_failed", message: "a engine recusou a pasta", retryable: false });
    box.querySelector<HTMLButtonElement>("[data-submit]")!.click();
    await settle();

    expect(dialog(el), "the modal stays open").toBeTruthy();
    expect(box.querySelector<HTMLElement>("[data-new-workspace-error]")!.textContent).toContain("a engine recusou a pasta");
    expect(commands(bridge.calls)).toEqual(["workspace_create"]);
    expect(commands(bridge.calls).filter((command) => command === "select_host")).toEqual([]);

    // The step that failed is retried on the same host once it is fixed; still one create.
    bridge.failCommand("workspace_create", null);
    box.querySelector<HTMLButtonElement>("[data-submit]")!.click();
    await settle();
    expect(commands(bridge.calls)).toEqual(["workspace_create", "workspace_create", "agent_start", "group_assign", "recent_folder_add"]);
    expect(dialog(el)).toBeNull();
  });

  // Would catch: a failed `agent_start` reported as success, or the modal closing over it.
  it("a refused agent_start keeps the modal open with the engine message", async () => {
    const { el, bridge } = await render({ startFails: { code: "start_agent_failed", message: "o agente não iniciou", retryable: true } });
    const box = await openDialog(el);
    typeFolder(box, "/w/acme/fiscal");
    box.querySelector<HTMLButtonElement>('[data-start="codex"]')!.click();
    await settle();
    box.querySelector<HTMLButtonElement>("[data-submit]")!.click();
    await settle();

    expect(box.querySelector<HTMLElement>("[data-new-workspace-error]")!.textContent).toContain("o agente não iniciou");
    expect(dialog(el)).toBeTruthy();
    expect(commands(bridge.calls)).toEqual(["workspace_create", "agent_start", "recent_folder_add"]);
  });

  // Would catch: an agent started on a pane of another workspace because the host never
  // published the new one.
  it("without the metadata of the new workspace no agent is started anywhere", async () => {
    const calls: string[] = [];
    const deps: NewWorkspaceDeps = {
      selectedEndpoint: () => "ssh-dev",
      selectHost: async () => calls.push("select_host"),
      createWorkspace: async () => {
        calls.push("workspace_create");
        return { workspace_id: "w9" };
      },
      confirmPane: async () => {
        calls.push("confirm");
        return null;
      },
      startAgent: async () => {
        calls.push("agent_start");
      },
      assignToCollection: async () => {
        calls.push("group_assign");
      },
      recordRecent: async () => {
        calls.push("recent_folder_add");
      },
      sessionOf: () => "hd046-remote",
    };
    const error = await createWorkspaceFlow(deps, { endpoint: "ssh-dev", folder: " ~/w/fiscal ", collectionId: "g1", start: "claude" });
    expect(error?.code).toBe("workspace_not_confirmed");
    expect(calls).toEqual(["workspace_create", "confirm", "recent_folder_add"]);
  });

  // Would catch: a label built from the whole path or a host list that hides what it refuses.
  it("the label is the last segment and every host is offered with its motive", () => {
    expect(workspaceLabel(" ~/w/acme/fiscal/ ")).toBe("fiscal");
    expect(workspaceLabel("/w/a")).toBe("a");
    const choices = hostChoices(fixture(null).hosts);
    expect(choices.map((choice) => [choice.endpoint, choice.online, choice.reason])).toEqual([
      ["local", true, null],
      ["ssh-dev", true, null],
      ["ssh-build", false, "offline"],
    ]);
  });
});

describe("AC-046-04 the folder picker defect", () => {
  // Would catch: the 016 native picker opened for an SSH host (a Local path sent to another
  // machine), or the switcher silently doing nothing instead of offering the path field.
  it("ProjectSwitcher on an SSH host never calls project_pick_folder and offers the modal field", async () => {
    const { el, ctx } = await render();
    const pick = vi.fn(async () => "/w/local-folder");
    const host = document.createElement("div");
    document.body.appendChild(host);
    const app = mount(ProjectSwitcher, { target: host, props: { ctx, session: "hd046", pickFolder: pick } });
    flushSync();
    mounted.push({ el: host, app: app as never });

    // The window is on the SSH host: the same switch the dialog uses marks it.
    await ctx.controllers.surface.select("ssh-dev");
    await settle();
    host.querySelector<HTMLButtonElement>("[data-topbar-item='project']")!.click();
    flushSync();
    host.querySelector<HTMLButtonElement>("[data-open-project]")!.click();
    await settle();

    expect(pick).not.toHaveBeenCalled();
    expect(tauri.invoked).not.toContain("project_pick_folder");
    const box = dialog(el)!;
    expect(box, "the path field of Novo workspace is offered instead").toBeTruthy();
    expect(box.querySelector<HTMLElement>('[data-host-option="ssh-dev"]')!.getAttribute("aria-pressed")).toBe("true");
    expect(box.querySelector("[data-pick-folder]")).toBeNull();
  });

  // Would catch: `openFolder` reaching the picker (or the engine) for a remote host.
  it("controller.openFolder on a remote host picks nothing and reports why", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-046" });
    const controller = createProjectsController(bridge);
    await controller.load();
    bridge.calls.length = 0;
    const pick = vi.fn(async () => "/w/local-folder");

    await controller.openFolder(pick, "hd046-remote", "ssh-dev");
    expect(pick).not.toHaveBeenCalled();
    expect(bridge.calls).toEqual([]);
    expect(controller.state.globalError?.code).toBe("remote_folder_picker_unavailable");

    // Local keeps the 016 behaviour untouched.
    await controller.openFolder(pick, "hd046", "local");
    expect(pick).toHaveBeenCalledTimes(1);
    expect(commands(bridge.calls)).toEqual(["projects_list", "group_create", "workspace_create", "group_assign"]);
  });
});
