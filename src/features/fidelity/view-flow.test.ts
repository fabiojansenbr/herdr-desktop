// Page seams of the resize-dpi and a11y-navigation phases (no DOM, GUI, compositor, keys or
// engine): literals equal the Rust parent, the page reports the ACTUAL observations of fake
// page/probe dependencies (never the expected values), waits for a real accepted Full frame and
// paint after each resize command, and keeps untrusted keys and exit-chord focus visible.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { TerminalGeometry, TerminalProbeCounters } from "../../terminal/probe";
import {
  A11Y_STEPS,
  accessibleName,
  carrierMarks,
  EXIT_CHORD,
  mainSweepPlan,
  RESIZE_STEPS,
  retainsTab,
  orderAfterMarker,
  splitAfterMarker,
  runA11yNavigation,
  runResizeDpi,
  sampleActiveElement,
  splitSweep,
  interleaveSweep,
  stateRecord,
  tabOrder,
  traceChords,
  traceFocusAfterKey,
  VIEW_STAGES,
  type A11yPage,
  type Control,
  type MarkerSplit,
  type Style,
  type ChordEntry,
  type FocusTraceEntry,
  type NameNode,
  type ResizePage,
  type StageView,
  type SweepEvent,
} from "./view-flow";

const rust = readFileSync(new URL("../../../tests/fidelity-native/view_flow.rs", import.meta.url), "utf8");
const list = (name: string) => Array.from((new RegExp(`pub const ${name}: \\[&str; \\d+\\] = \\[([\\s\\S]*?)\\];`).exec(rust)?.[1] ?? "").matchAll(/"([a-z0-9-]+)"/g), (m) => m[1]);

const LOCAL = { pane_id: "w1-p1", generation: "g7", boot_prefix: "b0a1", endpoint: "local · Local" };

/** Manual clock: sleep advances time, so waits are deterministic. */
function clock() {
  let t = 0;
  const sleeps: number[] = [];
  return { now: () => t, sleep: async (ms: number) => { sleeps.push(ms); t += ms; }, sleeps };
}

describe("view-flow literals", () => {
  it("stages, steps and exit chord equal view_flow.rs", () => {
    const stages = Array.from(rust.matchAll(/stage\("([a-z0-9]+)", (\d+), (\d+), (\d+)\)/g), (m) => ({ name: m[1], width: Number(m[2]), height: Number(m[3]), scale: Number(m[4]) }));
    expect(VIEW_STAGES).toEqual(stages);
    expect([...RESIZE_STEPS]).toEqual(list("RESIZE_STEPS"));
    expect([...A11Y_STEPS]).toEqual(list("A11Y_STEPS"));
    expect(rust).toContain(`"${EXIT_CHORD}" =>`);
  });
});

// ---------------------------------------------------------------- resize-dpi

function view(inner: number, dpr: number): StageView {
  return {
    inner_width: inner, inner_height: 683, dpr,
    terminal_rect: { width: inner - 200, height: 600 }, metrics: { cellWidth: 9, cellHeight: 18 },
    canvas: { width: 1000 * dpr, height: 594 * dpr, css_width: "1000px", css_height: "594px", transform: [dpr, 0, 0, dpr, 0, 0], rect: { left: 200, top: 30, width: 1000, height: 594 } },
  };
}

/** Engine+compositor fake: an apply command produces `fulls` Full frames painted after `lag` ms. */
function resizeWorld(opts: { fulls?: number; lag?: number; metaLag?: number; identityAt?: (step: string) => typeof LOCAL | null } = {}) {
  const c = clock();
  const counters: TerminalProbeCounters = { raf_requests: 0, raf_callbacks: 0, paint_calls: 0, painted_rows: 0, full_frames: 0 };
  let current = view(1280, 1);
  let pending: { at: number; view: StageView } | null = null;
  let step = "";
  // Frame metadata: surface = canvas cells, pane inner_rect one column narrower (engine gutter).
  const geometryOf = (v: StageView): TerminalGeometry => {
    // Distinct per stage (the fake canvas keeps one size), so a stale stage geometry is visible.
    const cols = Math.floor(v.inner_width / 10);
    const rows = 33;
    return { surface: { cols, rows }, panes: [{ pane_id: "w1-p1", inner_rect: { x: 0, y: 0, width: cols - 1, height: rows } }], full_frames: counters.full_frames };
  };
  let geometry = geometryOf(current);
  let metaAt: { at: number; geometry: TerminalGeometry } | null = null;
  const tick = () => {
    if (pending && c.now() >= pending.at) {
      current = pending.view;
      counters.full_frames += opts.fulls ?? 1;
      counters.painted_rows += 33;
      metaAt = { at: c.now() + (opts.metaLag ?? 0), geometry: geometryOf(pending.view) };
      pending = null;
    }
    if (metaAt && c.now() >= metaAt.at) [geometry, metaAt] = [metaAt.geometry, null];
  };
  const page: ResizePage = {
    identity: () => (opts.identityAt ? opts.identityAt(step) : LOCAL),
    view: () => (tick(), current),
  };
  const probe = { snapshot: () => (tick(), { ...counters }), geometry: () => (tick(), geometry) };
  const calls: { step: string; detail: Record<string, unknown> }[] = [];
  const next: Record<string, StageView> = { grow: view(1600, 1), scale2: view(800, 2), restore: view(1280, 1) };
  const awaitParent = async (s: string, detail: Record<string, unknown>) => {
    step = s;
    calls.push({ step: s, detail });
    const m = /^resize-apply-(\w+)$/.exec(s);
    if (m && (opts.fulls ?? 1) > 0) pending = { at: c.now() + (opts.lag ?? 120), view: next[m[1]!]! };
    return { step: s, answered: true };
  };
  return { deps: { awaitParent, sleep: c.sleep, now: c.now }, page, probe, calls, clock: c };
}

