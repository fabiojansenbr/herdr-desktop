<script lang="ts">
  // Nested tabs of one workspace of the sidebar (spec 042, PRD nova-lateral P2), rendered under
  // the row while it is expanded. Each line is one tab of that workspace: status icon, the type
  // avatar, the same title the center bar shows (026) and the time since the last state change
  // this client observed (014: the engine publishes no timestamp, so "—" until it sees one).
  //
  // Spec 052: expansion does not follow the focus, so this list also serves a workspace that is
  // not the focused one — including one on another host, read from the snapshot the hub carries
  // (039), the only source the sidebar has for a host the window is not attached to. Only the
  // focused workspace of the selected host marks a line as selected (the highlight of P7).
  //
  // Spec 057/074: the snapshot is also the selected host's source whenever the live list has no tab
  // to offer — a host whose agents API lane is absent (039) never answers `tab.list`, and an SSH
  // attach takes seconds of serial probes — so the rule of `tab-rows.ts` (`effectiveTabs`) is the
  // one the center bar takes too, with the snapshot's focused tab as the selected line.
  //
  // Clicking a line of the focused workspace sends the engine one `tab.focus`, the same action the
  // bar's tab sends; the focused line sends nothing when clicked again. Anywhere else the click
  // runs the 043 chain (`navigate.ts`): the host switch when it is another host, one
  // `workspace_focus`, and the `tab.focus` only after the metadata of that host confirms.
  // Nothing here reads a pane: the list ages with its own 30 s timer, which exists only while
  // the sidebar is mounted, so no pane is repainted by the clock.
  import { onDestroy, untrack } from "svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import { t } from "../../i18n/index.svelte";
  import { createActivityLedger } from "../agents/panel";
  import { createTabNavigator } from "./navigate";
  import type { SidebarRow } from "./sidebar-model";
  import StatusIcon from "./StatusIcon.svelte";
  import { effectiveTabs, hostSource, liveSource, TAB_TIME_TICK_MS, tabRows, type TabRow } from "./tab-rows";

  let { ctx, row, clock = () => Date.now() }: { ctx: FrameContext; row: SidebarRow; clock?: () => number } = $props();

  const ledger = createActivityLedger();
  /** The 30 s tick; it only exists to re-derive the rows while nothing else changes (0 = none). */
  let pulse = $state(0);
  const timer = setInterval(() => (pulse = clock()), TAB_TIME_TICK_MS);
  onDestroy(() => clearInterval(timer));

  const selectedEndpoint = $derived(ctx.selectedEndpoint ?? ctx.surface.selection?.endpoint ?? null);
  const rowHost = $derived((ctx.connections?.view?.hub.hosts ?? []).find((host) => host.endpoint === row.endpoint) ?? null);
  /**
   * Spec 074 (AC-074-01): the one rule of `tab-rows.ts` decides the source — the live list only
   * while it belongs to this host, is `connected` and carries a tab; the hub snapshot (039)
   * otherwise. That covers both hosts the window is not attached to (052), a host without the API
   * lane, which answers no `tab.list` (057), and the attach of the selected host while its serial
   * probes have not answered yet, which is what left this list empty in the report.
   */
  const decision = $derived(
    effectiveTabs({ endpoint: row.endpoint, host: rowHost, agents: ctx.agents, selectedEndpoint }),
  );
  const source = $derived(
    decision.live ? liveSource(ctx.agents, ctx.surface.identity?.pane_id ?? null) : hostSource(rowHost, row.workspaceId),
  );
  /**
   * Row the engine is already showing: the focused workspace of the selected host (036 gives
   * `active` to that one only) while the agents connection is this host's — live or, as the 057
   * host and the pending attach of 074 are, read from its snapshot. Only such a row carries the
   * highlight (P7) and focuses a tab with a single `tab.focus`.
   */
  const attached = $derived(row.active && decision.attached);

  const nav = untrack(() => createTabNavigator(ctx, () => selectedEndpoint));
  $effect(() => {
    nav.settle();
  });

  // Seeing the list is what dates each transition, so it is observed as the rows are derived: the
  // first paint already carries the time of the list it is showing.
  const rows = $derived.by(() => {
    const at = Math.max(pulse, clock());
    const current = source;
    ledger.observe([...current.agents], at);
    return tabRows({
      source: current,
      workspaceId: row.workspaceId,
      selectable: attached,
      ledger,
      now: at,
    });
  });

  function focus(tab: TabRow) {
    if (tab.selected) return;
    // The focused workspace of the selected host is already where the engine is: one `tab.focus`
    // and nothing else (AC-042-03), the same call the bar's tab makes — it routes by the endpoint's
    // own method (039), so a host without the API lane focuses its tab this way too (AC-057-02).
    if (attached) {
      void ctx.controllers.agents.focusTab(tab.tabId);
      return;
    }
    if (!row.workspaceId) return;
    void nav.open({ endpoint: row.endpoint, workspaceId: row.workspaceId, tabId: tab.tabId });
  }
</script>

{#if rows.length > 0}
  <ul class="tabs" data-sidebar-tabs aria-label={t("sidebar.tabs.list", { count: rows.length, name: row.name })}>
    {#each rows as tab (tab.tabId)}
      <li>
        <div
          class="tab"
          role="button"
          tabindex="0"
          data-tab-row={tab.tabId}
          data-selected={tab.selected ? "true" : undefined}
          aria-current={tab.selected ? "true" : undefined}
          aria-label={tab.label}
          title={tab.label}
          onclick={() => focus(tab)}
          onkeydown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            focus(tab);
          }}
        >
          <StatusIcon status={tab.status} />
          <span class="avatar" data-tab-avatar data-kind={tab.kind} aria-hidden="true">{tab.avatar}</span>
          <span class="title" data-tab-title><span class="names">{tab.title}</span>{#if tab.more > 0}<span class="more">+{tab.more}</span>{/if}</span>
          <span class="time" data-tab-time>{tab.time}</span>
        </div>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .tabs {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin: 1px 0 2px;
    padding: 0;
    list-style: none;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px 4px 38px;
    border-radius: 8px;
    cursor: pointer;
    user-select: none;
    font-size: 13.5px;
    color: var(--text-muted, #A3A3A3);
    transition: background-color 0.15s ease;
  }
  .tab:hover {
    background-color: var(--surface-2, #171717);
  }
  .tab:focus-visible {
    outline: 2px solid var(--accent, #D4D4D4);
    outline-offset: -2px;
  }
  .tab[data-selected="true"] {
    background-color: var(--surface-3, #242424);
    font-weight: 500;
    color: var(--text, #EDEDED);
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
    color: var(--text, #EDEDED);
    font-size: 10px;
    line-height: 1;
  }
  .title {
    display: inline-flex;
    align-items: center;
    flex: 1;
    min-width: 0;
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
  }
  .time {
    flex: none;
    font-size: 12px;
    color: var(--text-dim, #6B6B6B);
    font-variant-numeric: tabular-nums;
  }
</style>
