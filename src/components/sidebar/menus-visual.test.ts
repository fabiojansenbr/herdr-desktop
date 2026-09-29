import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount, type Component } from "svelte";
import { compile } from "svelte/compiler";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { FrameContext } from "../../shell/frame-context";
import { FALLBACK_CSS } from "../../theme/apply";
import WorkspaceMenu from "./WorkspaceMenu.svelte";
import CollectionMenu from "./CollectionMenu.svelte";
import HostPopover from "./HostPopover.svelte";
import NewWorkspaceDialog from "./NewWorkspaceDialog.svelte";
import ContextMenu from "../center/ContextMenu.svelte";
import NewAgentPopover from "../center/NewAgentPopover.svelte";
import CommandPalette from "../frame/CommandPalette.svelte";
import type { SidebarRow } from "./sidebar-model";

const noop = () => {};
const ctx = {
  selectedEndpoint: "local", agents: null, connections: null,
  navigator: { snapshot: { collections: [{ id: "g1", name: "Coleção", project_ids: ["p1"] }] } },
  controllers: { projects: {} },
} as unknown as FrameContext;
const row = {
  name: "Workspace", endpoint: "local", session: "hd065-visual", workspaceId: "w1",
  projectId: "p1", prefRoot: "/work", cwd: "/work", pinned: false, color: null,
} as SidebarRow;
const mounted: Record<string, unknown>[] = [];
const styles: HTMLStyleElement[] = [];

beforeEach(() => {
  const base = document.createElement("style");
  base.textContent = readFileSync("src/app.css", "utf8").replace(/:hover/g, "[data-test-hover]");
  document.head.appendChild(base);
  styles.push(base);
  for (const [token, value] of Object.entries(FALLBACK_CSS)) document.documentElement.style.setProperty(token, /^#[0-9a-f]{6}$/i.test(value)
    ? `rgb(${[1, 3, 5].map((offset) => Number.parseInt(value.slice(offset, offset + 2), 16)).join(", ")})` : value);
});
afterEach(async () => {
  for (const app of mounted.splice(0)) await unmount(app);
  for (const style of styles.splice(0)) style.remove();
  document.body.replaceChildren();
  document.documentElement.removeAttribute("style");
});

function render<Props extends object>(component: Component<Props>, props: NoInfer<Props>, path: string) {
  // Same authored-style seam as tabs-narrow.test.ts, compiled to retain Svelte's specificity
  // against app.css. Only hover selectors are mapped to an attribute: happy-dom has no pointer
  // hover state. focus-visible is exercised by actual focus(), which happy-dom supports.
  const filename = resolve(`src/components/${path}.svelte`);
  const source = readFileSync(filename, "utf8");
  const style = document.createElement("style");
  style.textContent = compile(source, { filename, css: "external", dev: true }).css!.code.replace(/:hover/g, "[data-test-hover]");
  document.head.appendChild(style);
  styles.push(style);
  const target = document.createElement("div");
  document.body.appendChild(target);
  mounted.push(mount(component, { target, props }));
  flushSync();
}
function element(selector: string) {
  const node = document.querySelector<HTMLElement>(selector);
  expect(node, selector).not.toBeNull();
  return node!;
}
function click(selector: string) { element(selector).click(); flushSync(); }
function panel(selector: string, radius = "10px") {
  const style = getComputedStyle(element(selector));
  expect(style.backgroundColor).toBe("rgb(23, 23, 23)");
  expect(style.borderTopWidth).toBe("1px");
  expect(style.borderTopStyle).toBe("solid");
  expect(style.borderTopColor).toBe("rgb(36, 36, 36)");
  expect(style.borderRadius).toBe(radius);
}
function items(selector: string) {
  const nodes = document.querySelectorAll<HTMLElement>(selector);
  expect(nodes.length).toBeGreaterThan(0);
  for (const node of nodes) {
    const style = getComputedStyle(node);
    expect(style.fontSize).toBe("13.5px");
    expect(style.borderRadius).toBe("6px");
    node.setAttribute("data-test-hover", "");
    expect(getComputedStyle(node).backgroundColor, `${node.textContent}: hover`).toBe("rgb(36, 36, 36)");
    node.removeAttribute("data-test-hover");
    node.focus();
    expect(document.activeElement).toBe(node);
    expect(getComputedStyle(node).backgroundColor, `${node.textContent}: focus`).toBe("rgb(36, 36, 36)");
    node.blur();
  }
}
function colorRow(selector: string) {
  const node = element(selector);
  expect(getComputedStyle(node).fontSize).toBe("13.5px");
  expect(getComputedStyle(node).borderRadius).toBe("6px");
  node.setAttribute("data-test-hover", "");
  expect(getComputedStyle(node).backgroundColor).toBe("rgb(36, 36, 36)");
  node.removeAttribute("data-test-hover");
}
function danger(selector: string) {
  expect(getComputedStyle(element(selector)).color).toBe("rgb(248, 113, 113)");
}
function noAccentText(selector: string) {
  // A different accent exposes accidental token use even if a future palette aliases it to text.
  document.documentElement.style.setProperty("--accent", "rgb(18, 52, 86)");
  for (const node of document.querySelectorAll<HTMLElement>(`${selector}, ${selector} *`)) {
    expect(getComputedStyle(node).color, node.textContent ?? "").not.toBe("rgb(18, 52, 86)");
  }
}
function fields(selector: string) {
  for (const node of document.querySelectorAll<HTMLElement>(selector)) {
    for (const state of ["rest", "hover", "focus"]) {
      if (state === "hover") node.setAttribute("data-test-hover", "");
      if (state === "focus") node.focus();
      const style = getComputedStyle(node);
      expect(style.backgroundColor, state).toBe("rgb(10, 10, 10)");
      expect(style.borderTopWidth, state).toBe("1px");
      expect(style.borderTopStyle, state).toBe("solid");
      expect(style.borderTopColor, state).toBe("rgb(36, 36, 36)");
      expect(style.borderRadius, state).toBe("8px");
      expect(style.fontSize, state).toBe("14px");
      node.removeAttribute("data-test-hover");
      node.blur();
    }
  }
  expect(document.querySelectorAll(selector).length).toBeGreaterThan(0);
}
function primary(selector: string) {
  document.documentElement.style.setProperty("--accent", "rgb(18, 52, 86)");
  const node = element(selector);
  for (const state of ["rest", "hover", "focus"]) {
    if (state === "hover") node.setAttribute("data-test-hover", "");
    if (state === "focus") node.focus();
    const style = getComputedStyle(node);
    expect(style.backgroundColor, state).toBe("rgb(237, 237, 237)");
    expect(style.color, state).toBe("rgb(10, 10, 10)");
    expect(style.fontWeight, state).toBe("600");
    expect(style.borderRadius, state).toBe("8px");
    expect(style.borderTopColor, state).not.toBe("rgb(18, 52, 86)");
    node.removeAttribute("data-test-hover");
    node.blur();
  }
}

