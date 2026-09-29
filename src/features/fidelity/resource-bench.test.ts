// Resource bench page driver (spec 007, hiding adapted in spec 009): order of parent steps,
// same-client visible/hidden windows, and refusal to report without a real probe. Since the clean
// casca (spec 018) the composed window has a single terminal layer and no Files layer, so the
// hidden window is a window that is not visible — the product's own remaining path to
// `setInterest(false)` (src/shell/interest.ts).
import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { runResourceBench, domPage, type BenchPage } from "./resource-bench";

function fakeProbe() {
  let n = 0;
  // Every snapshot differs, so a report built from the wrong moment is visible.
  return { snapshot: vi.fn(() => { n += 1; return { raf_requests: n, raf_callbacks: n * 10, paint_calls: n * 100, painted_rows: n * 1000, full_frames: 0 }; }) };
}

function fakePage(overrides: Partial<BenchPage> = {}) {
  const state = { hidden: false, editor: false };
  const page: BenchPage = {
    identity: async () => ({ pane_id: "w3:p2", generation: "7", boot_prefix: "b00t" }),
    canvases: () => 1,
    geometry: () => ({ canvas_width: 1188, canvas_height: 779, device_pixel_ratio: 1, font: "14px mono" }),
    setWindowHidden: async (hidden) => { state.hidden = hidden; },
    terminalHidden: () => state.hidden,
    editorOpen: () => state.editor,
    ...overrides,
  };
  return { page, state };
}

function parentLog(probe: ReturnType<typeof fakeProbe>, page: BenchPage) {
  const calls: { step: string; detail: Record<string, unknown>; hidden: boolean; snapshots: number }[] = [];
  const awaitParent = vi.fn(async (step: string, detail: Record<string, unknown>) => {
    calls.push({ step, detail, hidden: page.terminalHidden(), snapshots: probe.snapshot.mock.calls.length });
    return { ok: true, step };
  });
  return { calls, awaitParent };
}

