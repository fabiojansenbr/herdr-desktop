// @vitest-environment happy-dom
// Spec 022 — wheel over the focused pane must scroll (AC-022-01, AC-022-02); spec 033 — wheel over
// an unfocused pane focuses it and delivers the accumulated notches (AC-033-01).
// Would catch: wheel discarded unless the IME textarea has keyboard focus; a line-mode notch
// sent as 1 line instead of the engine's 3; click-to-focus not arming the next wheel; wheel on an
// unfocused pane using pane.scroll without focus (the spec 022 rule, wrong against the TUI);
// notches dropped while the focus is pending or delivered to the previously focused pane.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalView from "./TerminalView.svelte";
import { hitPane, wheelRoute } from "./interaction";
import type { FrameEvent, InputDto, PaneFocusRequest, PaneMeta, ScrollRequest } from "./types";

class FakeCtx {
  fillStyle: string | CanvasGradient | CanvasPattern = "";
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  globalAlpha = 1;
  fillRect() {}
  fillText() {}
  setTransform() {}
  measureText() {
    return { width: 10 };
  }
}

const originalGetContext = HTMLCanvasElement.prototype.getContext;

function installCanvasMock() {
  HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, type: string, attrs?: unknown) {
    void type;
    void attrs;
    return new FakeCtx() as unknown as CanvasRenderingContext2D;
  } as typeof HTMLCanvasElement.prototype.getContext;
}

const rafQueue: { id: number; cb: FrameRequestCallback }[] = [];
let rafNext = 1;

function flushFrames() {
  const batch = rafQueue.splice(0);
  for (const item of batch) item.cb(0);
}

const metrics = { cellWidth: 10, cellHeight: 20 };
const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

function pane(over: Partial<PaneMeta> = {}): PaneMeta {
  return {
    pane_id: "w1:p1",
    content_revision: 40,
    rect: { x: 0, y: 0, width: 8, height: 3 },
    inner_rect: { x: 0, y: 0, width: 8, height: 3 },
    scroll: { offset_from_bottom: 2, max_offset_from_bottom: 10, viewport_rows: 3 },
    focused: true,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: 80,
    pixel_height: 78,
    ...over,
  };
}

function fullFrame(revision: number, width = 8, height = 3): FrameEvent {
  const cells = Array.from({ length: width * height }, () => ({ s: " ", fg: 0, bg: 0, m: 0 }));
  return {
    type: "full",
    revision,
    width,
    height,
    cells,
    cursor: { x: 0, y: 0, visible: false, shape: 2 },
    panes: [{ pane_id: "w1:p1", x: 0, y: 0, width, height, focused: true }],
  };
}

function metadata(revision: number, panes: PaneMeta[]): FrameEvent {
  return { type: "metadata", revision, panes, hyperlinks: [] };
}

beforeEach(() => {
  installCanvasMock();
  rafQueue.length = 0;
  rafNext = 1;
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    const id = rafNext++;
    rafQueue.push({ id, cb });
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => {
    const i = rafQueue.findIndex((item) => item.id === id);
    if (i >= 0) rafQueue.splice(i, 1);
  });
});

afterEach(() => {
  for (const host of hosts.splice(0)) {
    unmount(host.app);
    host.el.remove();
  }
  HTMLCanvasElement.prototype.getContext = originalGetContext;
  vi.unstubAllGlobals();
});

function mountView() {
  let push: (event: FrameEvent) => void = () => {};
  const inputs: InputDto[] = [];
  const scrolls: ScrollRequest[] = [];
  const focuses: PaneFocusRequest[] = [];
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TerminalView, {
    target: el,
    props: {
      subscribe: (handler: (event: FrameEvent) => void) => {
        push = handler;
        return () => {
          push = () => {};
        };
      },
      onInput: (events: InputDto[]) => {
        inputs.push(...events);
      },
      onResize: () => {},
      onScroll: (request: ScrollRequest) => {
        scrolls.push(request);
      },
      onSelectPane: (request: PaneFocusRequest) => {
        focuses.push(request);
      },
      fontSize: 20,
    },
  });
  hosts.push({ el, app });
  flushSync();
  const canvas = () => {
    const node = el.querySelector("canvas");
    if (!node) throw new Error("no canvas");
    return node;
  };
  const target = () => {
    const ta = el.querySelector<HTMLTextAreaElement>("textarea.ime-target");
    if (!ta) throw new Error("no ime target");
    return ta;
  };
  const sizeCanvas = () => {
    const node = canvas();
    node.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 80, height: 78, right: 80, bottom: 78, x: 0, y: 0, toJSON() {} }) as DOMRect;
  };
  return {
    el,
    inputs,
    scrolls,
    focuses,
    push: (event: FrameEvent) => push(event),
    canvas,
    target,
    sizeCanvas,
    container: () => el.querySelector<HTMLElement>(".terminal")!,
  };
}

