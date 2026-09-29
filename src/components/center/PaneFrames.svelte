<script lang="ts">
  // Spec 019 AC-019-02 — frameless panes over the single surface: no wrap border, radius or
  // header. Geometry stays `inner_rect × cell`. Split panes get a 1 px divider; hover shows
  // split-right / split-down (existing engine actions).
  // Spec 028 AC-028-03 — right click on a pane opens the TUI context menu (TerminalView routes
  // it here when the pane does not take right clicks); every item calls one existing engine
  // action.
  import { onMount, untrack } from "svelte";
  import { t } from "../../i18n/index.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import type { FrameEvent, PaneMeta, ScrollRequest } from "../../terminal/types";
  import ContextMenu from "./ContextMenu.svelte";
  import {
    paneContextMenuItems,
    readRightClickPassthrough,
    writeRightClickPassthrough,
    type PaneContextMenuItemId,
  } from "./pane-menu";
  import {
    applySurfaceMetadata,
    metricsFromSurface,
    paneFrames,
    type Box,
    type CellMetrics,
    type PaneFrame,
    type SurfaceLayout,
  } from "./model";
  import { menuStateLine, uiTrace } from "../../shell/ui-trace";

  let {
    ctx,
    stage,
    onBand,
    onScroll,
  }: {
    ctx: FrameContext;
    stage: HTMLElement | undefined;
    onBand?: (px: number) => void;
    onScroll?: (request: ScrollRequest) => unknown;
  } = $props();

  let layout = $state<SurfaceLayout>({ revision: 0, panes: [] });
  const panes = $derived<PaneMeta[]>(layout.panes);
  let surface = $state<{ width: number; height: number } | null>(null);
  let origin = $state<{ left: number; top: number } | null>(null);
  let metrics = $state<CellMetrics | null>(null);
  let observer: ResizeObserver | null = null;
  let unsubscribe: (() => void) | null = null;
  /** Open pane menu: the clicked pane, its position inside the stage and the state it read. */
  let paneMenu = $state<{ paneId: string; x: number; y: number; passthrough: boolean; zoomed: boolean } | null>(null);
  /** Inline rename opened from the menu (the menu itself closes). */
  let renaming = $state<{ paneId: string; x: number; y: number; draft: string } | null>(null);

  const shown = $derived(ctx.view === "terminal");
  const live = $derived(ctx.surface.phase === "live");
  const caps = $derived(ctx.agents?.capabilities ?? null);
  const splitReason = $derived(!live ? t("center.reason.hostOffline") : caps?.split ? null : t("center.reason.noSplit"));
  const zoomReason = $derived(!live ? t("center.reason.hostOffline") : caps?.zoom ? null : t("center.reason.noZoom"));
  const focusReason = $derived(!live ? t("center.reason.hostOffline") : caps?.focus ? null : t("center.reason.noFocus"));
  const panePaths = $derived(
    Object.fromEntries((ctx.agents?.topology?.panes ?? []).flatMap((p) => (p.cwd ? [[p.pane_id, p.cwd] as const] : []))),
  );
  const frames = $derived<PaneFrame[]>(
    shown && origin && metrics && panes.length > 0
      ? paneFrames({ panes, agents: ctx.agents?.agents ?? [], panePaths, metrics, origin })
      : [],
  );
  const splitDisabled = $derived(splitReason !== null || frames.length === 0);

  function paneLabel(paneId: string): string | null {
    return ctx.agents?.topology?.panes.find((p) => p.pane_id === paneId)?.label ?? null;
  }

  function paneMenuItems(paneId: string) {
    const focused = ctx.agents?.topology?.focused_pane_id ?? null;
    return paneContextMenuItems({
      paneId,
      hasManualLabel: Boolean(paneLabel(paneId)),
      swapSourcePaneId: focused && focused !== paneId ? focused : null,
      rightClickPassthrough: paneMenu?.passthrough ?? readRightClickPassthrough(paneId),
      zoomed: paneMenu?.zoomed ?? ctx.agents?.zoomedPanes?.[paneId] ?? false,
    });
  }

  function closeMenu() {
    paneMenu = null;
    uiTrace(menuStateLine({ state: "closed" }));
  }

  function onPaneMenuSelect(id: string) {
    const open = paneMenu;
    if (!open) return;
    const item = id as PaneContextMenuItemId;
    const paneId = open.paneId;
    closeMenu();
    switch (item) {
      case "rename": {
        renaming = { paneId, x: open.x, y: open.y, draft: paneLabel(paneId) ?? "" };
        return;
      }
      case "clear-name":
        void ctx.controllers.agents.renamePane(paneId, null);
        return;
      case "swap":
        void ctx.controllers.agents.swapPane(paneId);
        return;
      case "split-right":
        void ctx.controllers.agents.split("right");
        return;
      case "split-down":
        void ctx.controllers.agents.split("down");
        return;
      case "zoom":
        void ctx.controllers.agents.zoomPane(paneId, "toggle");
        return;
      case "toggle-right-click": {
        const next = !open.passthrough;
        writeRightClickPassthrough(paneId, next);
        void ctx.controllers.agents.setPaneRightClick(paneId, next);
        return;
      }
      case "close":
        void ctx.controllers.agents.closePane(paneId);
        return;
    }
  }

  function onPaneMenuEvent(event: Event) {
    const detail = (event as CustomEvent<{ pane_id?: unknown; client_x?: unknown; client_y?: unknown }>).detail;
    if (!detail || typeof detail.pane_id !== "string") return;
    const paneId = detail.pane_id;
    if (!panes.some((pane) => pane.pane_id === paneId)) {
      // Debug trail (spec 028 r3): the event arrived but the pane of this layout is unknown —
      // never a silent drop when no menu appears.
      uiTrace(menuStateLine({ state: "ignored", paneId, known: panes.length }));
      return;
    }
    const rect = stage?.getBoundingClientRect();
    const x = Number(detail.client_x ?? 0) - (rect?.left ?? 0);
    const y = Number(detail.client_y ?? 0) - (rect?.top ?? 0);
    paneMenu = { paneId, x, y, passthrough: readRightClickPassthrough(paneId), zoomed: ctx.agents?.zoomedPanes?.[paneId] ?? false };
    uiTrace(menuStateLine({ state: "open", paneId, items: paneMenuItems(paneId).map((item) => item.label) }));
  }

  function confirmPaneRename() {
    const open = renaming;
    if (!open) return;
    const label = open.draft.trim();
    renaming = null;
    if (!label) return;
    void ctx.controllers.agents.renamePane(open.paneId, label);
  }

  function onPaneRenameKey(event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      renaming = { ...renaming!, draft: (event.currentTarget as HTMLInputElement).value };
      confirmPaneRename();
    } else if (event.key === "Escape") {
      event.preventDefault();
      renaming = null;
    }
  }

  function canvasOf(): HTMLCanvasElement | null {
    return stage?.querySelector<HTMLCanvasElement>(".terminal canvas") ?? null;
  }

  function measure() {
    const canvas = canvasOf();
    if (!stage || !canvas || !surface) {
      if (origin !== null) origin = null;
      return;
    }
    const box = canvas.getBoundingClientRect();
    const host = stage.getBoundingClientRect();
    const next = { left: box.left - host.left, top: box.top - host.top };
    const cell = metricsFromSurface({ width: box.width, height: box.height }, surface);
    if (!origin || origin.left !== next.left || origin.top !== next.top) origin = next;
    if (cell && (!metrics || metrics.cellWidth !== cell.cellWidth || metrics.cellHeight !== cell.cellHeight)) {
      metrics = cell;
      onBand?.(0);
    }
  }

  function onFrameEvent(event: FrameEvent) {
    if (event.type === "state" || event.type === "identity") return;
    if (event.type === "full") surface = { width: event.width, height: event.height };
    const next = applySurfaceMetadata(layout, event);
    if (next !== layout) {
      layout = next;
      measure();
    }
  }

  onMount(() => {
    unsubscribe = ctx.controllers.surface.subscribe(onFrameEvent);
    return () => unsubscribe?.();
  });

  // The stage only exists after the parent's `bind:this` is applied, which is after this component
  // mounts: the r3 window test showed onMount reading `undefined`, so the `herdr-pane-menu`
  // listener was never attached and the right click opened no menu. The listener, the resize
  // observer and the first measure follow the bound stage (spec 028 AC-028-03 r3).
  $effect(() => {
    const target = stage;
    if (!target) return;
    const resize = typeof ResizeObserver !== "undefined" ? new ResizeObserver(() => measure()) : null;
    observer = resize;
    resize?.observe(target);
    // TerminalView routes a right click that does not belong to the pane app to this event
    // (spec 028 AC-028-03): the menu opens over the stage, in stage coordinates.
    target.addEventListener("herdr-pane-menu", onPaneMenuEvent);
    // Only the bound stage re-triggers this effect; `measure` reads other state itself.
    untrack(() => measure());
    return () => {
      resize?.disconnect();
      if (observer === resize) observer = null;
      target.removeEventListener("herdr-pane-menu", onPaneMenuEvent);
    };
  });

  const style = (box: Box) => `left:${box.left}px;top:${box.top}px;width:${box.width}px;height:${box.height}px`;

  function dividerStyle(frame: PaneFrame): string | null {
    if (frame.edge === "none" || frames.length < 2) return null;
    const minLeft = Math.min(...frames.map((f) => f.rect.left));
    const minTop = Math.min(...frames.map((f) => f.rect.top));
    if (frame.rect.left > minLeft + 1) {
      return `left:${frame.rect.left - 1}px;top:${frame.rect.top}px;width:1px;height:${frame.rect.height}px`;
    }
    if (frame.rect.top > minTop + 1) {
      return `left:${frame.rect.left}px;top:${frame.rect.top - 1}px;width:${frame.rect.width}px;height:1px`;
    }
    return null;
  }

  function dividerEdge(frame: PaneFrame): PaneFrame["edge"] {
    const focused = frames.find((f) => f.focused);
    if (!focused) return frame.edge;
    const a = frame.rect;
    const b = focused.rect;
    const adjacent =
      Math.abs(a.left + a.width - b.left) <= 2 ||
      Math.abs(b.left + b.width - a.left) <= 2 ||
      Math.abs(a.top + a.height - b.top) <= 2 ||
      Math.abs(b.top + b.height - a.top) <= 2;
    return adjacent ? "accent" : "border";
  }

  function focusPane(pane_id: string) {
    if (focusReason !== null) return;
    void ctx.controllers.agents.focusPane(pane_id);
  }

  function split(direction: "right" | "down") {
    if (splitDisabled) return;
    void ctx.controllers.agents.split(direction);
  }

  function zoom(pane_id: string) {
    if (zoomReason !== null) return;
    void ctx.controllers.agents.zoomPane(pane_id);
  }

  function scrollOffset(pane_id: string): number {
    return panes.find((p) => p.pane_id === pane_id)?.scroll?.offset_from_bottom ?? 0;
  }

  function indicatorStyle(frame: PaneFrame): string {
    return `left:${frame.rect.left + frame.rect.width - 4}px;top:${frame.rect.top + 4}px`;
  }

  function scrollToBottom(pane_id: string, event: MouseEvent) {
    event.stopPropagation();
    const request: ScrollRequest = { pane_id, offset_from_bottom: 0 };
    if (onScroll) {
      void onScroll(request);
      return;
    }
    stage?.dispatchEvent(new CustomEvent("herdr-pane-scroll", { bubbles: true, detail: request }));
  }
