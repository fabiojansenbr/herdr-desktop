// @vitest-environment happy-dom
// Spec 021 — partial repaint must not wipe the terminal (AC-021-02, AC-021-03).
// Would catch: paint filling the whole canvas on hover/wheel, or getContext("2d") asked
// more than once / without { alpha: false } on the product canvas.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_THEME } from "./colors";
import { HostFrameCache } from "./frame-cache";
import * as renderer from "./renderer";
import TerminalView from "./TerminalView.svelte";
import type { FrameEvent, InputDto, PaneMeta } from "./types";

class FakeCtx {
  fillStyle: string | CanvasGradient | CanvasPattern = "";
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  globalAlpha = 1;
  rects: { x: number; y: number; w: number; h: number; color: string }[] = [];
  texts: { text: string; x: number; y: number; color: string }[] = [];
  fillRect(x: number, y: number, w: number, h: number) {
    this.rects.push({ x, y, w, h, color: String(this.fillStyle) });
  }
  fillText(text: string, x: number, y: number) {
    this.texts.push({ text, x, y, color: String(this.fillStyle) });
  }
  setTransform() {}
  measureText() {
    return { width: 10 };
  }
}

type GetContextCall = { canvas: HTMLCanvasElement; type: string; attrs: unknown };
const getContextCalls: GetContextCall[] = [];
const contexts = new WeakMap<HTMLCanvasElement, FakeCtx>();
const originalGetContext = HTMLCanvasElement.prototype.getContext;

function installCanvasMock() {
  getContextCalls.length = 0;
  HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, type: string, attrs?: unknown) {
    getContextCalls.push({ canvas: this, type, attrs });
    const existing = contexts.get(this);
    if (existing) return existing as unknown as CanvasRenderingContext2D;
    const ctx = new FakeCtx();
    contexts.set(this, ctx);
    return ctx as unknown as CanvasRenderingContext2D;
  } as typeof HTMLCanvasElement.prototype.getContext;
}

const rafQueue: { id: number; cb: FrameRequestCallback }[] = [];
let rafNext = 1;

function flushFrames() {
  const batch = rafQueue.splice(0);
  for (const item of batch) item.cb(0);
}

function ctxOf(canvas: HTMLCanvasElement): FakeCtx {
  const ctx = contexts.get(canvas);
  if (!ctx) throw new Error("main canvas has no fake context");
  return ctx;
}

function fullFill(r: { x: number; y: number; w: number; h: number }, w: number, h: number): boolean {
  return r.x === 0 && r.y === 0 && r.w === w && r.h === h;
}

function fullFrame(revision: number, height: number, glyph: (y: number) => string): FrameEvent {
  const width = 8;
  const cells = [];
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) cells.push({ s: x === 0 ? glyph(y) : " ", fg: 0, bg: 0, m: 0 });
  }
  return {
    type: "full",
    revision,
    width,
    height,
    cells,
    cursor: { x: 0, y: 1, visible: false, shape: 2 },
    panes: [{ pane_id: "p1", x: 0, y: 0, width, height, focused: true }],
  };
}

function metadata(revision: number, height: number): FrameEvent {
  const pane: PaneMeta = {
    pane_id: "p1",
    content_revision: 1,
    rect: { x: 0, y: 0, width: 8, height },
    inner_rect: { x: 0, y: 0, width: 8, height },
    scroll: { offset_from_bottom: 0, max_offset_from_bottom: 40, viewport_rows: height },
    focused: true,
    mouse_reporting: false,
    sgr_pixel_mouse: false,
    alternate_screen_active: false,
    pixel_width: 80,
    pixel_height: height * 26,
  };
  return { type: "metadata", revision, panes: [pane], hyperlinks: [] };
}

const hosts: { el: HTMLElement; app: ReturnType<typeof mount> }[] = [];

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

