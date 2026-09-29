<script lang="ts">
  // One pane surface on a canvas. Owns: cell metrics, ResizeObserver → resize, keyboard/
  // paste/IME → input, dirty-row repaints from FrameEvents. Never repaints while hidden.
  // The focus target is a real editable textarea: WebKitGTK only enables input methods for
  // editable elements, and InputRouter keeps preedit local and every event delivered once.
  import { onDestroy, onMount } from "svelte";
  import { t } from "../i18n/index.svelte";
  import { defaultFrameCache, HostFrameCache } from "./frame-cache";
  import { TerminalGrid } from "./grid";
  import { exitChordAllowed, InputRouter, nativePasteRoute } from "./composition";
  import * as renderer from "./renderer";
  import type { CellMetrics, DrawBounds, RowDecor } from "./renderer";
  import { DEFAULT_THEME, type Theme } from "./colors";
  import { bandOffset } from "./pane-band";
  import { probedFullFrame, probedMetadata, probedPointerRoute, probedScheduler, terminalProbe, type PointerExit } from "./probe";
  import { modifierBits } from "./input";
  import {
    AppMouseGesture,
    FocusGate,
    PendingWheel,
    SurfaceMeta,
    TerminalSelection,
    cellFromPoint,
    geometryChanged,
    hitPane,
    isCopyChord,
    linkAt,
    mouseInput,
    rightClickRoute,
    routePointer,
    rowDecor,
    wheelRoute,
    type LinkHit,
    type PaneHit,
  } from "./interaction";
  import { readRightClickPassthrough } from "../components/center/pane-menu";
  import { chordName, contextMenuLine, pasteLine, pasteResultLine, rightClickLine, targetName, uiTrace } from "../shell/ui-trace";
  import type { PaneMeta } from "./types";
  import type {
    ActionResult,
    FrameEvent,
    GeometryDto,
    InputDto,
    LinkRequest,
    MouseButton,
    PaneFocusRequest,
    ScrollRequest,
    SelectionRequest,
  } from "./types";

  type Action<T> = (request: T) => ActionResult | void | Promise<ActionResult | void>;

  interface Props {
    /** Receives frame events pushed by the host; returns an unsubscribe. */
    subscribe: (handler: (event: FrameEvent) => void) => () => void;
    onInput: (events: InputDto[]) => void | Promise<void>;
    onResize: (geometry: GeometryDto) => void | Promise<void>;
    onFocus?: (focused: boolean) => void;
    visible?: boolean;
    inputEnabled?: boolean;
    fontFamily?: string;
    fontSize?: number;
    /** Renderer palette; defaults to `DEFAULT_THEME`. A change repaints dirty rows locally (no Full). */
    theme?: Theme;
    /** Click on a pane that is not focused: ask the engine (`pane.focus`); input waits for the metadata confirmation. */
    onSelectPane?: Action<PaneFocusRequest>;
    /** Shift+wheel over any pane: `pane.scroll` of that pane, without changing the focus. */
    onScroll?: Action<ScrollRequest>;
    /** Ctrl+Shift+C / copy button: `pane.selection.read` + clipboard write by the host. */
    onCopySelection?: Action<SelectionRequest>;
    /**
     * Ctrl+Shift+V: the host reads the local clipboard and pastes once. WebKitGTK emits no
     * ClipboardEvent for this chord; without the callback the view shows a notice (nothing is
     * fabricated and nothing is read in the WebView).
     */
    onNativePaste?: () => void | Promise<unknown>;
    /** Ctrl+click / open button on a safe web link: opened by the desktop host, never the remote. */
    onOpenLink?: Action<LinkRequest>;
    /** Ctrl+Shift+F6: leave the terminal; default moves focus to the end-of-terminal marker. */
    onFocusExit?: () => void;
    /** Current endpoint selected in the Workbench (spec 037). */
    endpoint?: string | null;
    /** Display label of the current host. */
    hostLabel?: string;
    /** Frame cache by host (defaults to window-wide defaultFrameCache). */
    frameCache?: HostFrameCache;
  }

  let {
    subscribe,
    onInput,
    onResize,
    onFocus,
    visible = true,
    inputEnabled = true,
    fontFamily = "'JetBrains Mono', 'Fira Code', 'DejaVu Sans Mono', monospace",
    fontSize = 14,
    theme = DEFAULT_THEME,
    onSelectPane,
    onScroll,
    onCopySelection,
    onNativePaste,
    onOpenLink,
    onFocusExit,
    endpoint = null,
    hostLabel,
    frameCache = defaultFrameCache,
  }: Props = $props();

  let container: HTMLDivElement;
  let canvas: HTMLCanvasElement;
  let extCanvas: HTMLCanvasElement;
  let imeTarget: HTMLTextAreaElement;
  const router = new InputRouter();
  let preedit = $state("");
  let ctx: CanvasRenderingContext2D | null = null;
  /** Extension layer: the ring around the grid (container remainder + CSS padding), painted from
   *  the edge cells so an app with its own background shows no frame of the theme colour. */
  let extCtx: CanvasRenderingContext2D | null = null;
  let extBounds: DrawBounds | null = null;
  const grid = new TerminalGrid();
  let metrics: CellMetrics = { cellWidth: 9, cellHeight: 18, baseline: 14, font: "14px monospace" };
  let dpr = 1;
  let lastGeometry: GeometryDto | null = null;
  let paintedRows = $state(0);
  const meta = new SurfaceMeta();
  const focusGate = new FocusGate();
  // Spec 033: notches of a wheel that arrived before the focus it asked for was confirmed.
  const pendingWheel = new PendingWheel();
  const selection = new TerminalSelection();
  let exitMarker: HTMLSpanElement;
  let pendingFocus = $state<string | null>(null);
  let selectionRows = $state(0);
  let link = $state<LinkHit | null>(null);
  let notice = $state("");
  /** Bumped on every accepted metadata/full frame: the toolbar's band follows the layout. */
  let layout = $state(0);
  // Application gesture: bound to the captured pane/identity, cancelled when either changes.
  const appGesture = new AppMouseGesture();
  let selectGesture: { pane: PaneMeta; cell: string } | null = null;
  let hoverCell = "";
  /** Connection identity of the frames (boot/connection generation/generation). */
  let identity = "";
  let currentEndpoint = $state<string | null>(null);
  let previousEndpointProp: string | null | undefined = undefined;
  let isCached = $state(false);
  let loadingIndicator = $state<string | null>(null);
  let loadingTimer: ReturnType<typeof setTimeout> | null = null;
  let currentIdentity: { boot_id: string; connection_generation: number } | null = null;
  // Opt-in bench probe (null in the product: raw frame request, nothing counted).
  const repaintProbe = terminalProbe();
  /** Next paint fills the bitmap and draws every row (create, resize, theme). */
  let fullPaint = false;
  const scheduler = probedScheduler(
    repaintProbe,
    (cb) => requestAnimationFrame(cb),
    (h) => cancelAnimationFrame(h),
    (rows) => {
      if (!ctx) return 0;
      const drawn = fullPaint
        ? renderer.paintAll(ctx, grid, metrics, visible, theme, decorFor)
        : renderer.paint(ctx, grid, rows, metrics, visible, theme, decorFor);
      // Same dirty set on the extension layer: it never redraws the cells, only the ring.
      if (extCtx && extBounds) renderer.paintExtensions(extCtx, grid, rows, metrics, visible, theme, extBounds, fullPaint);
      if (visible) fullPaint = false;
      paintedRows += drawn;
      return drawn;
    },
  );

  /**
   * Band of the pane the toolbar talks about (link, selection or focused pane): the row above that
   * pane's `inner_rect`, the same band the center region's pane frames use. Over a content cell the
   * bar would swallow clicks meant for the terminal (spec 010 r4), so it never sits there; with no
   * metadata yet there is no band and the bar keeps its corner.
   */
  const toolbarBand = $derived.by(() => {
    void layout;
    const paneId = link?.request.pane_id ?? selection.paneId ?? pendingFocus ?? meta.focused()?.pane_id ?? null;
    // A link or selection captured on an older layout names a pane this metadata no longer has:
    // the bar then follows the focused pane's band. It never falls back to the corner over row 0
    // while the layout is known (spec 013, gate r7: the bar was measured over the content cells).
    const pane = (paneId ? meta.pane(paneId) : undefined) ?? meta.focused();
    if (!pane) return null;
    const band = bandOffset(pane.inner_rect, { cellWidth: metrics.cellWidth, cellHeight: metrics.cellHeight });
    return `left:${band.left}px;top:${band.top}px;width:${band.width}px;height:${band.height}px`;
  });

  /** Rows painted so far (harness/diagnostics). */
  export function getPaintedRows(): number {
    return paintedRows;
  }

  function measure() {
    const font = `${fontSize}px ${fontFamily}`;
    const probe = document.createElement("canvas").getContext("2d");
    if (!probe) return;
    probe.font = font;
    const m = probe.measureText("M");
    const cellWidth = Math.ceil(m.width);
    const cellHeight = Math.ceil(fontSize * 1.3);
    metrics = { cellWidth, cellHeight, baseline: Math.round(fontSize), font };
  }

  function geometryFor(width: number, height: number): GeometryDto {
    return {
      cols: Math.max(2, Math.floor(width / metrics.cellWidth)),
      rows: Math.max(1, Math.floor(height / metrics.cellHeight)),
      cell_width_px: Math.round(metrics.cellWidth * dpr),
      cell_height_px: Math.round(metrics.cellHeight * dpr),
    };
  }

  export function currentGeometry(): GeometryDto {
    const rect = container?.getBoundingClientRect();
    return geometryFor(rect?.width ?? 720, rect?.height ?? 432);
  }

  function ensureCanvasContext(): CanvasRenderingContext2D | null {
    if (!canvas) return null;
    if (!ctx) ctx = canvas.getContext("2d", { alpha: false });
    return ctx;
  }

  function clearLoadingTimer() {
    if (loadingTimer !== null) {
      clearTimeout(loadingTimer);
      loadingTimer = null;
    }
  }

  function saveCurrentFrame() {
    if (!currentEndpoint || !grid.hasSurface || !currentIdentity) return;
    frameCache.set({
      endpoint: currentEndpoint,
      boot_id: currentIdentity.boot_id,
      connection_generation: currentIdentity.connection_generation,
      width: grid.width,
      height: grid.height,
      cells: grid.cells.slice(),
      cursor: grid.cursor ? { ...grid.cursor } : null,
      panes: meta.panes.map((p) => ({ ...p, rect: { ...p.rect }, inner_rect: { ...p.inner_rect } })),
      links: meta.links.slice(),
      revision: grid.revision,
    });
  }

  function clearGrid() {
    grid.width = 0;
    grid.height = 0;
    grid.cells = [];
    grid.cursor = null;
    grid.panes = [];
    grid.revision = 0;
    meta.invalidate();
    meta.panes = [];
    selection.clear();
    link = null;
    notice = "";
  }

  export function hasCachedFrame(ep: string): boolean {
    return frameCache.has(ep);
  }

  export function switchHost(nextEndpoint: string | null) {
    if (nextEndpoint === currentEndpoint) return;
    previousEndpointProp = nextEndpoint;

    saveCurrentFrame();
    clearLoadingTimer();
    loadingIndicator = null;

    currentEndpoint = nextEndpoint;
    currentIdentity = null;

    if (!nextEndpoint) {
      isCached = false;
      clearGrid();
      return;
    }

    const cached = frameCache.get(nextEndpoint);
    if (cached) {
      grid.width = cached.width;
      grid.height = cached.height;
      grid.revision = cached.revision;
      grid.cells = cached.cells.slice();
      grid.cursor = cached.cursor ? { ...cached.cursor } : null;
      grid.panes = cached.panes.map((p) => ({
        pane_id: p.pane_id,
        x: p.rect.x,
        y: p.rect.y,
        width: p.rect.width,
        height: p.rect.height,
        focused: p.focused,
      }));
      meta.panes = cached.panes.map((p) => ({ ...p, rect: { ...p.rect }, inner_rect: { ...p.inner_rect } }));
      meta.links = cached.links.slice();
      meta.revision = cached.revision;
      meta.stale = true;

      isCached = true;
      fullPaint = true;

      ensureCanvasContext();
      sizeCanvas();
      if (ctx) {
        renderer.paintAll(ctx, grid, metrics, visible, theme, decorFor);
        if (extCtx && extBounds) {
          renderer.paintExtensions(extCtx, grid, allRows(), metrics, visible, theme, extBounds, true);
        }
        fullPaint = false;
      }
    } else {
      isCached = false;
      clearGrid();

      ensureCanvasContext();
      if (canvas && ctx) {
        const rect = container?.getBoundingClientRect();
        const w = (rect && rect.width > 0 ? rect.width : canvas.width) || 720;
        const h = (rect && rect.height > 0 ? rect.height : canvas.height) || 432;
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(h * dpr);
        canvas.style.width = `${w}px`;
        canvas.style.height = `${h}px`;
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx.fillStyle = theme.background;
        ctx.fillRect(0, 0, w, h);
      }

      const label = hostLabel ?? nextEndpoint;
      loadingTimer = setTimeout(() => {
        loadingIndicator = t("terminal.loading", { host: label });
      }, 400);
    }
  }

  function allRows(): number[] {
    return Array.from({ length: grid.height }, (_, i) => i);
  }

  function sizeCanvas() {
    if (!canvas || !grid.hasSurface) return;
    const w = grid.width * metrics.cellWidth;
    const h = grid.height * metrics.cellHeight;
    const nextW = Math.round(w * dpr);
    const nextH = Math.round(h * dpr);
    const created = ctx === null;
    const resized = canvas.width !== nextW || canvas.height !== nextH;
    if (resized) {
      canvas.width = nextW;
      canvas.height = nextH;
      canvas.style.width = `${w}px`;
      canvas.style.height = `${h}px`;
    }
    if (!ctx) ctx = canvas.getContext("2d", { alpha: false });
    ctx?.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (ctx && (created || resized)) {
      ctx.fillStyle = theme.background;
      ctx.fillRect(0, 0, w, h);
      fullPaint = true;
    }
    sizeExtCanvas();
  }

  /**
   * Area the extension layer covers, in grid coordinates: the container's sub-cell remainder
   * (right/bottom) plus the CSS padding around the terminal — the App terminal wrapper's padding
   * and, per side, the stage padding while the wrapper is flush with it (a banner between the two
   * must never be painted over). `left`/`top` come out negative; a zero ring leaves the grid.
   */
  function ringBounds(): DrawBounds {
    const gridW = grid.width * metrics.cellWidth;
    const gridH = grid.height * metrics.cellHeight;
    const surface: DrawBounds = { left: 0, top: 0, right: gridW, bottom: gridH };
    const rect = container?.getBoundingClientRect();
    const parentRect = container?.parentElement?.getBoundingClientRect() ?? null;
    if (!rect || rect.width <= 0 || rect.height <= 0 || !parentRect || parentRect.width <= 0) return surface;
    const stage = container.closest<HTMLElement>("[data-center-stage]");
    const stageRect = stage?.getBoundingClientRect() ?? null;
    const stageStyle = stage && typeof getComputedStyle === "function" ? getComputedStyle(stage) : null;
    const padding = (side: string): number => {
      const value = stageStyle ? Number.parseFloat(stageStyle.getPropertyValue(`padding-${side}`)) : 0;
      return Number.isFinite(value) && value > 0 ? value : 0;
    };
    // The stage side counts only while it is exactly one padding away from the wrapper: with a
    // banner in between the distance is content, and the ring stops at the wrapper.
    const outer = (parentSide: number, stageSide: number, pad: number): number =>
      stageRect && Math.abs(Math.abs(stageSide - parentSide) - pad) <= 1 ? stageSide : parentSide;
    const outerLeft = stageRect ? outer(parentRect.left, stageRect.left, padding("left")) : parentRect.left;
    const outerTop = stageRect ? outer(parentRect.top, stageRect.top, padding("top")) : parentRect.top;
    const outerRight = stageRect ? outer(parentRect.right, stageRect.right, padding("right")) : parentRect.right;
    const outerBottom = stageRect ? outer(parentRect.bottom, stageRect.bottom, padding("bottom")) : parentRect.bottom;
    return {
      left: Math.min(0, Math.round(outerLeft - rect.left)),
      top: Math.min(0, Math.round(outerTop - rect.top)),
      right: gridW + Math.max(0, Math.round(rect.width - gridW)) + Math.max(0, Math.round(outerRight - rect.right)),
      bottom: gridH + Math.max(0, Math.round(rect.height - gridH)) + Math.max(0, Math.round(outerBottom - rect.bottom)),
    };
  }

  /** Sizes the extension layer around the grid: position, backing store and grid-space transform. */
  function sizeExtCanvas() {
    if (!extCanvas || !grid.hasSurface) return;
    const bounds = ringBounds();
    extBounds = bounds;
    const w = Math.max(0, bounds.right - bounds.left);
    const h = Math.max(0, bounds.bottom - bounds.top);
    const nextW = Math.round(w * dpr);
    const nextH = Math.round(h * dpr);
    const created = extCtx === null;
    const resized = extCanvas.width !== nextW || extCanvas.height !== nextH;
    extCanvas.style.left = `${bounds.left}px`;
    extCanvas.style.top = `${bounds.top}px`;
    extCanvas.style.width = `${w}px`;
    extCanvas.style.height = `${h}px`;
    if (resized) {
      extCanvas.width = nextW;
      extCanvas.height = nextH;
    }
    if (!extCtx) extCtx = extCanvas.getContext("2d", { alpha: false });
    if (!extCtx) return;
    extCtx.setTransform(dpr, 0, 0, dpr, -bounds.left * dpr, -bounds.top * dpr);
    if (created || resized) {
      extCtx.fillStyle = theme.background;
      extCtx.fillRect(bounds.left, bounds.top, w, h);
      fullPaint = true;
    }
  }

  function schedule(rows: Iterable<number>) {
    scheduler.schedule(rows);
  }

  function decorFor(y: number): RowDecor | undefined {
    return rowDecor(meta, selection, grid, y);
  }

  /** Drops an application gesture whose pane/identity/layout is no longer the confirmed target. */
  function revalidateGesture() {
    if (appGesture.active && !appGesture.check(identity, meta, focusedPaneId())) hoverCell = "";
  }

  function repaintSelection(before: number[]) {
    const pane = selection.paneId ? meta.pane(selection.paneId) : undefined;
    const after = pane ? selection.surfaceRows(pane) : [];
    selectionRows = selection.nonEmpty ? after.length : 0;
    schedule([...before, ...after]);
  }

  function selectedRows(): number[] {
    const pane = selection.paneId ? meta.pane(selection.paneId) : undefined;
    return pane ? selection.surfaceRows(pane) : [];
  }

  function inputAllowed(): boolean {
    return !isCached && inputEnabled && focusGate.allowsInput();
  }

  async function runAction<T>(action: Action<T> | undefined, request: T, unavailable: string): Promise<boolean> {
    if (!action) {
      notice = unavailable;
      return false;
    }
    try {
      const result = await action(request);
      if (result && !result.ok) {
        notice = t("terminal.notice.refused", { message: result.message, code: result.code });
        return false;
      }
      return true;
    } catch (error) {
      notice = t("terminal.notice.failed", { reason: error instanceof Error ? error.message : String(error) });
      return false;
    }
  }

  function handleEvent(event: FrameEvent) {
    if (event.type === "identity") {
      currentIdentity = {
        boot_id: event.boot_id,
        connection_generation: event.connection_generation,
      };
      const next = `${event.boot_id}|${event.connection_generation}|${event.generation}`;
      if (next !== identity) {
        identity = next;
        appGesture.cancel();
        pendingWheel.clear();
        hoverCell = "";
      }
      return;
    }
    if (event.type === "state") {
      if (event.state === "disconnected") {
        clearLoadingTimer();
        loadingIndicator = null;
        isCached = false;
        if (currentEndpoint) {
          frameCache.drop(currentEndpoint);
        }
      }
      return;
    }
    if (event.type === "metadata") {
      if (!meta.apply(event, grid.revision)) return;
      probedMetadata(repaintProbe, grid, meta.panes);
      const waiting = focusGate.pending;
      if (focusGate.confirm(meta.panes)) {
        notice = "";
        // Spec 033: the wheel that asked for the focus is delivered now, to that pane, with every
        // accumulated notch (sendMouse rechecks stale metadata/focus).
        if (waiting) {
          const inputs = pendingWheel.take(waiting);
          if (inputs) sendMouse(inputs, waiting);
        }
      }
      pendingFocus = focusGate.pending;
      revalidateGesture();
      if (selection.active) repaintSelection(selectedRows());
      layout += 1;
      if (event.hyperlinks) schedule(allRows());
      return;
    }
    if (event.type !== "full" && event.type !== "patch") return;
    clearLoadingTimer();
    loadingIndicator = null;
    isCached = false;
    const result = grid.apply(event);
    if (!result.applied) return;
    if (result.full) {
      // Layout unknown until the metadata of this full frame: no pointer target is valid.
      meta.invalidate();
      revalidateGesture();
      sizeCanvas();
      layout += 1;
      probedFullFrame(repaintProbe, result);
    }
    schedule(result.dirtyRows);
  }

  function surfaceCell(e: MouseEvent): { x: number; y: number } | null {
    if (!canvas || !grid.hasSurface) return null;
    const rect = canvas.getBoundingClientRect();
    return cellFromPoint(e.clientX - rect.left, e.clientY - rect.top, metrics, grid.width, grid.height);
  }

  function paneLocalPoint(e: MouseEvent, hit: PaneHit): { x: number; y: number } {
    const rect = canvas.getBoundingClientRect();
    return {
      x: e.clientX - rect.left - hit.pane.inner_rect.x * metrics.cellWidth,
      y: e.clientY - rect.top - hit.pane.inner_rect.y * metrics.cellHeight,
    };
  }

  const BUTTONS: Record<number, MouseButton> = { 0: "left", 1: "middle", 2: "right" };

  function mouseMods(e: MouseEvent): number {
    return modifierBits({ key: "", ctrlKey: e.ctrlKey, altKey: e.altKey, shiftKey: e.shiftKey, metaKey: e.metaKey });
  }

  function focusedPaneId(): string | null {
    return focusGate.pending ? null : (meta.focused()?.pane_id ?? null);
  }

  /** Mouse input only for the confirmed focused pane of current metadata (never a stale target). */
  function sendMouse(input: InputDto | InputDto[] | null, paneId: string) {
    if (!input || meta.stale || !inputAllowed() || paneId !== focusedPaneId()) return;
    void onInput(Array.isArray(input) ? input : [input]);
  }

  async function requestFocus(hit: PaneHit) {
    if (!focusGate.request(hit.pane.pane_id)) return;
    pendingFocus = focusGate.pending;
    notice = t("terminal.notice.awaitingFocus", { pane: hit.pane.pane_id });
    const ok = await runAction(onSelectPane, { pane_id: hit.pane.pane_id, surface_revision: grid.revision }, t("terminal.unavailable.selectPane"));
    if (!ok) {
      focusGate.fail(hit.pane.pane_id);
      pendingWheel.clear();
      pendingFocus = focusGate.pending;
    }
  }

  /** Opt-in probe of the pointerdown exit (null probe: returns before building anything). */
  function tracePointer(e: PointerEvent, exit: PointerExit, cell: { x: number; y: number } | null, hit: PaneHit | null, route: string | null, found?: LinkHit | null) {
    if (!repaintProbe) return;
    probedPointerRoute(repaintProbe, {
      exit,
      cell,
      hit_pane: hit?.pane.pane_id ?? null,
      button: BUTTONS[e.button] ?? null,
      route,
      mouse_reporting: hit ? hit.pane.mouse_reporting : null,
      stale: meta.stale,
      input_allowed: inputAllowed(),
      link_found: found === undefined ? null : found !== null,
      link_safe: found ? found.safe : null,
      ctrl: e.ctrlKey,
    });
  }

  function onPointerDown(e: PointerEvent) {
    if (isCached) {
      e.preventDefault();
      tracePointer(e, "app_blocked", null, null, null);
      return;
    }
    const cell = surfaceCell(e);
    const hit = cell ? hitPane(meta.panes, cell.x, cell.y) : null;
    const button = BUTTONS[e.button];
    if (!cell || !hit || !button) {
      tracePointer(e, !cell ? "no_cell" : !hit ? "no_hit" : "no_button", cell, hit, null);
      return;
    }
    const route = routePointer(hit, { shift: e.shiftKey }, focusedPaneId());
    e.preventDefault();
    imeTarget?.focus();
    if (route === "focus") {
      tracePointer(e, "focus", cell, hit, route);
      if (button === "left") void requestFocus(hit);
      return;
    }
    const key = `${cell.x},${cell.y}`;
    if (route === "app") {
      // Spec 028 AC-028-03: a right click only reaches the app with the per-pane passthrough on
      // (and no Shift); otherwise it stays for the Herdr pane menu (contextmenu handler).
      if (
        button === "right" &&
        rightClickRoute(hit, { shift: e.shiftKey }, { focusedPaneId: focusedPaneId(), passthrough: readRightClickPassthrough(hit.pane.pane_id) }) !== "app"
      ) {
        tracePointer(e, "app_blocked", cell, hit, route);
        return;
      }
      if (meta.stale || !inputAllowed()) {
        tracePointer(e, "app_blocked", cell, hit, route);
        return;
      }
      tracePointer(e, "app", cell, hit, route);
      imeTarget?.setPointerCapture?.(e.pointerId);
      sendMouse(appGesture.start(hit, button, mouseMods(e), paneLocalPoint(e, hit), identity, metrics), hit.pane.pane_id);
      return;
    }
    if (button !== "left") {
      tracePointer(e, "non_left", cell, hit, route);
      return;
    }
    const found = linkAt(grid, meta, cell.x, cell.y);
    if (e.ctrlKey && found) {
      tracePointer(e, found.safe ? "link_open" : "link_unsafe", cell, hit, route, found);
      link = found;
      if (found.safe) void runAction(onOpenLink, found.request, t("terminal.unavailable.openLink"));
      else notice = t("terminal.notice.linkBlocked");
      return;
    }
    const before = selectedRows();
    selection.start(hit, hit.pane);
    selectGesture = { pane: hit.pane, cell: key };
    imeTarget?.setPointerCapture?.(e.pointerId);
    repaintSelection(before);
    tracePointer(e, "select", cell, hit, route, found);
  }

  function onPointerMove(e: PointerEvent) {
    if (isCached) return;
    const cell = surfaceCell(e);
    if (!cell) return;
    const key = `${cell.x},${cell.y}`;
    if (appGesture.active) {
      const hit = hitPane(meta.panes, cell.x, cell.y);
      const pane = hit?.pane.pane_id ?? "";
      sendMouse(appGesture.drag(hit, mouseMods(e), hit ? paneLocalPoint(e, hit) : null, identity, meta, focusedPaneId(), metrics), pane);
      return;
    }
    if (selectGesture) {
      if (key === selectGesture.cell) return;
      selectGesture.cell = key;
      const pane = meta.pane(selectGesture.pane.pane_id) ?? selectGesture.pane;
      const r = pane.inner_rect;
      const x = Math.min(r.x + r.width - 1, Math.max(r.x, cell.x));
      const y = Math.min(r.y + r.height - 1, Math.max(r.y, cell.y));
      const hit = hitPane([pane], x, y);
      if (!hit) return;
      const before = selectedRows();
      selection.extend(hit, hit.pane);
      repaintSelection(before);
      return;
    }
    if (key === hoverCell) return;
    hoverCell = key;
    const hit = hitPane(meta.panes, cell.x, cell.y);
    if (hit && hit.pane.mouse_reporting && !e.shiftKey) {
      sendMouse(mouseInput(hit, "moved", undefined, mouseMods(e), 3, paneLocalPoint(e, hit), metrics), hit.pane.pane_id);
    }
    const found = linkAt(grid, meta, cell.x, cell.y);
    if (found) link = found;
  }

  function onPointerUp(e: PointerEvent) {
    if (isCached) {
      e.preventDefault();
      return;
    }
    if (appGesture.active) {
      imeTarget?.releasePointerCapture?.(e.pointerId);
      e.preventDefault();
      const cell = surfaceCell(e);
      const hit = cell ? hitPane(meta.panes, cell.x, cell.y) : null;
      const focused = focusedPaneId();
      // A cancelled/invalid gesture yields null: no release reaches any pane, nothing is replayed.
      sendMouse(appGesture.release(hit, mouseMods(e), hit ? paneLocalPoint(e, hit) : null, identity, meta, focused, metrics), focused ?? "");
      return;
    }
    if (!selectGesture) return;
    selectGesture = null;
    imeTarget?.releasePointerCapture?.(e.pointerId);
    e.preventDefault();
    if (!selection.nonEmpty) {
      const before = selectedRows();
      selection.clear();
      repaintSelection(before);
    }
  }

  function onWheel(e: WheelEvent) {
    if (isCached) {
      e.preventDefault();
      return;
    }
    const cell = surfaceCell(e);
    const hit = cell ? hitPane(meta.panes, cell.x, cell.y) : null;
    if (!hit) return;
    const route = wheelRoute(hit, focusedPaneId(), { deltaY: e.deltaY, deltaMode: e.deltaMode, shift: e.shiftKey }, metrics);
    if (!route) return;
    e.preventDefault();
    if (route.kind === "input") {
      sendMouse(Array.from({ length: route.repeat }, () => route.input), hit.pane.pane_id);
      return;
    }
    if (route.kind === "focus") {
      // Spec 033 (TUI `mouse.rs:2264-2283`): the wheel over a pane that is not the confirmed
      // focus asks for that pane first; the notches wait here until the metadata confirms the
      // focus (a request already pending for this pane is not repeated).
      const lines = route.input.action === "scroll_up" ? -route.input.lines : route.input.lines;
      pendingWheel.add(hit.pane.pane_id, route.input.column, route.input.row, lines, route.repeat);
      void requestFocus(hit);
      return;
    }
    void runAction(onScroll, route.request, t("terminal.unavailable.scroll"));
  }

  function onFrameScroll(e: Event) {
    const request = (e as CustomEvent<ScrollRequest>).detail;
    if (!request || typeof request.pane_id !== "string" || typeof request.offset_from_bottom !== "number") return;
    void runAction(onScroll, request, t("terminal.unavailable.scroll"));
  }

  function scrollFocusedToBottom(): boolean {
    const focused = meta.focused();
    if (!focused?.scroll || focused.scroll.offset_from_bottom <= 0) return false;
    void runAction(onScroll, { pane_id: focused.pane_id, offset_from_bottom: 0 }, t("terminal.unavailable.scroll"));
    return true;
  }

  /**
   * Spec 028 AC-028-03 — right click on a pane: with `mouse_reporting` and the per-pane
   * passthrough on (and no Shift) the press already went to the app on pointerdown and the
   * browser menu stays suppressed; anything else opens the Herdr pane menu, which the center
   * region renders (`herdr-pane-menu` on the stage). Right clicks outside any pane are left to
   * the tab bar, which owns its own menu.
   * Bound to the surface container, not the IME textarea (r2c, same class as the spec 022 wheel):
   * the textarea does not receive the event over every cell, and the container sees the bubbling
   * clicks of all of them with one listener.
   */
  function onContextMenu(e: MouseEvent) {
    if (isCached) {
      e.preventDefault();
      return;
    }
    const cell = surfaceCell(e);
    const hit = cell ? hitPane(meta.panes, cell.x, cell.y) : null;
    // Debug trail of the user's right click (spec 028 r3): the point, the event target and the
    // pane hit, so a window test shows where the event stopped when no menu appears.
    uiTrace(contextMenuLine({ clientX: e.clientX, clientY: e.clientY, target: targetName(e.target), paneId: hit?.pane.pane_id ?? null }));
    if (!hit) return;
    e.preventDefault();
    const route = rightClickRoute(hit, { shift: e.shiftKey }, {
      focusedPaneId: focusedPaneId(),
      passthrough: readRightClickPassthrough(hit.pane.pane_id),
    });
    uiTrace(rightClickLine(hit.pane.pane_id, route));
    if (route === "app") return;
    (e.currentTarget as HTMLElement | null)?.dispatchEvent(
      new CustomEvent("herdr-pane-menu", {
        bubbles: true,
        composed: true,
        detail: { pane_id: hit.pane.pane_id, client_x: e.clientX, client_y: e.clientY },
      }),
    );
  }

  function copySelection() {
    const pane = selection.paneId ? meta.pane(selection.paneId) : undefined;
    const request = pane ? selection.request(pane) : null;
    if (!request) {
      notice = t("terminal.notice.nothingSelected");
      return;
    }
    void runAction(onCopySelection, request, t("terminal.unavailable.copy")).then((ok) => {
      if (ok) notice = t("terminal.notice.copied");
    });
  }

  function openLink() {
    if (!link) return;
    if (!link.safe) {
      notice = t("terminal.notice.linkBlocked");
      return;
    }
    void runAction(onOpenLink, link.request, t("terminal.unavailable.openLink"));
  }

  function leaveTerminal() {
    if (onFocusExit) onFocusExit();
    else exitMarker?.focus();
  }

  function onKeyDown(e: KeyboardEvent) {
    if (isCached) {
      e.preventDefault();
      return;
    }
    if (exitChordAllowed(e, router)) {
      e.preventDefault();
      leaveTerminal();
      return;
    }
    if (!router.composing && e.key === "End" && e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && scrollFocusedToBottom()) {
      e.preventDefault();
      return;
    }
    if (!router.composing && isCopyChord(e)) {
      e.preventDefault();
      copySelection();
      return;
    }
    // Spec 028 r4a: every paste chord (Ctrl+V, Cmd+V, Shift+Insert, Ctrl+Shift+V) goes to the
    // host command, mouse_reporting/alt-screen included; the Rust side decides from the clipboard
    // content (Local+image → ^V to the app, SSH+image → ClipboardImage, text → Paste).
    const focused = meta.focused();
    const decision = nativePasteRoute(e, {
      composing: router.composing,
      enabled: inputAllowed(),
      hasHandler: onNativePaste !== undefined,
      pane: focused ? { mouse_reporting: focused.mouse_reporting, alternate_screen_active: focused.alternate_screen_active } : null,
    });
    // The chord owns this gesture: the browser ClipboardEvent WebKit may still emit for it (empty
    // text/plain for an image) must be a no-op, never a second paste.
    if (decision) {
      e.preventDefault();
      router.pasteChordHandled();
      // Debug trail of the paste shortcut (spec 028 r3): the chord, the route and what the
      // backend answered, so a window test shows which path a paste took.
      uiTrace(pasteLine({ chord: chordName(e), route: decision, paneId: focused?.pane_id ?? null }));
      // WebKitGTK emits no ClipboardEvent for Ctrl+Shift+V: the host reads the local clipboard
      // at the paste's turn in the same input lane. A repeat or gated input does nothing; a
      // missing callback is an explicit notice, never a synthetic paste event. The backend
      // reports what it pasted (spec 028): an empty clipboard is a notice, an image is named.
      if (decision === "paste") {
        void Promise.resolve(onNativePaste?.()).then((result) => {
          const paste = result as { kind?: unknown; bytes?: unknown } | undefined;
          uiTrace(pasteResultLine(paste));
          if (!paste || typeof paste.kind !== "string") return;
          if (paste.kind === "empty") notice = t("terminal.notice.clipboardEmpty");
          else if (paste.kind === "image")
            notice = t("terminal.notice.imagePasted", { bytes: typeof paste.bytes === "number" ? paste.bytes : 0 });
        });
      } else if (decision === "unavailable") notice = t("terminal.unavailable.paste");
      return;
    }
    const input = router.keydown(e, inputAllowed());
    if (!input) return;
    e.preventDefault();
    void onInput([input]);
  }

  // The key release ends the paste gesture: a handled chord must not swallow a later clipboard
  // paste that has no chord (middle click) once its key is up.
  function onKeyUp() {
    router.pasteChordEnded();
  }

  // A handled chord already pasted through the host (spec 028 r4a): `router.paste` consumes the
  // marker, so WebKitGTK's ClipboardEvent of the same gesture sends nothing.
  function onPaste(e: ClipboardEvent) {
    if (isCached) {
      e.preventDefault();
      return;
    }
    const text = e.clipboardData?.getData("text/plain") ?? "";
    e.preventDefault();
    const input = router.paste(text, inputAllowed());
    if (input) void onInput([input]);
  }

  function onCompositionStart() {
    if (isCached) return;
    router.compositionStart();
    preedit = router.preedit;
  }

  function onCompositionUpdate(e: CompositionEvent) {
    if (isCached) return;
    router.compositionUpdate(e.data);
    preedit = router.preedit;
  }

  function onCompositionEnd(e: CompositionEvent) {
    if (isCached) return;
    const input = router.compositionEnd(e.data, inputAllowed());
    preedit = router.preedit;
    if (imeTarget) imeTarget.value = "";
    if (input) void onInput([input]);
  }

  // Foreign insertions (browser defaults, disabled input, IME leftovers) never reach the PTY;
  // clear them once the composition is no longer using the editable target. The composition
  // types stay untouched: WebKitGTK follows insertFromComposition with the real compositionend.
  function onTargetInput(e: Event) {
    const inputType = e instanceof InputEvent ? e.inputType : "";
    if (inputType.includes("Composition")) return;
    (e.currentTarget as HTMLTextAreaElement).value = "";
  }

  let unsubscribe: (() => void) | null = null;
  let observer: ResizeObserver | null = null;
  let dprQuery: MediaQueryList | null = null;

  function sendGeometry(geometry: GeometryDto) {
    if (!geometryChanged(lastGeometry, geometry)) return;
    lastGeometry = geometry;
    frameCache.invalidateGeometry(geometry.cols, geometry.rows);
    void onResize(geometry);
  }

  // DPI changes keep cols/rows but change device pixels: resize the canvas backing store,
  // repaint every row (deferred while hidden) and send the new cell pixels once.
  function watchDpr() {
    dprQuery?.removeEventListener("change", onDprChange);
    dprQuery = window.matchMedia?.(`(resolution: ${dpr}dppx)`) ?? null;
    dprQuery?.addEventListener("change", onDprChange);
  }

  function onDprChange() {
    dpr = window.devicePixelRatio || 1;
    watchDpr();
    sizeCanvas();
    schedule(allRows());
    const rect = container?.getBoundingClientRect();
    if (rect) sendGeometry(geometryFor(rect.width, rect.height));
  }

  onMount(() => {
    dpr = window.devicePixelRatio || 1;
    measure();
    ensureCanvasContext();
    watchDpr();
    unsubscribe = subscribe(handleEvent);
    observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      sendGeometry(geometryFor(entry.contentRect.width, entry.contentRect.height));
      // The container can change size without a new full frame (same cols/rows): the extension
      // ring follows the remainder anyway, and a full repaint keeps both layers in sync.
      sizeExtCanvas();
      if (fullPaint) schedule(allRows());
    });
    observer.observe(container);
    // On the surface container, not the IME textarea: a wheel over the focused pane must scroll
    // without keyboard focus on the textarea (spec 022). Non-passive so the page does not also scroll.
    container.addEventListener("wheel", onWheel, { passive: false });
    // Same place for the right click (spec 028 AC-028-03 r2c): the container sees the contextmenu
    // bubbling from every cell, which the IME textarea did not.
    container.addEventListener("contextmenu", onContextMenu);
    container.closest("[data-center-stage]")?.addEventListener("herdr-pane-scroll", onFrameScroll);
  });

  onDestroy(() => {
    clearLoadingTimer();
    unsubscribe?.();
    observer?.disconnect();
    dprQuery?.removeEventListener("change", onDprChange);
    container?.removeEventListener("wheel", onWheel);
    container?.removeEventListener("contextmenu", onContextMenu);
    container?.closest("[data-center-stage]")?.removeEventListener("herdr-pane-scroll", onFrameScroll);
    scheduler.dispose();
  });

  $effect(() => {
    scheduler.setVisible(visible);
  });

  $effect(() => {
    if (endpoint !== previousEndpointProp) {
      const prev = previousEndpointProp;
      previousEndpointProp = endpoint;
      if (prev === undefined) {
        currentEndpoint = endpoint ?? null;
      } else {
        switchHost(endpoint ?? null);
      }
    }
  });

  $effect(() => {
    void theme.background;
    if (grid.hasSurface) {
      fullPaint = true;
      schedule(allRows());
    }
  });