function live(view: ReturnType<typeof mountView>, panes: PaneMeta[] = [pane()]) {
  view.push({
    type: "identity",
    boot_id: "boot-022",
    generation: 1,
    connection_generation: 1,
    server_version: "0.9.0",
    pane_id: panes.find((p) => p.focused)?.pane_id ?? "w1:p1",
  });
  view.push(fullFrame(10));
  view.push(metadata(10, panes));
  flushFrames();
  view.sizeCanvas();
}

function wheel(target: EventTarget, init: WheelEventInit) {
  const event = new WheelEvent("wheel", { bubbles: true, cancelable: true, ...init });
  // happy-dom's WheelEvent ignores clientX/clientY in the init dict; the product reads them.
  Object.defineProperty(event, "clientX", { value: init.clientX ?? 15 });
  // TerminalView cellHeight is ceil(20 * 1.3) = 26; row 1 sits at y=26..51.
  Object.defineProperty(event, "clientY", { value: init.clientY ?? 31 });
  target.dispatchEvent(event);
}

describe("wheelRoute lines match the TUI (AC-022-01)", () => {
  // Would catch: a line-mode notch counted as |deltaY| (1) instead of mouse_scroll_lines (3), or
  // a pixel notch counted as raw pixels instead of cells.
  it("counts 3 lines per line-mode notch and deltaY/cellHeight in pixel mode", () => {
    const hit = hitPane([pane()], 1, 1)!;
    expect(wheelRoute(hit, "w1:p1", { deltaY: -1, deltaMode: 1, shift: false }, metrics)).toEqual({
      kind: "input",
      input: { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 3 },
      repeat: 1,
    });
    expect(wheelRoute(hit, "w1:p1", { deltaY: 3, deltaMode: 1, shift: false }, metrics)).toMatchObject({
      input: { action: "scroll_down", lines: 3 },
    });
    expect(wheelRoute(hit, "w1:p1", { deltaY: -60, deltaMode: 0, shift: false }, metrics)).toMatchObject({
      input: { action: "scroll_up", lines: 3 },
    });
    expect(wheelRoute(hit, "w1:p1", { deltaY: 10, deltaMode: 0, shift: false }, metrics)).toMatchObject({
      input: { action: "scroll_down", lines: 1 },
    });
  });

  // Would catch: a horizontal wheel turning into a vertical scroll_up/down when the app does not
  // report mouse, or Shift over a pane without history throwing instead of being a no-op.
  it("ignores horizontal-only wheels and Shift over a pane without scrollback", () => {
    const hit = hitPane([pane()], 1, 1)!;
    expect(wheelRoute(hit, "w1:p1", { deltaY: 0, deltaMode: 0, shift: false }, metrics)).toBeNull();
    const none = hitPane([pane({ scroll: null, focused: false, pane_id: "w1:p2" })], 1, 1)!;
    expect(wheelRoute(none, "w1:p1", { deltaY: -20, deltaMode: 0, shift: true }, metrics)).toBeNull();
  });

  // Spec 033 AC-033-01, rule measured in the TUI (mouse.rs:2264-2283). Would catch: the spec 022
  // rule (pane.scroll of the unfocused pane), the wheel dropped for a pane without scrollback, or
  // Shift changing the focus instead of moving that pane's local scrollback.
  it("asks for the focus of an unfocused pane and hands it the wheel input", () => {
    const b = hitPane(
      [pane({ pane_id: "w1:p2", focused: false, inner_rect: { x: 8, y: 0, width: 8, height: 3 }, scroll: null })],
      9,
      1,
    )!;
    expect(wheelRoute(b, "w1:p1", { deltaY: -1, deltaMode: 1, shift: false }, metrics)).toEqual({
      kind: "focus",
      input: { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 3 },
      repeat: 1,
    });
    expect(wheelRoute(b, "w1:p1", { deltaY: -1, deltaMode: 1, shift: true }, metrics)).toBeNull();
  });
});