function mountView(
  theme: typeof DEFAULT_THEME = DEFAULT_THEME,
  options: {
    endpoint?: string | null;
    hostLabel?: string;
    frameCache?: HostFrameCache;
    inputEnabled?: () => boolean;
  } = {},
) {
  let push: (event: FrameEvent) => void = () => {};
  const inputs: InputDto[] = [];
  const el = document.createElement("div");
  document.body.appendChild(el);
  const app = mount(TerminalView, {
    target: el,
    props: {
      endpoint: options.endpoint,
      hostLabel: options.hostLabel,
      frameCache: options.frameCache,
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
      onScroll: () => {},
      fontSize: 20,
      theme,
      get inputEnabled() {
        return options.inputEnabled ? options.inputEnabled() : true;
      },
    },
  });
  hosts.push({ el, app });
  flushSync();
  return {
    el,
    app: app as unknown as { switchHost: (ep: string | null) => void; hasCachedFrame: (ep: string) => boolean },
    push: (event: FrameEvent) => push(event),
    inputs,
    canvas: () => {
      const canvas = el.querySelector("canvas");
      if (!canvas) throw new Error("no canvas");
      return canvas;
    },
    target: () => {
      const ta = el.querySelector<HTMLTextAreaElement>("textarea.ime-target");
      if (!ta) throw new Error("no ime target");
      return ta;
    },
  };
}

describe("TerminalView partial repaint (AC-021-02)", () => {
  // Would catch: pointermove/wheel (or a later patch) filling the whole bitmap so rows
  // that were not dirty disappear — the hover/scroll wipe in design/bug-repaint-*.png.
  it("pointermove and wheel after a Full do not fill the canvas or redraw clean rows", () => {
    const view = mountView();
    view.push(fullFrame(10, 3, (y) => String(y)));
    view.push(metadata(10, 3));
    flushFrames();

    const canvas = view.canvas();
    const ctx = ctxOf(canvas);
    const cellH = 26;
    const bitmapW = 8 * 10;
    const bitmapH = 3 * cellH;
    expect(ctx.texts.map((t) => t.text).filter((t) => t !== " ")).toEqual(expect.arrayContaining(["0", "1", "2"]));
    const fillsAfterFull = ctx.rects.filter((r) => fullFill(r, bitmapW, bitmapH)).length;
    expect(fillsAfterFull).toBeGreaterThanOrEqual(1);

    const rectsAfterFull = ctx.rects.length;
    const textsAfterFull = ctx.texts.length;

    canvas.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: bitmapW, height: bitmapH, right: bitmapW, bottom: bitmapH, x: 0, y: 0, toJSON() {} }) as DOMRect;

    const target = view.target();
    target.dispatchEvent(new PointerEvent("pointermove", { clientX: 5, clientY: 5, bubbles: true }));
    target.dispatchEvent(new PointerEvent("pointerenter", { clientX: 5, clientY: 5, bubbles: true }));
    target.dispatchEvent(new PointerEvent("pointerleave", { clientX: 5, clientY: 5, bubbles: true }));
    target.dispatchEvent(new WheelEvent("wheel", { deltaY: 40, deltaMode: 0, bubbles: true, cancelable: true }));
    flushFrames();

    view.push({
      type: "patch",
      revision: 11,
      rows: [{ x: 0, y: 1, cells: [{ s: "Z", fg: 0, bg: 0, m: 0 }] }],
      cursor: { x: 0, y: 1, visible: false, shape: 2 },
    });
    flushFrames();

    const extra = ctx.rects.slice(rectsAfterFull);
    expect(extra.some((r) => fullFill(r, bitmapW, bitmapH))).toBe(false);
    const top = cellH;
    const bottom = cellH * 2;
    for (const r of extra) {
      expect(r.y).toBeGreaterThanOrEqual(top);
      expect(r.y + r.h).toBeLessThanOrEqual(bottom);
    }
    const newTexts = ctx.texts.slice(textsAfterFull);
    expect(newTexts.some((t) => t.text === "Z")).toBe(true);
    expect(newTexts.some((t) => t.text === "0" || t.text === "2")).toBe(false);
  });

  // Would catch: a scroll Full painting only the new dirty subset (old rows gone) or
  // filling the bitmap without marking every row.
  it("a scroll Full paints every row", () => {
    const view = mountView();
    view.push(fullFrame(10, 3, (y) => String(y)));
    view.push(metadata(10, 3));
    flushFrames();
    const ctx = ctxOf(view.canvas());
    ctx.texts.length = 0;
    ctx.rects.length = 0;

    view.target().dispatchEvent(new WheelEvent("wheel", { deltaY: 80, deltaMode: 0, bubbles: true, cancelable: true }));
    view.push(fullFrame(20, 3, (y) => ["A", "B", "C"][y]!));
    view.push(metadata(20, 3));
    flushFrames();

    const glyphs = ctx.texts.map((t) => t.text).filter((t) => t !== " ");
    expect(glyphs).toEqual(expect.arrayContaining(["A", "B", "C"]));
    const bitmapW = 80;
    const bitmapH = 3 * 26;
    expect(ctx.rects.some((r) => fullFill(r, bitmapW, bitmapH))).toBe(false);
    expect(ctx.rects.filter((r) => r.x === 0 && r.w === bitmapW && r.h === 26).map((r) => r.y)).toEqual([0, 26, 52]);
  });
});

