<script lang="ts">
  // Connections side by side (spec 003): each host shows its own state in text, its explicit
  // actions and its panes. Input goes only to the qualified target of an enabled pane; while a
  // host reconnects or needs attention its last screen stays visible as cache and input is off.
  import { onDestroy, onMount } from "svelte";
  import { errorText, t } from "../i18n/index.svelte";
  import ConnectionDialog from "../components/ConnectionDialog.svelte";
  import type { ConnectionsBridge } from "./bridge";
  import { createConnectionsController, inputKey, type ConnectionsState } from "./controller";
  import { ACTION_TEXT, hostActions, hostStatus, inputBlockText } from "./presentation";
  import type { HostDto } from "./types";

  interface Props {
    bridge: ConnectionsBridge;
  }

  let { bridge }: Props = $props();

  let current = $state<ConnectionsState | null>(null);
  // svelte-ignore state_referenced_locally
  const controller = createConnectionsController(bridge, (next) => (current = next));
  current = controller.state;

  onMount(() => {
    void controller.load().then(() => controller.watch());
  });
  onDestroy(() => controller.stop());

  // Spec 072 (AC-072-01): the Local host has no name of its own — since 071 the host sends it as
  // the English `This computer`, so the panel names it in the user's language, as the sidebar and
  // the home screen already do. An SSH host is named by the user and keeps its own label.
  function hostName(host: HostDto): string {
    return host.kind === "local" || host.endpoint === "local" ? t("connections.host.thisComputer") : host.label;
  }

  function act(host: HostDto, action: string) {
    if (action === "cancel") void controller.cancel(host.endpoint);
    else void controller.connect(host.endpoint);
  }

  function submitInput(event: SubmitEvent, endpoint: string, paneId: string) {
    event.preventDefault();
    void controller.sendInput(endpoint, paneId);
  }
</script>

