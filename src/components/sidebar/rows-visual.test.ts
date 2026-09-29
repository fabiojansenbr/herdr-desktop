// Spec 062: mounted components, their authored CSS, and the production sidebar model.
import { readFileSync } from "node:fs";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { hostFixture } from "../../connections/fake-bridge";
import type { FrameContext } from "../../shell/frame-context";
import { buildSidebar, type SidebarRow, UNGROUPED_ID } from "./sidebar-model";
import WorkspaceItem from "./WorkspaceItem.svelte";

const mounted: { app: ReturnType<typeof mount>; el: HTMLElement }[] = [];
const styles: HTMLStyleElement[] = [];

beforeEach(() => {
  localStorage.clear();
  for (const file of ["WorkspaceItem", "WorkspaceTabList"]) {
    const source = readFileSync(`src/components/sidebar/${file}.svelte`, "utf8");
    const style = document.createElement("style");
    // happy-dom has no pointer-driven :hover state. Apply the same authored declarations
    // to an explicit test state; every assertion below observes computed styles on the DOM.
    style.textContent = (source.match(/<style>([\s\S]*?)<\/style>/)?.[1] ?? "")
      .replaceAll(":hover", "[data-test-hover]");
    document.head.appendChild(style);
    styles.push(style);
  }
});

afterEach(async () => {
  for (const { app, el } of mounted.splice(0)) {
    await unmount(app);
    el.remove();
  }
  for (const style of styles.splice(0)) style.remove();
  localStorage.clear();
});

function fixture() {
  const names = ["herdr-desktop", "_tmp9", "~"];
  return buildSidebar({
    selectedEndpoint: "local",
    hosts: [hostFixture({ endpoint: "local", kind: "local", phase: "online", session: "hd062",
      workspaces: names.map((name, i) => ({ workspace_id: `w${i}`, number: i + 1,
        label: name, cwd: `/w/${i}`, focused: i === 0, branch: "main", tab_count: 2,
        pane_count: 2, active_tab_id: "t1", agent_status: "idle" })),
    })],
    projects: names.map((label, i) => ({ id: `p${i}`, label, root: `/w/${i}`,
      endpoint_profile_id: "local", session_name: "hd062", binding: null })),
    groups: [{ id: "g1", name: "Coleção", project_ids: ["p0", "p1"], color: "#5BD68A" }],
    prefs: [{ endpoint_profile_id: "local", root: "/w/0", color: "#B18CFF", pinned: false, hidden: false }],
  });
}

function render(row: SidebarRow) {
  const ctx = {
    selectedEndpoint: "local", connections: null, navigator: null,
    surface: { selection: { endpoint: "local" }, identity: null },
    // Spec 074: the live list is read only while the connection is `connected`, as it is here.
    agents: { phase: "connected", identity: { endpoint: "local" }, agents: [],
      tabs: [1, 2].map((number) => ({ tab_id: `t${number}`, workspace_id: row.workspaceId,
        number, label: `terminal ${number}`, pane_count: 1, focused: number === 1 })),
      tabFocus: { tab_id: "t1" },
    },
    controllers: { projects: {}, agents: {}, surface: {} },
  } as unknown as FrameContext;
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(WorkspaceItem, { target: el, props: { ctx, row, collectionId: UNGROUPED_ID, onDrop: () => {} } });
  mounted.push({ app, el });
  flushSync();
  return el;
}

function node(el: HTMLElement, selector: string): HTMLElement {
  const found = el.querySelector<HTMLElement>(selector);
  expect(found, selector).not.toBeNull();
  return found!;
}

function token(value: string, name: string) {
  const tokens: Record<string, string> = { "text-dim": "#6B6B6B", text: "#EDEDED", "text-muted": "#A3A3A3", "surface-2": "#171717", "surface-3": "#242424" };
  expect(value.toUpperCase()).toBe(tokens[name]);
}

function unadorned(el: HTMLElement) {
  const css = getComputedStyle(el);
  for (const side of ["Top", "Right", "Bottom", "Left"]) {
    expect(["", "none"]).toContain(css.getPropertyValue(`border-${side.toLowerCase()}-style`));
  }
  expect(["", "none"]).toContain(css.boxShadow);
  expect(el.querySelector("[data-drop-indicator]")).toBeNull();
}

