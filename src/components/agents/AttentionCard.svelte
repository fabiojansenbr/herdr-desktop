<script lang="ts">
  // Spec 014 AC-014-02 — one card of "Precisa da sua atenção". The card is a button: click or
  // Enter focuses that pane in the engine once (the GUI answers nothing and sends no key to
  // the agent). The last line is the engine's detection snapshot, shown as received.
  import { t } from "../../i18n/index.svelte";
  import type { PanelAgent } from "./panel";

  interface Props {
    agent: PanelAgent;
    cache: boolean;
    cacheText: string;
    onfocus: (paneId: string) => void;
  }

  let { agent, cache, cacheText, onfocus }: Props = $props();
</script>

<button
  type="button"
  class="card"
  data-attention-pane={agent.paneId}
  data-status={agent.status}
  disabled={!agent.canFocus}
  aria-label={t("agents.card.goTo", { name: agent.name, path: agent.path, state: agent.label })}
  title={agent.canFocus ? t("agents.card.focus") : t("agents.card.focusUnavailable")}
  onclick={() => onfocus(agent.paneId)}
>
  <span class="head">
    <span class="avatar" aria-hidden="true">{agent.avatar}</span>
    <span class="name">{agent.name}</span>
    <span class="path">{agent.path}</span>
    <span class="time" data-time>{agent.time}</span>
  </span>
  <span class="state">
    <span aria-hidden="true">{agent.icon}</span>
    {agent.label}
    {#if cache}
      <span class="cache" data-cache>{cacheText}</span>
    {/if}
  </span>
  {#if agent.lastLine}
    <span class="line" data-last-line>{agent.lastLine}</span>
  {/if}
</button>

<style>
  .card {
    display: grid;
    gap: 4px;
    width: 100%;
    min-width: 0;
    padding: 8px 10px;
    text-align: left;
    border: 1px solid var(--attention);
    border-radius: 10px;
    background: var(--surface-2);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .card:disabled {
    cursor: default;
    border-color: var(--border);
  }
  .card:hover:not(:disabled) {
    background: var(--surface-3);
  }
  .card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }
  .avatar {
    flex: 0 0 auto;
    width: 18px;
    height: 18px;
    display: inline-grid;
    place-items: center;
    border-radius: 6px;
    background: var(--accent-soft);
    color: var(--text);
    font-size: 11px;
    text-transform: uppercase;
  }
  .name {
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
  }
  .path,
  .state,
  .time,
  .line {
    font-size: 11px;
    color: var(--text-muted);
  }
  .path {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .time {
    flex: 0 0 auto;
  }
  .state {
    color: var(--attention);
  }
  .cache {
    margin-left: 4px;
    padding: 0 4px;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-muted);
  }
  .line {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
</style>
