<script lang="ts">
  // Spec 014 — the Agentes panel of the design: header with the collapse button, counters,
  // "Precisa da sua atenção" and "Em execução". Every state, title, tab and detection line comes
  // from the engine through the window's agents state; the only client-owned value is when this
  // window first saw each state change (the engine publishes no timestamp).
  //
  // Clicking a card or a row focuses that pane in the engine once, through the same action the
  // rest of the window uses. The panel never answers an agent and never sends it a key.
  import { onDestroy } from "svelte";
  import AttentionCard from "./AttentionCard.svelte";
  import Counters from "./Counters.svelte";
  import NewAgentForm from "./NewAgentForm.svelte";
  import RunningRow from "./RunningRow.svelte";
  import { createActivityLedger, panelView } from "./panel";
  import { t } from "../../i18n/index.svelte";
  import type { FrameContext } from "../../shell/frame-context";

  let { ctx }: { ctx: FrameContext } = $props();

  const ledger = createActivityLedger();
  let now = $state(Date.now());
  let formOpen = $state(false);

  // The relative times age without the engine sending anything; one timer for the whole panel.
  const tick = setInterval(() => (now = Date.now()), 30_000);
  onDestroy(() => clearInterval(tick));

  const live = $derived(ctx.agents?.phase === "connected");
  // Observing the list is what dates each transition; it runs on every engine list, not per row.
  $effect(() => {
    const agents = ctx.agents?.agents ?? [];
    const at = Date.now();
    ledger.observe(agents, at);
    now = at;
  });
  const view = $derived(
    panelView(ctx.agents, {
      ledger,
      now,
      snapshot: ctx.navigator?.snapshot ?? null,
      tabs: ctx.agents?.tabs ?? [],
      connected: live,
    }),
  );

  function focusPane(paneId: string) {
    ctx.controllers.agents.openAttention(paneId);
  }
</script>

<section class="panel" data-agents-panel aria-label={t("agents.panel.label")} data-cache={view.cache ? "true" : "false"}>
  <header>
    <h2>{t("agents.panel.heading")}</h2>
    <button
      type="button"
      class="collapse"
      data-collapse-agents
      aria-label={t("agents.panel.collapse")}
      title={t("agents.panel.collapse")}
      onclick={() => ctx.actions.toggleAgents()}
    >
      <span aria-hidden="true">⇥</span>
    </button>
  </header>

  <Counters counters={view.counters} />

  {#if view.empty}
    <div class="empty" data-agents-empty>
      <p>{view.emptyText}</p>
      <button
        type="button"
        class="primary"
        data-new-agent
        disabled={ctx.unavailable.newAgent !== null}
        title={ctx.unavailable.newAgent ?? t("agents.panel.newAgentTitle")}
        onclick={() => (formOpen = !formOpen)}
        aria-expanded={formOpen}
      >
        {t("agents.panel.newAgent")}
      </button>
      {#if ctx.unavailable.newAgent}
        <small class="hint">{ctx.unavailable.newAgent}</small>
      {/if}
    </div>
  {/if}

  {#if formOpen && ctx.agents}
    <div class="form" data-new-agent-form>
      <NewAgentForm controller={ctx.controllers.agents} state={ctx.agents} onstarted={() => (formOpen = false)} />
    </div>
  {/if}

  <div class="scroll">
    {#if view.attention.cards.length > 0}
      <section class="group" aria-label={t("agents.panel.attention")}>
        <h3>{t("agents.panel.attention")}</h3>
        <div class="cards">
          {#each view.attention.cards as card (card.paneId)}
            <AttentionCard agent={card} cache={view.cache} cacheText={view.cacheText} onfocus={focusPane} />
          {/each}
        </div>
        {#if view.attention.overflow > 0}
          <p class="overflow" data-attention-overflow>+{view.attention.overflow}</p>
        {/if}
      </section>
    {/if}

    {#if view.running.length > 0}
      <section class="group" aria-label={t("agents.panel.running")}>
        <h3>{t("agents.panel.running")}</h3>
        <ul>
          {#each view.running as row (row.paneId)}
            <RunningRow agent={row} onfocus={focusPane} />
          {/each}
        </ul>
      </section>
    {/if}
  </div>
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    background: var(--surface);
    color: var(--text);
    font-family: var(--font-ui);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: 36px;
    padding: 0 8px 0 12px;
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: var(--text-muted);
  }
  h3 {
    margin: 0;
    padding: 0 12px;
    font-size: 11px;
    font-weight: 400;
    color: var(--text-muted);
  }
  .collapse {
    display: inline-grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: none;
    color: var(--text-muted);
    font: inherit;
    cursor: pointer;
  }
  .collapse:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .collapse:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .scroll {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    align-content: start;
    gap: 12px;
    padding: 8px 0 12px;
  }
  .group {
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  .cards {
    display: grid;
    gap: 6px;
    padding: 0 12px;
  }
  ul {
    margin: 0;
    padding: 0 2px;
    display: grid;
    gap: 2px;
  }
  .overflow {
    margin: 0;
    padding: 0 12px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .empty {
    display: grid;
    gap: 8px;
    justify-items: start;
    padding: 8px 12px;
  }
  .empty p {
    margin: 0;
    font-size: 12px;
    color: var(--text-muted);
  }
  .form {
    padding: 4px 12px 8px;
  }
  .hint {
    font-size: 11px;
    color: var(--text-muted);
  }
  .primary {
    padding: 6px 10px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: var(--accent-soft);
    color: var(--text);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .primary:disabled {
    cursor: default;
    border-color: var(--border);
    background: var(--surface-2);
    color: var(--text-muted);
  }
  .primary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
