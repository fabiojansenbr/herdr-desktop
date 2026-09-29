<script lang="ts">
  // Spec 014 AC-014-03 — one row of "Em execução": avatar by engine kind, name, `projeto › tab`,
  // the engine's terminal title as the activity summary, the relative time of the last observed
  // transition and the state with its own text and icon (never colour alone).
  import { t } from "../../i18n/index.svelte";
  import type { PanelAgent } from "./panel";

  interface Props {
    agent: PanelAgent;
    onfocus: (paneId: string) => void;
  }

  let { agent, onfocus }: Props = $props();
</script>

<li data-running-pane={agent.paneId} data-status={agent.status}>
  <button
    type="button"
    class="row"
    disabled={!agent.canFocus}
    aria-label={t("agents.card.goTo", { name: agent.name, path: agent.path, state: agent.label })}
    onclick={() => onfocus(agent.paneId)}
  >
    <span class="avatar" data-kind={agent.kind} aria-hidden="true">{agent.avatar}</span>
    <span class="body">
      <span class="head">
        <span class="name">{agent.name}</span>
        <span class="path">{agent.path}</span>
      </span>
      <span class="summary" data-summary>{agent.summary ?? agent.label}</span>
    </span>
    <span class="meta">
      <span class="state tone-{agent.tone}" data-state>
        <span aria-hidden="true">{agent.icon}</span>
        {agent.label}
      </span>
      <span class="time" data-time>{agent.time}</span>
    </span>
  </button>
</li>

<style>
  li {
    list-style: none;
    min-width: 0;
  }
  .row {
    display: flex;
    gap: 8px;
    width: 100%;
    min-width: 0;
    padding: 6px 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:disabled {
    cursor: default;
  }
  .row:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .avatar {
    flex: 0 0 auto;
    width: 22px;
    height: 22px;
    display: inline-grid;
    place-items: center;
    border-radius: 50%;
    background: var(--surface-3);
    color: var(--text);
    font-size: 11px;
    text-transform: uppercase;
  }
  .body {
    flex: 1 1 auto;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .head {
    display: flex;
    gap: 6px;
    align-items: baseline;
    min-width: 0;
  }
  .name {
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
  }
  .path,
  .summary,
  .state,
  .time {
    font-size: 11px;
    color: var(--text-muted);
  }
  .path,
  .summary {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    flex: 0 0 auto;
    display: grid;
    justify-items: end;
    gap: 2px;
  }
  .state {
    white-space: nowrap;
  }
  .tone-busy {
    color: var(--working);
  }
  .tone-attention {
    color: var(--attention);
  }
  .tone-success {
    color: var(--working);
  }
  .tone-calm,
  .tone-muted {
    color: var(--text-muted);
  }
</style>
