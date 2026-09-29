<script lang="ts">
  // Spec 012 — one card of the Servers section: icon, name, type, the Local engine summary
  // (`herdr <versão> · <panes> panes`) or the SSH address, and the state (`conectado`, `<ms>`,
  // `offline`). Clicking selects the host through the window's existing selection action.
  import { t } from "../../i18n/index.svelte";
  import type { HomeServer } from "./home-model";

  interface Props {
    server: HomeServer;
    onselect: (endpoint: string) => void;
  }

  let { server, onselect }: Props = $props();
</script>

<button
  type="button"
  class="server"
  class:offline={server.tone === "offline"}
  data-server-card={server.endpoint}
  data-tone={server.tone}
  aria-pressed={server.active}
  title={t("home.server.select", { name: server.name })}
  onclick={() => onselect(server.endpoint)}
>
  <span class="icon" aria-hidden="true">
    {#if server.typeBadge === "Local"}
      <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect width="20" height="14" x="2" y="3" rx="2" /><line x1="8" x2="16" y1="21" y2="21" /><line x1="12" x2="12" y1="17" y2="21" />
      </svg>
    {:else}
      <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect width="20" height="8" x="2" y="2" rx="2" /><rect width="20" height="8" x="2" y="14" rx="2" /><line x1="6" x2="6.01" y1="6" y2="6" /><line x1="6" x2="6.01" y1="18" y2="18" />
      </svg>
    {/if}
  </span>
  <span class="head">
    <span class="name">{server.name}</span>
    <span class="badge">{server.typeBadge}</span>
  </span>
  <span class="detail" data-detail>{server.detail}</span>
  <span class="state" data-state>
    <span class="dot" aria-hidden="true"></span>
    {server.state}
  </span>
</button>

<style>
  .server {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 2px 8px;
    min-width: 0;
    padding: 10px 12px;
    text-align: left;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface-2);
    color: var(--text);
    font-family: var(--font-ui);
    cursor: pointer;
  }
  .server:hover {
    background: var(--surface-3);
  }
  .server:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .server[aria-pressed="true"] {
    border-color: var(--accent);
  }
  .icon {
    grid-row: span 2;
    display: inline-grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: var(--surface-3);
    color: var(--text-muted);
  }
  .head {
    grid-column: 2;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .name {
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    flex: 0 0 auto;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 10px;
    color: var(--text-muted);
  }
  .detail {
    grid-column: 2 / 3;
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .state {
    grid-row: 1 / span 2;
    grid-column: 3;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    align-self: center;
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--idle);
  }
  .server[data-tone="ok"] .dot {
    background: var(--working);
  }
  .server[data-tone="warn"] .dot,
  .server[data-tone="attention"] .dot {
    background: var(--attention);
  }
  .server[data-tone="offline"] .dot {
    background: var(--idle);
  }
</style>