describe("AC-062-01 workspace tiles", () => {
  it("uses preference before collection before UTF-16 name colour, with three distinct tiles", () => {
    const sections = fixture();
    const rows = sections.flatMap((group) => group.rows);
    // '~' = UTF-16 126: groupColor(126) = palette[0], not Sem coleção's palette[1].
    expect(rows.map((row) => row.tileColor)).toEqual(["#B18CFF", "#5BD68A", "#8FA8FF"]);
    const backgrounds = ["#B18CFF33", "#5BD68A33", "#8FA8FF33"];
    const colors = ["#B18CFF", "#5BD68A", "#8FA8FF"];
    rows.forEach((row, i) => {
      const tile = node(render(row), "[data-ws-tile]");
      expect(tile.textContent?.trim()).toBe(["H", "T", "~"][i]);
      expect(tile.querySelector("svg")).toBeNull();
      const css = getComputedStyle(tile);
      expect([css.width, css.height, css.borderRadius, css.fontSize, css.fontWeight])
        .toEqual(["20px", "20px", "5px", "11px", "700"]);
      expect(css.color).toBe(colors[i]);
      expect(css.backgroundColor).toBe(backgrounds[i]);
    });
  });

  it("keeps the collection's fallback colour and hashes UTF-16 code units, not code points", () => {
    const hosts = [hostFixture({ endpoint: "local", kind: "local", phase: "online", workspaces: [] })];
    const projects = ["collection", "😀"].map((label, i) => ({ id: `p${i}`, label,
      root: `/w/${i}`, endpoint_profile_id: "local", session_name: "hd062", binding: null }));
    const rows = buildSidebar({ hosts, projects, groups: [{ id: "g", name: "G", project_ids: ["p0"] }] })
      .flatMap((g) => g.rows);
    // Surrogates 55357 + 56832 = 112189, mod 6 = 1 (code point mod 6 would be 2).
    expect(rows.map((row) => row.tileColor)).toEqual(["#8FA8FF", "#5BD68A"]);
  });

  it.each([["", ""], ["  ~!", "~"], ["_9tmp", "9"], ["_équipe", "É"]])(
    "takes a letter/digit or the first visible symbol from %j", (name, initial) => {
      const tile = node(render({ ...fixture()[0]!.rows[0]!, name }), "[data-ws-tile]");
      expect(tile.textContent?.trim()).toBe(initial);
    },
  );

  it("dims closed letters and changes alpha to 26 while keeping offline/stale opacity", () => {
    const row = fixture()[0]!.rows[0]!;
    const closed = render({ ...row, kind: "closed", workspaceId: null });
    const tile = node(closed, "[data-ws-tile]");
    token(getComputedStyle(tile).color, "text-dim");
    expect(getComputedStyle(tile).backgroundColor).toBe("#B18CFF26");
    expect(getComputedStyle(node(closed, "[data-closed]")).opacity).toBe("0.7");
    for (const flag of ["offline", "stale"] as const) {
      const el = render({ ...row, [flag]: true });
      const line = node(el, `[data-${flag}]`);
      expect(getComputedStyle(line).opacity).toBe("0.55");
      expect(line.contains(node(el, "[data-ws-tile]"))).toBe(true);
    }
  });
});

describe("AC-062-02 workspace typography", () => {
  it.each([false, true])("keeps active=%s neutral, 14px/500, with dim 12px branch and grey hover", (active) => {
    const el = render({ ...fixture()[0]!.rows[0]!, active });
    const line = node(el, "[data-workspace-row]");
    const name = getComputedStyle(node(el, ".name"));
    expect([name.fontSize, name.fontWeight]).toEqual(["14px", "500"]);
    const branch = getComputedStyle(node(el, "[data-branch]"));
    expect(branch.fontSize).toBe("12px");
    token(branch.color, "text-dim");
    expect(getComputedStyle(line).borderRadius).toBe("8px");
    unadorned(line);
    line.setAttribute("data-test-hover", "");
    token(getComputedStyle(line).backgroundColor, "surface-2");
    unadorned(line);
  });
});

describe("AC-062-03 nested tabs", () => {
  it("selects only one grey rounded tab, with 13.5px titles and dim 12px times", () => {
    const el = render(fixture()[0]!.rows[0]!);
    const tabs = [...el.querySelectorAll<HTMLElement>("[data-tab-row]")];
    expect(tabs).toHaveLength(2);
    expect(tabs.map((tab) => tab.dataset.selected ?? null)).toEqual(["true", null]);
    tabs.forEach((tab, i) => {
      const title = getComputedStyle(node(tab, "[data-tab-title]"));
      expect(title.fontSize).toBe("13.5px");
      token(title.color, i === 0 ? "text" : "text-muted");
      const css = getComputedStyle(tab);
      if (i === 0) {
        expect(title.fontWeight).toBe("500");
        expect(css.borderRadius).toBe("8px");
        token(css.backgroundColor, "surface-3");
      } else {
        expect(["", "transparent", "rgba(0, 0, 0, 0)"]).toContain(css.backgroundColor);
      }
      const time = getComputedStyle(node(tab, "[data-tab-time]"));
      expect(time.fontSize).toBe("12px");
      token(time.color, "text-dim");
      unadorned(tab);
    });
  });
});
