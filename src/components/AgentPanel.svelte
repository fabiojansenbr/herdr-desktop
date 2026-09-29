<script lang="ts">
  // Agents, tabs and split panes (spec 004). Agent kinds, states and pane geometry come from
  // the server; every state is shown with text and icon. A blocked agent is surfaced with
  // "Abrir pane", which only focuses that pane: the answer is typed by the user in the agent's
  // own terminal UI. Keys on the pane surface are either GUI shortcuts (never forwarded) or
  // terminal input sent once to the pane the server confirmed focused.
  import { onDestroy, onMount } from "svelte";
  import { t } from "../i18n/index.svelte";
  import NewAgentForm from "./agents/NewAgentForm.svelte";
  import type { AgentsBridge } from "../agents/bridge";
  import { createAgentsController, type AgentsController } from "../agents/controller";
  import { viewModel, type AgentsState } from "../agents/reducer";
  import { endpointDisplay, type HostLabels } from "../projects/reducer";
  import { GUI_SHORTCUTS } from "../terminal/actions";
  import type { GeometryDto } from "../terminal/types";

  // Standalone (harness): pass `bridge`; the panel connects on mount and detaches on destroy,
  // and draws its own pane-layout surface. Composed window (spec 007): pass `controller` +
  // `state` owned by the composition, which connects/reattaches agents to the selected host;
  // hiding the panel neither detaches agents nor draws a second keyboard surface next to the
  // real terminal (`composed`).
  interface Props {
    bridge?: AgentsBridge;
    geometry?: GeometryDto;
    controller?: AgentsController;
    state?: AgentsState;
    composed?: boolean;
    /** Display names of endpoints already loaded by the App (identity stays the endpoint id). */
    hostLabels?: HostLabels;
  }

  let {
    bridge,
    geometry = { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 },
    controller: external,
    state: controlled,
    composed = false,
    hostLabels,
  }: Props = $props();

  let own = $state<AgentsState | null>(null);
  // svelte-ignore state_referenced_locally
  const controller = external ?? createAgentsController(bridge!, (next) => (own = next));
  // svelte-ignore state_referenced_locally
  if (!external) own = controller.state;
  const panel = $derived(controlled ?? own ?? controller.state);
  const view = $derived(panel ? viewModel(panel) : null);
  let ratioDraft = $state("");

  onMount(() => {
    // svelte-ignore state_referenced_locally
    if (!external) void controller.connect(geometry);
  });
  onDestroy(() => {
    // svelte-ignore state_referenced_locally
    if (!external) void bridge?.detach().catch(() => {});
  });

  function surfaceKeydown(event: KeyboardEvent) {
    if (controller.key(event)) event.preventDefault();
  }

  async function applyRatio(event: SubmitEvent) {
    event.preventDefault();
    const value = Number(ratioDraft.replace(",", "."));
    if (Number.isFinite(value)) await controller.setRatio(value);
  }
</script>

