<script lang="ts">
  // The collapsed sidebar (spec 047, `design/v2-04-lateral-recolhida.png`): a 56 px rail with the
  // mark, Novo agente, "Precisa de você" with its badge, a separator, one avatar per visible
  // workspace and the host at the foot (spec 056 took Buscar out of the rail, as it did out of
  // the open sidebar; the palette stays on Ctrl K). Nothing here is a second source of truth: the
  // groups and the status come from the 041 model (`buildSidebarSections`/`sidebarStatus`), the
  // badge is the 043 count (`inbox-model`), the host is the 048 switcher with its popover, and a
  // click on an avatar takes the same controller path as the open row (025/035 focus, 044 hidden
  // rows left out) — so the rail can never disagree with the sidebar it replaces.
  //
  // Spec 049 (AC-049-04): the rail is also the way back. The mark and every item expand the
  // sidebar through `ctx.actions.showProjects` — the very action "Precisa de você" already used
  // — and still do their own job. The host at the foot is the exception: expanding would close
  // the 048 popover it opens before the user can pick a host.
  import { onDestroy } from "svelte";
  import BrandMark from "./BrandMark.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { platformOf, shortcutLabel } from "../../shell/shortcuts";
  import { createActivityLedger } from "../agents/panel";
  import NewAgentPopover from "../center/NewAgentPopover.svelte";
  import HostSwitcher from "./HostSwitcher.svelte";
  import {
    createInboxSeen,
    inboxAgents,
    inboxItems,
    ledgerAgents,
    INBOX_TICK_MS,
    inboxTitle,
  } from "./inbox-model";
  import { buildSidebarSections, effectiveAgents, type SidebarCollection, type SidebarRow } from "./sidebar-model";
  import { TAB_STATUS_TEXT } from "./tab-rows";

  let { ctx, clock = () => Date.now() }: { ctx: FrameContext; clock?: () => number } = $props();

  const platform = platformOf(typeof navigator === "undefined" ? "" : navigator.userAgent);
  const newAgentHint = shortcutLabel("new-agent", platform);

  let popover = $state(false);
  /** Avatar under the pointer or holding focus: the tooltip of the design follows it. */
  let hovered = $state<{ key: string; top: number } | null>(null);

  const ledger = createActivityLedger();
  const seen = createInboxSeen();
  /** The 30 s tick of the box (014/043): the badge ages without the engine sending anything. */
  let pulse = $state(0);
  const timer = setInterval(() => (pulse = clock()), INBOX_TICK_MS);
  onDestroy(() => clearInterval(timer));

  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const selectedEndpoint = $derived(ctx.selectedEndpoint ?? ctx.surface.selection?.endpoint ?? null);
  const focusedTabId = $derived(ctx.agents?.tabFocus?.tab_id ?? ctx.agents?.tabs?.find((tab) => tab.focused)?.tab_id ?? null);

  /** Exactly the items the open box would list (AC-043-01), counted for the badge. */
  const waiting = $derived.by(() => {
    const at = Math.max(pulse, clock());
    const agents = inboxAgents({
      hosts,
      selectedEndpoint,
      live: ctx.agents?.agents ?? [],
      liveEndpoint: ctx.agents?.identity?.endpoint ?? null,
    });
    ledger.observe(ledgerAgents(agents), at);
    const all = inboxItems(agents, { ledger, now: at });
    seen.visit({ endpoint: selectedEndpoint, tabId: focusedTabId, items: all });
    return all.filter((item) => !seen.dismissed(item)).length;
  });

  const sections = $derived(
    buildSidebarSections({
      hosts,
      groups: ctx.navigator?.snapshot?.collections ?? [],
      projects: ctx.navigator?.snapshot?.projects ?? [],
      agents: effectiveAgents(ctx.agents?.agents ?? [], hosts),
      opening: ctx.navigator?.opening ?? {},
      selectedEndpoint: ctx.selectedEndpoint,
      prefs: ctx.navigator?.snapshot?.workspace_prefs ?? [],
    }),
  );

  interface RailEntry {
    readonly key: string;
    readonly row: SidebarRow;
    readonly collection: SidebarCollection;
    readonly initials: string;
    readonly color: string;
    readonly meta: string;
    readonly status: string;
    readonly label: string;
  }

  /**
   * Initials of the design (`erp-api` → `EA`, `portal-web` → `PW`, `herdr` → `HE`): the first
   * letter of the first two words, or the first two letters of a single word.
   */
  function initialsOf(name: string): string {
    const words = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
    if (words.length === 0) return name.slice(0, 2).toUpperCase();
    if (words.length === 1) return words[0]!.slice(0, 2).toUpperCase();
    return `${words[0]![0]}${words[1]![0]}`.toUpperCase();
  }

  // The hidden rows (044/P5) stay out: the rail shows what the open list shows, in its order.
  const entries = $derived<RailEntry[]>(
    sections.collections.flatMap((collection) =>
      collection.rows.map((row) => {
        const status = row.status.label || TAB_STATUS_TEXT[row.status.kind];
        const meta = [collection.name, row.branch].filter(Boolean).join(" · ");
        return {
          key: `${row.endpoint}:${row.id}`,
          row,
          collection,
          initials: initialsOf(row.name),
          color: row.color ?? collection.color,
          meta,
          status,
          label: [row.name, meta, row.offline ? t("sidebar.offline") : null, status].filter(Boolean).join(", "),
        };
      }),
    ),
  );

  const tip = $derived(entries.find((entry) => entry.key === hovered?.key) ?? null);

  function show(key: string, target: EventTarget | null) {
    const box = target instanceof HTMLElement ? target.getBoundingClientRect() : null;
    hovered = { key, top: box?.top ?? 0 };
  }

  /** AC-049-04: every item of the rail opens the sidebar, once, before doing its own job. */
  function expand() {
    ctx.actions.showProjects();
  }

  /** The row's own path (AC-025-02/03, 035): focus a live workspace, open a closed project. */
  function open(row: SidebarRow) {
    expand();
    if (row.disabled) return;
    if (row.kind === "closed") {
      if (row.projectId) void ctx.controllers.projects.openClosedProject(row.projectId);
      return;
    }
    if (row.workspaceId) void ctx.controllers.projects.focusWorkspace(row.endpoint, row.workspaceId);
  }