describe("AC-065-01 — neutral menus", () => {
  it("catches old borders, missing keyboard selection and accent checks in WorkspaceMenu", () => {
    render(WorkspaceMenu, { ctx, row, onclose: noop, onrename: noop }, "sidebar/WorkspaceMenu");
    panel("[data-workspace-menu]");
    items("[data-workspace-menu] button.item");
    colorRow('[data-menu-item="color"]');
    danger('[data-menu-item="close"]');
    click('[data-menu-item="move"]');
    panel("[data-collection-submenu]");
    items("[data-collection-submenu] button.item");
    noAccentText(".menu");
    click('[data-menu-item="close"]');
    items(".confirm-action");
    danger("[data-confirm-close]");
  });
  it("catches old collection styling and preserves destructive confirmation color", () => {
    render(CollectionMenu, {
      ctx, collection: { id: "g1", name: "Coleção", color: "#4ade80", count: 1, empty: false, ungrouped: false, collapsed: false, rows: [row] },
      onclose: noop, onrename: noop,
    }, "sidebar/CollectionMenu");
    panel("[data-collection-menu]");
    items("button.item");
    colorRow('[data-collection-menu-item="color"]');
    danger('[data-collection-menu-item="delete"]');
    click('[data-collection-menu-item="delete"]');
    items(".confirm-action");
    danger(".confirm-action.danger");
    noAccentText(".menu");
  });
  it("catches host actions using accent, square nested menus or missing removal color", () => {
    render(HostPopover, {
      items: [{ endpoint: "ssh1", name: "Host", typeBadge: "SSH", latencyText: "5 ms", statusText: "Online", tone: "ok", active: true, online: true, retryable: false, tooltip: "Host", latency: "5 ms", connectOnOpen: true }],
      onSelect: noop, onDisconnect: noop, onReconnect: noop, onEdit: noop, onRemove: noop,
      onSetConnectOnOpen: noop, onNewConnection: noop, onClose: noop,
    }, "sidebar/HostPopover");
    panel("[data-host-popover]");
    // Inactive options must also receive a keyboard selection background.
    element("[data-host-option]").classList.remove("active");
    items("[data-host-option], [data-host-new], [data-host-menu]");
    click("[data-host-menu]");
    panel("[data-host-menu-for]");
    items("[data-host-menu-for] button");
    danger('[data-action="remove"]');
    noAccentText("[data-host-popover]");
    click('[data-action="remove"]');
    panel("[data-host-confirm]");
    items("[data-host-confirm] button");
    danger("[data-host-confirm-remove]");
  });
  it("catches small context-menu corners and a neutral close action including arrow navigation", () => {
    render(ContextMenu, { items: [{ id: "new-tab", label: "Nova aba" }, { id: "close", label: "Fechar" }], x: 10, y: 10, onselect: noop, onclose: noop }, "center/ContextMenu");
    panel("[data-context-menu]");
    items("[data-context-menu] button");
    danger('[data-menu-item="close"]');
    element("[data-context-menu]").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    flushSync();
    expect(getComputedStyle(element('[data-menu-item="close"]')).backgroundColor).toBe("rgb(36, 36, 36)");
    noAccentText("[data-context-menu]");
  });
});

describe("AC-065-02 — neutral dialogs and primary buttons", () => {
  it("catches an accent primary button and old panel/field geometry in NewAgentPopover", () => {
    render(NewAgentPopover, { ctx, onclose: noop }, "center/NewAgentPopover");
    panel("[data-new-agent-popover]");
    // The text fields of the popover; the auto mode switch of spec 076 is a checkbox, which this
    // popover's own neutral style has excluded from the field geometry since 065.
    fields(
      '[data-new-agent-popover] input:not([type="checkbox"]):not([type="radio"]), [data-new-agent-popover] select',
    );
    primary("[data-start-agent]");
    noAccentText("[data-new-agent-popover]");
  });
  it("catches a borderless workspace input or an accent-colored primary button", () => {
    render(NewWorkspaceDialog, { ctx, onclose: noop }, "sidebar/NewWorkspaceDialog");
    panel(".sheet", "12px");
    fields(".sheet input, .sheet select");
    primary("[data-submit]");
  });
  it("catches a transparent borderless palette input and the old panel border", () => {
    render(CommandPalette, { sections: () => [], onRun: noop, onClose: noop }, "frame/CommandPalette");
    panel("[data-palette]", "12px");
    fields("[data-palette] input");
  });
});