describe("runResourceBench", () => {
  it("measures visible then hidden in the same window and brackets each parent step with probe snapshots", async () => {
    const probe = fakeProbe();
    const { page } = fakePage();
    const { calls, awaitParent } = parentLog(probe, page);
    const events: string[] = [];
    const setHidden = page.setWindowHidden;
    page.setWindowHidden = async (hidden) => { events.push(`hide:${hidden}`); await setHidden(hidden); };
    const logged = vi.fn(async (name: string, detail: Record<string, unknown>) => { events.push(name); return awaitParent(name, detail); });
    const report = await runResourceBench({ page, probe, awaitParent: logged, sleep: async (ms) => { events.push(`sleep:${ms}`); } }, { settle_ms: 7 });
    // The canvas geometry is settled before bench-ready; interest suspension/reactivation needs a
    // settle after each layer switch before measuring.
    expect(events).toEqual(["sleep:7", "bench-ready", "sleep:7", "bench-visible", "hide:true", "sleep:7", "bench-hidden", "hide:false", "sleep:7", "bench-stop"]);

    expect(calls.map((c) => c.step)).toEqual(["bench-ready", "bench-visible", "bench-hidden", "bench-stop"]);
    const call = (i: number) => calls[i] ?? expect.unreachable(`parent step ${i} missing`);
    expect(call(0).detail).toMatchObject({ pane_id: "w3:p2", canvases: 1, geometry: { canvas_width: 1188 } });
    expect(call(1).hidden).toBe(false);
    expect(call(2).hidden).toBe(true);
    // before snapshot immediately precedes the step, after snapshot immediately follows it
    const visible = report.visible as { before: { raf_requests: number }; after: { raf_requests: number } };
    const hidden = report.hidden as { before: { raf_requests: number }; after: { raf_requests: number }; terminal_hidden: boolean; editor_open: boolean };
    expect(visible.before.raf_requests).toBe(call(1).snapshots);
    expect(visible.after.raf_requests).toBe(call(1).snapshots + 1);
    expect(hidden.before.raf_requests).toBe(call(2).snapshots);
    expect(hidden.after.raf_requests).toBe(call(2).snapshots + 1);
    expect(hidden.terminal_hidden).toBe(true);
    expect(hidden.editor_open).toBe(false);
    const reactivated = report.reactivated as { before: { raf_requests: number }; after: { raf_requests: number } };
    expect(reactivated.after.raf_requests).toBeGreaterThan(reactivated.before.raf_requests);
    expect(report.parent).toMatchObject({ "bench-visible": { step: "bench-visible" }, "bench-hidden": { step: "bench-hidden" } });
    expect(page.terminalHidden()).toBe(false);
  });

  // Spec 009, TASK-009-01: the composed layout resizes the terminal canvas once the engine's
  // confirmed pane arrives, and the parent reads `pane.layout` at bench-ready. Reporting the
  // first canvas size made the parent compare a transient 972x608 canvas against a 108x30 engine
  // grid and reject a complete run ("canvas 972x608 px is not a whole cell grid of 108x30",
  // evidencias/009/base1-control.runner/stdout.log).
  it("reports the settled canvas geometry at bench-ready, not the size of the first paint", async () => {
    const probe = fakeProbe();
    const sizes = [
      { canvas_width: 972, canvas_height: 608, device_pixel_ratio: 1, font: "14px mono" },
      { canvas_width: 972, canvas_height: 570, device_pixel_ratio: 1, font: "14px mono" },
      { canvas_width: 972, canvas_height: 570, device_pixel_ratio: 1, font: "14px mono" },
    ];
    let read = 0;
    const { page } = fakePage({ geometry: () => sizes[Math.min(read++, sizes.length - 1)]! });
    const { calls, awaitParent } = parentLog(probe, page);
    const report = await runResourceBench(
      { page, probe, awaitParent, sleep: async () => {} },
      { settle_ms: 7, windows: ["idle"] },
    );
    const call = (i: number) => calls[i] ?? expect.unreachable(`parent step ${i} missing`);
    expect(call(0).step).toBe("bench-ready");
    expect(call(0).detail).toMatchObject({ geometry: { canvas_height: 570 } });
    expect(report.geometry).toMatchObject({ canvas_height: 570 });
    expect(report.geometry_after).toMatchObject({ canvas_height: 570 });
  });

  it("fails loudly when the canvas geometry never settles instead of reporting a transient size", async () => {
    const probe = fakeProbe();
    let n = 0;
    const { page } = fakePage({
      geometry: () => ({ canvas_width: 972, canvas_height: 600 + n++, device_pixel_ratio: 1, font: "14px mono" }),
    });
    const { calls, awaitParent } = parentLog(probe, page);
    await expect(
      runResourceBench({ page, probe, awaitParent, sleep: async () => {} }, { settle_ms: 7, windows: ["idle"] }),
    ).rejects.toThrow(/geometry did not settle/);
    expect(calls).toEqual([]);
  });

  it("refuses to run without the opt-in probe (no fabricated zero counters)", async () => {
    const { page } = fakePage();
    const awaitParent = vi.fn(async () => ({}));
    await expect(runResourceBench({ page, probe: null, awaitParent, sleep: async () => {} }, {})).rejects.toThrow(/probe/);
    expect(awaitParent).not.toHaveBeenCalled();
  });

  it("never asks for the hidden measurement when the window did not become hidden, and still stops the generator", async () => {
    const probe = fakeProbe();
    const { page } = fakePage({ setWindowHidden: async () => {}, terminalHidden: () => false });
    const { calls, awaitParent } = parentLog(probe, page);
    await expect(runResourceBench({ page, probe, awaitParent, sleep: async () => {} }, {})).rejects.toThrow(/hidden/);
    expect(calls.map((c) => c.step)).toEqual(["bench-ready", "bench-visible", "bench-stop"]);
  });

  it("rejects a hidden window with an editor open (no file may be opened while hidden)", async () => {
    const probe = fakeProbe();
    const { page, state } = fakePage();
    page.setWindowHidden = async (hidden) => { state.hidden = hidden; state.editor = hidden; };
    const { calls, awaitParent } = parentLog(probe, page);
    await expect(runResourceBench({ page, probe, awaitParent, sleep: async () => {} }, {})).rejects.toThrow(/editor/);
    expect(calls.map((c) => c.step)).not.toContain("bench-hidden");
  });
  it("idle window: only settle → ready → settle → idle → stop, window visible, never hidden", async () => {
    const probe = fakeProbe();
    const { page } = fakePage();
    const events: string[] = [];
    page.setWindowHidden = async (hidden) => { events.push(`hide:${hidden}`); };
    const { calls, awaitParent } = parentLog(probe, page);
    const logged = vi.fn(async (name: string, detail: Record<string, unknown>) => { events.push(name); return awaitParent(name, detail); });
    const report = await runResourceBench({ page, probe, awaitParent: logged, sleep: async (ms) => { events.push(`sleep:${ms}`); } }, { settle_ms: 7, windows: ["idle"] });
    expect(events).toEqual(["sleep:7", "bench-ready", "sleep:7", "bench-idle", "bench-stop"]);
    const call = (i: number) => calls[i] ?? expect.unreachable(`parent step ${i} missing`);
    expect(call(1).hidden).toBe(false);
    const idle = report.idle as { before: { raf_requests: number }; after: { raf_requests: number }; terminal_hidden: boolean };
    expect(idle.before.raf_requests).toBe(call(1).snapshots);
    expect(idle.after.raf_requests).toBe(call(1).snapshots + 1);
    expect(idle.terminal_hidden).toBe(false);
    expect(report).not.toHaveProperty("hidden");
    expect(report).not.toHaveProperty("visible");
  });

  it("refuses an unknown or mixed window selection before asking the parent anything", async () => {
    for (const windows of [["idle", "visible"], ["hidden"], ["bogus"], "idle", []]) {
      const probe = fakeProbe();
      const { page } = fakePage();
      const awaitParent = vi.fn(async () => ({}));
      await expect(runResourceBench({ page, probe, awaitParent, sleep: async () => {} }, { windows })).rejects.toThrow(/windows/);
      expect(awaitParent).not.toHaveBeenCalled();
    }
  });

  it("never measures idle with the terminal hidden, and still stops", async () => {
    const probe = fakeProbe();
    const { page } = fakePage({ terminalHidden: () => true });
    const { calls, awaitParent } = parentLog(probe, page);
    await expect(runResourceBench({ page, probe, awaitParent, sleep: async () => {} }, { windows: ["idle"] })).rejects.toThrow(/visible/);
    expect(calls.map((c) => c.step)).toEqual(["bench-ready", "bench-stop"]);
  });

  it("branches to latency page after mount and sends latency steps with identity", async () => {
    let keyHandler: ((e: any) => void) | null = null;
    const { page } = fakePage({
      latencyMetrics: () => ({
        devicePixelRatio: 1.25,
        canvasRect: { left: 12, top: 40, width: 630, height: 493 },
        cellWidth: 8.4,
        cellHeight: 17,
        cols: 75,
        rows: 29,
      }),
      listenKeydown: (handler: (e: any) => void) => {
        keyHandler = handler;
        return () => { keyHandler = null; };
      },
    });
    const calls: { step: string; detail: Record<string, unknown> }[] = [];
    const awaitParent = vi.fn(async (step: string, detail: Record<string, unknown>) => {
      calls.push({ step, detail });
      if (step.startsWith("latency-attempt-")) {
        // Simulate native wtype key event hitting the terminal element while awaiting parent
        keyHandler?.({ type: "keydown", key: "a", isTrusted: true, repeat: false, timeStamp: 10.0 });
      }
      return { ok: true, step };
    });
    const report = await runResourceBench(
      { page, probe: null, awaitParent, sleep: async () => {} },
      { latency: true, transitions: 3 }
    );
    expect(calls.map((c) => c.step)).toEqual([
      "latency-ready",
      "latency-attempt-000",
      "latency-attempt-001",
      "latency-attempt-002",
      "latency-stop",
    ]);
    // Identity sent in every attempt
    for (const c of calls) {
      expect(c.detail).toMatchObject({
        pane_id: "w3:p2",
        generation: "7",
        boot_prefix: "b00t",
      });
    }
    // Ready payload contains page metrics and client innerWidth/innerHeight
    expect(calls[0]?.detail).toMatchObject({
      page: { cols: 75, rows: 29, cell_w_css: 8.4, cell_h_css: 17 },
    });
    // Attempts recorded in report
    const attempts = report.attempts as any[];
    expect(attempts).toHaveLength(3);
    for (let i = 0; i < 3; i++) {
      expect(attempts[i]).toMatchObject({
        attempt: i,
        trusted_keydowns: 1,
        untrusted_keydowns: 0,
        errors: [],
      });
    }
  });

  it("refuses latency run if page metrics cannot contain marker cells", async () => {
    const { page } = fakePage({
      latencyMetrics: () => ({
        devicePixelRatio: 1.0,
        canvasRect: { left: 12, top: 40, width: 100, height: 100 },
        cellWidth: 8.4,
        cellHeight: 17,
        cols: 20, // too few cols
        rows: 10,
      }),
    });
    const awaitParent = vi.fn(async () => ({}));
    await expect(
      runResourceBench({ page, probe: null, awaitParent, sleep: async () => {} }, { latency: true })
    ).rejects.toThrow(/cols|canvas/);
    expect(awaitParent).not.toHaveBeenCalled();
  });

  it("focuses the terminal target before every latency attempt, not after the parent step", async () => {
    // Would catch: wtype firing while textarea.ime-target is unfocused, so the trusted key never lands.
    const events: string[] = [];
    let keyHandler: ((e: any) => void) | null = null;
    const { page } = fakePage({
      latencyMetrics: () => ({
        devicePixelRatio: 1,
        canvasRect: { left: 12, top: 40, width: 630, height: 493 },
        cellWidth: 9,
        cellHeight: 19,
        cols: 75,
        rows: 29,
      }),
      listenKeydown: (handler: (e: any) => void) => {
        keyHandler = handler;
        return () => { keyHandler = null; };
      },
      focusTerminal: () => { events.push("focus"); },
    });
    const awaitParent = vi.fn(async (step: string) => {
      events.push(step);
      if (step.startsWith("latency-attempt-")) {
        keyHandler?.({ type: "keydown", key: "a", isTrusted: true, repeat: false, timeStamp: 1 });
      }
      return { ok: true, step };
    });
    await runResourceBench(
      { page, probe: null, awaitParent, sleep: async () => {} },
      { latency: true, transitions: 2 },
    );
    expect(events).toEqual([
      "focus",
      "latency-ready",
      "focus",
      "latency-attempt-000",
      "focus",
      "latency-attempt-001",
      "latency-stop",
    ]);
  });

  it("records trusted keydowns on window in capture phase so a focused ime-target still reaches the observer", () => {
    // Would catch: listening on .terminal/bubble so a key handled on textarea.ime-target is missed.
    const attached: { type: string; capture: boolean }[] = [];
    const prevWindow = (globalThis as { window?: unknown }).window;
    const fakeWindow = {
      addEventListener(type: string, _handler: unknown, options?: boolean | { capture?: boolean }) {
        attached.push({ type, capture: typeof options === "boolean" ? options : !!options?.capture });
      },
      removeEventListener() {},
    };
    (globalThis as { window: unknown }).window = fakeWindow;
    try {
      const root = {
        querySelector: (sel: string) => (sel === "textarea.ime-target" ? { focus() {} } : null),
      } as unknown as HTMLElement;
      const stop = domPage(root).listenKeydown?.(() => {});
      expect(attached).toEqual([{ type: "keydown", capture: true }]);
      stop?.();
    } finally {
      (globalThis as { window?: unknown }).window = prevWindow;
    }
    const source = readFileSync(new URL("./resource-bench.ts", import.meta.url), "utf8");
    expect(source).toMatch(/window\.addEventListener\(\s*"keydown"/);
    expect(source).toMatch(/capture:\s*true/);
    expect(source).not.toMatch(/\.terminal"\)[\s\S]{0,80}addEventListener\(\s*"keydown"/);
  });

  // Spec 009, TASK-009-01: the parent rejects a run whose window mounted more than one terminal
  // view. TerminalView has painted two canvas layers since the window-padding extend (spec 028
  // r4c) — the grid canvas plus `canvas.extend` — so counting every canvas rejected a correct
  // single-view window ("gui_1: 2 terminal canvas(es), expected exactly 1",
  // evidencias/009/base2-control/run-report.json). Only grid canvases are counted, and a second
  // mounted view is still two.
  it("counts one terminal grid canvas per mounted view, not the extension layer", async () => {
    const { Window } = await import("happy-dom");
    const dom = new Window();
    const view = '<div class="terminal"><canvas></canvas><canvas class="extend"></canvas></div>';
    const root = dom.document.createElement("div") as unknown as HTMLElement;
    const count = (html: string) => {
      root.innerHTML = html;
      return domPage(root).canvases();
    };
    expect(count(view)).toBe(1);
    expect(count(view + view)).toBe(2);
    expect(count("")).toBe(0);
    await dom.close();
  });

  // Spec 009, round 3 — a hidden terminal is a window that is not visible. That is the product's
  // own remaining path to `setInterest(false)`: `src/shell/interest.ts` drops the surface lease on
  // `document.visibilityState === "hidden"`, minimize or loss of native visibility, and the engine
  // then sends no frame at all. The page reads that state, it never asserts it.
  describe("domPage hiding the window", () => {
    async function mounted(options: { hide?: (hidden: boolean) => Promise<void> } = {}) {
      const { Window } = await import("happy-dom");
      const dom = new Window();
      let visibility = "visible";
      Object.defineProperty(dom.document, "visibilityState", {
        get: () => visibility,
        configurable: true,
      });
      const globals = globalThis as { document?: unknown; window?: unknown };
      const saved = { document: globals.document, window: globals.window };
      globals.document = dom.document;
      globals.window = dom;
      const root = dom.document.createElement("div") as unknown as HTMLElement;
      dom.document.body.appendChild(root as never);
      root.innerHTML = '<div class="terminal"><canvas></canvas></div>';
      const calls: boolean[] = [];
      const hide =
        options.hide ??
        (async (hidden: boolean) => {
          calls.push(hidden);
          visibility = hidden ? "hidden" : "visible";
        });
      const page = domPage(root, { setWindowVisible: async (visible) => hide(!visible), visibilityTimeoutMs: 200 });
      const close = async () => {
        globals.document = saved.document;
        globals.window = saved.window;
        await dom.close();
      };
      return { dom, root, page, calls, close, setVisibility: (v: string) => (visibility = v) };
    }

    it("reads the terminal as hidden exactly when the window is not visible", async () => {
      const { page, close, setVisibility } = await mounted();
      expect(page.terminalHidden()).toBe(false);
      setVisibility("hidden");
      expect(page.terminalHidden()).toBe(true);
      setVisibility("visible");
      expect(page.terminalHidden()).toBe(false);
      await close();
    });

    it("hides the window through the harness and brings it back", async () => {
      const { page, calls, close } = await mounted();
      await page.setWindowHidden(true);
      expect(calls).toEqual([true]);
      expect(page.terminalHidden()).toBe(true);
      await page.setWindowHidden(false);
      expect(calls).toEqual([true, false]);
      expect(page.terminalHidden()).toBe(false);
      await close();
    });

    // Would catch: a helper that asks for the hide and reports success without the window ever
    // leaving the screen, so the "hidden" window would really be a visible one.
    it("refuses to report a hidden window when the window did not become hidden", async () => {
      const { page, close } = await mounted({ hide: async () => {} });
      await expect(page.setWindowHidden(true)).rejects.toThrow(/window hidden/i);
      expect(page.terminalHidden()).toBe(false);
      await close();
    });

    it("refuses to continue when the window does not come back visible", async () => {
      let visible = true;
      const { page, close, setVisibility } = await mounted({
        hide: async (hidden: boolean) => {
          if (hidden) {
            visible = false;
            setVisibility("hidden");
          }
        },
      });
      await page.setWindowHidden(true);
      expect(visible).toBe(false);
      await expect(page.setWindowHidden(false)).rejects.toThrow(/window visible/i);
      await close();
    });
  });

  it("waits for bench-stop (and long idle/visible/hidden windows) with timeout derived from parent duration plus ≥120s", async () => {
    // Would catch: harness_await default 110s aborting bench-stop at ~220s while memory collector still runs 1800s.
    const timeoutOf = (awaitParent: ReturnType<typeof vi.fn>, step: string) => {
      const call = awaitParent.mock.calls.find((c) => c[0] === step);
      if (!call) throw new Error(`${step} was not awaited`);
      return call[2] as number | undefined;
    };

    const memoryParent = vi.fn(async () => ({ ok: true }));
    await runResourceBench(
      { page: fakePage().page, probe: fakeProbe(), awaitParent: memoryParent, sleep: async () => {} },
      { windows: ["idle"], warmup_s: 0, measured_s: 1800, settle_ms: 0 },
    );
    expect(timeoutOf(memoryParent, "bench-stop")).toBeGreaterThanOrEqual(1_920_000);
    expect(timeoutOf(memoryParent, "bench-idle")).toBeGreaterThanOrEqual(1_920_000);

    const acceptParent = vi.fn(async () => ({ ok: true }));
    await runResourceBench(
      { page: fakePage().page, probe: fakeProbe(), awaitParent: acceptParent, sleep: async () => {} },
      { warmup_s: 5, measured_s: 60, settle_ms: 0 },
    );
    expect(timeoutOf(acceptParent, "bench-stop")).toBeGreaterThanOrEqual(110_000);
    expect(timeoutOf(acceptParent, "bench-visible")).toBeGreaterThanOrEqual(110_000);
    expect(timeoutOf(acceptParent, "bench-hidden")).toBeGreaterThanOrEqual(110_000);
  });
});