<section class="connections" aria-label={t("connections.panel.title")}>
  <header>
    <h2>{t("connections.panel.title")}</h2>
    <div class="toolbar">
      <button onclick={() => controller.openDialog()}>{t("connections.panel.addSsh")}</button>
      <button onclick={() => controller.importProfiles()}>{t("connections.panel.importProfiles")}</button>
    </div>
  </header>

  {#if current?.globalError}
    <p class="error" role="alert"><b>{current.globalError.code}</b> — {errorText(current.globalError)}</p>
  {/if}
  {#if current?.view?.store_error}
    <p class="error" role="alert"><b>{current.view.store_error.code}</b> — {errorText(current.view.store_error)}</p>
  {/if}
  {#if current?.importReport}
    <p class="notice" data-import>
      {t("connections.panel.importSummary", {
        imported: current.importReport.imported.length,
        present: current.importReport.already_present,
        skipped: current.importReport.skipped.map((s) => `${s.label} (${s.code})`).join(", ") || t("connections.panel.none"),
      })}
    </p>
  {/if}

  {#if current?.view}
    {#if !current.view.hub.hosts.some((h) => h.kind === "ssh")}
      <p class="onboarding">{t("connections.panel.onboarding")}</p>
    {/if}
    <div class="hosts">
      {#each current.view.hub.hosts as host (host.endpoint)}
        {@const status = hostStatus(host)}
        <article
          class="host"
          data-host={host.endpoint}
          data-kind={host.kind}
          data-phase={host.phase}
          data-generation={host.generation ?? ""}
          data-boot={host.boot_id ?? ""}
          data-attempt={host.attempt}
          aria-label={`${hostName(host)}, ${status.text}`}
        >
          <header>
            <span class="host-label">{hostName(host)}</span>
            <span class="kind">{host.kind === "ssh" ? "SSH" : t("connections.kind.local")}</span>
            <span class={`status ${status.tone}`} aria-live="polite">{status.text}</span>
          </header>
          <div class="meta">
            {host.target ? `${host.target} · ` : ""}{t("connections.session", { name: host.session })}{host.cached ? " · cache" : ""}
          </div>
          {#if status.detail}<div class="detail">{status.detail}</div>{/if}
          {#if status.guidance}<div class="guidance" role="note">{status.guidance}</div>{/if}
          {#if current.hostErrors[host.endpoint]}
            <div class="error" role="alert">
              <b>{current.hostErrors[host.endpoint]!.code}</b> — {errorText(current.hostErrors[host.endpoint]!)}
            </div>
          {/if}
          {#if host.action_error}
            <div class="error action-error"><b>{host.action_error.code}</b> — {errorText(host.action_error)}</div>
          {/if}
          <div class="actions">
            {#each hostActions(host) as action (action)}
              <button onclick={() => act(host, action)}>{ACTION_TEXT[action]}</button>
            {/each}
            {#if host.phase === "online"}
              <button onclick={() => controller.listWorkspaces(host.endpoint)}>{t("connections.panel.listWorkspaces")}</button>
            {/if}
          </div>
          {#if current.workspaces[host.endpoint]}
            <div class="workspaces" data-workspaces={current.workspaces[host.endpoint]!.length}>
              {t("connections.panel.workspaces", {
                list: current.workspaces[host.endpoint]!.map((w) => w.workspace_id).join(", ") || t("connections.panel.none"),
              })}
            </div>
          {/if}
          {#if host.screen.length > 0}
            <pre class="screen" class:stale={host.cached} aria-label={t("connections.panel.screen", { host: hostName(host) })}>{host.screen.join("\n")}</pre>
          {/if}
          {#each host.panes as pane (pane.pane_id)}
            {@const key = inputKey(host.endpoint, pane.pane_id)}
            <form
              class="pane"
              data-pane={pane.pane_id}
              data-input-enabled={pane.input_enabled ? "true" : "false"}
              data-block={pane.input_block ?? ""}
              onsubmit={(e) => submitInput(e, host.endpoint, pane.pane_id)}
            >
              <span class="pane-id">{pane.pane_id}</span>
              <input
                aria-label={t("connections.panel.paneInput", { pane: pane.pane_id, host: hostName(host) })}
                disabled={!pane.input_enabled}
                value={current.inputs[key] ?? ""}
                oninput={(e) => controller.editInput(host.endpoint, pane.pane_id, (e.currentTarget as HTMLInputElement).value)}
              />
              <button type="submit" disabled={!pane.input_enabled || current.sending[key]}>{t("connections.panel.send")}</button>
              {#if !pane.input_enabled}<small class="block">{inputBlockText(pane.input_block)}</small>{/if}
            </form>
          {/each}
        </article>
      {/each}
    </div>
    <ConnectionDialog {controller} state={current} />
  {:else}
    <p class="muted">{current?.loading ? t("connections.panel.loading") : ""}</p>
  {/if}
</section>

<style>
  .connections {
    color: #e7e9ee;
    font-family: Inter, system-ui, sans-serif;
    font-size: 13px;
    display: grid;
    gap: 10px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    justify-content: space-between;
  }
  h2 {
    margin: 0;
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: #8c93a3;
  }
  .toolbar,
  .actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .hosts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 8px;
  }
  .host {
    background: #111318;
    border: 1px solid #242833;
    border-radius: 10px;
    padding: 10px;
    display: grid;
    gap: 6px;
  }
  .host header {
    justify-content: flex-start;
  }
  .host-label {
    font-weight: 600;
  }
  .kind {
    color: #8c93a3;
    border: 1px solid #242833;
    border-radius: 6px;
    padding: 0 6px;
  }
  .status {
    margin-left: auto;
    border-radius: 6px;
    padding: 0 6px;
  }
  .status.ok {
    color: #5bd68a;
  }
  .status.warn,
  .status.attention {
    color: #f4b454;
  }
  .status.attention {
    font-weight: 600;
  }
  .status.progress {
    color: #8fa8ff;
  }
  .status.idle,
  .meta,
  .detail,
  .muted,
  .block {
    color: #8c93a3;
  }
  .guidance {
    background: #f4b4541f;
    border-radius: 6px;
    padding: 6px;
  }
  .error {
    color: #f2777a;
  }
  .screen {
    background: #0b0c10;
    border-radius: 8px;
    padding: 8px;
    margin: 0;
    max-height: 220px;
    overflow: auto;
    font-family: "JetBrains Mono", monospace;
    font-size: 12px;
    white-space: pre;
  }
  .screen.stale {
    opacity: 0.6;
  }
  .pane {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }
  .pane input {
    flex: 1;
    min-width: 120px;
    background: #171a21;
    border: 1px solid #242833;
    border-radius: 6px;
    color: #e7e9ee;
    padding: 6px;
    font-family: "JetBrains Mono", monospace;
  }
  .pane input:disabled {
    opacity: 0.5;
  }
  button {
    background: #1e222b;
    color: #e7e9ee;
    border: 1px solid #242833;
    border-radius: 6px;
    padding: 4px 10px;
  }
  button:disabled {
    opacity: 0.5;
  }
  button:focus-visible,
  input:focus-visible {
    outline: 2px solid #8fa8ff;
  }
</style>
