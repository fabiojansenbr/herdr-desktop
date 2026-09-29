// @vitest-environment happy-dom
// Spec 054 — the collection count on the same right edge in both collection headers.
//
// Measured cause (see the spec): the count lived inside the header button and only an editable
// collection carried the `…` after it, 18 px wide plus its margin, invisible until the hover but
// still taking room in the flow. So `Meus projetos` printed its number further left than
// `Sem coleção`. The fix takes the `…` out of the flow and lays it over the count slot.
//
// The layout itself is read from the component CSS (the technique of 010/041/051): happy-dom
// mounts the markup but runs no layout, so a pixel read would say 0 for every header.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionsState } from "../../connections/controller";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import { createFakeProjectsBridge } from "../../projects/fake-bridge";
import { createProjectsController } from "../../projects/controller";
import type { NavigatorState } from "../../projects/reducer";
import type { CollectionDto, ProjectDto, WorkspacePrefDto } from "../../projects/types";
import type { SurfaceState } from "../../shell/controller";
import type { FrameContext } from "../../shell/frame-context";
import type { MenuAction } from "../frame/menus";
import { reactiveHolder } from "../center/reactive-test-state.svelte";
import { UNGROUPED_ID } from "./sidebar-model";
import SidebarV2 from "./SidebarV2.svelte";

const sources = import.meta.glob("./*.svelte", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

type Rule = { selectors: string[]; body: string };

/** Every rule of a component's `<style>`, comments dropped, as selector list + declarations. */
function rules(source: string): Rule[] {
  const style = source.slice(source.indexOf("<style>") + 7, source.indexOf("</style>"));
  return [...style.replace(/\/\*[^]*?\*\//g, "").matchAll(/([^{}]+)\{([^}]*)\}/g)].map((m) => ({
    selectors: m[1]!.split(",").map((s) => s.trim().replace(/\s+/g, " ")),
    body: m[2]!,
  }));
}

/** Declarations of the first rule whose selector list contains `selector` exactly (as in 010/041). */
function rule(source: string, selector: string): string {
  return rules(source).find((r) => r.selectors.includes(selector))?.body ?? "";
}

function decl(body: string, prop: string): string | null {
  for (const one of body.split(";")) {
    const at = one.indexOf(":");
    if (at < 0) continue;
    if (one.slice(0, at).trim() === prop) return one.slice(at + 1).trim();
  }
  return null;
}

/** The right inset a header leaves, whether written long hand or in the `padding` short hand. */
function paddingRight(body: string): string | null {
  const direct = decl(body, "padding-right");
  if (direct) return direct;
  const short = decl(body, "padding");
  if (!short) return null;
  const parts = short.trim().split(/\s+/);
  return parts.length === 1 ? parts[0]! : parts[1]!;
}

const group = "./CollectionGroup.svelte";

const mounted: { el: HTMLElement; app: Record<string, unknown> }[] = [];

