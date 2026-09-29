// Seam of the mouse-scroll-links linkage in e2e.ts: every pointer step carries the page's client
// viewport (the parent turns it into the observed GTK header offset); nothing else is changed.
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../../terminal/probe", () => ({ terminalProbe: vi.fn() }));

const { withClientViewport } = await import("./e2e");

describe("pointer step linkage", () => {
  it("adds the observed client viewport to each step and returns the parent answer untouched", async () => {
    // Would catch: points sent without the viewport (parent would have to guess the header bar),
    // a stale viewport captured once, or a detail/answer rewritten on the way.
    const seen: Array<[string, Record<string, unknown>]> = [];
    const parent = async (step: string, detail: Record<string, unknown>) => {
      seen.push([step, detail]);
      return { step, ok: true };
    };
    const win = { innerWidth: 1280, innerHeight: 673, devicePixelRatio: 1, screenX: 0, screenY: 0 };
    const wrapped = withClientViewport(parent, win);
    const answer = await wrapped("alt-screen-mouse", { pane_id: "w1:p1", points: { click: { x: 5, y: 9 } } });
    win.innerHeight = 650;
    win.screenX = 3;
    await wrapped("scrollback-wheel", { pane_id: "w1:p1" });
    expect(answer).toEqual({ step: "alt-screen-mouse", ok: true });
    expect(seen).toEqual([
      ["alt-screen-mouse", { pane_id: "w1:p1", points: { click: { x: 5, y: 9 } }, client: { coordinate_space: "client", width: 1280, height: 673, dpr: 1, screen_x: 0, screen_y: 0 } }],
      ["scrollback-wheel", { pane_id: "w1:p1", client: { coordinate_space: "client", width: 1280, height: 650, dpr: 1, screen_x: 3, screen_y: 0 } }],
    ]);
  });

  it("dispatches resize-dpi and a11y-navigation to the prepared runners on the Local host", async () => {
    // Would catch: the phases still throwing as pending, or running without returning to Local.
    const { readFileSync } = await import("node:fs");
    const source = readFileSync(new URL("./e2e.ts", import.meta.url), "utf8");
    expect(source).toMatch(/case "resize-dpi":\s*await returnToLocal\(root\);\s*return runResizeDpi\(domResizePage\(root\), terminalProbe\(\), windowDeps\(awaitParent\)\);/);
    expect(source).toMatch(/case "a11y-navigation":\s*await returnToLocal\(root\);\s*return runA11yNavigation\(domA11yPage\(root\), windowDeps\(awaitParent\)\);/);
  });
});
