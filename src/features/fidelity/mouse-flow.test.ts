// Pure seams of the mouse-scroll-links phase (no DOM, engine, GUI, pointer or opener): names and
// literal cells equal the Rust parent, output points refuse to leave the private output, and the
// recorder keeps untrusted events visible instead of dropping them.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ActionLogEntry } from "../../shell/actions";
import type { PointerRouteTrace } from "../../terminal/probe";
import { MOUSE_CELLS, MOUSE_STEPS, clientPoint, elementFromPoint, linkActionSummary, outputPoint, recordPointer, runMouseScrollLinks, surfaceError } from "./mouse-flow";

const rust = readFileSync(new URL("../../../tests/fidelity-native/mouse_flow.rs", import.meta.url), "utf8");
const cell = (name: string) => {
  const m = new RegExp(`pub const ${name}: \\(u16, u16\\) = \\((\\d+), (\\d+)\\);`).exec(rust);
  return m ? [Number(m[1]), Number(m[2])] : null;
};

describe("mouse-scroll-links seams", () => {
  it("steps and cells equal mouse_flow.rs", () => {
    const block = /pub const STEPS: \[&str; \d+\] = \[([\s\S]*?)\];/.exec(rust)?.[1] ?? "";
    expect(Array.from(block.matchAll(/"([a-z-]+)"/g), (m) => m[1])).toEqual([...MOUSE_STEPS]);
    expect(MOUSE_CELLS).toEqual({ click: cell("CLICK_CELL"), wheel: cell("WHEEL_CELL"), scroll: cell("SCROLL_CELL"), link: cell("LINK_CELL") });
  });

  it("output points add the window origin and refuse points outside the private output", () => {
    expect(outputPoint({ x: 55.5, y: 45 }, { x: 0, y: 0 })).toEqual({ x: 55.5, y: 45 });
    expect(outputPoint({ x: 10, y: 20 }, { x: 100, y: 30 })).toEqual({ x: 110, y: 50 });
    expect(() => outputPoint({ x: 0.4, y: 10 }, { x: 0, y: 0 })).toThrow(/output/);
    expect(() => outputPoint({ x: 1279.5, y: 719.5 }, { x: 1, y: 1 })).toThrow(/output/);
    expect(() => outputPoint({ x: Number.NaN, y: 10 }, { x: 0, y: 0 })).toThrow(/output/);
  });

  it("records pointer and wheel events with trust, button and ctrl; untrusted stay visible", () => {
    const target = new EventTarget();
    const rec = recordPointer(target);
    const synthetic = Object.assign(new Event("pointerdown"), { button: 0, ctrlKey: true, clientX: 3, clientY: 4 });
    target.dispatchEvent(synthetic);
    target.dispatchEvent(Object.assign(new Event("wheel"), { deltaY: -53, ctrlKey: false }));
    target.dispatchEvent(new Event("keydown"));
    expect(rec.take()).toEqual([
      { type: "pointerdown", trusted: false, button: 0, ctrl: true, deltaY: 0, x: 3, y: 4 },
      { type: "wheel", trusted: false, button: -1, ctrl: false, deltaY: -53, x: 0, y: 0 },
    ]);
    expect(rec.take()).toEqual([]);
  });

  it("refuses to run without root before any parent step", async () => {
    const calls: string[] = [];
    const parent = async (step: string) => (calls.push(step), {});
    await expect(runMouseScrollLinks(null as unknown as HTMLElement, parent)).rejects.toThrow(/root/);
    expect(calls).toEqual([]);
  });

  it("sends client points unchanged: window.screenX/Y is never added, and the point must be inside the viewport", () => {
    // Would catch: adding WebKit's bogus screenX/Y (r2: 20,20) to the point, or a point outside the client.
    const viewport = { innerWidth: 1280, innerHeight: 673, screenX: 20, screenY: 20 };
    expect(clientPoint({ x: 383.5, y: 137.5 }, viewport)).toEqual({ x: 383.5, y: 137.5 });
    expect(() => clientPoint({ x: 1280, y: 10 }, viewport)).toThrow(/client/);
    expect(() => clientPoint({ x: 10, y: 673.5 }, viewport)).toThrow(/client/);
    expect(() => clientPoint({ x: -1, y: 10 }, viewport)).toThrow(/client/);
    expect(() => clientPoint({ x: Number.NaN, y: 10 }, viewport)).toThrow(/client/);
    const source = readFileSync(new URL("./mouse-flow.ts", import.meta.url), "utf8");
    expect(source).not.toMatch(/window\.screen[XY]/);
  });

  it("records pointer events on the .terminal container in capture phase, capturing child canvas events", () => {
    class FakeElement extends EventTarget {
      listeners: { type: string; capture: boolean }[] = [];
      override addEventListener(type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | AddEventListenerOptions): void {
        const capture = typeof options === "boolean" ? options : !!options?.capture;
        this.listeners.push({ type, capture });
        super.addEventListener(type, listener, options);
      }
    }
    const container = new FakeElement();
    recordPointer(container);
    expect(container.listeners).toEqual([
      { type: "pointerdown", capture: true },
      { type: "pointerup", capture: true },
      { type: "wheel", capture: true },
    ]);

    // In mouse-flow.ts, pointer recorder must be bound to the .terminal container, not textarea target
    const source = readFileSync(new URL("./mouse-flow.ts", import.meta.url), "utf8");
    expect(source).toMatch(/recordPointer\(\s*terminal\s*\)/);
    expect(source).not.toMatch(/recordPointer\(\s*target\s*\)/);
  });

  it("samples document.elementFromPoint returning tag and classes even when null or with multiple classes", () => {
    const fakeDoc = {
      elementFromPoint: (x: number, y: number) => {
        if (x > 0 && y > 0) {
          return { tagName: "DIV", classList: ["terminal-pane", "active", "focused"] } as unknown as Element;
        }
        return null;
      },
    };
    expect(elementFromPoint({ x: 50, y: 100 }, fakeDoc)).toEqual({
      tag: "div",
      classes: ["terminal-pane", "active", "focused"],
    });
    expect(elementFromPoint({ x: -10, y: -10 }, fakeDoc)).toBeNull();

    const prev = (globalThis as any).document;
    try {
      (globalThis as any).document = fakeDoc;
      expect(elementFromPoint({ x: 50, y: 100 })).toEqual({
        tag: "div",
        classes: ["terminal-pane", "active", "focused"],
      });
      expect(elementFromPoint({ x: -10, y: -10 })).toBeNull();
    } finally {
      (globalThis as any).document = prev;
    }
  });

  it("runMouseScrollLinks records elementFromPoint for all requested points even when no events arrive", async () => {
    const prev = (globalThis as any).document;
    const prevWindow = (globalThis as any).window;
    try {
      const fakeOverlay = { tagName: "DIV", classList: ["blocking-overlay", "backdrop"] };
      (globalThis as any).document = {
        elementFromPoint: () => fakeOverlay,
      };
      (globalThis as any).window = { innerWidth: 1280, innerHeight: 720 };

      const calls: string[] = [];
      const awaitParent = async (step: string) => {
        calls.push(step);
        return { step, status: "ok" };
      };

      // Mock DOM structure
      const elements: Record<string, any> = {
        "textarea.ime-target": { focus: () => {} },
        ".status-bar [data-phase=live]": { title: "pane p1 · geração g1 · boot b1" },
        ".terminal": {
          addEventListener: () => {},
          querySelector: (sel: string) => (sel === "canvas" ? elements["canvas"] : null),
        },
        canvas: {
          getContext: () => ({
            font: "14px monospace",
            measureText: () => ({ width: 8.4 }),
          }),
          getBoundingClientRect: () => ({ left: 200, top: 100, width: 800, height: 600 }),
        },
        "button[data-pane=\"p1\"][data-cells]": {
          dataset: { cells: "0,0,80,24" },
          getAttribute: (name: string) => (name === "data-cells" ? "0,0,80,24" : null),
        },
        '[role=toolbar][aria-label="Ações do terminal"] input.uri': { value: "https://example.com" },
        '[role=toolbar][aria-label="Ações do terminal"] [role=status]': { innerText: "ready" },
      };

      const root = {
        querySelector: (sel: string) => {
          if (sel.includes('data-pane="p1"')) return elements['button[data-pane="p1"][data-cells]'];
          return elements[sel] ?? null;
        },
        querySelectorAll: (sel: string) => {
          if (sel === ".status-bar .status-item") {
            return [{ innerText: "item0" }, { innerText: "local" }];
          }
          if (sel.includes("button[data-pane]")) {
            return [elements["button[data-pane=\"p1\"][data-cells]"]];
          }
          return [];
        },
      } as unknown as HTMLElement;

      const report = await runMouseScrollLinks(root, awaitParent);
      expect(report.mouse_events).toEqual([]);
      expect(report.elements_from_point).toEqual({
        click: { tag: "div", classes: ["blocking-overlay", "backdrop"] },
        wheel: { tag: "div", classes: ["blocking-overlay", "backdrop"] },
        scroll: { tag: "div", classes: ["blocking-overlay", "backdrop"] },
        link: { tag: "div", classes: ["blocking-overlay", "backdrop"] },
      });
      expect(report.element_from_point).toEqual(report.elements_from_point);
    } finally {
      (globalThis as any).document = prev;
      (globalThis as any).window = prevWindow;
    }
  });
});


