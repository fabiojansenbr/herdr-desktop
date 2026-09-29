// Spec 013 — `visual-center` phase of the native flow (AC-013-01/02/03). The page drives the real
// composed window like a user (header actions, tab bar, pane frames) and reports raw observations:
// measured boxes, texts, computed colours and the terminal probe's counters. Every expectation is
// computed by the parent (tests/fidelity-native/visual_center.rs) from the engine's own data.

import type { TerminalProbe, TerminalProbeCounters } from "../../terminal/probe";
import { waitFor } from "../../harness/dom";
import { parseIdentity } from "./ssh-flow";
import type { AwaitParent } from "./paste-flow";

export const CENTER_STEPS = [
  "center-engine-before",
  "center-actions",
  "center-observe-start",
  "center-apply-grow",
  "center-observe-grow",
  "center-apply-scale2",
  "center-observe-scale2",
  "center-apply-restore",
  "center-observe-restore",
] as const;
/** Same stages as resize-dpi (view_flow::STAGES): the frames are measured at each one. */
export const CENTER_STAGES = ["start", "grow", "scale2", "restore"] as const;
/** Quiet window with the terminal hidden (AC-013-03). */
export const HIDDEN_MS = 2000;

export type Identity = { pane_id: string; generation: string; boot_prefix: string; endpoint: string };
export type Rect = { left: number; top: number; width: number; height: number };
export type HeaderSeen = { crumbs: string[]; branch: string | null; path: string | null; actions: string[]; empty: boolean };
export type TabSeen = { tab_id: string; label: string; panes: string; dot: string | null; active: boolean };
export type FrameSeen = {
  pane_id: string;
  rect: Rect;
  band: Rect | null;
  name: string;
  state: string;
  state_tone: string;
  path: string | null;
  edge: string;
  edge_color: string;
  edge_width: string;
  focused: boolean;
  cache: boolean;
};
export type StageSeen = {
  stage: string;
  inner_width: number;
  inner_height: number;
  dpr: number;
  canvas: Rect | null;
  stage_rect: Rect | null;
  surface: { cols: number; rows: number } | null;
  panes: { pane_id: string; inner_rect: { x: number; y: number; width: number; height: number } }[];
  frames: FrameSeen[];
  toolbar: Rect | null;
  probe: TerminalProbeCounters;
};

export interface VisualCenterPage {
  identity(): Identity | null;
  header(): HeaderSeen;
  tabs(): TabSeen[];
  frames(): FrameSeen[];
  stage(name: string, probe: TerminalProbe): StageSeen;
  /** Panes of the last accepted metadata (terminal probe, never a DOM guess). */
  panes(): string[];
  focusedPane(): string | null;
  click(selector: string): string;
  /** True when the control exists and is enabled (the window only offers a confirmed action). */
  enabled(selector: string): boolean;
  showFiles(): void;
  showTerminal(): void;
  sleep(ms: number): Promise<void>;
  waitFor<T>(what: string, probe: () => T | null | undefined | false, timeoutMs?: number): Promise<T>;
  viewport(): { inner_width: number; inner_height: number; dpr: number };
  waitViewport(timeoutMs: number, differentFrom: { inner_width: number; inner_height: number; dpr: number }): Promise<boolean>;
}