describe("runResizeDpi", () => {
  it("refuses without the opt-in probe or off Local before any parent step", async () => {
    const w = resizeWorld();
    await expect(runResizeDpi(w.page, null, w.deps)).rejects.toThrow(/probe/);
    const ssh = resizeWorld({ identityAt: () => ({ ...LOCAL, endpoint: "dev · SSH" }) });
    await expect(runResizeDpi(ssh.page, ssh.probe, ssh.deps)).rejects.toThrow(/Local/);
    expect([...w.calls, ...ssh.calls]).toEqual([]);
  });

  // Would catch: observing right after the command (stale canvas), sleeping a fixed time instead
  // of waiting for the accepted Full frame + paint, or reporting the commanded instead of seen view.
  it("observes each stage only after an accepted Full frame and paint, reporting the actual view", async () => {
    const w = resizeWorld({ lag: 700 });
    const report = await runResizeDpi(w.page, w.probe, w.deps);
    expect(w.calls.map((c) => c.step)).toEqual([...RESIZE_STEPS]);
    const observeGrow = w.calls.find((c) => c.step === "resize-observe-grow")!;
    expect(observeGrow.detail.stage).toBe("grow");
    expect((observeGrow.detail.view as StageView).inner_width).toBe(1600);
    const stages = report.stages as Record<string, unknown>[];
    expect(stages.map((s) => s.stage)).toEqual(["start", "grow", "scale2", "restore"]);
    expect(stages.map((s) => s.dpr)).toEqual([1, 1, 2, 1]);
    expect(stages.map((s) => (s.probe as TerminalProbeCounters).full_frames)).toEqual([0, 1, 2, 3]);
    expect(stages.map((s) => (s.probe as TerminalProbeCounters).painted_rows)).toEqual([0, 33, 66, 99]);
    expect(stages[2]!.canvas).toEqual(view(800, 2).canvas);
    expect(stages[1]!.identity).toEqual(LOCAL);
    expect(report).not.toHaveProperty("error");
    expect(JSON.stringify(report)).not.toMatch(/"(ok|passed|expected_[a-z]+)"/);
  });

  // Would catch (GUI r4): a stage without the pane inner_rect the frame carried (the parent then
  // compares stty with the canvas surface), or a stage observed before the metadata of its Full.
  it("records the frame surface and pane inner_rect of each stage, after the metadata settles", async () => {
    // Metadata lands 600 ms after the Full frame, longer than the 400 ms quiet period.
    const w = resizeWorld({ lag: 120, metaLag: 600 });
    const report = await runResizeDpi(w.page, w.probe, w.deps);
    const stages = report.stages as (StageView & TerminalGeometry)[];
    expect(stages.map((s) => s.surface?.cols)).toEqual([128, 160, 80, 128]);
    expect(stages.map((s) => s.panes[0]!.inner_rect.width)).toEqual([127, 159, 79, 127]);
    const observeGrow = w.calls.find((c) => c.step === "resize-observe-grow")!;
    expect((observeGrow.detail.view as TerminalGeometry).panes).toEqual([{ pane_id: "w1-p1", inner_rect: { x: 0, y: 0, width: 159, height: 33 } }]);
    expect((observeGrow.detail.view as TerminalGeometry).surface).toEqual({ cols: 160, rows: 33 });
  });

  it("a stage never observed before the frame metadata arrived (surface null) is a phase error", async () => {
    const w = resizeWorld();
    const nothing = { surface: null, panes: [], full_frames: 0 };
    await expect(runResizeDpi(w.page, { snapshot: w.probe.snapshot, geometry: () => nothing }, w.deps)).rejects.toThrow(/start.*metadata/);
    expect(w.calls).toEqual([]);
  });

  it("a command without any Full frame is a phase error, never an observed stage", async () => {
    const w = resizeWorld({ fulls: 0 });
    await expect(runResizeDpi(w.page, w.probe, w.deps)).rejects.toThrow(/grow.*Full frame/);
    expect(w.calls.map((c) => c.step)).toEqual(["resize-observe-start", "resize-apply-grow"]);
  });

  it("identity is read at every stage (a switched pane is reported, not copied from the start)", async () => {
    const other = { ...LOCAL, pane_id: "w1-p9" };
    const w = resizeWorld({ identityAt: (s) => (s.startsWith("resize-apply-scale2") ? other : LOCAL) });
    const report = await runResizeDpi(w.page, w.probe, w.deps);
    const ids = (report.stages as { identity: typeof LOCAL }[]).map((s) => s.identity.pane_id);
    expect(ids).toEqual(["w1-p1", "w1-p1", "w1-p9", "w1-p1"]);
  });
});

// ---------------------------------------------------------------- a11y-navigation

const ctl = (id: string, extra: Partial<Control> = {}): Control => ({
  id, tag: "button", role: "button", name: `Controle ${id}`, name_source: "text", disabled: false, tabindex: 0, focus_visible: false,
  style: { outline_style: "none", outline_width: "0px", outline_color: "rgb(0, 0, 0)", box_shadow: "none", border_color: "rgb(0, 0, 0)", background_color: "rgb(0, 0, 0)" }, baseline: null, ...extra,
});
const key = (k: string, shift = false, ctrl = false, trusted = true): SweepEvent => ({ kind: "key", key: k, shift, ctrl, trusted });
const focus = (c: Control): SweepEvent => ({ kind: "focus", control: c });