describe("terminal action results of the link step", () => {
  const uri = "https://a.invalid/?n=1";
  const settled = (over: Record<string, unknown> = {}): ActionLogEntry => ({ kind: "link", pane_id: "w3:p1", uri, at_ms: 1, stage: "settled", called: true, receipt: { pane_id: "w3:p1", sent: true }, result: { ok: true }, ...over }) as ActionLogEntry;
  const requested = (over: Record<string, unknown> = {}): ActionLogEntry => ({ kind: "link", pane_id: "w3:p1", uri, at_ms: 1, stage: "requested", waiting: 0, running: false, ...over }) as ActionLogEntry;
  const called = (over: Record<string, unknown> = {}): ActionLogEntry => ({ kind: "link", pane_id: "w3:p1", uri, at_ms: 1, stage: "called", ...over }) as ActionLogEntry;

  it("summarises the link step from the app's action log: requested, called, receipt sent and refusal code", () => {
    // Would catch: reporting "called" for a refusal before the bridge or for a link still queued,
    // dropping the refusal code, or turning sent:false into an opened link.
    expect(linkActionSummary(null)).toEqual({ log: "unavailable", link_action_requested: null, link_action_queue: null, link_action_called: null, link_action_settled: null, link_action_sent: null, link_action_error: null, other_kinds: [] });
    expect(linkActionSummary([])).toEqual({ log: "empty", link_action_requested: false, link_action_queue: null, link_action_called: false, link_action_settled: false, link_action_sent: null, link_action_error: null, other_kinds: [] });
    expect(linkActionSummary([requested(), called(), settled({ receipt: { pane_id: "w3:p1", sent: false } })])).toMatchObject({ log: "recorded", link_action_requested: true, link_action_queue: { waiting: 0, running: false }, link_action_called: true, link_action_settled: true, link_action_sent: false, link_action_error: null });
    expect(linkActionSummary([requested(), called(), settled({ receipt: null, result: { ok: false, code: "focus_changed", message: "m" } })])).toMatchObject({ link_action_called: true, link_action_sent: null, link_action_error: { code: "focus_changed", message: "m" } });
    expect(linkActionSummary([requested(), settled({ called: false, receipt: null, result: { ok: false, code: "no_identity", message: "n" } })])).toMatchObject({ link_action_requested: true, link_action_called: false, link_action_error: { code: "no_identity", message: "n" } });
    expect(linkActionSummary([called({ kind: "scroll", uri: null }), requested({ running: true })])).toEqual({ log: "recorded", link_action_requested: true, link_action_queue: { waiting: 0, running: true }, link_action_called: false, link_action_settled: false, link_action_sent: null, link_action_error: null, other_kinds: ["scroll"] });
  });

  it("runMouseScrollLinks reports only the actions seen during the link step, with the banner before and after", async () => {
    // Would catch: attributing the scroll step's action to the link, reading the log before the
    // parent clicked, or omitting SurfaceState.error.
    const prevDoc = (globalThis as any).document;
    const prevWindow = (globalThis as any).window;
    try {
      (globalThis as any).document = { elementFromPoint: () => null };
      (globalThis as any).window = { innerWidth: 1280, innerHeight: 720 };
      let pending: ActionLogEntry[] = [];
      let banner: unknown = null;
      const refused = settled({ receipt: null, result: { ok: false, code: "stale_surface", message: "s" } });
      const awaitParent = async (step: string) => {
        if (step === "scrollback-wheel") pending.push(settled({ kind: "scroll", uri: null }));
        if (step === "link-ctrl-click") {
          pending.push(requested(), called(), refused);
          banner = { dataset: { error: "surface_stale" }, innerText: "resync" };
        }
        return { step };
      };
      const log = { take: () => pending.splice(0) };
      const canvas = { getContext: () => ({ font: "14px monospace", measureText: () => ({ width: 8.4 }) }), getBoundingClientRect: () => ({ left: 0, top: 0, width: 800, height: 600 }) };
      const pane = { dataset: { cells: "0,0,80,24" }, getAttribute: (n: string) => (n === "data-cells" ? "0,0,80,24" : null) };
      const elements: Record<string, unknown> = {
        "textarea.ime-target": { focus: () => {} },
        ".status-bar [data-phase=live]": { title: "pane w3:p1 · geração 2 · boot b1" },
        ".terminal": { addEventListener: () => {}, querySelector: (sel: string) => (sel === "canvas" ? canvas : null) },
        '[role=toolbar][aria-label="Ações do terminal"] input.uri': { value: uri },
        '[role=toolbar][aria-label="Ações do terminal"] [role=status]': { innerText: "" },
      };
      const root = {
        querySelector: (sel: string) => (sel === "[role=alert][data-error]" ? banner : sel.includes('data-pane="w3:p1"') ? pane : (elements[sel] ?? null)),
        querySelectorAll: (sel: string) => (sel === ".status-bar .status-item" ? [{ innerText: "x" }, { innerText: "local" }] : sel.includes("button[data-pane]") ? [pane] : []),
      } as unknown as HTMLElement;
      const report = await runMouseScrollLinks(root, awaitParent, log, 0);
      expect(report.link_actions).toEqual([requested(), called(), refused]);
      expect(report.link_action).toMatchObject({ log: "recorded", link_action_requested: true, link_action_called: true, link_action_settled: true, link_action_sent: null, link_action_error: { code: "stale_surface", message: "s" } });
      expect(report.surface_error_before_link).toBeNull();
      expect(report.surface_error_after_link).toEqual({ code: "surface_stale", message: "resync" });
    } finally {
      (globalThis as any).document = prevDoc;
      (globalThis as any).window = prevWindow;
    }
  });

  it("reads the surface error banner (SurfaceState.error) code and message, or null", () => {
    // Would catch: reporting the banner text without its code, or a stale non-null when absent.
    const banner = { dataset: { error: "surface_stale" }, innerText: " a superfície está sendo ressincronizada " };
    expect(surfaceError({ querySelector: (s: string) => (s === "[role=alert][data-error]" ? banner : null) } as unknown as HTMLElement)).toEqual({ code: "surface_stale", message: "a superfície está sendo ressincronizada" });
    expect(surfaceError({ querySelector: () => null } as unknown as HTMLElement)).toBeNull();
  });

  // Spec 010 r4: which exit of TerminalView.onPointerDown the Ctrl+click took.
  // Would catch: attributing the alt-screen click or the wheel step's pointerdown to the link,
  // reading the probe before the parent clicked, or reporting [] when the window has no probe
  // (an absent probe must not look like "no pointerdown reached the terminal").
  it("runMouseScrollLinks reports the terminal pointer routes of each step from the opt-in probe, null without it", async () => {
    const prevDoc = (globalThis as any).document;
    const prevWindow = (globalThis as any).window;
    try {
      (globalThis as any).document = { elementFromPoint: () => null };
      (globalThis as any).window = { innerWidth: 1280, innerHeight: 720 };
      const route = (over: Partial<PointerRouteTrace>): PointerRouteTrace => ({ exit: "select", cell: { x: 0, y: 3 }, hit_pane: "w3:p1", button: "left", route: "select", mouse_reporting: false, stale: false, input_allowed: true, link_found: false, link_safe: null, ctrl: false, ...over });
      let routes: PointerRouteTrace[] = [];
      const click = route({ exit: "app", route: "app", mouse_reporting: true, cell: { x: 2, y: 5 }, link_found: null });
      const stray = route({ exit: "focus", route: "focus", cell: { x: 3, y: 4 }, link_found: null });
      const ctrl = route({ exit: "app_blocked", route: "app", mouse_reporting: true, input_allowed: false, link_found: null, ctrl: true });
      const awaitParent = async (step: string) => {
        if (step === "alt-screen-mouse") routes.push(click);
        if (step === "scrollback-wheel") routes.push(stray);
        if (step === "link-ctrl-click") routes.push(ctrl);
        return { step };
      };
      const probe = { takePointerRoutes: () => routes.splice(0) };
      const canvas = { getContext: () => ({ font: "14px monospace", measureText: () => ({ width: 8.4 }) }), getBoundingClientRect: () => ({ left: 0, top: 0, width: 800, height: 600 }) };
      const pane = { dataset: { cells: "0,0,80,24" }, getAttribute: (n: string) => (n === "data-cells" ? "0,0,80,24" : null) };
      const elements: Record<string, unknown> = {
        "textarea.ime-target": { focus: () => {} },
        ".status-bar [data-phase=live]": { title: "pane w3:p1 · geração 2 · boot b1" },
        ".terminal": { addEventListener: () => {}, querySelector: (sel: string) => (sel === "canvas" ? canvas : null) },
        '[role=toolbar][aria-label="Ações do terminal"] input.uri': { value: uri },
        '[role=toolbar][aria-label="Ações do terminal"] [role=status]': { innerText: "" },
      };
      const root = {
        querySelector: (sel: string) => (sel.includes('data-pane="w3:p1"') ? pane : (elements[sel] ?? null)),
        querySelectorAll: (sel: string) => (sel === ".status-bar .status-item" ? [{ innerText: "x" }, { innerText: "local" }] : sel.includes("button[data-pane]") ? [pane] : []),
      } as unknown as HTMLElement;
      const log = { take: () => [] as ActionLogEntry[] };
      const report = await runMouseScrollLinks(root, awaitParent, log, 0, probe);
      expect(report.mouse_pointer_routes).toEqual([click]);
      expect(report.scroll_pointer_routes).toEqual([stray]);
      expect(report.link_pointer_routes).toEqual([ctrl]);
      routes = [];
      const without = await runMouseScrollLinks(root, awaitParent, log, 0, null);
      expect(without.mouse_pointer_routes).toBeNull();
      expect(without.scroll_pointer_routes).toBeNull();
      expect(without.link_pointer_routes).toBeNull();
    } finally {
      (globalThis as any).document = prevDoc;
      (globalThis as any).window = prevWindow;
    }
  });
});