</script>

<div class="terminal" bind:this={container}>
  <canvas bind:this={canvas} aria-hidden="true"></canvas>
  <!-- Extension layer (spec 028 r4c window-padding extend): behind the grid canvas, painting the
       CSS padding and the sub-cell remainder with the edge cells' background colours. Kept after
       the grid canvas so `.terminal canvas` (PaneFrames, tests) still measures the grid. -->
  <canvas class="extend" bind:this={extCanvas} aria-hidden="true"></canvas>
  <!-- Real editable target: gives WebKitGTK an input-method context while staying invisible
       over the canvas. Keyboard focus, accessible name and preedit display live here. -->
  <textarea
    class="ime-target"
    bind:this={imeTarget}
    aria-label="Terminal Herdr"
    aria-multiline="true"
    autocapitalize="off"
    autocomplete="off"
    spellcheck="false"
    wrap="off"
    onkeydown={onKeyDown}
    onkeyup={onKeyUp}
    onpaste={onPaste}
    oninput={onTargetInput}
    oncompositionstart={onCompositionStart}
    oncompositionupdate={onCompositionUpdate}
    oncompositionend={onCompositionEnd}
    onfocus={() => {
      router.focusChanged(true);
      preedit = router.preedit;
      onFocus?.(true);
    }}
    onblur={() => {
      router.focusChanged(false);
      preedit = router.preedit;
      onFocus?.(false);
    }}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
    aria-describedby="terminal-keys"
  ></textarea>
  <!-- Exit marker right after the textarea (Ctrl+Shift+F6 focuses it): the next Tab reaches the
       terminal actions below before the rest of the window; Shift+Tab returns to the textarea. -->
  <span class="exit-marker" tabindex="-1" bind:this={exitMarker}>{t("terminal.sr.exitMarker")}</span>
  {#if preedit}<span class="ime" aria-live="polite">{preedit}</span>{/if}
  <span id="terminal-keys" class="sr-only">{t("terminal.sr.keys")}</span>
  {#if pendingFocus || selectionRows > 0 || link || notice}
    <div class="actions" role="toolbar" class:in-band={toolbarBand !== null} aria-label={t("terminal.toolbar.label")} style={toolbarBand}>
      <span class="status" role="status" aria-live="polite">
        {#if pendingFocus}{t("terminal.toolbar.awaitingFocus", { pane: pendingFocus })}{:else if selectionRows > 0}{t("terminal.toolbar.selection", { count: selectionRows })}{/if}
        {notice}
      </span>
      {#if selectionRows > 0}
        <button type="button" onclick={copySelection} disabled={!onCopySelection} title={onCopySelection ? t("terminal.toolbar.copyTitle") : t("terminal.unavailable.copy")}>
          {onCopySelection ? t("terminal.toolbar.copy") : t("terminal.toolbar.copyUnavailable")}
        </button>
      {/if}
      {#if link}
        <input class="uri" readonly value={link.request.uri} aria-label={link.safe ? t("terminal.link.address") : t("terminal.link.blockedLabel")} />
        <button type="button" onclick={openLink} disabled={!onOpenLink || !link.safe} title={link.safe ? t("terminal.link.openTitle") : t("terminal.link.schemeOnly")}>
          {!link.safe ? t("terminal.link.blocked") : onOpenLink ? t("terminal.link.open") : t("terminal.link.openUnavailable")}
        </button>
        <button type="button" onclick={() => (link = null)} aria-label={t("terminal.link.close")}>×</button>
      {/if}
    </div>
  {/if}
  {#if loadingIndicator}
    <div class="loading-indicator" role="status" aria-live="polite">{loadingIndicator}</div>
  {/if}
</div>

<style>
  .terminal {
    position: relative;
    width: 100%;
    height: 100%;
    /* Visible so the toolbar can sit in the band above row 0 (the region reserves it); the canvas
       is sized to the surface and never overflows. */
    overflow: visible;
    background: var(--bg);
    outline: none;
  }
  canvas {
    display: block;
  }
  /* Window-padding extend layer: the ring is drawn by the canvas, not by the container colour. */
  canvas.extend {
    position: absolute;
    z-index: 0;
    pointer-events: none;
  }
  canvas:not(.extend) {
    position: relative;
    z-index: 1;
  }
  .ime-target {
    position: absolute;
    inset: 0;
    z-index: 1;
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 0;
    border: 0;
    outline: none;
    resize: none;
    overflow: hidden;
    white-space: pre;
    background: transparent;
    color: transparent;
    caret-color: transparent;
    cursor: text;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
  .exit-marker {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .exit-marker:focus {
    width: auto;
    height: auto;
    clip: auto;
    left: 8px;
    bottom: 8px;
    z-index: 3;
    padding: 2px 6px;
    background: var(--bg);
    color: #e6e6e6;
    outline: 2px solid #61afef;
  }
  .actions {
    position: absolute;
    top: 6px;
    right: 6px;
    z-index: 3;
    display: flex;
    gap: 6px;
    align-items: center;
    max-width: calc(100% - 12px);
    padding: 4px 6px;
    border-radius: 4px;
    background: rgb(16 20 24 / 0.92);
    color: #e6e6e6;
    font-size: 12px;
  }
  /* In the pane's band: right aligned inside that pane's width, never over its content cells. */
  .actions.in-band {
    top: auto;
    right: auto;
    justify-content: flex-end;
    max-width: none;
    padding: 0 6px;
    border-radius: 6px;
    overflow: hidden;
  }
  .actions button:focus-visible,
  .actions input:focus-visible {
    outline: 2px solid #61afef;
  }
  .uri {
    min-width: 12ch;
    max-width: 40ch;
    font: inherit;
  }
  .ime {
    position: absolute;
    right: 8px;
    bottom: 8px;
    z-index: 2;
    max-width: calc(100% - 16px);
    padding: 2px 6px;
    border-radius: 4px;
    background: rgb(16 20 24 / 0.9);
    color: #e5c07b;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-all;
    pointer-events: none;
  }
  .loading-indicator {
    position: absolute;
    bottom: 12px;
    right: 12px;
    z-index: 4;
    padding: 4px 8px;
    border-radius: 4px;
    background: rgb(16 20 24 / 0.85);
    color: var(--text-muted, #8b949e);
    font-family: var(--font-ui, system-ui, sans-serif);
    font-size: 12px;
    pointer-events: none;
  }
</style>