{#if panel && view}
  <div class="agents" class:composed data-phase={panel.phase}>
    <header>
      <h2>{t("terminal.panel.title")}</h2>
      <span class="identity" data-boot={panel.identity?.boot_id ?? ""}>
        {panel.identity ? `${endpointDisplay(panel.identity.endpoint, hostLabels)} · ${panel.identity.session}` : t("terminal.panel.noConnection")}
      </span>
      <span class="phase">{panel.phase}</span>
    </header>
    {#if view.connectionError}
      <p class="error" role="alert" data-error-code={panel.connectionError?.code ?? ""}>{view.connectionError}</p>
    {/if}
    {#if view.streamError}
      <p class="error" role="status" data-error-code={panel.streamError?.code ?? ""}>{t("terminal.panel.streamError", { reason: view.streamError })}</p>
    {/if}

    {#if view.attention.length > 0}
      <section class="attention" aria-label={t("terminal.panel.attention")}>
        {#each view.attention as item (item.paneId)}
          <div class="attention-item" data-attention={item.paneId}>
            <span>{item.icon} {t("terminal.panel.waitsForYou", { name: item.name, pane: item.paneId })}</span>
            <button disabled={!item.canOpen} onclick={() => controller.openAttention(item.paneId)}>{t("terminal.panel.openPane")}</button>
          </div>
        {/each}
      </section>
    {/if}

    <section aria-label={t("terminal.panel.agents")}>
      <h3>{t("terminal.panel.agents")}</h3>
      {#if view.onboarding}
        <p class="onboarding">{view.onboarding}</p>
      {/if}
      <NewAgentForm {controller} state={panel} />

      <ul>
        {#each view.agents as agent (agent.paneId)}
          <li data-agent-pane={agent.paneId}>
            <div class="row">
              <strong class="name">{agent.name}</strong>
              <span class="kind">{agent.kind}</span>
              <span class="status tone-{agent.tone}" data-status={agent.status}>
                <span aria-hidden="true">{agent.icon}</span>
                {agent.label}
              </span>
              <span class="readiness">{agent.readiness}</span>
            </div>
            <textarea
              aria-label={t("terminal.panel.promptFor", { name: agent.name })}
              value={agent.promptText}
              oninput={(e) => controller.editPrompt(agent.paneId, e.currentTarget.value)}
              rows="2"
            ></textarea>
            <div class="row">
              <button disabled={!agent.canPrompt} title={agent.promptDisabledReason ?? ""} onclick={() => controller.sendPrompt(agent.paneId)}>
                {agent.promptLabel}
              </button>
              {#if agent.promptDisabledReason && agent.promptDisabledReason !== "escreva o prompt"}
                <small class="hint">{agent.promptDisabledReason}</small>
              {/if}
            </div>
            {#if agent.outcome}
              <p class="outcome" data-outcome={panel.prompts[agent.paneId]?.outcome ?? ""}>{agent.outcome}</p>
            {/if}
            {#if agent.error}
              <p class="error">{agent.error}</p>
            {/if}
          </li>
        {/each}
      </ul>
    </section>

    <section aria-label={t("terminal.panel.panes")}>
      <h3>{t("terminal.panel.tabsAndPanes")}</h3>
      <div class="row tabs">
        {#each view.tabs as tab (tab.tab_id)}
          <button aria-pressed={tab.focused} data-tab={tab.tab_id} disabled={!view.canFocusTab || tab.focused} onclick={() => controller.focusTab(tab.tab_id)}>
            {tab.label || tab.tab_id}
          </button>
        {/each}
        <button disabled={!view.canCreateTab} onclick={() => controller.createTab()}>{t("terminal.panel.newTab")}</button>
      </div>
      <div class="row">
        <button disabled={!view.split.enabled || panel.layoutBusy} onclick={() => controller.split("right")}>{t("terminal.panel.splitRight")}</button>
        <button disabled={!view.split.enabled || panel.layoutBusy} onclick={() => controller.split("down")}>{t("terminal.panel.splitDown")}</button>
        <form class="row" aria-label={t("terminal.panel.ratioForm")} onsubmit={applyRatio}>
          <label>
            <span>{t("terminal.panel.ratio")}</span>
            <input
              class="ratio"
              inputmode="decimal"
              placeholder={view.focusedSplit ? view.focusedSplit.ratio.toFixed(2) : "—"}
              value={ratioDraft}
              oninput={(e) => (ratioDraft = e.currentTarget.value)}
              disabled={!view.ratio.enabled}
            />
          </label>
          <button type="submit" disabled={!view.ratio.enabled || panel.layoutBusy}>{t("terminal.panel.applyRatio")}</button>
        </form>
      </div>
      {#if view.layoutError}
        <p class="error">{view.layoutError}</p>
      {/if}
      {#if !composed}
      <div
        class="surface"
        role="textbox"
        aria-multiline="true"
        tabindex="0"
        aria-label={t("terminal.panel.surface")}
        data-confirmed={view.confirmed ?? ""}
        data-split-ratio={view.focusedSplit ? String(view.focusedSplit.ratio) : ""}
        onkeydown={surfaceKeydown}
      >
        {#each view.panes as pane (pane.paneId)}
          <button
            class="pane"
            class:confirmed={pane.confirmed}
            class:pending={pane.pending}
            data-pane={pane.paneId}
            data-cells="{pane.cells.x},{pane.cells.y},{pane.cells.width},{pane.cells.height}"
            data-focused={pane.confirmed ? "true" : "false"}
            data-pending={pane.pending ? "true" : "false"}
            style="left: {pane.left}%; top: {pane.top}%; width: {pane.width}%; height: {pane.height}%;"
            tabindex="-1"
            onclick={() => controller.focusPane(pane.paneId)}
          >
            <span class="pane-id">{pane.paneId}</span>
            <span class="pane-state">{pane.confirmed ? t("terminal.panel.focusConfirmed") : pane.pending ? t("terminal.panel.awaitingConfirmation") : ""}</span>
            {#if pane.agent}
              <span class="status tone-{pane.agent.tone}" data-status={pane.agent.status}>{pane.agent.icon} {pane.agent.label}</span>
            {/if}
          </button>
        {/each}
      </div>
      {:else}
        <ul class="pane-list" aria-label={t("terminal.panel.confirmedPanes")}>
          {#each view.panes as pane (pane.paneId)}
            <li>
              <button
                aria-pressed={pane.confirmed}
                data-pane={pane.paneId}
                data-cells="{pane.cells.x},{pane.cells.y},{pane.cells.width},{pane.cells.height}"
                disabled={!panel.capabilities?.focus || panel.layoutBusy || pane.confirmed}
                onclick={() => controller.focusPane(pane.paneId)}
              >
                {pane.paneId} · {pane.cells.width}×{pane.cells.height}
                {pane.confirmed ? ` · ${t("terminal.panel.focusConfirmed")}` : pane.pending ? ` · ${t("terminal.panel.awaitingConfirmation")}` : ""}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      {#if panel.notice}
        <p class="notice" role="status">{panel.notice}</p>
      {/if}
      <details>
        <summary>{t("terminal.panel.shortcuts")}</summary>
        <ul class="shortcuts">
          {#each GUI_SHORTCUTS as shortcut (shortcut.label)}
            <li>{shortcut.label}</li>
          {/each}
        </ul>
      </details>
    </section>
  </div>
{/if}

<style>
  .agents {
    display: grid;
    gap: 10px;
    width: 560px;
    padding: 10px;
    background: #111318;
    color: #e7e9ee;
    font-family: Inter, system-ui, sans-serif;
  }
  header {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  h2,
  h3 {
    margin: 0;
    font-size: 13px;
  }
  .identity,
  .phase,
  .kind,
  .readiness,
  .hint {
    color: #8c93a3;
    font-size: 11px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  li[data-agent-pane] {
    padding: 8px;
    border: 1px solid #242833;
    border-radius: 6px;
    background: #171a21;
    display: grid;
    gap: 4px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  label {
    display: grid;
    gap: 2px;
    font-size: 11px;
  }
  .status {
    font-size: 11px;
    padding: 0 6px;
    border-radius: 6px;
    border: 1px solid #2b303b;
  }
  .tone-busy {
    color: #9ecbff;
  }
  .tone-attention {
    color: #ffd479;
    border-color: #ffd479;
  }
  .tone-calm {
    color: #c7ccd6;
  }
  .tone-success {
    color: #9be29b;
  }
  .tone-muted {
    color: #8c93a3;
    border-style: dashed;
  }
  .attention {
    border: 1px solid #ffd479;
    border-radius: 6px;
    padding: 6px;
    display: grid;
    gap: 4px;
  }
  .attention-item {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
  }
  .surface {
    position: relative;
    height: 220px;
    border: 1px solid #242833;
    border-radius: 6px;
    background: #0b0e11;
  }
  .surface:focus-visible {
    outline: 2px solid #8fa8ff;
  }
  .pane {
    position: absolute;
    display: grid;
    align-content: start;
    gap: 2px;
    padding: 4px;
    border: 1px solid #2b303b;
    background: #141821;
    color: inherit;
    text-align: left;
    font-size: 11px;
  }
  .pane.confirmed {
    border-color: #8fa8ff;
  }
  .pane.pending {
    border-style: dashed;
  }
  .ratio {
    width: 56px;
  }
  .error {
    color: #ff8f8f;
    margin: 0;
  }
  .onboarding,
  .outcome,
  .notice {
    margin: 0;
    color: #c7ccd6;
  }
  /* Composed window: the panel fills its column (256 px) instead of the 560 px harness page;
     rows wrap and fields shrink, every control stays inside the column. */
  .agents.composed {
    width: 100%;
    min-width: 0;
    max-width: 100%;
    overflow-wrap: anywhere;
  }
  .agents.composed :is(header, section, ul, li, form, label, .row, .attention-item) {
    min-width: 0;
    max-width: 100%;
  }
  .agents.composed :is(header, .row, form, .attention-item) {
    flex-wrap: wrap;
  }
  .agents.composed label {
    flex: 1 1 100%;
  }
  .agents.composed :is(input:not(.ratio), textarea) {
    width: 100%;
    min-width: 0;
    max-width: 100%;
  }
  .agents.composed button {
    max-width: 100%;
    white-space: normal;
    text-align: left;
  }
  textarea {
    width: 100%;
    font: inherit;
    font-size: 12px;
    color: #e7e9ee;
    background: #171a21;
    border: 1px solid #242833;
    border-radius: 6px;
    padding: 6px 8px;
    resize: vertical;
    box-sizing: border-box;
    transition: border-color 0.15s ease;
  }
  textarea:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
    border-color: #8fa8ff;
  }
  .pane-list {
    display: grid;
    gap: 4px;
  }
  .pane-list button {
    width: 100%;
    text-align: left;
    padding: 6px 8px;
    background: #171a21;
    border: 1px solid #242833;
    border-radius: 6px;
    font-size: 11px;
    color: inherit;
    transition: border-color 0.15s ease, background-color 0.15s ease;
  }
  .pane-list button:hover:not(:disabled) {
    border-color: #8fa8ff;
    background: #1e222b;
  }
  .pane-list button:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
  }
  .pane-list button[aria-pressed="true"] {
    border-color: #8fa8ff;
    background: #1e222b;
  }
  details {
    font-size: 11px;
    color: #8c93a3;
  }
  summary {
    cursor: pointer;
    padding: 4px 0;
  }
  summary:hover {
    color: #e7e9ee;
  }
  summary:focus-visible {
    outline: 2px solid #8fa8ff;
    outline-offset: 1px;
  }
  .shortcuts {
    margin-top: 4px;
    font-family: "JetBrains Mono", monospace;
    font-size: 10px;
    color: #8c93a3;
  }
</style>
