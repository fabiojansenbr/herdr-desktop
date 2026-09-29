<script lang="ts">
  // Connections grouped by host for the composed window (spec 007). Each host shows its state in
  // text, its explicit connect/cancel/retry actions and whether it is the selected host. Choosing
  // a host is explicit; nothing here connects on render. Pane input stays on the one terminal.
  import { errorText, t } from "../i18n/index.svelte";
  import type { ConnectionsController, ConnectionsState } from "../connections/controller";
  import { ACTION_TEXT, hostActions, hostStatus } from "../connections/presentation";
  import type { HostDto } from "../connections/types";

  interface Props {
    controller: ConnectionsController;
    state: ConnectionsState;
    selected: string | null;
    onSelect: (endpoint: string) => void;
  }

  let { controller, state, selected, onSelect }: Props = $props();

  const hosts = $derived(state.view?.hub.hosts ?? []);

  function act(host: HostDto, action: string) {
    if (action === "cancel") void controller.cancel(host.endpoint);
    else void controller.connect(host.endpoint);
  }
</script>

<section class="hosts" aria-label={t("connections.panel.title")}>
  <header>
    <h2>{t("connections.panel.title")}</h2>
    <button type="button" onclick={() => controller.openDialog()}>{t("shell.hosts.newSsh")}</button>
  </header>

  {#if state.globalError}
    <p class="host-error" role="alert">{errorText(state.globalError)}</p>
  {/if}
  {#if state.loading && hosts.length === 0}
    <p class="muted">{t("connections.panel.loading")}</p>
  {:else if hosts.length === 0}
    <p class="muted">{t("shell.hosts.empty")}</p>
  {/if}

  <ul>
    {#each hosts as host (host.endpoint)}
      {@const status = hostStatus(host)}
      {@const isSelected = host.endpoint === selected}
      <li class="host" class:selected={isSelected} data-host={host.endpoint} data-phase={host.phase}>
        <div class="row">
          <strong>{host.label}</strong>
          <span class="kind">{host.kind === "local" ? t("connections.kind.local") : "SSH"}</span>
          <span class="status tone-{status.tone}">{status.text}</span>
        </div>
        {#if host.target}<div class="target">{host.target}</div>{/if}
        <div class="session">{t("connections.session", { name: host.session })}</div>
        {#if status.detail}<div class="muted">{status.detail}</div>{/if}
        {#if host.guidance}<div class="muted">{host.guidance}</div>{/if}
        {#if host.connection_error}<p class="host-error">{errorText(host.connection_error)}</p>{/if}
        {#if state.hostErrors[host.endpoint]}<p class="host-error">{errorText(state.hostErrors[host.endpoint]!)}</p>{/if}
        <div class="row actions">
          {#each hostActions(host) as action (action)}
            <button type="button" onclick={() => act(host, action)}>{ACTION_TEXT[action]}</button>
          {/each}
          <button
            type="button"
            aria-pressed={isSelected}
            disabled={isSelected}
            onclick={() => onSelect(host.endpoint)}
          >
            {isSelected ? t("shell.hosts.selected") : t("shell.hosts.use")}
          </button>
        </div>
      </li>
    {/each}
  </ul>

  <button type="button" class="link" onclick={() => controller.importProfiles()}>{t("shell.hosts.import")}</button>
  {#if state.importReport}
    <p class="muted" role="status">
      {t("shell.hosts.imported", { count: state.importReport.imported.length })}, {t("shell.hosts.skipped", {
        count: state.importReport.skipped.length,
      })}, {t("shell.hosts.alreadyPresent", { count: state.importReport.already_present })}.
    </p>
  {/if}
</section>

<style>
  .hosts {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    height: 100%;
    overflow: auto;
    color: var(--pen-text);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--pen-text-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .host {
    border: 1px solid var(--pen-border);
    border-radius: 8px;
    padding: 8px;
    background: var(--pen-surface-2);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .host.selected {
    border-color: var(--pen-accent);
  }
  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }
  .kind,
  .target,
  .session,
  .muted {
    color: var(--pen-text-muted);
    font-size: 12px;
  }
  .target,
  .session {
    font-family: var(--pen-mono);
    overflow-wrap: anywhere;
  }
  .status {
    font-size: 12px;
  }
  .tone-ok {
    color: var(--pen-ok);
  }
  .tone-warn,
  .tone-progress {
    color: var(--pen-warn);
  }
  .tone-attention {
    color: var(--pen-danger);
  }
  .host-error {
    margin: 0;
    color: var(--pen-danger);
    font-size: 12px;
  }
  button {
    font: inherit;
    font-size: 12px;
    border-radius: 6px;
    background: var(--pen-surface-3);
    border: 1px solid var(--pen-border);
    color: var(--pen-text);
    padding: 3px 8px;
    cursor: pointer;
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  button:hover:not(:disabled):not(.link) {
    border-color: var(--pen-accent);
    background: #242a36;
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  button:focus-visible {
    outline: 2px solid var(--pen-accent);
    outline-offset: 1px;
  }
  .link {
    align-self: flex-start;
    background: transparent;
    border: none;
    color: var(--pen-accent);
    padding: 0;
    font-size: 11px;
    text-decoration: underline;
    cursor: pointer;
  }
  .link:hover {
    color: #b0c2ff;
    background: transparent;
    border: none;
  }
  .link:focus-visible {
    outline: 2px solid var(--pen-accent);
    outline-offset: 1px;
  }
</style>
