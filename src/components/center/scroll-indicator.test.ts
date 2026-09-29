// @vitest-environment happy-dom
// Spec 022 AC-022-03 — scroll indicator on the pane frame. Would catch: no badge when
// offset_from_bottom > 0, a badge that stays at 0, or a click that does not ask pane.scroll 0.
import { readFileSync } from "node:fs";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "../../agents/controller";
import { createFakeAgentsBridge } from "../../agents/fake-bridge";
import type { FrameContext } from "../../shell/frame-context";
import type { SurfaceState } from "../../shell/controller";
import type { MenuAction } from "../frame/menus";
import type { FrameEvent, PaneMeta, ScrollRequest } from "../../terminal/types";
import PaneFrames from "./PaneFrames.svelte";

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];
const ACTIONS = Object.fromEntries(
  ["showProjects", "openConnections", "paste", "toggleProjects", "toggleAgents", "toggleFiles", "openPalette", "newAgent", "split", "newTab", "reconnect"].map(
    (name) => [name, () => {}],
  ),
) as Record<MenuAction, () => void>;

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
      boot_id: "boot-022",
      server_version: "0.9.0",
      pane_id: "w1:p1",
      last_error: null,
    },
    phase: "live",
    reason: null,
    error: null,
    identity: { endpoint: "local", session: "default", connection_generation: 1, boot_id: "boot-022", pane_id: "w1:p1" },
    surfaceKey: 1,
    busy: false,
  };
}

function metaPane(offset: number): PaneMeta {
  return {
    pane_id: "w1:p1",
    content_revision: 1,
    rect: { x: 0, y: 0, width: 100, height: 40 },
    inner_rect: { x: 0, y: 0, width: 100, height: 40 },
    scroll: { offset_from_bottom: offset, max_offset_from_bottom: 40, viewport_rows: 40 },
    focused: true,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: 0,
    pixel_height: 0,
  };
}

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
});

async function mountFrames(offset: number, onScroll?: (request: ScrollRequest) => void) {
  const bridge = createFakeAgentsBridge({
    bootId: "boot-022",
    generation: 1,
    session: "default",
    kinds: ["claude"],
    agents: [],
  });
  const controller = createAgentsController(bridge);
  await controller.connect({ cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 });
  const el = document.createElement("div");
  document.body.appendChild(el);
  const stage = document.createElement("div");
  stage.className = "stage";
  stage.setAttribute("data-center-stage", "");
  const terminal = document.createElement("div");
  terminal.className = "terminal";
  const canvas = document.createElement("canvas");
  canvas.getBoundingClientRect = () =>
    ({ top: 0, left: 0, width: 900, height: 760, bottom: 760, right: 900, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  terminal.appendChild(canvas);
  stage.getBoundingClientRect = () =>
    ({ top: 0, left: 0, width: 900, height: 780, bottom: 780, right: 900, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  stage.appendChild(terminal);
  el.appendChild(stage);

  const pane = metaPane(offset);
  const ctx: FrameContext = {
    surface: liveSurface(),
    agents: controller.state,
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
      surface: {
        subscribe: (handler: (event: FrameEvent) => void) => {
          handler({
            type: "full",
            revision: 1,
            width: 100,
            height: 40,
            cells: [{ s: " ", fg: 0, bg: 0, m: 0 }],
            cursor: null,
            panes: [],
          });
          handler({ type: "metadata", revision: 1, panes: [pane], hyperlinks: [] });
          return () => {};
        },
      } as FrameContext["controllers"]["surface"],
      agents: controller,
      projects: {} as FrameContext["controllers"]["projects"],
      connections: {} as FrameContext["controllers"]["connections"],
    },
    actions: { ...ACTIONS },
    unavailable: { split: null, newTab: null, newAgent: null },
  };
  const Original = globalThis.ResizeObserver;
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as typeof ResizeObserver;
  const app = mount(PaneFrames, { target: el, props: { ctx, stage, onScroll } });
  flushSync();
  hosts.push({ el, app });
  globalThis.ResizeObserver = Original;
  return el;
}

describe("pane scroll indicator (AC-022-03)", () => {
  it("shows ↑ N linhas only while offset_from_bottom > 0 and a click asks offset 0", async () => {
    const onScroll = vi.fn();
    const shown = await mountFrames(7, onScroll);
    const badge = shown.querySelector<HTMLElement>("[data-scroll-indicator]");
    expect(badge, "badge present at offset 7").toBeTruthy();
    expect(badge!.textContent?.replace(/\s+/g, " ").trim()).toBe("↑ 7 linhas");
    const css = readFileSync("src/components/center/PaneFrames.svelte", "utf8");
    expect(css).toMatch(/\.scroll-indicator \{[^}]*font-size:\s*11px/);
    expect(css).toMatch(/\.scroll-indicator \{[^}]*color:\s*var\(--text-muted\)/);
    expect(css).toMatch(/\.scroll-indicator \{[^}]*background:\s*var\(--surface-2\)/);
    expect(css).toMatch(/\.scroll-indicator \{[^}]*border-radius:\s*4px/);
    badge!.click();
    expect(onScroll).toHaveBeenCalledTimes(1);
    expect(onScroll).toHaveBeenCalledWith({ pane_id: "w1:p1", offset_from_bottom: 0 });

    const hidden = await mountFrames(0, onScroll);
    expect(hidden.querySelector("[data-scroll-indicator]")).toBeNull();
  });
});