describe("wheel reports for panes whose app takes the wheel (TUI parity + 1)", () => {
  // Measured 2026-09-23 on the reference host (Hyprland, WebKitGTK): one notch is one WebKit wheel
  // event of deltaY ±144 px, while the TUI in foot delivers ~2 wheel reports per equivalent event.
  // The engine sends one wheel report (MouseReport) or one arrow (AlternateScroll) per input and
  // ignores `lines`, so the desktop repeats the input: 3 per notch by user decision (TUI + 1).
  // Would catch: one report per WebKit event (half the TUI), repeats on host-scroll panes (which
  // already scroll by `lines`), or a touchpad micro-delta turning into zero or many reports.
  const app = (over: Partial<PaneMeta> = {}) => hitPane([pane({ mouse_reporting: true, scroll: null, ...over })], 1, 1)!;

  it("repeats a pixel notch 3 times on a mouse-reporting pane and scales with deltaY", () => {
    expect(wheelRoute(app(), "w1:p1", { deltaY: -144, deltaMode: 0, shift: false }, metrics)).toEqual({
      kind: "input",
      input: { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 7 },
      repeat: 3,
    });
    expect(wheelRoute(app(), "w1:p1", { deltaY: 288, deltaMode: 0, shift: false }, metrics)).toMatchObject({
      input: { action: "scroll_down" },
      repeat: 6,
    });
    expect(wheelRoute(app(), "w1:p1", { deltaY: -10, deltaMode: 0, shift: false }, metrics)).toMatchObject({ repeat: 1 });
    expect(wheelRoute(app(), "w1:p1", { deltaY: -100000, deltaMode: 0, shift: false }, metrics)).toMatchObject({ repeat: 64 });
  });

  it("repeats on the alternate screen, counts a line-mode notch as one notch, and keeps focus routes", () => {
    const alt = app({ mouse_reporting: false, alternate_screen_active: true });
    expect(wheelRoute(alt, "w1:p1", { deltaY: 144, deltaMode: 0, shift: false }, metrics)).toMatchObject({ repeat: 3 });
    expect(wheelRoute(app(), "w1:p1", { deltaY: -1, deltaMode: 1, shift: false }, metrics)).toMatchObject({ repeat: 3 });
    expect(wheelRoute(app(), "w1:p1", { deltaY: -6, deltaMode: 1, shift: false }, metrics)).toMatchObject({ repeat: 6 });
    expect(wheelRoute(app({ focused: false }), "w1:p9", { deltaY: -144, deltaMode: 0, shift: false }, metrics)).toMatchObject({
      kind: "focus",
      repeat: 3,
    });
  });

  it("never repeats on a host-scroll pane", () => {
    const shell = hitPane([pane()], 1, 1)!;
    expect(wheelRoute(shell, "w1:p1", { deltaY: -144, deltaMode: 0, shift: false }, metrics)).toMatchObject({
      input: { lines: 7 },
      repeat: 1,
    });
  });
});

describe("TerminalView wheel on the focused pane (AC-022-01)", () => {
  // Would catch: onWheel attached only to the IME textarea, so a wheel over the focused pane is
  // dropped when that textarea does not have keyboard focus (the user report: roda não rola).
  it("sends exactly one scroll_up with 3 lines per line-mode notch without focusing the textarea", () => {
    const view = mountView();
    live(view);
    expect(document.activeElement === view.target()).toBe(false);

    wheel(view.container(), { deltaY: -1, deltaMode: 1 });

    expect(view.inputs).toEqual([
      { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 3 },
    ]);
    expect(view.scrolls).toEqual([]);
  });

  // Would catch: pixel-mode wheels counted as 1 regardless of cell height, or discarded when
  // inputEnabled is true but the IME target is blurred.
  it("uses deltaY/cellHeight in pixel mode and still sends when the textarea is blurred", () => {
    const view = mountView();
    live(view);
    view.target().blur();
    wheel(view.target(), { deltaY: -78, deltaMode: 0, clientX: 15, clientY: 31 });
    expect(view.inputs).toEqual([
      { kind: "mouse", action: "scroll_up", column: 1, row: 1, modifiers: 0, lines: 3 },
    ]);
  });

  // Would catch: a wheel during stale metadata (full frame, no metadata yet) forwarded with the
  // previous pane identity. A focused pane without local history still sends mouse input: the
  // engine's apply_scroll decides HostScroll vs AlternateScroll vs MouseReport.
  it("waits for metadata when stale and still sends scroll_up once the surface is confirmed", () => {
    const view = mountView();
    live(view);
    view.push(fullFrame(11));
    flushFrames();
    view.sizeCanvas();
    wheel(view.target(), { deltaY: -1, deltaMode: 1 });
    expect(view.inputs).toEqual([]);
    expect(view.scrolls).toEqual([]);

    view.push(metadata(11, [pane({ scroll: null })]));
    flushFrames();
    view.sizeCanvas();
    view.inputs.length = 0;
    wheel(view.target(), { deltaY: -1, deltaMode: 1 });
    expect(view.scrolls).toEqual([]);
    expect(view.inputs).toHaveLength(1);
    expect(view.inputs[0]).toMatchObject({ kind: "mouse", action: "scroll_up", lines: 3 });
  });

  // Would catch: the repeat computed but only one input reaching the host.
  it("sends 3 scroll_up inputs per pixel notch on a mouse-reporting pane", () => {
    const view = mountView();
    live(view);
    view.push(metadata(10, [pane({ mouse_reporting: true })]));
    flushFrames();
    view.sizeCanvas();
    view.inputs.length = 0;
    wheel(view.container(), { deltaY: -144, deltaMode: 0 });
    expect(view.inputs).toHaveLength(3);
    for (const input of view.inputs) expect(input).toMatchObject({ kind: "mouse", action: "scroll_up" });
  });
});

