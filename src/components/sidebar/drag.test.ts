// @vitest-environment happy-dom
// Spec 045 — dragging a workspace by its handle (AC-045-01). The plan is decided by the pure
// `planDrop`, so the cases below pin both the decision table and the wiring: synthetic drag
// events on the mounted sidebar, over the real controller and the fake bridge, asserting the
// store commands that actually left the WebView and the order they persisted.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import { createFakeProjectsBridge, type RecordedCall } from "../../projects/fake-bridge";
import { createProjectsController } from "../../projects/controller";
import type { NavigatorState } from "../../projects/reducer";
import type { CollectionDto, ProjectDto } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import { crossHostHint, placementOf, planDrop, type DragRow } from "./drag";
import { UNGROUPED_ID } from "./sidebar-model";
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
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd045", online: true, identity: null },
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
 * `G1 = [a, b, c]` on Local (the AC fixture), the empty `G2`, one loose workspace `d` without a
 * ProjectRef (so "Sem coleção" exists and the upsert by root can be exercised) and `e` on an
 * SSH host, for the refusal that is out of scope.
 */
function fixture() {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd045",
      target: null,
      workspaces: [
        workspace({ workspace_id: "w-a", number: 1, label: "a", cwd: "/w/a", branch: "main", focused: true }),
        workspace({ workspace_id: "w-b", number: 2, label: "b", cwd: "/w/b" }),
        workspace({ workspace_id: "w-c", number: 3, label: "c", cwd: "/w/c" }),
        workspace({ workspace_id: "w-d", number: 4, label: "d", cwd: "/w/d" }),
      ],
    }),
    hostFixture({
      endpoint: "ssh-dev",
      label: "dev-box",
      kind: "ssh",
      phase: "online",
      phase_label: "Online",
      session: "hd045",
      workspaces: [workspace({ workspace_id: "w-e", number: 1, label: "e", cwd: "/w/e" })],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-a", label: "a", endpoint_profile_id: "local", session_name: "hd045", root: "/w/a", binding: null },
    { id: "p-b", label: "b", endpoint_profile_id: "local", session_name: "hd045", root: "/w/b", binding: null },
    { id: "p-c", label: "c", endpoint_profile_id: "local", session_name: "hd045", root: "/w/c", binding: null },
    { id: "p-e", label: "e", endpoint_profile_id: "ssh-dev", session_name: "hd045", root: "/w/e", binding: null },
  ];
  const collections: CollectionDto[] = [
    { id: "g1", name: "G1", project_ids: ["p-a", "p-b", "p-c"] },
    { id: "g2", name: "G2", project_ids: ["p-e"] },
  ];
  return { hosts, projects, collections };
}

async function render(options = fixture()) {
  const bridge = createFakeProjectsBridge({
    bootId: "boot-045",
    seed: { version: 3, projects: options.projects, collections: options.collections, workspace_prefs: [] },
  });
  const nav = reactiveHolder<NavigatorState | null>(null);
  const controller = createProjectsController(bridge, (state) => (nav.value = state));
  await controller.load();
  const actions = Object.fromEntries(
    ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map((name) => [name, () => {}]),
  ) as Record<MenuAction, () => void>;
  const holder = reactiveHolder<string | null>("local");
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
  document.body.appendChild(el);
  const app = mount(SidebarV2, { target: el, props: { ctx } });
  flushSync();
  mounted.push({ el, app: app as never });
  return { el, calls: bridge.calls, controller, nav };
}

async function settle() {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
  flushSync();
}