// Fixture narrowed (Planner precision 2026-09-17): a correct modal focuses its first field on open,
// so the dialog is modelled with that initial focus and a cyclic Tab stream (three controls so wrap
// and backward order are distinct); the previous fake left focus on the opener outside the dialog.
function a11yWorld(opts: { dialogOpens?: boolean; untrustedTab?: boolean; identity?: typeof LOCAL; escapeCloses?: boolean; main?: MarkerSplit<Control> } = {}) {
  const c = clock();
  // Real layout (r8+): the exit marker follows the terminal, so the cycle after it ends on the terminal.
  // r12: the stops are split as orderAfterMarker (following the marker, then preceding it).
  const stops = opts.main ?? { following: [ctl("a"), ctl("b")], preceding: [ctl("opener", { name: "Nova conexão SSH" }), ctl("term", { tag: "textarea", role: "", classes: ["ime-target"] })] };
  const main = [...stops.following, ...stops.preceding];
  const dialog = [ctl("d", { tag: "input" }), ctl("e"), ctl("f")];
  const modal = { tag: "dialog", role: "dialog", name: "Conectar a um servidor herdr", name_source: "aria-labelledby", modal: true };
  let events: SweepEvent[] = [];
  let open = false;
  let active: Control | null = ctl("term", { tag: "textarea", role: "" });
  const log: string[] = [];
  const traced = { installedAfterSteps: -1, stoppedAfterSteps: -1 };
  const chords = { installedBeforeFocus: false, stoppedAfterSteps: -1 };
  const chordEntry = { t: 7, active: null, target: null, is_composing: true, default_prevented_capture: false, default_prevented_bubble: null, default_prevented_after: false, active_after: null, terminal_focus_in_t: 3, compositions_since_focus: [{ type: "compositionstart", data: "", t: 4, target: null }] } as ChordEntry;
  const opener = { tag: "button", id: "", classes: [], focus_visible: false, focus: false, document_has_focus: false };
  const trace: FocusTraceEntry[] = [
    { at: "keydown", t: 1, active: { ...opener, tag: "input" } },
    { at: "window-blur", t: 2, active: opener },
    { at: "microtask", t: 2, active: opener },
  ];
  const page: A11yPage = {
    identity: () => opts.identity ?? LOCAL,
    focusTerminal: () => (log.push("focusTerminal"), true),
    mainStops: () => stops,
    expected: () => (open ? dialog : null),
    record: () => ({ take: () => events.splice(0) }),
    // Resolves only after a later macrotask (like HostsPanel mounting on the next Svelte tick).
    openerSetup: () => {
      log.push("openerSetup");
      return new Promise((resolve) => setTimeout(() => (log.push("openerSetup resolved"), (active = main[2]!), resolve(["activity Conexões remotas: click (aria-pressed was false)", "focus button Nova conexão SSH"])), 0));
    },
    active: () => active,
    dialogOpen: () => open,
    modal: () => (log.push("modal"), open ? modal : null),
    cancelDialog: () => {
      log.push("cancel");
      // Cleanup fallback only: closes asynchronously (like the Svelte update after the state change).
      void Promise.resolve().then(() => ((open = false), (active = main[2]!)));
    },
    states: () => [{ status: "working", own_text: "", carrier_text: "" }],
    // Recorded as the number of parent steps asked so far (kept out of `log`, whose order other tests pin).
    traceChords: () => {
      chords.installedBeforeFocus = !log.includes("focusTerminal");
      return { take: () => [chordEntry], stop: () => void (chords.stoppedAfterSteps = calls.length) };
    },
    traceClose: () => {
      traced.installedAfterSteps = calls.length;
      return { take: () => [...trace], stop: () => void (traced.stoppedAfterSteps = calls.length) };
    },
  };
  const calls: { step: string; detail: Record<string, unknown> }[] = [];
  const awaitParent = async (step: string, detail: Record<string, unknown>) => {
    calls.push({ step, detail });
    log.push(step);
    const keys = detail.keys as string[];
    for (const k of keys) {
      if (k === EXIT_CHORD) events.push(key("Control", false, true), key("Shift", true, true), key("F6", true, true), focus(ctl("marker", { tag: "span", tabindex: -1 })));
      if (k === "Tab") events.push(key("Tab", false, false, !opts.untrustedTab));
      if (k === "shift+Tab") events.push(key("Shift", true), key("Tab", true));
      // The product closes on Escape asynchronously and restores focus to the opener.
      if (k === "Escape") {
        events.push(key("Escape"));
        if (opts.escapeCloses !== false) void Promise.resolve().then(() => ((open = false), (active = main[2]!)));
      }
      if (k === "Return") {
        events.push(key("Enter"));
        if (opts.dialogOpens !== false) open = true;
      }
    }
    if (step === "a11y-sweep-main") {
      // r10 plan: every stop once, the terminal left by the chord, Shift+Tab back into the terminal.
      for (const x of [...main, main[main.length - 1]!]) events.push(focus({ ...x, focus_visible: x.id !== "b" }));
      active = main[main.length - 1]!;
    } else if (step === "a11y-sweep-dialog") {
      for (const x of [dialog[1]!, dialog[2]!, dialog[0]!, dialog[2]!]) events.push(focus({ ...x, focus_visible: true }));
      active = dialog[2]!;
    } else if (step === "a11y-open-dialog" && open) active = dialog[0]!;
    return { step, keys };
  };
  return { page, deps: { awaitParent, sleep: c.sleep, now: c.now }, calls, log, traced, chords, chordEntry };
}

