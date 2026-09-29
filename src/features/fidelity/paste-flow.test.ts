// Pure seams of the paste-selection phase (no DOM, engine, GUI or clipboard): names and literal
// cells equal the Rust parent, metrics come from the terminal canvas font and near misses throw.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { cellCenter, cellMetrics, paneCells, PASTE_STEPS, SELECTION_CELLS, runPasteSelection, record } from "./paste-flow";

const rust = readFileSync(new URL("../../../tests/fidelity-native/paste_flow.rs", import.meta.url), "utf8");

describe("paste-selection seams", () => {
  it("steps and dragged cells equal paste_flow.rs", () => {
    const block = /pub const STEPS: \[&str; \d+\] = \[([\s\S]*?)\];/.exec(rust)?.[1] ?? "";
    expect(Array.from(block.matchAll(/"([a-z-]+)"/g), (m) => m[1])).toEqual([...PASTE_STEPS]);
    const end = Number(/pub const SELECTION_END_COL: u16 = (\d+);/.exec(rust)?.[1]);
    expect(SELECTION_CELLS).toEqual({ anchor: [0, 0], cursor: [1, end] });
  });

  it("metrics follow TerminalView.measure from the painted canvas font", () => {
    const measure = (font: string) => (font === "14px monospace" ? 8.4 : 0);
    expect(cellMetrics("14px monospace", measure)).toEqual({ cellWidth: 9, cellHeight: 19 });
    expect(() => cellMetrics("10px sans-serif", () => 0)).toThrow(/cell width/);
    expect(() => cellMetrics("monospace", () => 8)).toThrow(/font size/);
  });

  it("cell centers land inside the intended cell, offset by the pane origin", () => {
    const m = { cellWidth: 9, cellHeight: 19 };
    const p = cellCenter({ left: 100, top: 50 }, m, [2, 1], 1, 18);
    expect(Math.floor((p.x - 100) / 9)).toBe(20);
    expect(Math.floor((p.y - 50) / 19)).toBe(2);
  });

  it("records Ctrl+Shift+V and Ctrl+Shift+C keydowns and isolates paste events", () => {
    const listeners: Record<string, ((e: Event) => void)[]> = {};
    const fakeTarget = {
      addEventListener: (type: string, fn: (e: Event) => void) => {
        (listeners[type] ??= []).push(fn);
      },
    } as unknown as HTMLElement;
    const rec = record(fakeTarget);
    const dispatch = (e: Event) => listeners[e.type]?.forEach((fn) => fn(e));

    // Dispatch Ctrl+Shift+V
    dispatch({ type: "keydown", isTrusted: true, key: "v", ctrlKey: true, shiftKey: true } as unknown as Event);
    // Dispatch unrelated keydown
    dispatch({ type: "keydown", isTrusted: true, key: "x", ctrlKey: true, shiftKey: false } as unknown as Event);
    // Dispatch Ctrl+Shift+C
    dispatch({ type: "keydown", isTrusted: true, key: "c", ctrlKey: true, shiftKey: true } as unknown as Event);

    const keydowns = rec.take("keydown");
    expect(keydowns).toHaveLength(2);
    expect(keydowns[0]!.key).toBe("v");
    expect(keydowns[1]!.key).toBe("c");

    // Zero paste events
    expect(rec.take("paste")).toEqual([]);
  });

  it("window capture records the chord keydown the terminal textarea preventDefaults, once, and take keeps other types", () => {
    // GUI r4: the PTY received the pasted bytes once while paste_keydowns was []. Two recorder defects
    // are caught here: listening on the textarea (a product listener that stops the event first hides
    // it) and take(type) dropping every other type (take("paste") before take("keydown") erased it).
    type L = { fn: (e: Event) => void; capture: boolean };
    type N = { name: string; parent: N | null; listeners: Record<string, L[]> };
    const node = (name: string, parent: N | null): N => ({ name, parent, listeners: {} });
    const target = (n: N) =>
      ({
        addEventListener: (type: string, fn: (e: Event) => void, o?: boolean | AddEventListenerOptions) =>
          (n.listeners[type] ??= []).push({ fn, capture: typeof o === "boolean" ? o : !!o?.capture }),
        removeEventListener: (type: string, fn: (e: Event) => void) => {
          n.listeners[type] = (n.listeners[type] ?? []).filter((l) => l.fn !== fn);
        },
      }) as unknown as EventTarget;
    const win = node("window", null);
    const doc = node("document", win);
    const textarea = node("textarea.ime-target", doc);
    // Product listener on the textarea itself (TerminalView): preventDefault + stop.
    target(textarea).addEventListener("keydown", (e: Event) => {
      e.preventDefault();
      e.stopImmediatePropagation();
    });
    const dispatch = (at: N, init: Record<string, unknown>) => {
      let stopped = false;
      let prevented = false;
      const e = { ...init, preventDefault: () => (prevented = true), stopPropagation: () => (stopped = true), stopImmediatePropagation: () => (stopped = true) } as unknown as Event;
      const path: N[] = [];
      for (let n: N | null = at; n; n = n.parent) path.unshift(n);
      for (const n of path) {
        for (const l of n.listeners[init.type as string] ?? []) {
          if (stopped) break;
          if (l.capture || n === at) l.fn(e);
        }
        if (stopped) break;
      }
      return prevented;
    };
    const rec = record(target(win));
    expect(dispatch(textarea, { type: "keydown", isTrusted: true, key: "V", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(rec.take("paste")).toEqual([]);
    const keydowns = rec.take("keydown");
    expect(keydowns).toHaveLength(1);
    expect(keydowns[0]).toMatchObject({ type: "keydown", trusted: true, key: "V", ctrl: true, shift: true });
    expect(rec.take("keydown")).toEqual([]);
    rec.stop();
    dispatch(textarea, { type: "keydown", isTrusted: true, key: "V", ctrlKey: true, shiftKey: true });
    expect(rec.take("keydown")).toEqual([]);
    // The phase attaches the recorder to window, never to the textarea the terminal stops.
    const flow = readFileSync(new URL("./paste-flow.ts", import.meta.url), "utf8");
    expect(flow).toMatch(/const events = record\(window\);/);
    expect(flow).not.toMatch(/record\(target\)/);
  });

  it("refuses to run without the harness bridge before any parent step", async () => {
    const calls: string[] = [];
    const parent = async (step: string) => (calls.push(step), {});
    await expect(runPasteSelection(null as unknown as HTMLElement, parent)).rejects.toThrow(/root/);
    expect(calls).toEqual([]);
  });
});

describe("pane origin lookup", () => {
  // Minimal matcher for `.class`, `[attr]` and `[attr="v"]` compound selectors over fake elements.
  type El = { classes: string[]; attrs: Record<string, string>; dataset: Record<string, string> };
  const el = (classes: string[], attrs: Record<string, string>): El => ({
    classes,
    attrs,
    dataset: Object.fromEntries(Object.entries(attrs).filter(([k]) => k.startsWith("data-")).map(([k, v]) => [k.slice(5), v])),
  });
  const matches = (e: El, selector: string) =>
    Array.from(selector.matchAll(/\.([\w-]+)|\[([\w-]+)(?:="([^"]*)")?\]/g)).every(([, cls, attr, value]) =>
      cls ? e.classes.includes(cls) : attr! in e.attrs && (value === undefined || e.attrs[attr!] === value),
    ) && selector.replace(/\.([\w-]+)|\[([\w-]+)(?:="([^"]*)")?\]/g, "") === "";
  const root = (els: El[]) => ({ querySelector: (sel: string) => els.find((e) => matches(e, sel)) ?? null }) as unknown as HTMLElement;

  it("reads the cells of the confirmed pane in the list branch the native window showed", () => {
    // Native r1 (pointer-live): AgentPanel rendered the pane list (`button[data-pane][data-cells]`,
    // no `.pane` class) and the phase threw "pane w3:p1 cells not shown". Would catch: requiring the
    // surface-branch class, reading ConnectionsPanel's data-pane (no cells) or another pane's cells.
    const connections = el(["row"], { "data-pane": "w3:p1" });
    const other = el([], { "data-pane": "w3:p2", "data-cells": "40,0,35,29" });
    const list = el([], { "data-pane": "w3:p1", "data-cells": "0,0,75,29" });
    expect(paneCells(root([connections, other, list]), "w3:p1")).toEqual([0, 0, 75, 29]);
    const surface = el(["pane", "confirmed"], { "data-pane": "w3:p1", "data-cells": "2,1,70,20" });
    expect(paneCells(root([connections, surface]), "w3:p1")).toEqual([2, 1, 70, 20]);
    expect(paneCells(root([connections, other]), "w3:p1")).toBeNull();
  });
});