const row = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-workspace-row="${id}"]`)!;
const head = (el: HTMLElement, id: string) => el.querySelector<HTMLElement>(`[data-collection="${id}"] [data-collection-head]`)!;
const names = (calls: RecordedCall[]) => calls.map((call) => call.command);
const storeCalls = (calls: RecordedCall[]) =>
  calls.filter((call) => ["collection_move_project", "collection_remove_project", "group_assign"].includes(call.command));

/** Drag events carry no rect in happy-dom: pin one so `placementOf` sees a real half. */
function withRect(node: HTMLElement, top: number, height: number) {
  node.getBoundingClientRect = () => ({ top, height, bottom: top + height, left: 0, right: 100, width: 100, x: 0, y: top, toJSON: () => ({}) }) as DOMRect;
}

function fire(node: HTMLElement, type: string, clientY = 0) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientY });
  node.dispatchEvent(event);
  return event;
}

/** Grabs `id` by its handle and drops it on `target` at `clientY`. */
async function dragOnto(el: HTMLElement, id: string, target: HTMLElement, clientY = 0) {
  fire(row(el, id).querySelector<HTMLElement>("[data-workspace-handle]")!, "dragstart");
  await settle();
  fire(target, "dragover", clientY);
  flushSync();
  fire(target, "drop", clientY);
  await settle();
}

const source = (over: Partial<DragRow> = {}): DragRow => ({
  rowId: "w-c",
  projectId: "p-c",
  endpoint: "local",
  session: "hd045",
  cwd: "/w/c",
  label: "c",
  collectionId: "g1",
  ...over,
});

describe("AC-045-01 planDrop decides one plan per gesture", () => {
  // Would catch: a reorder inside the collection turning into an assign (a second association),
  // or a drop on the dragged row itself producing a command.
  it("reorders inside the collection and does nothing over itself", () => {
    expect(planDrop(source(), { kind: "row", collectionId: "g1", projectId: "p-a", endpoint: "local", placement: "before" })).toEqual({
      steps: [{ kind: "move", collectionId: "g1", overProjectId: "p-a", placement: "before" }],
      refused: null,
    });
    expect(planDrop(source(), { kind: "row", collectionId: "g1", projectId: "p-c", endpoint: "local", placement: "after" }).steps).toEqual([]);
    expect(planDrop(source(), { kind: "collection", collectionId: "g1" }).steps).toEqual([]);
  });

  // Would catch: `group_assign` alone leaving the row in both collections (the 025 tree keeps the
  // first group, so the move would be invisible), and a loose row being "removed" from nothing.
  it("moves to another collection by leaving the old one and assigning the new", () => {
    expect(planDrop(source(), { kind: "collection", collectionId: "g2" })).toEqual({
      steps: [
        { kind: "remove", collectionId: "g1" },
        { kind: "assign", groupId: "g2" },
      ],
      refused: null,
    });
    expect(planDrop(source({ collectionId: UNGROUPED_ID, projectId: null }), { kind: "collection", collectionId: "g2" })).toEqual({
      steps: [{ kind: "assign", groupId: "g2" }],
      refused: null,
    });
  });

  // Would catch: `Sem coleção` closing the workspace or deleting the project instead of dropping
  // the association only.
  it("removes the association when dropped on Sem coleção", () => {
    expect(planDrop(source(), { kind: "collection", collectionId: UNGROUPED_ID })).toEqual({
      steps: [{ kind: "remove", collectionId: "g1" }],
      refused: null,
    });
    expect(planDrop(source({ collectionId: UNGROUPED_ID, projectId: null }), { kind: "collection", collectionId: UNGROUPED_ID }).steps).toEqual([]);
  });

  // Out of scope of 045: a workspace never changes host, so the gesture is refused with a hint
  // instead of silently recreating it elsewhere.
  it("refuses a drop on a row of another host with the hint", () => {
    const plan = planDrop(source(), { kind: "row", collectionId: "g2", projectId: "p-e", endpoint: "ssh-dev", placement: "before" });
    expect(plan.steps).toEqual([]);
    expect(plan.refused).toBe(crossHostHint());
    expect(crossHostHint()).toBe("Workspaces não mudam de host");
  });

  // Would catch: a workspace the engine reported without cwd being keyed by an empty root.
  it("does nothing for a workspace the engine reported without a cwd", () => {
    expect(planDrop(source({ cwd: null, projectId: null, collectionId: UNGROUPED_ID }), { kind: "collection", collectionId: "g2" }).steps).toEqual([]);
  });

  // Would catch: an indicator always drawn on the same edge (the insertion point would lie).
  it("places before on the top half of the row and after on the bottom half", () => {
    expect(placementOf({ top: 100, height: 20 }, 104)).toBe("before");
    expect(placementOf({ top: 100, height: 20 }, 116)).toBe("after");
    expect(placementOf({ top: 100, height: 0 }, 100)).toBe("after");
  });
});

describe("AC-045-01 dragging in the mounted sidebar", () => {
  // The AC fixture: `c` dropped above `a` persists `[c, a, b]` with exactly one move command.
  it("reorders G1 to [c, a, b] with a single collection_move_project", async () => {
    const { el, calls, nav } = await render();
    const target = row(el, "w-a");
    withRect(target, 100, 20);
    await dragOnto(el, "w-c", target, 104);

    expect(storeCalls(calls)).toEqual([
      { command: "collection_move_project", args: { collectionId: "g1", projectId: "p-c", toIndex: 0 } },
    ]);
    expect(nav.value!.snapshot!.collections.find((c) => c.id === "g1")!.project_ids).toEqual(["p-c", "p-a", "p-b"]);
    const g1 = el.querySelector<HTMLElement>('[data-collection="g1"]')!;
    expect([...g1.querySelectorAll<HTMLElement>("[data-workspace-row]")].map((n) => n.dataset.workspaceRow)).toEqual(["w-c", "w-a", "w-b"]);
  });

  // Would catch: the header refusing the drop, or the row landing anywhere but the end of G2.
  it("moves c to the end of G2 when dropped on its header", async () => {
    const { el, calls, nav } = await render();
    await dragOnto(el, "w-c", head(el, "g2"));

    expect(storeCalls(calls)).toEqual([
      { command: "collection_remove_project", args: { collectionId: "g1", projectId: "p-c" } },
      { command: "group_assign", args: { groupId: "g2", endpoint_profile_id: "local", session_name: "hd045", cwd: "/w/c", label: "c" } },
    ]);
    const after = nav.value!.snapshot!.collections;
    expect(after.find((c) => c.id === "g1")!.project_ids).toEqual(["p-a", "p-b"]);
    expect(after.find((c) => c.id === "g2")!.project_ids).toEqual(["p-e", "p-c"]);
  });

  // Would catch: `Sem coleção` deleting the project, or the row staying in G1.
  it("drops the association when c is dropped on Sem coleção", async () => {
    const { el, calls, nav } = await render();
    await dragOnto(el, "w-c", head(el, UNGROUPED_ID));

    expect(storeCalls(calls)).toEqual([
      { command: "collection_remove_project", args: { collectionId: "g1", projectId: "p-c" } },
    ]);
    expect(nav.value!.snapshot!.collections.find((c) => c.id === "g1")!.project_ids).toEqual(["p-a", "p-b"]);
    expect(nav.value!.snapshot!.projects.some((p) => p.id === "p-c"), "the project survives").toBe(true);
    expect(el.querySelector<HTMLElement>(`[data-collection="${UNGROUPED_ID}"] [data-workspace-row="w-c"]`)).toBeTruthy();
  });

  // Spec 025: a live workspace with no ProjectRef gets one by its root before the move.
  it("creates the ProjectRef by root when a loose workspace is dropped on a collection", async () => {
    const { el, calls, nav } = await render();
    expect(nav.value!.snapshot!.projects.some((p) => p.root === "/w/d")).toBe(false);
    await dragOnto(el, "w-d", head(el, "g2"));

    expect(storeCalls(calls)).toEqual([
      { command: "group_assign", args: { groupId: "g2", endpoint_profile_id: "local", session_name: "hd045", cwd: "/w/d", label: "d" } },
    ]);
    const created = nav.value!.snapshot!.projects.find((p) => p.root === "/w/d")!;
    expect(created, "the ProjectRef was upserted by root").toBeTruthy();
    expect(nav.value!.snapshot!.collections.find((c) => c.id === "g2")!.project_ids).toEqual(["p-e", created.id]);
  });

  // Would catch (verify round 1): the ProjectRef created by the assign not being carried into the
  // move, so the row the indicator promised to insert above silently landed at the end instead.
  it("inserts a loose workspace at the marked position of another collection", async () => {
    const { el, calls, nav } = await render();
    const target = row(el, "w-a");
    withRect(target, 100, 20);
    await dragOnto(el, "w-d", target, 104);

    const created = nav.value!.snapshot!.projects.find((p) => p.root === "/w/d")!;
    expect(created, "the ProjectRef was upserted by root").toBeTruthy();
    expect(storeCalls(calls)).toEqual([
      { command: "group_assign", args: { groupId: "g1", endpoint_profile_id: "local", session_name: "hd045", cwd: "/w/d", label: "d" } },
      { command: "collection_move_project", args: { collectionId: "g1", projectId: created.id, toIndex: 0 } },
    ]);
    expect(nav.value!.snapshot!.collections.find((c) => c.id === "g1")!.project_ids).toEqual([
      created.id,
      "p-a",
      "p-b",
      "p-c",
    ]);
    const g1 = el.querySelector<HTMLElement>('[data-collection="g1"]')!;
    expect([...g1.querySelectorAll<HTMLElement>("[data-workspace-row]")].map((n) => n.dataset.workspaceRow)).toEqual([
      "w-d",
      "w-a",
      "w-b",
      "w-c",
    ]);
  });

  // Would catch: no insertion point during the drag, or one drawn with the wrong thickness/colour.
  it("shows a 2 px accent indicator on the edge the drop would use", async () => {
    const { el } = await render();
    const target = row(el, "w-a");
    withRect(target, 100, 20);
    fire(row(el, "w-c").querySelector<HTMLElement>("[data-workspace-handle]")!, "dragstart");
    await settle();

    fire(target, "dragover", 104);
    flushSync();
    expect(target.querySelector<HTMLElement>("[data-drop-indicator]")!.dataset.dropIndicator).toBe("before");

    fire(target, "dragover", 116);
    flushSync();
    expect(target.querySelector<HTMLElement>("[data-drop-indicator]")!.dataset.dropIndicator).toBe("after");

    fire(target, "dragleave");
    flushSync();
    expect(target.querySelector("[data-drop-indicator]")).toBeNull();

    const css = rule(sources["./WorkspaceItem.svelte"]!, ".drop-indicator");
    expect(css).toMatch(/height:\s*2px/);
    expect(css).toMatch(/var\(--accent/);
  });

  // Would catch: Esc leaving the drag armed, so the next drop still moved the row.
  it("cancels the drag on Esc without any command", async () => {
    const { el, calls } = await render();
    const target = row(el, "w-a");
    withRect(target, 100, 20);
    fire(row(el, "w-c").querySelector<HTMLElement>("[data-workspace-handle]")!, "dragstart");
    await settle();
    fire(target, "dragover", 104);
    flushSync();

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();
    expect(target.querySelector("[data-drop-indicator]"), "the indicator goes with the drag").toBeNull();

    fire(target, "drop", 104);
    await settle();
    expect(storeCalls(calls)).toEqual([]);
    expect(names(calls)).toEqual(["projects_list"]);
  });

  // Out of scope of 045: dragging between hosts is refused, with the hint on the row.
  it("refuses a drop on a row of another host and shows the hint", async () => {
    const { el, calls } = await render();
    const target = row(el, "w-e");
    withRect(target, 100, 20);
    fire(row(el, "w-c").querySelector<HTMLElement>("[data-workspace-handle]")!, "dragstart");
    await settle();
    fire(target, "dragover", 104);
    flushSync();
    expect(target.querySelector("[data-drop-indicator]")).toBeNull();
    expect(target.getAttribute("data-drop-refused")).toBe(crossHostHint());

    fire(target, "drop", 104);
    await settle();
    expect(storeCalls(calls)).toEqual([]);
  });
});