</script>

<nav class="rail" data-slot="rail" aria-label={t("sidebar.rail.label")}>
  <button type="button" class="mark" data-rail-item="brand" aria-label={t("sidebar.rail.expand")} title={t("sidebar.rail.brand")} onclick={expand}>
    <BrandMark size={32} />
  </button>

  <div class="new-agent">
    <button
      type="button"
      class="icon-button primary"
      data-rail-item="newAgent"
      aria-haspopup="dialog"
      aria-expanded={popover}
      aria-label={t("sidebar.rail.newAgent", { hint: newAgentHint })}
      title={t("sidebar.rail.newAgent", { hint: newAgentHint })}
      onclick={() => {
        expand();
        popover = !popover;
      }}
    >
      <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
      </svg>
    </button>
    {#if popover}
      <NewAgentPopover {ctx} onclose={() => (popover = false)} />
    {/if}
  </div>

  <button
    type="button"
    class="icon-button"
    data-rail-item="inbox"
    data-attention={waiting > 0 ? "true" : undefined}
    aria-label={waiting > 0
      ? t("sidebar.rail.inboxCount", { title: inboxTitle(), count: waiting })
      : t("sidebar.rail.inboxEmpty", { title: inboxTitle() })}
    title={inboxTitle()}
    onclick={expand}
  >
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 13h5l1.5 3h5L16 13h5" />
      <path d="M5.5 5.5h13L21 13v5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18v-5Z" />
    </svg>
    {#if waiting > 0}
      <span class="badge" data-rail-inbox-count>{waiting}</span>
    {/if}
  </button>

  <span class="separator" data-rail-separator aria-hidden="true"></span>

  <div class="avatars">
    {#each entries as entry (entry.key)}
      <button
        type="button"
        class="avatar"
        data-rail-workspace={entry.key}
        data-rail-color={entry.color}
        data-active={entry.row.active ? "true" : undefined}
        data-offline={entry.row.offline ? "true" : undefined}
        aria-current={entry.row.active ? "true" : undefined}
        aria-label={entry.label}
        onmouseenter={(event) => show(entry.key, event.currentTarget)}
        onmouseleave={() => (hovered = null)}
        onfocus={(event) => show(entry.key, event.currentTarget)}
        onblur={() => (hovered = null)}
        onclick={() => open(entry.row)}
      >
        <span class="initials" data-rail-initials style={entry.row.active ? undefined : `color: ${entry.color};`}>{entry.initials}</span>
        {#if entry.row.status.kind !== "idle"}
          <span class="dot" data-rail-dot data-tone={entry.row.status.kind}></span>
        {/if}
      </button>
    {/each}
  </div>

  <div class="host" data-rail-item="host">
    <HostSwitcher {ctx} />
  </div>

  {#if tip}
    <div class="tooltip" data-rail-tooltip role="tooltip" style={`top: ${hovered?.top ?? 0}px;`}>
      <span class="tip-name" data-rail-tip-name>{tip.row.name}</span>
      <span class="tip-meta" data-rail-tip-meta>{tip.meta}</span>
      <span class="tip-status" data-rail-tip-status data-tone={tip.row.status.kind}>{tip.status}</span>
    </div>
  {/if}
</nav>

<style>
  .rail {
    position: relative;
    height: 100%;
    min-height: 0;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 10px 0 8px;
    background-color: var(--surface, #111318);
    overflow: visible;
  }
  /* AC-049-04: the mark is the shortest way back to the open sidebar, so it is a real button. */
  .mark {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    flex-shrink: 0;
    padding: 0;
    border: none;
    border-radius: 8px;
    background: transparent;
    margin-bottom: 4px;
    cursor: pointer;
  }
  .mark:hover {
    opacity: 0.85;
  }
  .mark:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .new-agent {
    position: relative;
    flex-shrink: 0;
  }
  /* The rail is 56 px wide: the popover opens beside it, not inside it. */
  .new-agent :global([data-new-agent-popover]) {
    left: calc(100% + 8px);
    right: auto;
    top: 0;
  }
  .icon-button {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    flex-shrink: 0;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 8px;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease;
  }
  .icon-button:hover {
    background-color: var(--surface-2, #171a21);
    color: var(--text, #e7e9ee);
  }
  .icon-button:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -2px;
  }
  .icon-button.primary {
    background-color: var(--surface-2, #171a21);
    color: var(--accent, #8fa8ff);
  }
  .icon-button.primary:hover {
    background-color: var(--surface-3, #1e222b);
  }
  .badge {
    position: absolute;
    top: 0;
    right: 0;
    min-width: 15px;
    height: 15px;
    padding: 0 4px;
    border-radius: 999px;
    background-color: var(--attention, #f4b454);
    color: var(--bg, #0b0c10);
    font-size: 10px;
    font-weight: 700;
    line-height: 15px;
    font-variant-numeric: tabular-nums;
  }
  .separator {
    flex-shrink: 0;
    width: 24px;
    height: 1px;
    margin: 2px 0;
    background-color: var(--border, #242833);
  }
  .avatars {
    flex: 1;
    min-height: 0;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
    overflow-y: auto;
    scrollbar-width: none;
  }
  .avatar {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    flex-shrink: 0;
    padding: 0;
    background-color: var(--surface-2, #171a21);
    border: 1px solid transparent;
    border-radius: 9px;
    font: inherit;
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background-color 0.15s ease, border-color 0.15s ease;
  }
  .avatar:hover {
    background-color: var(--surface-3, #1e222b);
  }
  .avatar:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: 1px;
  }
  .avatar[data-active="true"] {
    background-color: var(--surface-3, #242424);
    border-color: transparent;
    color: var(--text, #ededed);
  }
  .avatar[data-active="true"] .initials {
    color: var(--text, #ededed);
  }
  .avatar[data-offline="true"] {
    opacity: 0.55;
  }
  .initials {
    letter-spacing: 0.02em;
  }
  .dot {
    position: absolute;
    right: -2px;
    bottom: -2px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    border: 2px solid var(--surface, #111318);
    box-sizing: content-box;
  }
  [data-rail-dot][data-tone="working"] {
    background-color: var(--working, #5bd68a);
  }
  [data-rail-dot][data-tone="waiting"] {
    background-color: var(--attention, #f4b454);
  }
  /* Concluído: the green outline of the design, not a filled dot. */
  [data-rail-dot][data-tone="done"] {
    background-color: var(--surface, #111318);
    box-shadow: inset 0 0 0 2px var(--working, #5bd68a);
  }
  .host {
    flex-shrink: 0;
    width: 100%;
  }
  .host :global([data-sidebar-hosts]) {
    padding: 6px 4px 0;
    align-items: center;
  }
  .host :global([data-host-trigger]) {
    justify-content: center;
    padding: 8px 0;
  }
  .host :global([data-host-trigger] .text),
  .host :global([data-host-trigger] .chevrons) {
    display: none;
  }
  /* The 048 popover is anchored to the foot; beside the rail it needs its own width. */
  .host :global([data-host-popover]) {
    left: calc(100% + 8px);
    right: auto;
    width: 264px;
  }
  .tooltip {
    position: fixed;
    left: 62px;
    z-index: 95;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: 260px;
    padding: 8px 10px;
    border-radius: 8px;
    background-color: var(--surface-3, #1e222b);
    border: 1px solid var(--border, #242833);
    box-shadow: 0 10px 24px rgba(0, 0, 0, 0.45);
    pointer-events: none;
  }
  .tip-name {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text, #e7e9ee);
  }
  .tip-meta {
    font-size: 11.5px;
    color: var(--text-muted, #8c93a3);
  }
  .tip-status {
    font-size: 11.5px;
    color: var(--text-muted, #8c93a3);
  }
  [data-rail-tip-status][data-tone="working"],
  [data-rail-tip-status][data-tone="done"] {
    color: var(--working, #5bd68a);
  }
  [data-rail-tip-status][data-tone="waiting"] {
    color: var(--attention, #f4b454);
  }
</style>