export async function runVisualCenter(page: VisualCenterPage, probe: TerminalProbe | null, awaitParent: AwaitParent): Promise<Record<string, unknown>> {
  if (!probe) throw new Error("visual-center: terminal probe is not enabled for this window (params.terminal_probe)");
  const identity = page.identity();
  if (!identity || !identity.endpoint.endsWith(" · Local")) {
    throw new Error(`precondition: visual-center needs the confirmed Local identity; got ${JSON.stringify(identity)}`);
  }
  const parent: Record<string, Record<string, unknown>> = {};
  const step = async (name: (typeof CENTER_STEPS)[number], detail: Record<string, unknown> = {}) => {
    const current = page.identity() ?? identity;
    parent[name] = await awaitParent(name, { ...current, ...detail });
    return parent[name]!;
  };

  await step("center-engine-before");
  const before = { header: page.header(), tabs: page.tabs(), panes: page.panes() };

  // Header actions and tab bar: each click is one user action; the parent reads the engine after.
  // Between two layout actions the surface must be settled again (a pane confirmed in the metadata),
  // otherwise the controller has no confirmed target and the action is refused before it is sent.
  // A control is clicked only while the window offers it: the header disables its actions until the
  // surface and the agents topology agree on the pane they would address.
  const clickWhenEnabled = async (what: string, selector: string) => {
    await page.waitFor(`${what}: enabled control ${selector}`, () => page.enabled(selector), 60000);
    return page.click(selector);
  };
  const settle = async (what: string) => {
    await page.waitFor(`${what}: a confirmed pane in the metadata`, () => page.panes().length > 0 && page.focusedPane() !== null, 60000);
    // The path of a new pane arrives with the next pane event; waiting for it keeps the next action
    // from being sent before the client has this layout (the value itself is checked by the parent).
    await page
      .waitFor(`${what}: a path on every frame`, () => page.frames().length > 0 && page.frames().every((f) => f.path !== null), 5000)
      .catch(() => null);
    await page.sleep(700);
  };
  const actions: Record<string, unknown> = {};
  actions.plus_click = await clickWhenEnabled("new tab", "[data-center-tabs] [data-new-tab]");
  await page.waitFor("a new tab in the bar", () => page.tabs().length > before.tabs.length, 60000);
  await settle("new tab");

  actions.split_click = await clickWhenEnabled("split", "[data-split-right]");
  await page.waitFor("two panes in the metadata", () => page.panes().length === 2, 60000);
  await settle("split");
  actions.panes_after_split = page.panes();

  // Clicking a frame focuses that pane in the engine (once); the agent then starts in that pane.
  const other = page.panes().find((p) => p !== page.focusedPane());
  if (!other) throw new Error(`visual-center: no second pane to focus (${page.panes().join(", ")})`);
  actions.focus_target = other;
  actions.focus_click = await clickWhenEnabled("focus", `[data-pane-frame="${other}"]`);
  await page.waitFor("the clicked pane confirmed focused", () => page.focusedPane() === other, 60000);
  await settle("focus");
  actions.focused_after_click = page.focusedPane();

  actions.agent_click = await clickWhenEnabled("agent", '[data-action="newAgent"]');
  await page.waitFor("new agent popover", () => page.enabled("[data-new-agent-popover] [data-start-agent]"), 5000);
  actions.agent_start = page.click("[data-new-agent-popover] [data-start-agent]");
  actions.agent_pane = other;
  await page.waitFor(
    "the agent named in its pane frame",
    () => page.frames().some((f) => f.pane_id === other && f.name !== "" && f.name !== "shell"),
    60000,
  );
  actions.frames_after_agent = page.frames().map((f) => ({ pane_id: f.pane_id, name: f.name, state: f.state }));

  // Expand asks the engine to zoom that pane; the parent checks the engine state and restores it.
  actions.zoom_click = await clickWhenEnabled("zoom", `[data-zoom="${other}"], [data-zoom]`);
  actions.panes_zoomed = await page
    .waitFor("the zoomed pane alone in the metadata", () => (page.panes().length === 1 ? page.panes() : null), 20000)
    .catch(() => page.panes());

  await step("center-actions", { actions });
  // The parent restored the layout; the frames are measured with both panes again.
  await page.waitFor("both panes back after the parent restored the zoom", () => page.panes().length === 2, 60000);
  // The path of a pane created during the phase reaches the client with the next pane event; the
  // check compares it against the engine's own cwd, this only waits for it to arrive.
  await page
    .waitFor("a path on every frame", () => page.frames().length > 0 && page.frames().every((f) => f.path !== null), 20000)
    .catch(() => null);
  const seen = { header: page.header(), tabs: page.tabs(), frames: page.frames(), panes: page.panes(), focused: page.focusedPane() };

  // Four stages of resize/DPI (the same as resize-dpi): the frames are measured at each one.
  const stages: StageSeen[] = [];
  for (const stage of CENTER_STAGES) {
    if (stage !== "start") {
      const view = page.viewport();
      await step(`center-apply-${stage}` as (typeof CENTER_STEPS)[number], { stage, view });
      await page.waitViewport(20000, view);
      await page.sleep(600);
    }
    await page.waitFor("panes of the settled surface", () => page.panes().length > 0, 30000);
    await page.sleep(200);
    const observed = page.stage(stage, probe);
    stages.push(observed);
    await step(`center-observe-${stage}` as (typeof CENTER_STEPS)[number], { stage, panes: observed.panes, canvas: observed.canvas, surface: observed.surface });
  }

  // Hidden terminal: no frame, no repaint, no overlay work (AC-013-03).
  const hiddenBefore = probe.snapshot();
  page.showFiles();
  await page.sleep(HIDDEN_MS);
  const hidden = { before: hiddenBefore, after: probe.snapshot(), frames: page.frames().length, ms: HIDDEN_MS };
  page.showTerminal();
  await page.waitFor("the frames back with the terminal", () => page.frames().length > 0, 30000);

  return { identity, before, actions, seen, stages, hidden, parent };
}

// ------------------------------------------------------------------------------------ DOM page

const rectOf = (el: Element | null): Rect | null => {
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { left: r.left, top: r.top, width: r.width, height: r.height };
};

const text = (el: Element | null | undefined): string => (el instanceof HTMLElement ? el.innerText.trim() : (el?.textContent?.trim() ?? ""));

