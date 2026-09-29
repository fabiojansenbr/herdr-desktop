<script lang="ts">
  // Title bar of the clean casca (spec 018 AC-018-01, spec 040 AC-040-01), reframed by spec 047
  // (AC-047-04) to the approved design: the bar sits only over the main area — the sidebar owns
  // the top-left corner — and carries the actions of the center area and the three window
  // controls. The mark, the 460 px search and the host pill left the bar: the search is
  // `Buscar`/Ctrl K in the sidebar (041) and the host is the foot switcher (048).
  //
  // Spec 049 (AC-049-03): the panel button left it too. Collapsing had two controls doing the
  // same thing — this one and `Recolher lateral` in the sidebar head — so the sidebar keeps the
  // only one, and Ctrl+B keeps toggling from the window (`shortcuts.ts`).
  //
  // Spec 055 (AC-055-01/02): the bar is now the workspace tabs. The `coleção › workspace`
  // breadcrumb left it — the branch and the path are in the status bar, so nothing unique was
  // lost — and the strip the center region used to draw in a row of its own moved up here, the
  // same `WorkspaceTabs` with its `+`, the × of every tab and the context menu of 028/040/041.
  // Left to right: the strip, a free area that is the drag region, the bell, the three controls.
  //
  // Dragging the bar still moves the window; the three controls call the injected (or Tauri)
  // API (spec 017).
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import WorkspaceTabs from "../center/WorkspaceTabs.svelte";
  import { runWindowControl, tauriWindowApi, type WindowApi, type WindowControl } from "./window-api";

  export interface TitleBarHost {
    label: string;
    kind: string;
    state: string;
    tone: "live" | "busy" | "offline" | "none";
  }
  /** @deprecated alias kept so 010 call sites type-check after the rename. */
  export type TopBarHost = TitleBarHost;

  let {
    host,
    searchHint,
    onOpenPalette,
    onHost,
    windowApi = null,
    sidebarOpen = true,
    onToggleSidebar = () => {},
    waitingCount = 0,
    onBell = () => {},
    projectLabel = t("frame.switcher.session", { name: "default" }),
    ctx = null,
    pickFolder = null,
    layoutWidth = null,
  }: {
    /** @deprecated spec 047: the host pill moved to the sidebar foot (048). */
    host: TitleBarHost | null;
    /** @deprecated spec 047: the search moved to `Buscar`/Ctrl K in the sidebar (041). */
    searchHint: string;
    /** @deprecated spec 047: the palette opens from the sidebar. */
    onOpenPalette: () => void;
    /** @deprecated spec 047: the host popover opens from the sidebar foot. */
    onHost: () => void;
    windowApi?: WindowApi | null;
    /** @deprecated spec 049: the bar no longer draws a collapse control. */
    sidebarOpen?: boolean;
    /** @deprecated spec 049: the sidebar head owns the only collapse control. */
    onToggleSidebar?: () => void;
    waitingCount?: number;
    onBell?: () => void;
    /** @deprecated spec 055: the breadcrumb left the bar; the tabs name the workspace. */
    projectLabel?: string;
    /** The window's context: the bar draws this workspace's tabs (spec 055). */
    ctx?: FrameContext | null;
    /** @deprecated spec 047: opening a folder belongs to the sidebar (025/046). */
    pickFolder?: (() => Promise<string | null>) | null;
    /** @deprecated spec 055: the strip measures its own box; the bar has nothing else to narrow. */
    layoutWidth?: number | null;
  } = $props();

  let loadedApi = $state<WindowApi | null>(null);

  function stopDrag(event: MouseEvent) {
    event.stopPropagation();
  }

  async function resolveApi(): Promise<WindowApi> {
    if (windowApi) return windowApi;
    if (loadedApi) return loadedApi;
    loadedApi = await tauriWindowApi();
    return loadedApi;
  }

  function call(action: WindowControl) {
    if (windowApi) {
      void runWindowControl(windowApi, action);
      return;
    }
    void resolveApi().then((api) => runWindowControl(api, action));
  }

  function onBarDblClick(event: MouseEvent) {
    const target = event.target as HTMLElement | null;
    if (target?.closest("button, a, input, select, textarea, [role='menu'], [role='tab'], [role='tablist']")) return;
    call("maximize");
  }

  const bellLabel = $derived(
    waitingCount > 0 ? t("frame.titleBar.waiting", { count: waitingCount }) : t("frame.titleBar.noneWaiting"),
  );

  function ringBell() {
    if (waitingCount === 0) return;
    onBell();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<header
  class="topbar"
  data-region="topbar"
  data-tauri-drag-region
  aria-label={t("frame.titleBar.label")}
  ondblclick={onBarDblClick}
>
  {#if ctx}
    <WorkspaceTabs {ctx} />
  {/if}

  <!-- Spec 055 AC-055-02: the free bar between the last tab/`+` and the bell drags the window. -->
  <div class="drag" data-topbar-drag data-tauri-drag-region></div>

  <div class="right">
    <button
      type="button"
      class="icon bell"
      data-topbar-item="notifications"
      aria-label={bellLabel}
      title={bellLabel}
      onclick={ringBell}
      onmousedown={stopDrag}
    >
      <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
        <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
      </svg>
      {#if waitingCount > 0}
        <span class="badge" data-waiting-count>{waitingCount}</span>
      {/if}
    </button>
    <button type="button" class="icon" data-topbar-item="window-minimize" aria-label={t("frame.window.minimize")} title={t("frame.window.minimize")} onclick={() => call("minimize")} onmousedown={stopDrag}>
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="5" y1="12" x2="19" y2="12" /></svg>
    </button>
    <button type="button" class="icon" data-topbar-item="window-maximize" aria-label={t("frame.window.maximize")} title={t("frame.window.maximize")} onclick={() => call("maximize")} onmousedown={stopDrag}>
      <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="2" /></svg>
    </button>
    <button type="button" class="icon" data-topbar-item="window-close" aria-label={t("frame.window.closeWindow")} title={t("frame.window.close")} onclick={() => call("close")} onmousedown={stopDrag}>
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="6" y1="6" x2="18" y2="18" /><line x1="18" y1="6" x2="6" y2="18" /></svg>
    </button>
  </div>
</header>

<style>
  .topbar {
    flex: none;
    height: 48px;
    display: flex;
    align-items: stretch;
    gap: 10px;
    padding: 0 12px 0 10px;
    background: var(--surface);
    border-bottom: none;
    border-bottom-width: 0px;
    color: var(--text);
    font-family: var(--font-ui);
    user-select: none;
    position: relative;
    z-index: 20;
    /* No clipping: the strip already yields (min-width: 0) and scrolls inside itself, and the
       tab context menu of 028 drops out of the 48 px bar — an `overflow` here would cut it. */
  }
  /* The tab strip is the bar's first item and the only one that yields: it fills the bar's own
     height and scrolls inside itself, so the bar draws one line, not two. */
  .topbar :global([data-center-tabs]) {
    flex: 0 1 auto;
    min-width: 0;
    height: 48px;
    padding-left: 0;
    border-bottom: none;
    border-bottom-width: 0px;
    background: var(--surface);
  }
  /* Spec 055 AC-055-02: the leftover width, and nothing else, is the grab area of the window. */
  .drag {
    flex: 1;
    min-width: 24px;
    align-self: stretch;
  }
  .right {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
  }
  .icon {
    width: 30px;
    height: 30px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    position: relative;
  }
  .icon:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-3);
    border-radius: 6px;
  }
  .badge {
    position: absolute;
    top: 2px;
    right: 2px;
    min-width: 14px;
    height: 14px;
    padding: 0 3px;
    border-radius: 7px;
    background: var(--attention);
    color: var(--bg);
    font-size: 10px;
    font-weight: 700;
    line-height: 14px;
  }
</style>
