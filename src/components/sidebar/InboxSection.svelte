<script lang="ts">
  // "Precisa de você" (spec 043, `design/v2-01-workspace.png`): the agents of every host that are
  // waiting for the user (amber) or have finished (green), one click away. The box is not drawn
  // at all when nothing needs the user.
  //
  // One click takes the user to the agent through the paths that already exist: the host switch
  // of 035/037 (no reconnect), the workspace focus of 025 and the `tab.focus` of 042 — the last
  // one only after the new host's metadata confirms, so the focus never lands on the host the
  // user just left. Nothing here answers an agent or sends it a key: the interaction is in the
  // terminal.
  //
  // Spec 051: the box has no decoration of its own. Its header is the `WORKSPACES` header (same
  // structure, same classes, same declarations) and each item is a line of the tab list (042):
  // the status icon, the 16 px avatar, the workspace and the time — a card, a coloured edge or a
  // repeated "Concluído" would only make the sidebar louder than what it is reporting.
  import { onDestroy } from "svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { createActivityLedger } from "../agents/panel";
  import StatusIcon from "./StatusIcon.svelte";
  import {
    createInboxSeen,
    inboxAgents,
    inboxItems,
    ledgerAgents,
    INBOX_TICK_MS,
    inboxTitle,
    type InboxItem,
  } from "./inbox-model";

  let { ctx, clock = () => Date.now() }: { ctx: FrameContext; clock?: () => number } = $props();

  const ledger = createActivityLedger();
  const seen = createInboxSeen();
  /** The 30 s tick; it only exists to re-derive the items while nothing else changes (0 = none). */
  let pulse = $state(0);
  const timer = setInterval(() => (pulse = clock()), INBOX_TICK_MS);
  onDestroy(() => clearInterval(timer));

  /** Tab the click is waiting to focus once the new host's metadata confirms (AC-043-02). */
  let pending: { endpoint: string; tabId: string } | null = null;
  let busy = false;

  const hosts = $derived(ctx.connections?.view?.hub.hosts ?? []);
  const selectedEndpoint = $derived(ctx.selectedEndpoint ?? ctx.surface.selection?.endpoint ?? null);
  const focusedTabId = $derived(
    ctx.agents?.tabFocus?.tab_id ?? ctx.agents?.tabs?.find((tab) => tab.focused)?.tab_id ?? null,
  );

  // Seeing the engine lists is what dates each transition and what tells the box the user is
  // already in a finished agent's tab, so both happen as the items are derived.
  const items = $derived.by(() => {
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
    return all.filter((item) => !seen.dismissed(item));
  });

  /**
   * Sends the pending `tab.focus` as soon as the window holds the metadata of the host the item
   * lives on: the engine only accepts a tab of the connection it confirmed (spec 027/042).
   */
  function focusPendingTab() {
    const state = ctx.agents;
    if (!pending) return;
    if (state?.identity?.endpoint !== pending.endpoint) return;
    if (!state.tabs.some((tab) => tab.tab_id === pending!.tabId)) return;
    const { tabId } = pending;
    pending = null;
    void ctx.controllers.agents.focusTab(tabId);
  }

  $effect(() => {
    focusPendingTab();
  });

  async function open(item: InboxItem) {
    if (busy) return;
    busy = true;
    try {
      if (item.endpoint !== selectedEndpoint) {
        try {
          await ctx.controllers.surface.select(item.endpoint);
        } catch {
          // The refusal is shown on the surface; nothing else is sent to a host we did not reach.
          return;
        }
      }
      await ctx.controllers.projects.focusWorkspace(item.endpoint, item.workspaceId);
      if (!item.tabId) return;
      pending = { endpoint: item.endpoint, tabId: item.tabId };
      focusPendingTab();
    } finally {
      busy = false;
    }
  }
</script>

{#if items.length > 0}
  <section class="inbox" data-sidebar-inbox-section aria-label={inboxTitle()}>
    <header class="section-head">
      <span class="section-title" data-inbox-title>{inboxTitle()}</span>
      <span class="section-count" data-inbox-count>{items.length}</span>
    </header>
    <ul>
      {#each items as item (item.id)}
        <li>
          <button
            type="button"
            class="item"
            data-inbox-item={item.id}
            data-kind={item.kind}
            data-endpoint={item.endpoint}
            aria-label={item.label}
            title={item.label}
            onclick={() => void open(item)}
          >
            <span class="line">
              <StatusIcon status={item.kind === "blocked" ? "waiting" : "done"} />
              <span class="avatar" data-inbox-avatar aria-hidden="true">{item.avatar}</span>
              <span class="text" data-inbox-text>{item.workspace}</span>
              <span class="time" data-inbox-time>{item.time}</span>
            </span>
            <!-- Only what the engine detected earns a second line; the state is already the icon. -->
            {#if item.detection}
              <span class="detail" data-inbox-detail>{item.detection}</span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .inbox {
    display: flex;
    flex-direction: column;
    padding-top: 6px;
  }
  /* AC-063-02: cabeçalho em destaque âmbar, bloco com fundo --attention-soft e border-radius: 8px. */
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 32px;
    padding: 0 10px;
    margin: 0 4px 2px;
    flex-shrink: 0;
    background-color: var(--attention-soft, #2b2112);
    border: none;
    border-radius: 8px;
  }
  .section-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--attention, #e9a23b);
  }
  .section-count {
    font-size: 14px;
    font-weight: 600;
    color: var(--attention, #e9a23b);
    flex-shrink: 0;
  }
  ul {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0 4px;
    list-style: none;
  }
  .item {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-width: 0;
    padding: 0 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text, #ededed);
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }
  .item:hover,
  .item:focus-visible {
    background-color: var(--surface-2, #171717);
  }
  .item:focus-visible {
    outline: 2px solid var(--accent, #d4d4d4);
    outline-offset: -2px;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 30px;
    min-width: 0;
  }
  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    flex: none;
    border-radius: 4px;
    background-color: var(--surface-3, #242424);
    color: var(--text, #ededed);
    font-size: 10px;
    line-height: 1;
  }
  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 14px;
    font-weight: 600;
    color: var(--text, #ededed);
  }
  .time {
    flex: none;
    font-size: 12px;
    color: var(--text-dim, #6b6b6b);
    font-variant-numeric: tabular-nums;
  }
  /* Aligned with the text of the line above it, and never more than one line. */
  .detail {
    min-width: 0;
    padding: 0 0 4px 46px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-muted, #8c93a3);
  }
</style>