describe("TerminalView canvas context (AC-021-03)", () => {
  // Would catch: getContext on the product canvas asked twice (resize/theme) or without
  // { alpha: false }, so WebKitGTK would composite the wallpaper through the bitmap.
  it("creates the main 2d context once with { alpha: false }", () => {
    const view = mountView();
    view.push(fullFrame(10, 3, (y) => String(y)));
    flushFrames();
    const canvas = view.canvas();
    view.push(fullFrame(11, 5, (y) => String(y)));
    flushFrames();

    const mainCalls = getContextCalls.filter((c) => c.canvas === canvas);
    expect(mainCalls).toHaveLength(1);
    expect(mainCalls[0]).toMatchObject({ type: "2d", attrs: { alpha: false } });
    expect(canvas.getContext("2d")).toBe(ctxOf(canvas));
  });
});

describe("Instant host switch (AC-037-01, AC-037-02)", () => {
  it("sair de A e voltar -> paintAll com o quadro em cache antes de qualquer evento do backend; Full novo substitui; input bloqueado até lá (AC-037-01)", () => {
    const cache = new HostFrameCache();
    let currentInput = true;
    const view = mountView(DEFAULT_THEME, {
      endpoint: "local",
      frameCache: cache,
      inputEnabled: () => currentInput,
    });

    const canvas = view.canvas();
    const ctx = ctxOf(canvas);

    // 1. Host A is visited and live
    view.push({
      type: "identity",
      boot_id: "boot-local",
      connection_generation: 1,
      generation: 1,
      server_version: "0.9.0",
      pane_id: "p1",
    });
    view.push(fullFrame(1, 3, (y) => (y === 0 ? "A" : " ")));
    view.push(metadata(1, 3));
    flushFrames();

    // Check that host A painted "A"
    expect(ctx.texts.map((t) => t.text).filter((t) => t !== " ")).toContain("A");

    // 2. Switch to host B (ssh-dev)
    const paintAllSpy = vi.spyOn(renderer, "paintAll");
    paintAllSpy.mockClear();

    view.app.switchHost("ssh-dev");
    flushSync();

    // Host A's frame is now cached in cache
    expect(cache.has("local")).toBe(true);

    // Host B receives identity and full frame
    view.push({
      type: "identity",
      boot_id: "boot-ssh",
      connection_generation: 1,
      generation: 1,
      server_version: "0.9.0",
      pane_id: "p1",
    });
    view.push(fullFrame(10, 3, (y) => (y === 0 ? "B" : " ")));
    view.push(metadata(10, 3));
    flushFrames();
    expect(ctx.texts.map((t) => t.text).filter((t) => t !== " ")).toContain("B");

    // 3. Switch back to host A (local)
    // BEFORE any backend event for local:
    paintAllSpy.mockClear();
    ctx.texts.length = 0;
    currentInput = false; // controller blocks input during switch

    view.app.switchHost("local");
    flushSync();

    // paintAll MUST have been executed with the cached frame before any event from backend!
    expect(paintAllSpy).toHaveBeenCalled();
    expect(ctx.texts.map((t) => t.text).filter((t) => t !== " ")).toContain("A");

    // Input MUST be blocked while showing cached frame (the cached frame is só imagem)
    const target = view.target();
    target.dispatchEvent(new KeyboardEvent("keydown", { key: "x", bubbles: true }));
    expect(view.inputs).toHaveLength(0);

    // Mouse click on cached frame does not request focus or send input
    target.dispatchEvent(new PointerEvent("pointerdown", { clientX: 10, clientY: 10, button: 0, bubbles: true }));
    expect(view.inputs).toHaveLength(0);
    expect(view.el.querySelector(".actions")).toBeNull(); // no visible warning

    // 4. Now backend delivers the new Full frame for host A
    paintAllSpy.mockClear();
    view.push({
      type: "identity",
      boot_id: "boot-local",
      connection_generation: 1,
      generation: 1,
      server_version: "0.9.0",
      pane_id: "p1",
    });
    view.push(fullFrame(2, 3, (y) => (y === 0 ? "N" : " ")));
    view.push(metadata(2, 3));
    flushFrames();

    // New Full replaces the cached frame
    expect(ctx.texts.map((t) => t.text).filter((t) => t !== " ")).toContain("N");

    // Now input is liberated once controller enables it
    currentInput = true;
    flushSync();
    target.dispatchEvent(new KeyboardEvent("keydown", { key: "z", bubbles: true }));
    expect(view.inputs.length).toBeGreaterThan(0);
  });

  it("host Online sem quadro em cache na primeira visita mostra fundo e após 400ms indicador discreto Carregando (AC-037-02)", () => {
    vi.useFakeTimers();
    try {
      const cache = new HostFrameCache();
      const view = mountView(DEFAULT_THEME, {
        endpoint: null,
        hostLabel: "mac-mini",
        frameCache: cache,
      });

      const canvas = view.canvas();
      const ctx = ctxOf(canvas);

      // Switch to an unvisited host
      view.app.switchHost("mac-mini");
      flushSync();

      // Shows theme background (fillRect with theme background)
      expect(ctx.rects.some((r) => r.color === DEFAULT_THEME.background)).toBe(true);

      // Before 400ms: no loading indicator yet
      vi.advanceTimersByTime(200);
      flushSync();
      expect(view.el.querySelector(".loading-indicator")).toBeNull();

      // After 400ms: discreet indicator appears in the corner
      vi.advanceTimersByTime(250);
      flushSync();
      const indicator = view.el.querySelector(".loading-indicator");
      expect(indicator).not.toBeNull();
      expect(indicator?.textContent).toContain("Carregando mac-mini…");

      // When full frame arrives: indicator disappears
      view.push(fullFrame(1, 3, (y) => String(y)));
      flushFrames();
      flushSync();
      expect(view.el.querySelector(".loading-indicator")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("descarta cache quando a conexão cai ou a geometria muda (AC-037-01)", () => {
    const cache = new HostFrameCache();
    const view = mountView(DEFAULT_THEME, {
      endpoint: "local",
      frameCache: cache,
    });

    // Populate local
    view.push({
      type: "identity",
      boot_id: "boot-local",
      connection_generation: 1,
      generation: 1,
      server_version: "0.9.0",
      pane_id: "p1",
    });
    view.push(fullFrame(1, 3, (y) => (y === 0 ? "A" : " ")));
    flushFrames();

    // Switch to remote -> saves local to cache
    view.app.switchHost("remote");
    flushSync();
    expect(cache.has("local")).toBe(true);

    // Connection for local drops -> cache for local is discarded
    cache.drop("local");
    expect(cache.has("local")).toBe(false);

    // Switching back to local: no cache available
    const paintAllSpy = vi.spyOn(renderer, "paintAll");
    paintAllSpy.mockClear();
    view.app.switchHost("local");
    flushSync();
    expect(cache.has("local")).toBe(false);
  });
});

