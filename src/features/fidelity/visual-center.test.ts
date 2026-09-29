// Spec 013 — the page scenario of the `visual-center` phase: the order of the parent steps, one
// click per user action and observations that are raw (the parent computes every expectation).
import { describe, expect, it } from "vitest";
import type { TerminalProbe } from "../../terminal/probe";
import { CENTER_STAGES, CENTER_STEPS, runVisualCenter, type FrameSeen, type VisualCenterPage } from "./visual-center";

const rect = (left: number, top: number, width: number, height: number) => ({ left, top, width, height });

function frame(pane_id: string, name: string, focused: boolean): FrameSeen {
  return {
    pane_id,
    rect: rect(0, 0, 100, 100),
    band: rect(0, -19, 100, 19),
    name,
    state: "Ocioso",
    state_tone: "idle",
    path: "/work",
    edge: focused ? "accent" : "border",
    edge_color: "rgb(143, 168, 255)",
    edge_width: "1px",
    focused,
    cache: false,
  };
}

function probe(): TerminalProbe {
  return {
    snapshot: () => ({ raf_requests: 3, raf_callbacks: 3, paint_calls: 3, painted_rows: 30, full_frames: 1 }),
    geometry: () => ({ surface: { cols: 100, rows: 40 }, panes: [{ pane_id: "w1:p1", inner_rect: { x: 0, y: 0, width: 50, height: 40 } }], full_frames: 1 }),
    takePointerRoutes: () => [],
  };
}

function fakePage(state: { panes: string[]; focused: string; tabs: number; agentNamed: boolean; clicks: string[] }): VisualCenterPage {
  const tabs = () => Array.from({ length: state.tabs }, (_, i) => ({ tab_id: `w1:t${i + 1}`, label: `t${i + 1}`, panes: "1", dot: null, active: i === 0 }));
  return {
    identity: () => ({ pane_id: "w1:p1", generation: "3", boot_prefix: "boot", endpoint: "Este computador · Local" }),
    header: () => ({ crumbs: ["Fidelidade", "Local"], branch: "⎇ hd010-branch", path: "/work", actions: ["split", "newTab", "newAgent"], empty: false }),
    tabs,
    frames: () => state.panes.map((p) => frame(p, state.agentNamed && p === state.focused ? "pi" : "shell", p === state.focused)),
    stage: (name) => ({
      stage: name,
      inner_width: 1440,
      inner_height: 900,
      dpr: 1,
      canvas: rect(0, 0, 900, 760),
      stage_rect: rect(0, 0, 900, 780),
      surface: { cols: 100, rows: 40 },
      panes: state.panes.map((p) => ({ pane_id: p, inner_rect: { x: 0, y: 0, width: 50, height: 40 } })),
      frames: state.panes.map((p) => frame(p, "shell", p === state.focused)),
      toolbar: null,
      probe: { raf_requests: 3, raf_callbacks: 3, paint_calls: 3, painted_rows: 30, full_frames: 1 },
    }),
    panes: () => state.panes,
    focusedPane: () => state.focused,
    enabled: () => true,
    click: (selector) => {
      state.clicks.push(selector);
      if (selector.includes("newTab") || selector.includes("new-tab")) state.tabs += 1;
      if (selector.includes("split")) state.panes = ["w1:p1", "w1:p2"];
      if (selector.includes("newAgent") || selector.includes("data-start-agent")) state.agentNamed = true;
      if (selector.includes("data-identity") || selector.includes("data-pane-frame")) state.focused = "w1:p2";
      if (selector.includes("data-zoom")) state.panes = ["w1:p2"];
      return selector;
    },
    showFiles: () => state.clicks.push("files"),
    showTerminal: () => state.clicks.push("terminal"),
    sleep: async () => {},
    waitFor: async (_what, p) => {
      const value = p();
      if (!value) throw new Error(`fake page: ${_what}`);
      return value as never;
    },
    viewport: () => ({ inner_width: 1440, inner_height: 900, dpr: 1 }),
    waitViewport: async () => true,
  };
}

describe("visual-center page scenario", () => {
  // Would catch: a step renamed or dropped on the page while the parent still drives it (the phase
  // would hang or the parent would never measure a stage).
  it("asks the parent for every step of the phase, in order", async () => {
    const state = { panes: ["w1:p1"], focused: "w1:p1", tabs: 1, agentNamed: false, clicks: [] as string[] };
    const asked: string[] = [];
    await runVisualCenter(fakePage(state), probe(), async (step) => {
      asked.push(step);
      // The parent restores the zoom it measured in the engine.
      if (step === "center-actions") state.panes = ["w1:p1", "w1:p2"];
      return {};
    });
    expect(asked).toEqual([...CENTER_STEPS]);
    expect(CENTER_STAGES.map((s) => `center-observe-${s}`).every((s) => asked.includes(s))).toBe(true);
  });

  // Would catch: an action clicked twice (an agent started twice, two splits) or an action skipped.
  it("clicks each header/tab/frame action exactly once", async () => {
    const state = { panes: ["w1:p1"], focused: "w1:p1", tabs: 1, agentNamed: false, clicks: [] as string[] };
    const report = await runVisualCenter(fakePage(state), probe(), async (step) => {
      if (step === "center-actions") state.panes = ["w1:p1", "w1:p2"];
      return {};
    });
    const clicks = state.clicks.filter((c) => c.startsWith("["));
    expect(clicks.filter((c) => c.includes('data-action="newAgent"'))).toHaveLength(1);
    expect(clicks.filter((c) => c.includes("data-start-agent"))).toHaveLength(1);
    expect(clicks.filter((c) => c.includes("split-right") || c.includes('data-action="split"'))).toHaveLength(1);
    expect(clicks.filter((c) => c.includes("data-new-tab"))).toHaveLength(1);
    expect(clicks.filter((c) => c.includes("data-pane-frame") || c.includes("data-identity"))).toHaveLength(1);
    expect(clicks.filter((c) => c.includes("data-zoom"))).toHaveLength(1);
    expect((report.stages as unknown[]).length).toBe(CENTER_STAGES.length);
  });

  // Would catch: the phase reporting a verdict instead of observations (the parent owns the checks).
  it("reports raw observations only: no boolean check is computed on the page", async () => {
    const state = { panes: ["w1:p1"], focused: "w1:p1", tabs: 1, agentNamed: false, clicks: [] as string[] };
    const report = await runVisualCenter(fakePage(state), probe(), async (step) => {
      if (step === "center-actions") state.panes = ["w1:p1", "w1:p2"];
      return {};
    });
    const flat = JSON.stringify(report);
    expect(Object.keys(report).sort()).toEqual(["actions", "before", "hidden", "identity", "parent", "seen", "stages"]);
    expect(flat).not.toMatch(/"(ok|pass|passed|check|verdict)":/);
    expect((report.hidden as { ms: number }).ms).toBe(2000);
  });

  // Would catch: the phase running without the probe, where "no repaint while hidden" cannot be measured.
  it("refuses to run without the terminal probe", async () => {
    const state = { panes: ["w1:p1"], focused: "w1:p1", tabs: 1, agentNamed: false, clicks: [] as string[] };
    await expect(runVisualCenter(fakePage(state), null, async () => ({}))).rejects.toThrow(/terminal probe/);
  });
});