beforeEach(() => {
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

function surface(): SurfaceState {
  return {
    selection: { endpoint: "local", kind: "local", label: "Local", session: "hd054", online: true, identity: null },
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

/** The captured case: `Meus projetos` counts 2 (`c` is hidden), `Sem coleção` counts 2 too. */
function fixture() {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd054",
      target: null,
      workspaces: [
        workspace({ workspace_id: "w-a", number: 1, label: "a", cwd: "/w/a", focused: true }),
        workspace({ workspace_id: "w-b", number: 2, label: "b", cwd: "/w/b" }),
        workspace({ workspace_id: "w-c", number: 3, label: "c", cwd: "/w/c" }),
        workspace({ workspace_id: "w-d", number: 4, label: "d", cwd: "/w/d" }),
        workspace({ workspace_id: "w-e", number: 5, label: "e", cwd: "/w/e" }),
      ],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-a", label: "a", endpoint_profile_id: "local", session_name: "hd054", root: "/w/a", binding: null },
    { id: "p-b", label: "b", endpoint_profile_id: "local", session_name: "hd054", root: "/w/b", binding: null },
    { id: "p-c", label: "c", endpoint_profile_id: "local", session_name: "hd054", root: "/w/c", binding: null },
  ];
  const prefs: WorkspacePrefDto[] = [
    { endpoint_profile_id: "local", root: "/w/c", color: null, pinned: false, hidden: true },
  ];
  const collections: CollectionDto[] = [{ id: "g1", name: "Meus projetos", project_ids: ["p-a", "p-b", "p-c"] }];
  return { hosts, projects, prefs, collections };
}

async function render() {
  const options = fixture();
  const bridge = createFakeProjectsBridge({
    bootId: "boot-054",
    seed: { version: 3, projects: options.projects, collections: options.collections, workspace_prefs: options.prefs },
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
  return { el };
}

async function settle() {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
  flushSync();
}

const head = (el: HTMLElement, id: string) =>
  el.querySelector<HTMLElement>(`[data-collection="${id}"] [data-collection-head]`)!;

describe("AC-054-01 the count on the same right edge in both collection headers", () => {
  // Would catch the measured cause coming back: the `…` in the flow of the header, pushing the
  // count of an editable collection left of the count of `Sem coleção`.
  it("keeps the `…` out of the flow, over the count slot", () => {
    const menuButton = rule(sources[group]!, ".menu-button");
    expect(menuButton, ".menu-button must exist").not.toBe("");
    expect(decl(menuButton, "position"), "the `…` leaves the flow of the header").toBe("absolute");
    expect(decl(menuButton, "right"), "anchored to the right edge of the header").not.toBeNull();
    expect(menuButton, "out of the flow it must not reserve room with a margin").not.toMatch(/margin/);
    // It can only be laid over the header because the header is the positioned ancestor.
    expect(decl(rule(sources[group]!, ".collection-head"), "position")).toBe("relative");
  });

  // Would catch: a slot declared per header type, so the two counts land on different edges again.
  it("gives both collection headers the one count slot, with its minimum width", () => {
    const slotRules = rules(sources[group]!).filter((r) => r.selectors.some((s) => s.includes(".count-slot")));
    expect(slotRules.length, ".count-slot must exist").toBeGreaterThan(0);
    const base = slotRules.filter((r) => r.selectors.includes(".count-slot"));
    expect(base, "one shared rule, not one per header type").toHaveLength(1);
    expect(decl(base[0]!.body, "min-width"), "the slot has a minimum width").not.toBeNull();
    // No later rule may re-measure the slot for a single header type.
    for (const other of slotRules.filter((r) => !r.selectors.includes(".count-slot"))) {
      expect(decl(other.body, "min-width"), other.selectors.join(",")).toBeNull();
      expect(decl(other.body, "padding"), other.selectors.join(",")).toBeNull();
      expect(decl(other.body, "margin"), other.selectors.join(",")).toBeNull();
    }
  });

  // Would catch: one of the two collection headers inset on its own — which is the gap the user
  // saw — or the `…` anchored beside the slot instead of over it.
  // `Ocultos (N)` keeps its count inside the label (P5) and is out of this rule.
  it("leaves the same right inset in the editable header and in Sem coleção", () => {
    const toggle = paddingRight(rule(sources[group]!, ".toggle"));
    expect(toggle, ".toggle must declare its padding").not.toBeNull();
    // The one `.toggle` rule draws both collection headers, and nothing may re-inset one of them.
    const overrides = rules(sources[group]!).filter(
      (r) => !r.selectors.includes(".toggle") && r.selectors.some((s) => s.split(/[\s>+~]+/).some((part) => part.startsWith(".toggle"))),
    );
    for (const other of overrides) expect(paddingRight(other.body), other.selectors.join(",")).toBeNull();
    // And the `…` sits on that very inset, so it lands over the slot instead of beside it.
    expect(decl(rule(sources[group]!, ".menu-button"), "right")).toBe(toggle);
  });

  // The markup proof of the same slot: both headers close the button with the slot, and the `…`
  // is a sibling of that button, not another item inside it.
  it("renders the same slot in Meus projetos and in Sem coleção", async () => {
    const { el } = await render();
    for (const id of ["g1", UNGROUPED_ID]) {
      const toggle = head(el, id).querySelector<HTMLElement>("[data-collection-toggle]")!;
      const slot = toggle.lastElementChild as HTMLElement;
      expect(slot.dataset.collectionCountSlot, id).toBe("");
      expect(slot.querySelector("[data-collection-count]")!.textContent!.trim(), id).toBe("2");
    }
    const button = head(el, "g1").querySelector<HTMLElement>("[data-collection-menu-button]")!;
    expect(button.parentElement!.dataset.collectionHead, "the `…` hangs from the header").toBe("");
    expect(head(el, UNGROUPED_ID).querySelector("[data-collection-menu-button]")).toBeNull();
  });
});

describe("AC-054-02 the `…` takes the place of the count", () => {
  // Would catch: a swap that hides the count with `display`/`width`, which would shift the header.
  it("hides the count on hover and on keyboard focus, without moving anything", () => {
    const hide = rules(sources[group]!).filter((r) => r.selectors.some((s) => s.includes(".count-slot")) && decl(r.body, "opacity") === "0");
    expect(hide, "one rule hides the slot").toHaveLength(1);
    const selectors = hide[0]!.selectors;
    expect(selectors.some((s) => s.includes(":hover"))).toBe(true);
    expect(selectors.some((s) => s.includes(":focus-within"))).toBe(true);
    for (const one of selectors) {
      expect(one, "only an editable header ever hides its count").toMatch(/\[data-collection-(editable|menu-open)/);
      expect(one).toMatch(/\.count-slot$/);
    }
    // Nothing else may take the slot out of the layout.
    for (const r of rules(sources[group]!).filter((x) => x.selectors.some((s) => s.includes(".count-slot")))) {
      expect(decl(r.body, "display"), r.selectors.join(",")).not.toBe("none");
      expect(decl(r.body, "width"), r.selectors.join(",")).toBeNull();
    }
    // And on the same states the `…` shows up.
    const show = rules(sources[group]!).find((r) => r.selectors.includes(".collection-head:hover .menu-button"))!;
    expect(show, "the `…` still appears on hover").toBeTruthy();
    expect(decl(show.body, "opacity")).toBe("1");
    expect(show.selectors).toContain(".collection-head:focus-within .menu-button");
  });

  // Would catch: the rule reaching `Sem coleção` (and, by the same flag, any bucket), whose count
  // must stay readable because it has no `…` to show in its place.
  it("marks only the editable header, so Sem coleção never hides its count", async () => {
    const { el } = await render();
    expect(head(el, "g1").dataset.collectionEditable).toBe("true");
    expect(head(el, UNGROUPED_ID).dataset.collectionEditable).toBeUndefined();
  });

  // Would catch: the swap breaking the 045/049 menu, which opens from the `…` and from a right
  // click on the header.
  it("keeps the menu opening from the `…` and from a right click", async () => {
    const { el } = await render();
    head(el, "g1").querySelector<HTMLButtonElement>("[data-collection-menu-button]")!.click();
    await settle();
    expect(document.querySelector("[data-collection-menu]")).toBeTruthy();

    head(el, "g1").querySelector<HTMLButtonElement>("[data-collection-menu-button]")!.click();
    await settle();
    expect(document.querySelector("[data-collection-menu]")).toBeNull();

    head(el, "g1").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    await settle();
    expect(document.querySelector("[data-collection-menu]")!.getAttribute("aria-label")).toBe("Ações da coleção Meus projetos");

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    head(el, UNGROUPED_ID).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    await settle();
    expect(document.querySelector("[data-collection-menu]"), "Sem coleção has no stored row to edit").toBeNull();
  });
});