export function domVisualCenterPage(root: HTMLElement, probe: TerminalProbe): VisualCenterPage {
  const header = () => root.querySelector<HTMLElement>("[data-center-header]");
  const geometry = (probe: TerminalProbe) => probe.geometry();
  const page: VisualCenterPage = {
    identity: () => {
      const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
      const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim())[1] ?? "";
      return id && endpoint ? { ...id, endpoint } : null;
    },
    header: () => {
      const host = header();
      return {
        crumbs: Array.from(host?.querySelectorAll<HTMLElement>("[data-crumb]") ?? []).map((el) => el.innerText.trim()),
        branch: text(host?.querySelector("[data-branch]")) || null,
        path: host?.querySelector<HTMLElement>("[data-path]")?.getAttribute("title") ?? null,
        actions: Array.from(host?.querySelectorAll<HTMLElement>("[data-action]") ?? []).map((el) => el.dataset.action ?? ""),
        empty: host?.querySelector("[data-empty]") !== null && host?.querySelector("[data-empty]") !== undefined,
      };
    },
    tabs: () =>
      Array.from(root.querySelectorAll<HTMLElement>("[data-center-tabs] [data-tab]")).map((el) => ({
        tab_id: el.dataset.tab ?? "",
        label: text(el.querySelector("[data-label]")),
        panes: text(el.querySelector("[data-panes]")),
        dot: el.querySelector<HTMLElement>("[data-dot]")?.dataset.dot ?? null,
        active: el.dataset.active === "true",
      })),
    frames: () =>
      Array.from(root.querySelectorAll<HTMLElement>("[data-pane-frame]")).map((frame) => {
        const pane_id = frame.dataset.paneFrame ?? "";
        const divider = root.querySelector<HTMLElement>(`[data-edge-of="${pane_id}"]`);
        const style = divider ? getComputedStyle(divider) : null;
        return {
          pane_id,
          rect: rectOf(frame)!,
          band: rectOf(divider),
          name: frame.dataset.name ?? "",
          state: frame.dataset.stateLabel ?? "",
          state_tone: "",
          path: frame.dataset.panePath || null,
          edge: frame.dataset.edge ?? "",
          edge_color: style?.backgroundColor ?? "",
          edge_width: divider ? "1px" : "0px",
          focused: frame.dataset.focused === "true",
          cache: false,
        };
      }),
    stage: (name, probe) => {
      const g = geometry(probe);
      return {
        stage: name,
        ...page.viewport(),
        canvas: rectOf(root.querySelector(".terminal canvas")),
        stage_rect: rectOf(root.querySelector("[data-center-stage]")),
        surface: g.surface,
        panes: g.panes,
        frames: page.frames(),
        toolbar: rectOf(root.querySelector('[role=toolbar][aria-label="Ações do terminal"]')),
        probe: probe.snapshot(),
      };
    },
    // The panes are the last accepted frame metadata (probe), never a count of overlay elements.
    panes: () => probe.geometry().panes.map((p) => p.pane_id),
    focusedPane: () => root.querySelector<HTMLElement>('[data-pane-frame][data-focused="true"]')?.dataset.paneFrame ?? null,
    enabled: (selector) => {
      const control = root.querySelector<HTMLElement>(selector);
      if (!control) return false;
      // Unavailable controls stay focusable with `aria-disabled`; they must not be clicked.
      return control.getAttribute("aria-disabled") !== "true" && !(control instanceof HTMLButtonElement && control.disabled);
    },
    click: (selector) => {
      const button = root.querySelector<HTMLElement>(selector);
      if (!button) throw new Error(`visual-center: ${selector} not found`);
      if (button instanceof HTMLButtonElement && button.disabled) throw new Error(`visual-center: ${selector} is disabled`);
      button.click();
      return selector;
    },
    showFiles: () => root.querySelector<HTMLButtonElement>('button[aria-label="Arquivos e diff"]')?.click(),
    showTerminal: () => {
      const hidden = root.querySelector('section.layer[aria-label="Terminal"].hidden');
      if (hidden) root.querySelector<HTMLButtonElement>('button[aria-label="Arquivos e diff"]')?.click();
    },
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    waitFor: (what, probe, timeoutMs = 20000) => waitFor(what, probe, timeoutMs),
    viewport: () => ({ inner_width: window.innerWidth, inner_height: window.innerHeight, dpr: window.devicePixelRatio }),
    waitViewport: async (timeoutMs, differentFrom) => {
      const deadline = performance.now() + timeoutMs;
      while (performance.now() < deadline) {
        const now = page.viewport();
        if (now.inner_width !== differentFrom.inner_width || now.inner_height !== differentFrom.inner_height || now.dpr !== differentFrom.dpr) return true;
        await page.sleep(50);
      }
      return false;
    },
  };
  return page;
}

