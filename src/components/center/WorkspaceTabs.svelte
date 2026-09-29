<script lang="ts">
  // Spec 019 AC-019-01 — Orca tab bar: status icon, agent glyph, display title, × on every
  // tab (inactive included, no hover needed), + creating a new tab immediately after the last tab (spec 040).
  // Spec 026 — an unnamed tab is titled by the app each pane runs (agent, terminal title, cwd),
  // with `+N` for panes past the first two; a named tab keeps the engine label and lists the
  // panes in the tooltip. Never reads a pane: everything comes from the subscribed metadata.
  // Spec 028 AC-028-01/02/03 — `+` creates one tab in the focused workspace (never a split);
  // each tab is as wide as its content (64..280 px) with no `…` button; the tab context menu
  // (right click) carries `Nova aba`, `Renomear` and `Fechar`; renaming is a double click.
  // Spec 028 r2 — the width is measured by the CSS over the text the tab actually renders
  // (`tabText`: the label of a named tab), never by an inline estimate of a pane-derived title.
  // Spec 077 AC-077-02/03 — the × closes the tab through the path that owns it: `agents.closeTab`
  // for a tab of the live list, `host_tab_close` on the host's own connection for a tab listed
  // from the hub snapshot (074), whose tab the agents connection does not know. A refusal is shown
  // in the bar; a host with no live connection keeps the × disabled, as before.
  import type { FrameContext } from "../../shell/frame-context";
  import { errorText, t } from "../../i18n/index.svelte";
  import type { RuntimeError } from "../../terminal/types";
  import { hostTabClose } from "../../projects/workspace-bridge";
  import type { AgentDto, AgentStatus, TabDto } from "../../agents/types";
  import { actionAvailability, confirmedTarget, focusedWorkspaceId, workspaceTabs } from "./model";
  import ContextMenu from "./ContextMenu.svelte";
  import { TAB_MAX_PX, TAB_MIN_PX, tabText } from "./tabs-narrow";
  import { effectiveTabs } from "../sidebar/tab-rows";

  let { ctx, layoutWidth = null }: { ctx: FrameContext; layoutWidth?: number | null } = $props();

  const agents = $derived(ctx.agents);
  const selectedEndpoint = $derived(
    ctx.selectedEndpoint ?? ctx.surface.selection?.endpoint ?? ctx.surface.identity?.endpoint ?? null,
  );
  const selectedHost = $derived((ctx.connections?.view?.hub.hosts ?? []).find((h) => h.endpoint === selectedEndpoint));

  const isSnapshotOnly = $derived(selectedHost?.api === false);

  const hostAgents = $derived.by<AgentDto[]>(() => {
    if (!isSnapshotOnly && (agents?.agents ?? []).length > 0) return agents!.agents;
    if (!selectedHost) return agents?.agents ?? [];
    if (selectedHost.agents && selectedHost.agents.length > 0) {
      return selectedHost.agents.map((a) => ({
        pane_id: a.pane_id,
        workspace_id: a.workspace_id,
        tab_id: a.tab_id,
        name: a.name,
        kind: a.agent ?? a.display_agent ?? a.name,
        status: (a.agent_status as AgentStatus) ?? "idle",
        launch_pending: false,
        ready: true,
        focused: a.focused,
        terminal_title: a.terminal_title_stripped ?? a.terminal_title ?? a.title ?? null,
      }));
    }
    if (selectedHost.panes && selectedHost.panes.length > 0) {
      return selectedHost.panes.map((p) => ({
        pane_id: p.pane_id,
        workspace_id: p.workspace_id,
        tab_id: p.tab_id ?? "",
        name: p.agent ?? null,
        kind: p.agent ?? null,
        status: (p.agent_status as AgentStatus) ?? "idle",
        launch_pending: false,
        ready: true,
        focused: p.focused,
        terminal_title: p.terminal_title ?? p.title ?? null,
      }));
    }
    return agents?.agents ?? [];
  });

  // Spec 074 (AC-074-01): the bar reads its tabs from the one rule the sidebar reads them from
  // (`tab-rows.ts`), so a host never lists one set here and another there — the live list while it
  // belongs to the selected host, is `connected` and carries a tab, the hub snapshot otherwise
  // (which is what a host without the API lane publishes, 039/057).
  const barSource = $derived(
    effectiveTabs({ endpoint: selectedEndpoint, host: selectedHost ?? null, agents, selectedEndpoint }),
  );
  const barTabs = $derived<TabDto[]>([...barSource.tabs]);

  // Spec 077 (AC-077-02): the bar shows the host snapshot, so the tab belongs to that host's own
  // connection and not to the agents one; the × has to go through the hub command.
  const hostClosePath = $derived(!barSource.live && selectedEndpoint !== null);

  const focusedPaneId = $derived(
    ctx.surface.identity?.pane_id ??
      agents?.topology?.focused_pane_id ??
      selectedHost?.panes.find((p) => p.focused)?.pane_id ??
      null,
  );

  // Spec 025 AC-025-04: the bar is the focused workspace's own tabs, from the live host snapshot
  // (a focus changed in the TUI replaces the whole bar) or the confirmed tab as fallback.
  const workspaceId = $derived(
    focusedWorkspaceId({
      hosts: ctx.connections?.view?.hub.hosts ?? null,
      endpoint: selectedEndpoint,
      tabs: barTabs,
      focusedTabId: isSnapshotOnly
        ? (selectedHost?.tabs?.find((t) => t.focused)?.tab_id ?? null)
        : (agents?.tabFocus?.tab_id ?? null),
    }),
  );

  const defaultTabFocus = $derived({ focusedTabId: agents?.tabFocus?.tab_id ?? null });

  const effectiveFocusedTabId = $derived(
    isSnapshotOnly
      ? (selectedHost?.workspaces?.find((w) => (workspaceId ? w.workspace_id === workspaceId : w.focused))?.active_tab_id ??
         selectedHost?.tabs?.find((t) => t.focused && (workspaceId ? t.workspace_id === workspaceId : true))?.tab_id ??
         selectedHost?.tabs?.find((t) => t.focused)?.tab_id ??
         selectedHost?.tabs?.[0]?.tab_id ??
         null)
      : defaultTabFocus.focusedTabId,
  );

  const effectiveLayoutPanes = $derived.by(() => {
    if (!isSnapshotOnly) return agents?.topology?.panes ?? [];
    if (selectedHost?.panes && selectedHost.panes.length > 0) {
      return selectedHost.panes
        .filter((p) => (effectiveFocusedTabId && p.tab_id ? p.tab_id === effectiveFocusedTabId : true))
        .map((p) => ({
          pane_id: p.pane_id,
          x: 0,
          y: 0,
          width: 0,
          height: 0,
          focused: p.focused,
          cwd: p.foreground_cwd ?? p.cwd ?? null,
          label: p.title ?? null,
        }));
    }
    return [];
  });

  const effectivePanePaths = $derived.by(() => {
    if (!isSnapshotOnly) {
      return Object.fromEntries(
        (agents?.topology?.panes ?? []).flatMap((p) => (p.cwd ? [[p.pane_id, p.cwd] as const] : [])),
      );
    }
    if (selectedHost?.panes) {
      return Object.fromEntries(
        selectedHost.panes.flatMap((p) => {
          const path = p.foreground_cwd ?? p.cwd;
          return path ? [[p.pane_id, path] as const] : [];
        }),
      );
    }
    return {};
  });

  const tabs = $derived(
    workspaceTabs({
      tabs: barTabs,
      agents: hostAgents,
      focusedTabId: effectiveFocusedTabId,
      focusedPaneId,
      layoutPanes: effectiveLayoutPanes,
      panePaths: effectivePanePaths,
      workspaceId,
    }),
  );
  const live = $derived(ctx.surface.phase === "live");
  const target = $derived(confirmedTarget({ identity: ctx.surface.identity, agents: ctx.agents }));
  const unavailable = $derived({
    ...ctx.unavailable,
    newTab:
      !live
        ? t("center.reason.hostOffline")
        : ctx.agents?.capabilities?.create_tab === false
          ? t("center.reason.newTab")
          : ctx.unavailable.newTab,
  });
  const available = $derived(actionAvailability({ target, unavailable }));
  const focusUnavailable = $derived(
    !live
      ? t("center.reason.hostOffline")
      : ctx.agents?.capabilities?.focus_tab === false
        ? t("center.reason.switchTab")
        : null,
  );
  const closeUnavailable = $derived(
    !live
      ? t("center.reason.hostOffline")
      : hostClosePath
        ? null
        : ctx.agents?.capabilities?.close_tab === false
          ? t("center.reason.closeTab")
          : null,
  );
  const renameUnavailable = $derived(
    !live
      ? t("center.reason.hostOffline")
      : ctx.agents?.capabilities?.rename_tab === false
        ? t("center.reason.renameTab")
        : null,
  );
  let strip: HTMLDivElement | undefined = $state();
  let barEl = $state<HTMLDivElement | undefined>();
  let observed = $state(0);
  let renaming = $state<string | null>(null);
  let draft = $state("");
  /** Open tab menu: `tabId` null when the right click landed on the bar, outside a tab. */
  let tabMenu = $state<{ tabId: string | null; x: number; y: number } | null>(null);
  /** Last refusal of a close (AC-077-03); the bar shows it instead of doing nothing. */
  let closeError = $state<RuntimeError | null>(null);

  const width = $derived(layoutWidth ?? (observed > 0 ? observed : 1440));
  const tabMenuItems = $derived(
    tabMenu?.tabId
      ? [
          { id: "new-tab", label: t("center.tabs.new"), disabled: available.newTab !== null, reason: available.newTab ?? undefined },
          { id: "rename", label: t("center.tabs.rename"), disabled: renameUnavailable !== null, reason: renameUnavailable ?? undefined },
          { id: "close", label: t("center.tabs.close"), disabled: closeUnavailable !== null, reason: closeUnavailable ?? undefined },
        ]
      : [{ id: "new-tab", label: t("center.tabs.new"), disabled: available.newTab !== null, reason: available.newTab ?? undefined }],
  );

  $effect(() => {
    if (!barEl || layoutWidth != null) return;
    const ro = new ResizeObserver((entries) => {
      observed = Math.round(entries[0]?.contentRect.width ?? 0);
    });
    ro.observe(barEl);
    return () => ro.disconnect();
  });

  $effect(() => {
    const id = tabs.find((tab) => tab.active)?.tab_id;
    if (!id || !strip) return;
    const node = strip.querySelector<HTMLElement>(`[data-tab="${id}"]`);
    node?.scrollIntoView({ inline: "nearest", block: "nearest" });
  });

  function horizontalWheel(node: HTMLElement) {
    const onWheel = (event: WheelEvent) => {
      const delta = Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;
      if (delta === 0) return;
      event.preventDefault();
      node.scrollLeft += delta;
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return { destroy: () => node.removeEventListener("wheel", onWheel) };
  }

  const scrollBy = (delta: number) => strip?.scrollBy({ left: delta, behavior: "smooth" });

  /** Spec 028 AC-028-01: one `tab.create` in the focused workspace; nothing is split. */
  function newTab() {
    void ctx.controllers.agents.createTab();
  }

  /**
   * AC-077-02/03: one close per intent, on the path that owns the tab. A snapshot tab goes to
   * `host_tab_close` on its own host; a tab of the live list keeps the agents connection. A
   * refusal is kept and rendered, never swallowed.
   */
  async function runClose(tabId: string) {
    if (closeUnavailable !== null) return;
    closeError = null;
    if (hostClosePath) {
      try {
        await hostTabClose(selectedEndpoint!, tabId);
      } catch (error) {
        closeError = asRuntimeError(error);
      }
      return;
    }
    await ctx.controllers.agents.closeTab(tabId);
  }

  function asRuntimeError(error: unknown): RuntimeError {
    if (error && typeof error === "object" && "code" in error && "message" in error) return error as RuntimeError;
    return { code: "ipc_error", message: String(error), retryable: true };
  }

  function closeTab(tabId: string, event: MouseEvent) {
    event.stopPropagation();
    event.preventDefault();
    void runClose(tabId);
  }

  function menuPosition(clientX: number, clientY: number) {
    const rect = barEl?.getBoundingClientRect();
    return { x: clientX - (rect?.left ?? 0), y: clientY - (rect?.top ?? 0) };
  }

  function openTabMenu(tabId: string, event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();
    const { x, y } = menuPosition(event.clientX, event.clientY);
    tabMenu = { tabId, x, y };
  }

  /** Right click on the bar outside any tab: only `Nova aba` (spec 028 edge case). */
  function openBarMenu(event: MouseEvent) {
    const target = event.target as HTMLElement | null;
    if (target?.closest?.("[data-tab]")) return;
    event.preventDefault();
    const { x, y } = menuPosition(event.clientX, event.clientY);
    tabMenu = { tabId: null, x, y };
  }

  function onTabMenuSelect(id: string) {
    const tabId = tabMenu?.tabId ?? null;
    tabMenu = null;
    if (id === "new-tab") {
      newTab();
      return;
    }
    if (!tabId) return;
    if (id === "rename") {
      const tab = tabs.find((candidate) => candidate.tab_id === tabId);
      if (tab) startRename(tabId, tab.label);
      return;
    }
    if (id === "close") void runClose(tabId);
  }

  function startRename(tabId: string, label: string, event?: Event) {
    event?.stopPropagation();
    event?.preventDefault();
    tabMenu = null;
    if (renameUnavailable !== null) return;
    renaming = tabId;
    draft = label;
  }

  function cancelRename() {
    renaming = null;
    draft = "";
  }

  function confirmRename(tabId: string) {
    const label = draft.trim();
    cancelRename();
    if (!label) return;
    void ctx.controllers.agents.renameTab(tabId, label);
  }

  function onRenameKey(tabId: string, event: KeyboardEvent) {
    if (event.key === "Enter") {
      event.preventDefault();
      draft = (event.currentTarget as HTMLInputElement).value;
      confirmRename(tabId);
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancelRename();
    }
  }
</script>

{#if tabs.length > 0 || live}
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    class="tabs"
    bind:this={barEl}
    data-center-tabs
    data-tab-min={TAB_MIN_PX}
    data-tab-max={TAB_MAX_PX}
    role="tablist"
    aria-label={t("center.tabs.label")}
    oncontextmenu={openBarMenu}
  >
    {#if tabs.length > 8}
      <button type="button" class="arrow" onclick={() => scrollBy(-240)} aria-label={t("center.tabs.scrollLeft")} title={t("center.tabs.scrollLeft")}>‹</button>
    {/if}
    <div class="strip" bind:this={strip} use:horizontalWheel data-tab-strip data-overflow={tabs.length > 8 ? "scroll" : null}>
      {#each tabs as tab (tab.tab_id)}
        <div
          role="tab"
          class="tab"
          class:active={tab.active}
          data-tab={tab.tab_id}
          data-active={tab.active}
          aria-selected={tab.active}
          title={tab.title}
          tabindex="0"
          aria-disabled={focusUnavailable !== null}
          onclick={() => {
            if (focusUnavailable !== null) return;
            void ctx.controllers.agents.focusTab(tab.tab_id);
          }}
          ondblclick={(event) => startRename(tab.tab_id, tab.label, event)}
          oncontextmenu={(event) => openTabMenu(tab.tab_id, event)}
          onkeydown={(event) => {
            if (renaming === tab.tab_id) return;
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            if (focusUnavailable !== null) return;
            void ctx.controllers.agents.focusTab(tab.tab_id);
          }}
        >
          <span class="status" data-tab-icon={tab.icon} aria-hidden="true">
            {#if tab.icon === "idle"}
              <svg viewBox="0 0 16 16" width="14" height="14"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.6" /><path d="M5 8.2 7.1 10.4 11 5.8" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
            {:else if tab.icon === "working"}
              <span class="pulse"></span>
            {:else if tab.icon === "attention"}
              <span class="dot"></span>
            {:else}
              <svg viewBox="0 0 16 16" width="14" height="14"><rect x="2.5" y="3.5" width="11" height="9" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M5 12.5v1.5M11 12.5v1.5M5 3.5V2M11 3.5V2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" /></svg>
            {/if}
          </span>
          {#if tab.glyph}
            <span class="glyph" data-glyph data-kind={tab.kind} aria-hidden="true">{tab.glyph}</span>
          {/if}
          {#if renaming === tab.tab_id}
            <input
              class="rename"
              data-tab-rename
              bind:value={draft}
              aria-label={t("center.tabs.renameField")}
              onkeydown={(event) => onRenameKey(tab.tab_id, event)}
              onblur={() => cancelRename()}
              onclick={(event) => event.stopPropagation()}
            />
          {:else}
            <span class="label" data-label>
              <span class="names">{tabText(tab)}</span>{#if tab.more > 0}<span class="more" data-more>+{tab.more}</span>{/if}
            </span>
          {/if}
          <button
            type="button"
            class="close"
            data-close-tab={tab.tab_id}
            aria-label={t("center.tabs.closeNamed", { name: tabText(tab) })}
            title={closeUnavailable ?? t("center.tabs.closeTab")}
            aria-disabled={closeUnavailable !== null}
            onclick={(event) => closeTab(tab.tab_id, event)}
          >×</button>
          {#if tab.icon === "attention"}<span class="sr-only">{t("center.tabs.agentWaiting")}</span>{:else if tab.icon === "working"}<span class="sr-only">{t("center.tabs.agentWorking")}</span>{/if}
        </div>
      {/each}
      <button
        type="button"
        class="new"
        data-new-tab
        onclick={newTab}
        aria-disabled={available.newTab !== null}
        aria-label={t("center.tabs.new")}
        title={available.newTab ?? t("center.tabs.newShortcut")}
      >
        +
      </button>
    </div>
    {#if tabs.length > 8}
      <button type="button" class="arrow" onclick={() => scrollBy(240)} aria-label={t("center.tabs.scrollRight")} title={t("center.tabs.scrollRight")}>›</button>
    {/if}
    {#if closeError}
      <span class="bar-error" data-tabs-error role="alert">{errorText(closeError)}</span>
    {/if}
    {#if tabMenu}
      <ContextMenu
        items={tabMenuItems}
        x={tabMenu.x}
        y={tabMenu.y}
        label={t("center.tabs.actions")}
        onselect={onTabMenuSelect}
        onclose={() => (tabMenu = null)}
      />
    {/if}
  </div>
{/if}

<style>
  /* AC-077-03: the refusal sits in the bar itself, where its warnings already are. */
  .bar-error {
    flex: none;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 0 8px;
    font-size: 12px;
    color: var(--error);
  }
  .tabs {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 48px;
    padding: 0 8px;
    border-bottom: none;
    border-bottom-width: 0px;
    background: var(--surface);
  }
  .strip {
    display: flex;
    align-items: center;
    flex: 1;
    gap: 0;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .strip::-webkit-scrollbar {
    display: none;
    height: 0;
  }
  .tab {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    flex: 0 0 auto;
    width: max-content;
    min-width: 64px;
    max-width: 280px;
    height: 32px;
    box-sizing: border-box;
    padding: 0 10px;
    border: none;
    border-bottom-width: 0px;
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    font-size: 13.5px;
    white-space: nowrap;
    cursor: pointer;
  }
  .tab.active {
    color: var(--text);
    background: var(--surface-3);
    font-weight: 500;
  }
  .status {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    flex: none;
    color: var(--working);
  }
  .status[data-tab-icon="attention"] {
    color: var(--attention);
  }
  .status[data-tab-icon="shell"] {
    color: var(--text-muted);
  }
  .pulse,
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
  }
  .pulse {
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    0%, 100% { opacity: 1; transform: scale(1); }
    50% { opacity: 0.45; transform: scale(0.7); }
  }
  .glyph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    flex: none;
    border-radius: 50%;
    background: var(--surface-3);
    color: var(--text);
    font-size: 10px;
    text-transform: lowercase;
  }
  .label {
    display: inline-flex;
    align-items: center;
    /* The title takes part in the tab's intrinsic width; it shrinks only at the 280 px cap. */
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .names {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .more {
    flex: none;
    padding-left: 4px;
    color: var(--text-muted);
  }
  .rename {
    flex: 1;
    min-width: 4ch;
    max-width: 280px;
    padding: 0 4px;
    border: 1px solid var(--accent);
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text);
    font: inherit;
  }
  .close {
    /* Every tab shows its ×, active or not, without waiting for the hover. */
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 18px;
    height: 18px;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    line-height: 1;
  }
  .arrow {
    border: none;
    background: transparent;
    color: var(--text-muted);
    padding: 0 8px;
    cursor: pointer;
  }
  .new {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    align-self: center;
    width: 28px;
    height: 28px;
    margin-left: 6px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
    outline: none;
    box-shadow: none;
  }
  .new:hover:not([aria-disabled="true"]),
  .new:focus-visible:not([aria-disabled="true"]) {
    border: none;
    background: var(--surface-3);
    color: var(--text);
    outline: none;
    box-shadow: none;
  }
  .new:focus-visible {
    outline: none;
    box-shadow: none;
  }
  .new[aria-disabled="true"] {
    opacity: 0.45;
    cursor: default;
    background: transparent;
    color: var(--text-muted);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