</script>

<div class="overlay" data-pane-overlay>
  {#each frames as frame (frame.pane_id)}
    <div
      class="frame"
      data-pane-frame={frame.pane_id}
      data-edge={frame.edge}
      data-focused={frame.focused}
      data-name={frame.name}
      data-state-label={frame.status.label}
      data-pane-path={frame.path ?? ""}
      style={style(frame.rect)}
      role="presentation"
      onclick={() => focusPane(frame.pane_id)}
    ></div>
    {#if dividerStyle(frame)}
      <span class="divider" data-edge-of={frame.pane_id} data-edge={dividerEdge(frame)} style={dividerStyle(frame)}></span>
    {/if}
    {#if scrollOffset(frame.pane_id) > 0}
      <button
        type="button"
        class="scroll-indicator"
        data-scroll-indicator={frame.pane_id}
        style={indicatorStyle(frame)}
        aria-label={t("center.pane.backToEnd")}
        onclick={(event) => scrollToBottom(frame.pane_id, event)}
      >
        ↑ {t("center.pane.scrollLines", { count: scrollOffset(frame.pane_id) })}
      </button>
    {/if}
  {/each}
  <div class="splits" data-pane-splits>
    <button
      type="button"
      class="icon-button"
      data-split-right
      aria-disabled={splitDisabled}
      aria-label={t("center.pane.splitRight")}
      title={splitDisabled ? (splitReason ?? t("center.pane.splitRight")) : t("center.pane.splitRight")}
      onclick={() => split("right")}
    >
      <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 2h5v12H2zM9 2h5v12H9z" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
    </button>
    <button
      type="button"
      class="icon-button"
      data-split-down
      aria-disabled={splitDisabled}
      aria-label={t("center.pane.splitDown")}
      title={splitDisabled ? (splitReason ?? t("center.pane.splitDown")) : t("center.pane.splitDown")}
      onclick={() => split("down")}
    >
      <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><path d="M2 2h12v5H2zM2 9h12v5H2z" fill="none" stroke="currentColor" stroke-width="1.4" /></svg>
    </button>
    {#if frames[0]}
      <button
        type="button"
        class="icon-button"
        data-zoom={frames.find((f) => f.focused)?.pane_id ?? frames[0].pane_id}
        aria-disabled={zoomReason !== null}
        aria-label={t("center.pane.expand")}
        title={zoomReason ?? t("center.pane.expandTitle")}
        onclick={() => {
          const id = frames.find((f) => f.focused)?.pane_id ?? frames[0]!.pane_id;
          zoom(id);
        }}
      >
        ⤢
      </button>
    {/if}
  </div>
  {#if paneMenu}
    <ContextMenu
      items={paneMenuItems(paneMenu.paneId)}
      x={paneMenu.x}
      y={paneMenu.y}
      label={t("center.pane.actions", { pane: paneMenu.paneId })}
      onselect={onPaneMenuSelect}
      onclose={closeMenu}
    />
  {/if}
  {#if renaming}
    <input
      class="pane-rename"
      data-pane-rename={renaming.paneId}
      value={renaming.draft}
      aria-label={t("center.pane.renameField")}
      style={`left:${renaming.x}px;top:${renaming.y}px`}
      oninput={(event) => (renaming = { ...renaming!, draft: (event.currentTarget as HTMLInputElement).value })}
      onkeydown={onPaneRenameKey}
      onblur={() => (renaming = null)}
      oncontextmenu={(event) => event.stopPropagation()}
    />
  {/if}
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    pointer-events: none;
    z-index: 2;
  }
  .frame {
    position: absolute;
    pointer-events: none;
    border: none;
    border-radius: 0;
    background: transparent;
  }
  .divider {
    position: absolute;
    pointer-events: none;
    background: var(--border);
  }
  .divider[data-edge="accent"] {
    background: var(--accent);
  }
  .splits {
    position: absolute;
    top: 8px;
    right: 8px;
    display: none;
    gap: 4px;
    pointer-events: auto;
    z-index: 3;
  }
  :global([data-center-stage]:hover) .splits {
    display: flex;
  }
  .icon-button {
    width: 28px;
    height: 28px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .icon-button[aria-disabled="true"] {
    cursor: default;
    opacity: 0.5;
  }
  .scroll-indicator {
    position: absolute;
    z-index: 3;
    pointer-events: auto;
    transform: translateX(-100%);
    padding: 2px 6px;
    border: 0;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.2;
    cursor: pointer;
  }
  .pane-rename {
    position: absolute;
    z-index: 8;
    pointer-events: auto;
    min-width: 12ch;
    max-width: 280px;
    padding: 3px 8px;
    border: 1px solid var(--accent);
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text);
    font: inherit;
  }
</style>