describe("click activates then wheel; wheel over an unfocused pane focuses it (AC-022-02, AC-033-01)", () => {
  const left = () => pane({ pane_id: "w1:p1", inner_rect: { x: 0, y: 0, width: 4, height: 3 }, focused: true });
  const right = (focused = false) =>
    pane({
      pane_id: "w1:p2",
      inner_rect: { x: 4, y: 0, width: 4, height: 3 },
      focused,
      mouse_reporting: false,
      scroll: { offset_from_bottom: 1, max_offset_from_bottom: 8, viewport_rows: 3 },
    });

  // Would catch: a click that does not ask pane.focus, or the next wheel still targeting the old
  // pane / discarded until a second click.
  it("click focuses once; the next wheel after metadata confirmation scrolls the new pane", () => {
    const view = mountView();
    live(view, [left(), right(false)]);

    view.target().dispatchEvent(
      new PointerEvent("pointerdown", { button: 0, clientX: 45, clientY: 20, bubbles: true }),
    );
    expect(view.focuses).toEqual([{ pane_id: "w1:p2", surface_revision: 10 }]);

    view.push(metadata(10, [{ ...left(), focused: false }, right(true)]));
    flushFrames();
    view.sizeCanvas();
    view.inputs.length = 0;
    view.scrolls.length = 0;
    view.focuses.length = 0;

    wheel(view.container(), { deltaY: -1, deltaMode: 1, clientX: 45, clientY: 31 });
    expect(view.inputs).toEqual([
      { kind: "mouse", action: "scroll_up", column: 0, row: 1, modifiers: 0, lines: 3 },
    ]);
    expect(view.focuses).toEqual([]);
  });

  // Spec 033 AC-033-01: wheel over an unfocused pane sends pane.focus once; the notches received
  // while the focus is pending are accumulated and delivered to that pane as one mouse input when
  // the metadata confirms the focus. Would catch: the spec 022 rule (pane.scroll without focus),
  // notches sent to the previously focused pane, sent before the confirmation, dropped, or a
  // second pane.focus request; the second gesture over the already focused pane needs no focus.
  it("wheel over an unfocused pane focuses it once and delivers the accumulated notches", () => {
    const view = mountView();
    live(view, [left(), right(false)]);

    wheel(view.container(), { deltaY: -1, deltaMode: 1, clientX: 45, clientY: 31 });
    wheel(view.container(), { deltaY: -1, deltaMode: 1, clientX: 45, clientY: 31 });
    expect(view.focuses).toEqual([{ pane_id: "w1:p2", surface_revision: 10 }]);
    expect(view.inputs).toEqual([]);
    expect(view.scrolls).toEqual([]);

    view.push(metadata(10, [{ ...left(), focused: false }, right(true)]));
    flushFrames();
    view.sizeCanvas();
    expect(view.inputs).toEqual([
      { kind: "mouse", action: "scroll_up", column: 0, row: 1, modifiers: 0, lines: 6 },
    ]);
    expect(view.scrolls).toEqual([]);

    view.inputs.length = 0;
    view.focuses.length = 0;
    wheel(view.container(), { deltaY: 1, deltaMode: 1, clientX: 45, clientY: 31 });
    expect(view.focuses).toEqual([]);
    expect(view.inputs).toEqual([
      { kind: "mouse", action: "scroll_down", column: 0, row: 1, modifiers: 0, lines: 3 },
    ]);
  });
});

describe("Ctrl+End returns to the bottom (AC-022-03)", () => {
  // Would catch: Ctrl+End forwarded to the PTY while the pane is scrolled away from the bottom.
  it("Ctrl+End asks pane.scroll with offset_from_bottom 0 when the pane is scrolled", () => {
    const view = mountView();
    live(view, [pane({ scroll: { offset_from_bottom: 6, max_offset_from_bottom: 10, viewport_rows: 3 } })]);
    view.target().dispatchEvent(
      new KeyboardEvent("keydown", { key: "End", ctrlKey: true, bubbles: true, cancelable: true }),
    );
    expect(view.scrolls).toEqual([{ pane_id: "w1:p1", offset_from_bottom: 0 }]);
    expect(view.inputs).toEqual([]);
  });
});
