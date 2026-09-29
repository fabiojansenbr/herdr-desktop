<script lang="ts">
  // Spec 014 — the "start an agent" form of spec 004, extracted so the panel's "Novo agente"
  // button, the workspace header (013) and the palette open the same one. Kinds, panes and the
  // reasons an action is unavailable all come from the engine through the controller's state.
  import type { AgentsController } from "../../agents/controller";
  import { t } from "../../i18n/index.svelte";
  import { viewModel, type AgentsState } from "../../agents/reducer";

  interface Props {
    controller: AgentsController;
    state: AgentsState;
    /** Called after a start request was accepted by the engine (to close a popover). */
    onstarted?: () => void;
  }

  let { controller, state, onstarted }: Props = $props();
  const view = $derived(viewModel(state));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const before = state.start.error;
    await controller.startAgent();
    if (state.start.error === before) onstarted?.();
  }
</script>

<form aria-label={t("agents.form.label")} onsubmit={submit}>
  <label>
    <span>{t("agents.form.pane")}</span>
    <select value={state.start.paneId} onchange={(e) => controller.editStart("paneId", e.currentTarget.value)}>
      {#each view.start.panes as pane (pane)}
        <option value={pane}>{pane}</option>
      {/each}
    </select>
  </label>
  <label>
    <span>{t("agents.form.kind")}</span>
    <select value={state.start.kind} onchange={(e) => controller.editStart("kind", e.currentTarget.value)} disabled={state.kinds.length === 0}>
      {#each state.kinds as kind (kind)}
        <option value={kind}>{kind}</option>
      {/each}
    </select>
  </label>
  <label>
    <span>{t("agents.form.name")}</span>
    <input value={state.start.name} oninput={(e) => controller.editStart("name", e.currentTarget.value)} placeholder={t("agents.form.namePlaceholder")} />
  </label>
  <button type="submit" disabled={!view.start.enabled} title={view.start.reason ?? ""}>{t("agents.form.submit")}</button>
  {#if view.start.reason && !view.start.enabled}
    <small class="hint">{view.start.reason}</small>
  {/if}
  {#if view.start.error}
    <p class="error">{view.start.error}</p>
  {/if}
</form>

<style>
  form {
    display: flex;
    gap: 6px;
    align-items: end;
    flex-wrap: wrap;
  }
  label {
    display: grid;
    gap: 2px;
    font-size: 11px;
    /* Fits the 256 px agents column as well as the wider harness page. */
    flex: 1 1 100%;
    min-width: 0;
    max-width: 100%;
  }
  select,
  input {
    width: 100%;
    min-width: 0;
    max-width: 100%;
  }
  .hint {
    color: #8c93a3;
    font-size: 11px;
  }
  .error {
    color: #ff8f8f;
    margin: 0;
  }
</style>