describe("runA11yNavigation", () => {
  it("refuses off Local before focusing or asking the parent", async () => {
    const w = a11yWorld({ identity: { ...LOCAL, endpoint: "dev · SSH" } });
    await expect(runA11yNavigation(w.page, w.deps)).rejects.toThrow(/Local/);
    expect(w.log).toEqual([]);
  });

  // Would catch: a plan without the terminal exit chord (Tab consumed by the textarea), a Tab count
  // unrelated to the expected controls, the dialog opened by a click, or focus flags copied from
  // expectations instead of the recorded focus.
  it("asks native key plans, records actual focus/keys, and records the exact opener steps", async () => {
    const w = a11yWorld();
    const report = await runA11yNavigation(w.page, w.deps);
    expect(w.calls.map((c) => c.step)).toEqual([...A11Y_STEPS]);
    // r12: one extra Tab for the document wrap after the last following stop ("b", index 1).
    expect(w.calls[0]!.detail.keys).toEqual([EXIT_CHORD, "Tab", "Tab", "Tab", "Tab", "Tab", EXIT_CHORD, "shift+Tab"]);
    expect(w.calls[0]!.detail.wrap_after).toBe(1);
    expect(w.calls[0]!.detail.split).toEqual({ following: ["a", "b"], preceding: ["opener", "term"] });
    expect(w.calls[1]!.detail.keys).toEqual(["Return"]);
    expect(w.calls[2]!.detail.keys).toEqual(["Tab", "Tab", "Tab", "shift+Tab"]);
    expect(w.calls[3]!.detail.keys).toEqual(["Escape"]);
    expect(w.log.indexOf("openerSetup")).toBeLessThan(w.log.indexOf("a11y-open-dialog"));
    // Would catch: the parent Return requested before the async opener setup finished.
    expect(w.log.indexOf("openerSetup resolved")).toBeGreaterThan(-1);
    expect(w.log.indexOf("openerSetup resolved")).toBeLessThan(w.log.indexOf("a11y-open-dialog"));
    expect(w.log.slice(-2)).toEqual(["a11y-sweep-dialog", "a11y-close-dialog"]);
    const main = report.main as Record<string, unknown>;
    expect((main.focus as Control[]).map((f) => [f.id, f.focus_visible])).toEqual([["a", true], ["b", false], ["opener", true], ["term", true], ["term", true]]);
    expect((main.exit_focus as Control[]).map((f) => f.id)).toEqual(["marker", "marker"]);
    expect((main.keys_seen as unknown[]).length).toBe(13);
    expect(main.wrap_after).toBe(1);
    expect(main.split).toEqual({ following: ["a", "b"], preceding: ["opener", "term"] });
    // Observation r11: interleaved order of the same recorded events (non-modifier keys + focus).
    const order = main.order as { kind: string; index?: number; keys_before?: number; id?: string }[];
    expect(order.filter((o) => o.kind === "key").map((o) => o.index)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    expect(order.filter((o) => o.kind === "focus").map((o) => o.id)).toEqual(["marker", "marker", "a", "b", "opener", "term", "term"]);
    expect((report.dialog as Record<string, unknown>).order).toBeUndefined();
    expect(report.dialog_open_keys).toEqual([key("Enter")].map(({ kind: _k, ...k }) => k));
    expect(report.setup).toContain("activity Conexões remotas: click (aria-pressed was false)");
    // Would catch: start focus read before the Return or copied from the opener/expected list.
    expect((report.dialog_start_focus as Control).id).toBe("d");
    expect((report.dialog as { focus: Control[] }).focus.map((f) => f.id)).toEqual(["e", "f", "d", "f"]);
    expect(report.states).toEqual([{ status: "working", own_text: "", carrier_text: "" }]);
  });

  // Would catch: the modal read before the Return (null) or after cleanup, or a hard-coded record.
  it("records the actual modal element right after the native Return, before the dialog sweep", async () => {
    const w = a11yWorld();
    const report = await runA11yNavigation(w.page, w.deps);
    expect(report.dialog_modal).toEqual({ tag: "dialog", role: "dialog", name: "Conectar a um servidor herdr", name_source: "aria-labelledby", modal: true });
    expect(w.log.indexOf("modal")).toBeGreaterThan(w.log.indexOf("a11y-open-dialog"));
    expect(w.log.indexOf("modal")).toBeLessThan(w.log.indexOf("a11y-sweep-dialog"));
  });

  // Replaces the programmatic-Cancelar observation (approved a11y-close-dialog step, Root 2026-09-17).
  // Would catch: closing by a programmatic click, closure/restoration assumed instead of observed
  // after the native Escape, or the identity copied from the start.
  it("closes with one native Escape and records keys, closure, focus and identity after it", async () => {
    const w = a11yWorld();
    const report = await runA11yNavigation(w.page, w.deps);
    expect(w.calls[3]).toMatchObject({ step: "a11y-close-dialog", detail: { ...LOCAL, keys: ["Escape"] } });
    expect((w.calls[3]!.detail.opener as Control).id).toBe("opener");
    expect(w.log).not.toContain("cancel");
    expect(report.dialog_close).toEqual({ identity: LOCAL, keys_seen: [{ key: "Escape", shift: false, ctrl: false, trusted: true }], closed: true, focus_after: ctl("opener", { name: "Nova conexão SSH" }), focus_trace: expect.any(Array) });
    expect(report.setup).not.toContainEqual(expect.stringMatching(/Cancelar/));
    expect((report.parent as { close: unknown }).close).toEqual({ step: "a11y-close-dialog", keys: ["Escape"] });
  });

  // GUI r5: after Escape the opener was activeElement with document_has_focus false and :focus false.
  // Would catch a trace installed after the Escape (missing the keydown sample) or not reported.
  it("traces focus around the native Escape from before the close step and reports it", async () => {
    const w = a11yWorld();
    const report = await runA11yNavigation(w.page, w.deps);
    expect(w.calls.map((c) => c.step).indexOf("a11y-close-dialog")).toBe(3);
    expect(w.traced).toEqual({ installedAfterSteps: 3, stoppedAfterSteps: 4 });
    expect((report.dialog_close as { focus_trace: FocusTraceEntry[] }).focus_trace.map((e) => e.at)).toEqual(["keydown", "window-blur", "microtask"]);
  });

  // GUI r8: the 2nd exit chord did not leave the terminal; cause not observable. Would catch: the
  // chord trace installed after the setup focus (missing the terminal focus-in), left running into
  // the dialog steps, or not reported with the main sweep.
  it("traces exit chords from before the terminal focus through the main sweep and reports them", async () => {
    const w = a11yWorld();
    const report = await runA11yNavigation(w.page, w.deps);
    expect(w.chords.installedBeforeFocus).toBe(true);
    expect(w.chords.stoppedAfterSteps).toBe(1);
    expect((report.main as { chord_trace: ChordEntry[] }).chord_trace).toEqual([w.chordEntry]);
  });

  // Would catch: the cleanup click turning a failed Escape into a closed dialog in the report.
  it("Escape that does not close stays a failure; cleanup Cancelar only afterwards and labelled", async () => {
    const w = a11yWorld({ escapeCloses: false });
    const report = await runA11yNavigation(w.page, w.deps);
    const close = report.dialog_close as { closed: boolean; focus_after: Control | null };
    expect(close.closed).toBe(false);
    expect(close.focus_after?.id).toBe("f");
    expect(w.log.slice(-2)).toEqual(["a11y-close-dialog", "cancel"]);
    expect(report.setup).toContain("click Cancelar (cleanup after Escape did not close; not measured)");
  });

  // GUI r7: Tab pressed on the terminal textarea (kept by the PTY). GUI r9: the re-entry Tab through
  // the end of the document and the second chord after it never reached the terminal. Would catch:
  // the page still asking a plain Tab×N plan, the r8/r9 wrap Tab and re-entry stop, or reporting an
  // expected list different from the plan the parent pressed.
  // r12: would catch the wrap declaration missing from the parent request or the report, or a
  // declaration not derived from the page's own following/preceding split (the default world
  // splits after index 1, this one after index 0).
  it("main sweep with the terminal: parent gets the r10 exit-chord plan and the report its expected stops", async () => {
    const term = ctl("term", { tag: "textarea", role: "", classes: ["ime-target"] });
    const copy = ctl("copy", { name: "Copiar seleção" });
    const w = a11yWorld({ main: { following: [copy], preceding: [ctl("a"), ctl("opener", { name: "Nova conexão SSH" }), term] } });
    const report = await runA11yNavigation(w.page, w.deps);
    expect(w.calls[0]!.detail.keys).toEqual([EXIT_CHORD, "Tab", "Tab", "Tab", "Tab", "Tab", EXIT_CHORD, "shift+Tab"]);
    expect((w.calls[0]!.detail.expected as Control[]).map((c) => c.id)).toEqual(["copy", "a", "opener", "term", "term"]);
    expect(w.calls[0]!.detail.wrap_after).toBe(0);
    expect(w.calls[0]!.detail.split).toEqual({ following: ["copy"], preceding: ["a", "opener", "term"] });
    const main = report.main as { expected: Control[]; wrap_after: number; split: unknown };
    expect(main.expected.map((c) => c.id)).toEqual(["copy", "a", "opener", "term", "term"]);
    expect(main.wrap_after).toBe(0);
    expect(main.split).toEqual({ following: ["copy"], preceding: ["a", "opener", "term"] });
  });

  // r12: would catch a sweep sent without a wrap to declare (marker last in the document).
  it("main sweep with no control following the exit marker is a phase error before any parent step", async () => {
    const term = ctl("term", { tag: "textarea", role: "", classes: ["ime-target"] });
    const w = a11yWorld({ main: { following: [], preceding: [ctl("a"), ctl("opener", { name: "Nova conexão SSH" }), term] } });
    await expect(runA11yNavigation(w.page, w.deps)).rejects.toThrow(/a11y-sweep-main: no control follows the exit marker/);
    expect(w.calls).toEqual([]);
  });

  // Would catch: a sweep sent to the parent when the page has no terminal to leave with the chord.
  it("main sweep without the terminal as its last stop is a phase error before any parent step", async () => {
    const w = a11yWorld({ main: { following: [ctl("a")], preceding: [ctl("b"), ctl("opener", { name: "Nova conexão SSH" })] } });
    await expect(runA11yNavigation(w.page, w.deps)).rejects.toThrow(/a11y-sweep-main/);
    expect(w.calls).toEqual([]);
  });

  it("untrusted keys stay visible in the report", async () => {
    const w = a11yWorld({ untrustedTab: true });
    const report = await runA11yNavigation(w.page, w.deps);
    const seen = (report.main as { keys_seen: { key: string; trusted: boolean }[] }).keys_seen;
    expect(seen.filter((k) => k.key === "Tab" && !k.trusted)).toHaveLength(5);
  });

  it("a Return that opens no dialog is a phase error (no dialog sweep fabricated)", async () => {
    const w = a11yWorld({ dialogOpens: false });
    await expect(runA11yNavigation(w.page, w.deps)).rejects.toThrow(/dialog/);
    expect(w.calls.map((c) => c.step)).toEqual(["a11y-sweep-main", "a11y-open-dialog"]);
  });
});

describe("a11y pure seams", () => {
  it("splitSweep separates exit-chord focus from Tab focus and keeps raw keys", () => {
    const out = splitSweep([key("Control", false, true), key("F6", true, true), focus(ctl("marker")), key("Tab"), focus(ctl("a")), key("Tab", true), focus(ctl("b"))]);
    expect(out.exit_focus.map((c) => c.id)).toEqual(["marker"]);
    expect(out.focus.map((c) => c.id)).toEqual(["a", "b"]);
    expect(out.keys_seen).toEqual([{ key: "Control", shift: false, ctrl: true, trusted: true }, { key: "F6", shift: true, ctrl: true, trusted: true }, { key: "Tab", shift: false, ctrl: false, trusted: true }, { key: "Tab", shift: true, ctrl: false, trusted: true }]);
  });

  // GUI r8–r10: one Tab of the main sweep moves no focus, but keys_seen and focus are separate lists,
  // so which Tab is lost is not observable. Would catch: focus attributed to the wrong key (modifier
  // keydowns counted, index shifted), the key before a focus not recorded, the active element at the
  // keydown dropped, or a lost Tab hidden (two keys with no focus between them must stay visible).
  it("interleaveSweep keeps key/focus order: key index, active element at keydown, keys before each focus and the previous key", () => {
    const at = (tag: string, id = "") => ({ tag, id, classes: [], focus_visible: true, focus: true, document_has_focus: true });
    const k = (name: string, shift: boolean, ctrl: boolean, active: ReturnType<typeof at> | null): SweepEvent => ({ kind: "key", key: name, shift, ctrl, trusted: true, active });
    const events: SweepEvent[] = [
      k("Control", false, true, at("textarea")),
      k("Shift", true, true, at("textarea")),
      k("F6", true, true, at("textarea")),
      focus(ctl("marker", { tag: "span" })),
      k("Tab", false, false, at("span")),
      k("Tab", false, false, at("span")),
      focus(ctl("copy")),
      k("Shift", true, false, at("button")),
      k("Tab", true, false, at("button")),
      focus(ctl("term", { tag: "textarea" })),
    ];
    expect(interleaveSweep(events)).toEqual([
      { kind: "key", index: 0, token: "ctrl+shift+F6", trusted: true, active: at("textarea") },
      { kind: "focus", id: "marker", tag: "span", name: "Controle marker", keys_before: 1, previous_key: "ctrl+shift+F6", active_element: null },
      { kind: "key", index: 1, token: "Tab", trusted: true, active: at("span") },
      { kind: "key", index: 2, token: "Tab", trusted: true, active: at("span") },
      { kind: "focus", id: "copy", tag: "button", name: "Controle copy", keys_before: 3, previous_key: "Tab", active_element: null },
      { kind: "key", index: 3, token: "shift+Tab", trusted: true, active: at("button") },
      { kind: "focus", id: "term", tag: "textarea", name: "Controle term", keys_before: 4, previous_key: "shift+Tab", active_element: null },
    ]);
    // A focus before any key, and a key recorded without an active sample (null, never invented).
    expect(interleaveSweep([focus(ctl("x")), key("Tab")])).toEqual([
      { kind: "focus", id: "x", tag: "button", name: "Controle x", keys_before: 0, previous_key: null, active_element: null },
      { kind: "key", index: 0, token: "Tab", trusted: true, active: null },
    ]);
    // keys_seen keeps its published shape (the Rust evaluator reads it): no active element there.
    expect(splitSweep(events).keys_seen[2]).toEqual({ key: "F6", shift: true, ctrl: true, trusted: true });
  });

  it("tabOrder: positive tabindex ascending first, then 0 in DOM order; skips disabled/negative/hidden/inert", () => {
    const n = (id: string, tabIndex: number, x: Partial<{ disabled: boolean; hidden: boolean; inert: boolean; after: boolean }> = {}) => ({ id, tabIndex, disabled: false, hidden: false, inert: false, ...x });
    const els = [n("z0", 0), n("p2", 2), n("dis", 0, { disabled: true }), n("neg", -1), n("p1", 1), n("hid", 0, { hidden: true }), n("in", 0, { inert: true }), n("ce", 0), n("p2b", 2)];
    expect(tabOrder(els).map((e) => e.id)).toEqual(["p1", "p2", "p2b", "z0", "ce"]);
    // After a tabindex=-1 start point only tabindex 0 controls following it are reachable.
    expect(tabOrder(els, (e) => e.id !== "z0").map((e) => e.id)).toEqual(["ce"]);
  });

  it("orderAfterMarker returns controls following the marker in DOM order, wrapping to preceding controls", () => {
    const n = (id: string, tabIndex: number, x: Partial<{ disabled: boolean; hidden: boolean; inert: boolean }> = {}) => ({ id, tabIndex, disabled: false, hidden: false, inert: false, ...x });
    const els = [n("btn1", 0), n("btn2", 0), n("dis", 0, { disabled: true }), n("neg", -1), n("btn3", 0), n("btn4", 0)];

    // Marker between btn2 and btn3: btn3, btn4 follow; wraps to btn1, btn2
    const following = (e: { id: string }) => e.id === "btn3" || e.id === "btn4";
    expect(orderAfterMarker(els, following).map((e) => e.id)).toEqual(["btn3", "btn4", "btn1", "btn2"]);

    // Marker after all controls (empty following, as observed in live r1): wraps around to all controls in DOM order
    expect(orderAfterMarker(els, () => false).map((e) => e.id)).toEqual(["btn1", "btn2", "btn3", "btn4"]);

    // Marker before all controls: all controls follow
    expect(orderAfterMarker(els, () => true).map((e) => e.id)).toEqual(["btn1", "btn2", "btn3", "btn4"]);
  });

  // r12 (GUI r11: the document wrap sits between the last following and the first preceding stop).
  // Would catch: positive tabindex counted as following (before the wrap), the split disagreeing
  // with orderAfterMarker, or skipped controls kept on either side.
  it("splitAfterMarker: following in DOM order, then positive tabindex and preceding; concatenation is orderAfterMarker", () => {
    const n = (id: string, tabIndex: number, x: Partial<{ disabled: boolean; hidden: boolean; inert: boolean }> = {}) => ({ id, tabIndex, disabled: false, hidden: false, inert: false, ...x });
    const els = [n("btn1", 0), n("p1", 1), n("dis", 0, { disabled: true }), n("btn3", 0), n("p2", 2), n("hid", 0, { hidden: true }), n("btn4", 0)];
    const following = (e: { id: string }) => ["p2", "hid", "btn4"].includes(e.id);
    const s = splitAfterMarker(els, following);
    expect(s.following.map((e) => e.id)).toEqual(["btn4"]);
    expect(s.preceding.map((e) => e.id)).toEqual(["p1", "p2", "btn1", "btn3"]);
    expect(orderAfterMarker(els, following)).toEqual([...s.following, ...s.preceding]);
  });


  // Would catch: any textarea or any control carrying the class treated as the terminal, or a
  // missing class list read as the terminal.
  it("retainsTab: only textarea.ime-target keeps Tab for the PTY", () => {
    expect(retainsTab(ctl("t", { tag: "textarea", classes: ["ime-target", "svelte-x"] }))).toBe(true);
    expect(retainsTab(ctl("t", { tag: "textarea", classes: ["notes"] }))).toBe(false);
    expect(retainsTab(ctl("t", { tag: "textarea" }))).toBe(false);
    expect(retainsTab(ctl("b", { tag: "button", classes: ["ime-target"] }))).toBe(false);
  });

  describe("mainSweepPlan (focus starts on the terminal, the lead chord moves it to the exit marker)", () => {
    const ids = (p: { expected: { id: string }[] }) => p.expected.map((c) => c.id);
    const n = (id: string, keeps = false) => ({ id, keeps });
    const keeps = (c: { keeps: boolean }) => c.keeps;

    // Planner decision r10 (GUI r9: the wrap Tab through the end of the document re-entered the
    // page by host behaviour and the second chord after it never reached the terminal). Would catch:
    // a Tab pressed on the terminal (goes to the PTY), the r8/r9 wrap Tab and re-entry stop, a second
    // chord after a wrap, or the closing Shift+Tab not listed as the terminal again.
    const split = <T>(following: T[], preceding: T[]) => ({ following, preceding });
    // Planner decision r12 (GUI r11: key[14] Tab on the last following control reached body without
    // focusin; WebKitGTK spends two Tabs on the document wrap). Would catch: no extra Tab (every stop
    // after the wrap one key late), more than one, a declared index not the last following stop,
    // or the expected stops changed by the extra key.
    it("terminal last: chord, one Tab per stop plus one declared wrap Tab, chord once on the terminal, Shift+Tab back into it", () => {
      const p = mainSweepPlan(split([n("copy"), n("a")], [n("term", true)]), keeps);
      expect(p.keys).toEqual([EXIT_CHORD, "Tab", "Tab", "Tab", "Tab", EXIT_CHORD, "shift+Tab"]);
      expect(ids(p)).toEqual(["copy", "a", "term", "term"]);
      expect(p.keys.filter((k) => k === "Tab")).toHaveLength(p.expected.length);
      expect(p.keys.filter((k) => k === EXIT_CHORD)).toHaveLength(2);
      expect(p.keys.slice(p.keys.lastIndexOf(EXIT_CHORD))).toEqual([EXIT_CHORD, "shift+Tab"]);
      expect(p.wrap_after).toBe(1);
      expect(p.split).toEqual({ following: ["copy", "a"], preceding: ["term"] });
      const q = mainSweepPlan(split([n("copy")], [n("a"), n("term", true)]), keeps);
      expect(q.keys).toEqual(p.keys);
      expect(ids(q)).toEqual(ids(p));
      expect(q.wrap_after).toBe(0);
      expect(q.split).toEqual({ following: ["copy"], preceding: ["a", "term"] });
    });

    // Would catch: a plan whose closing Shift+Tab cannot land on the terminal (no terminal, terminal
    // not last, a second terminal needing another chord), an empty/one-stop sweep, or a wrap that
    // cannot be declared (nothing follows the marker).
    it("refuses plans without exactly one terminal as the last of at least two stops, or without a following stop", () => {
      expect(() => mainSweepPlan(split([n("a"), n("b")], [n("c")]), keeps)).toThrow(/last stop/);
      expect(() => mainSweepPlan(split([n("a")], [n("term", true), n("b")]), keeps)).toThrow(/last stop/);
      expect(() => mainSweepPlan(split([n("a")], [n("t2", true), n("t1", true)]), keeps)).toThrow(/only/);
      expect(() => mainSweepPlan(split([n("t2", true), n("a")], [n("t1", true)]), keeps)).toThrow(/only/);
      expect(() => mainSweepPlan(split([], [n("term", true)]), keeps)).toThrow(/two/);
      expect(() => mainSweepPlan(split([], []), keeps)).toThrow(/two/);
      expect(() => mainSweepPlan(split([], [n("a"), n("term", true)]), keeps)).toThrow(/no control follows the exit marker/);
    });
  });

  it("accessibleName follows labelledby → aria-label → label → text (aria-hidden excluded) → title", () => {
    const text = (t: string): NameNode => ({ nodeType: 3, textContent: t, childNodes: [] });
    const el = (attrs: Record<string, string>, children: NameNode[] = [], labels: NameNode[] = []): NameNode => ({ nodeType: 1, tagName: "BUTTON", textContent: children.map((c) => c.textContent).join(""), childNodes: children, labels, getAttribute: (k) => attrs[k] ?? null });
    const byId = (id: string) => (id === "lbl" ? el({}, [text("Rótulo  externo")]) : null);
    const icon = el({ "aria-hidden": "true" }, [text("×")]);
    expect(accessibleName(el({ "aria-labelledby": "lbl missing", "aria-label": "A" }, [text("T")]), byId)).toEqual({ name: "Rótulo externo", source: "aria-labelledby" });
    expect(accessibleName(el({ "aria-label": " Fechar ", title: "t" }, [text("T")]), byId)).toEqual({ name: "Fechar", source: "aria-label" });
    expect(accessibleName(el({}, [], [el({}, [text("Porta")])]), byId)).toEqual({ name: "Porta", source: "label" });
    expect(accessibleName(el({ title: "Dica" }, [icon, text(" Abrir ")]), byId)).toEqual({ name: "Abrir", source: "text" });
    expect(accessibleName(el({ title: "Conexões remotas" }, [icon]), byId)).toEqual({ name: "Conexões remotas", source: "title" });
    expect(accessibleName(el({}, [icon]), byId)).toEqual({ name: "", source: "none" });
  });

  it("stateRecord takes the nearest ancestor text containing the token, empty when none", () => {
    expect(stateRecord("online", "", ["● ", "local LOCAL", "Conexão: local - ONLINE", "online tudo"])).toEqual({ status: "online", own_text: "", carrier_text: "Conexão: local - ONLINE" });
    expect(stateRecord("working", " Trabalhando ", ["x"])).toEqual({ status: "working", own_text: "Trabalhando", carrier_text: "" });
    expect(stateRecord("", "", ["qualquer"])).toEqual({ status: "", own_text: "", carrier_text: "" });
  });

  // GUI r4: the dialog Host input was activeElement with focus_visible false and no outline. Would
  // catch a sample that cannot tell "document without focus under the compositor" (hasFocus false,
  // element :focus) from "element not focused yet" (:focus false) or conflates :focus with :focus-visible.
  // Distinguishes a product that restores focus asynchronously (:focus false at keydown/microtask,
  // true later with hasFocus true) from a window that loses focus (window blur, hasFocus false).
  // Would catch samples taken at the wrong moment, a missed window blur, other keys traced, or
  // samples recorded after stop.
  it("traceFocusAfterKey samples at keydown, microtask, next frame and 100 ms, plus window blur/focus", () => {
    type L = (e: Event) => void;
    const target = () => {
      const ls = new Map<string, Set<L>>();
      return {
        addEventListener: (t: string, fn: L) => void (ls.get(t) ?? ls.set(t, new Set()).get(t)!).add(fn),
        removeEventListener: (t: string, fn: L) => void ls.get(t)?.delete(fn),
        fire: (t: string, e: Record<string, unknown> = {}) => ls.get(t)?.forEach((fn) => fn({ type: t, ...e } as unknown as Event)),
        size: () => Array.from(ls.values()).reduce((n, x) => n + x.size, 0),
      };
    };
    const doc = target();
    const win = target();
    const micro: (() => void)[] = [];
    const frames: (() => void)[] = [];
    const timers: { ms: number; cb: () => void }[] = [];
    let state = { focus: false, document_has_focus: true };
    let t = 0;
    const env = {
      doc, win,
      sample: () => ({ tag: "button", id: "", classes: [], focus_visible: state.focus, ...state }),
      microtask: (cb: () => void) => void micro.push(cb),
      frame: (cb: () => void) => void frames.push(cb),
      later: (ms: number, cb: () => void) => void timers.push({ ms, cb }),
      now: () => ++t,
    };
    const trace = traceFocusAfterKey(env, "Escape");
    doc.fire("keydown", { key: "Tab" });
    expect(trace.take()).toEqual([]);
    doc.fire("keydown", { key: "Escape" });
    state = { focus: false, document_has_focus: false };
    win.fire("blur");
    micro.splice(0).forEach((cb) => cb());
    frames.splice(0).forEach((cb) => cb());
    expect(timers.map((x) => x.ms)).toEqual([100]);
    state = { focus: true, document_has_focus: true };
    win.fire("focus");
    timers.splice(0).forEach((x) => x.cb());
    const got = trace.take();
    expect(got.map((e) => [e.at, e.active?.focus, e.active?.document_has_focus])).toEqual([
      ["keydown", false, true],
      ["window-blur", false, false],
      ["microtask", false, false],
      ["frame", false, false],
      ["window-focus", true, true],
      ["100ms", true, true],
    ]);
    expect(got.every((e, i) => i === 0 || e.t > got[i - 1]!.t)).toBe(true);
    // Pending callbacks after stop are not recorded; listeners are removed.
    doc.fire("keydown", { key: "Escape" });
    trace.stop();
    micro.splice(0).forEach((cb) => cb());
    expect(trace.take().map((e) => e.at)).toEqual(["keydown"]);
    expect(doc.size() + win.size()).toBe(0);
  });

  type L = (e: Event) => void;
  const fakeTarget = (name: string) => {
    const ls = new Map<string, Set<L>>();
    return {
      name,
      addEventListener: (t: string, fn: L) => void (ls.get(t) ?? ls.set(t, new Set()).get(t)!).add(fn),
      removeEventListener: (t: string, fn: L) => void ls.get(t)?.delete(fn),
      fire: (t: string, e: Record<string, unknown> = {}) => {
        const ev = { type: t, ...e } as unknown as Event;
        ls.get(t)?.forEach((fn) => fn(ev));
        return ev;
      },
      /** Same event object on another target (a real dispatch reaches every phase with one Event). */
      dispatch: (t: string, ev: Event) => ls.get(t)?.forEach((fn) => fn(ev)),
      size: () => Array.from(ls.values()).reduce((n, x) => n + x.size, 0),
    };
  };
  const el = (tag: string, classes: string[] = []) => ({ tag, classes });
  const describeTarget = (win: object) => (t: EventTarget | null) => (t === win ? "window" : t ? { tag: (t as unknown as { tag: string }).tag, id: "", classes: (t as unknown as { classes: string[] }).classes } : null);

  // GUI r8 Escape: window blur +4 ms with the compositor keeping the window. Would catch: focusout/
  // focusin not recorded, relatedTarget dropped (null vs element is the platform/page decision),
  // the blur target not told apart (window vs element), or product close marks missing/out of order.
  it("traceFocusAfterKey also records document focusout/focusin with relatedTarget, the blur target and marks", () => {
    const doc = fakeTarget("doc");
    const win = fakeTarget("win");
    let t = 0;
    const trace = traceFocusAfterKey({ doc, win, sample: () => null, microtask: () => {}, frame: () => {}, later: () => {}, now: () => ++t, describe: describeTarget(win) }, "Escape");
    const conectar = el("button", ["primary"]);
    const opener = el("button", ["opener"]);
    doc.fire("keydown", { key: "Escape" });
    trace.mark("closer-release:true");
    doc.fire("focusout", { target: conectar, relatedTarget: null });
    doc.fire("focusin", { target: opener, relatedTarget: conectar });
    trace.mark("closer-focus:true");
    win.fire("blur", { target: win });
    doc.fire("focusout", { target: opener, relatedTarget: null });
    const got = trace.take();
    expect(got.map((e) => [e.at, e.target ?? null, e.related_target ?? null])).toEqual([
      ["keydown", null, null],
      ["closer-release:true", null, null],
      ["focusout", { tag: "button", id: "", classes: ["primary"] }, null],
      ["focusin", { tag: "button", id: "", classes: ["opener"] }, { tag: "button", id: "", classes: ["primary"] }],
      ["closer-focus:true", null, null],
      ["window-blur", "window", null],
      ["focusout", { tag: "button", id: "", classes: ["opener"] }, null],
    ]);
    trace.stop();
    trace.mark("late");
    doc.fire("focusin", { target: opener, relatedTarget: null });
    expect(trace.take()).toEqual([]);
    expect(doc.size() + win.size()).toBe(0);
  });

  // Would catch (r8 unknowns): the chord keydown recorded without activeElement/isComposing, the
  // defaultPrevented read only at capture (before the terminal handler), compositions kept from
  // before the last terminal focus-in, other keys traced, or recording after stop.
  it("traceChords records each exit chord with active element, isComposing, defaultPrevented and compositions since the terminal focus-in", () => {
    const doc = fakeTarget("doc");
    const win = fakeTarget("win");
    const tasks: (() => void)[] = [];
    let t = 0;
    let active = "textarea";
    const term = el("textarea", ["ime-target"]);
    const marker = el("span", ["exit-marker"]);
    const trace = traceChords({
      doc, win,
      sample: () => ({ tag: active, id: "", classes: [], focus_visible: false, focus: true, document_has_focus: true }),
      describe: describeTarget(win),
      isTerminal: (x) => x === (term as unknown as EventTarget),
      later: (_ms, cb) => void tasks.push(cb),
      now: () => ++t,
    });
    doc.fire("compositionstart", { target: term, data: "" }); // before the focus-in: dropped
    doc.fire("focusin", { target: term }); // t=1
    doc.fire("compositionstart", { target: term, data: "" }); // t=2
    doc.fire("focusin", { target: marker }); // not the terminal: list kept
    doc.fire("keydown", { key: "Tab", ctrlKey: false, shiftKey: false, target: term });
    const ev = doc.fire("keydown", { key: "F6", ctrlKey: true, shiftKey: true, altKey: false, metaKey: false, isComposing: true, defaultPrevented: false, target: term }); // t=3
    (ev as unknown as { defaultPrevented: boolean }).defaultPrevented = true; // the terminal handler
    win.dispatch("keydown", ev);
    active = "span";
    tasks.splice(0).forEach((cb) => cb());
    const [entry, ...rest] = trace.take();
    expect(rest).toEqual([]);
    expect(entry).toMatchObject({
      t: 3,
      active: { tag: "textarea" },
      target: { tag: "textarea", classes: ["ime-target"] },
      is_composing: true,
      default_prevented_capture: false,
      default_prevented_bubble: true,
      default_prevented_after: true,
      active_after: { tag: "span" },
      terminal_focus_in_t: 1,
      compositions_since_focus: [{ type: "compositionstart", data: "", t: 2, target: { tag: "textarea", id: "", classes: ["ime-target"] } }],
    });
    // A new terminal focus-in restarts the list; a chord whose bubble never ran keeps null there.
    doc.fire("focusin", { target: term });
    doc.fire("compositionupdate", { target: term, data: "ni" });
    doc.fire("compositionend", { target: term, data: "你" });
    doc.fire("keydown", { key: "F6", ctrlKey: true, shiftKey: true, altKey: false, metaKey: false, isComposing: false, defaultPrevented: false, target: term });
    tasks.splice(0).forEach((cb) => cb());
    const [second] = trace.take();
    expect(second!.compositions_since_focus.map((c) => [c.type, c.data])).toEqual([["compositionupdate", "ni"], ["compositionend", "你"]]);
    expect(second!.default_prevented_bubble).toBeNull();
    doc.fire("keydown", { key: "F6", ctrlKey: true, shiftKey: true, altKey: false, metaKey: false, isComposing: false, defaultPrevented: false, target: term });
    trace.stop();
    tasks.splice(0).forEach((cb) => cb());
    expect(trace.take().map((e) => e.active_after)).toEqual([null]);
    doc.fire("keydown", { key: "F6", ctrlKey: true, shiftKey: true, altKey: false, metaKey: false, target: term });
    expect(trace.take()).toEqual([]);
    expect(doc.size() + win.size()).toBe(0);
  });

  it("sampleActiveElement records document.hasFocus() and :focus next to :focus-visible", () => {
    const globalDoc = (globalThis as unknown as { document?: unknown }).document;
    const set = (hasFocus: boolean, matching: string[]) =>
      ((globalThis as unknown as { document: unknown }).document = {
        hasFocus: () => hasFocus,
        activeElement: { tagName: "INPUT", id: "host", classList: [], matches: (sel: string) => matching.includes(sel) },
      });
    try {
      set(false, [":focus"]);
      expect(sampleActiveElement()).toMatchObject({ tag: "input", document_has_focus: false, focus: true, focus_visible: false });
      set(true, []);
      expect(sampleActiveElement()).toMatchObject({ document_has_focus: true, focus: false, focus_visible: false });
      set(true, [":focus", ":focus-visible"]);
      expect(sampleActiveElement()).toMatchObject({ document_has_focus: true, focus: true, focus_visible: true });
    } finally {
      if (globalDoc === undefined) delete (globalThis as unknown as { document?: unknown }).document;
      else (globalThis as unknown as { document: unknown }).document = globalDoc;
    }
  });

  it("sampleActiveElement returns null without DOM activeElement or extracts tag/id/classes/focus_visible", () => {
    expect(sampleActiveElement()).toBeNull();
    const fakeEl = {
      tagName: "BUTTON",
      id: "btn-ssh",
      classList: ["primary", "active"],
      matches: (selector: string) => selector === ":focus-visible",
    };
    const globalDoc = (globalThis as unknown as { document?: unknown }).document;
    try {
      (globalThis as unknown as { document: unknown }).document = {
        activeElement: Object.setPrototypeOf(fakeEl, (globalThis as unknown as { HTMLElement?: { prototype: object } }).HTMLElement?.prototype ?? Object.prototype),
      };
      // In node env without HTMLElement, sampleActiveElement handles type guard safely
      const sample = sampleActiveElement();
      if (sample) {
        expect(sample).toEqual({
          tag: "button",
          id: "btn-ssh",
          classes: ["primary", "active"],
          focus_visible: true,
          focus: false,
          document_has_focus: null,
        });
      }
    } finally {
      if (globalDoc === undefined) {
        delete (globalThis as unknown as { document?: unknown }).document;
      } else {
        (globalThis as unknown as { document: unknown }).document = globalDoc;
      }
    }
  });
  it("carrierMarks: only textarea.ime-target records its .terminal container, with the baseline kept from the expected pass", () => {
    // Would catch: container recorded for other controls, a baseline overwritten by the focused
    // (ringed) style, or classes not echoed for the parent's ime-target identification.
    const sty = (outline: string): Style => ({ outline_style: outline, outline_width: "2px", outline_color: "rgb(97, 175, 239)", box_shadow: "none", border_color: "rgb(1, 2, 3)", background_color: "rgb(4, 5, 6)" });
    const box = { tag: "terminal" };
    const node = (classes: string[], ime: boolean) => ({ classList: classes, matches: (sel: string) => sel === ".ime-target" && ime, closest: (sel: string) => (sel === ".terminal" ? box : null) });
    const calls: object[] = [];
    // First container read is the plain baseline, every later read the ringed focus style.
    const style = (el: object): Style => (calls.push(el), calls.length === 1 ? sty("none") : sty("solid"));
    const store = new WeakMap<object, Style>();
    const ime = node(["ime-target", "svelte-kfm3zr"], true);
    expect(carrierMarks(ime, true, store, style)).toEqual({ classes: ["ime-target", "svelte-kfm3zr"], container_style: sty("none"), container_baseline: sty("none") });
    expect(carrierMarks(ime, false, store, style)).toEqual({ classes: ["ime-target", "svelte-kfm3zr"], container_style: sty("solid"), container_baseline: sty("none") });
    expect(calls.every((c) => c === box)).toBe(true);
    const input = node(["field"], false);
    expect(carrierMarks(input, true, store, style)).toEqual({ classes: ["field"], container_style: null, container_baseline: null });
    // Never described in the expected pass: no baseline invented from the current style.
    expect(carrierMarks(node(["ime-target"], true), false, new WeakMap(), () => sty("solid"))).toEqual({ classes: ["ime-target"], container_style: sty("solid"), container_baseline: null });
  });
});
