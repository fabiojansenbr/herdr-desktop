<script lang="ts">
  // Spec 012 — the "N agentes esperando" strip of the design. Each chip is the project and the
  // engine's own line of one agent waiting for the user; clicking it opens the project and
  // focuses the pane once (the window answers nothing and never sends a key to the agent).
  import { t } from "../../i18n/index.svelte";
  import type { AttentionChip } from "./home-model";

  interface Props {
    chips: AttentionChip[];
    cache: boolean;
    onselect: (chip: AttentionChip) => void;
  }

  let { chips, cache, onselect }: Props = $props();
</script>

{#if chips.length > 0}
  <section class="strip" data-attention-strip aria-label={t("home.attention.label", { count: chips.length })}>
    <span class="bell" aria-hidden="true">🔔</span>
    <strong class="count">{t("home.attention.count", { count: chips.length })}</strong>
    <ul class="chips">
      {#each chips as chip (chip.paneId)}
        <li>
          <button
            type="button"
            class="chip"
            data-attention-chip={chip.paneId}
            title={t("home.attention.chip")}
            onclick={() => onselect(chip)}
          >
            <span class="dot" aria-hidden="true"></span>
            <span class="label">{chip.label}</span>
          </button>
        </li>
      {/each}
    </ul>
    {#if cache}
      <span class="cache" data-attention-cache>{t("home.cache")}</span>
    {/if}
    <span class="cta" aria-hidden="true">{t("home.attention.cta")}</span>
  </section>
{/if}

<style>
  .strip {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 10px 12px;
    border: 1px solid var(--attention);
    border-radius: 10px;
    background: var(--surface-2);
    font-family: var(--font-ui);
  }
  .bell {
    flex: 0 0 auto;
    font-size: 13px;
  }
  .count {
    flex: 0 0 auto;
    font-size: 12px;
    font-weight: 600;
    color: var(--attention);
  }
  .chips {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    margin: 0;
    padding: 0;
    list-style: none;
    overflow: hidden;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 280px;
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-3);
    color: var(--text);
    font: inherit;
    font-size: 12px;
  }
  .chip:hover {
    border-color: var(--accent);
  }
  .chip:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .dot {
    flex: 0 0 auto;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--attention);
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cache {
    flex: 0 0 auto;
    padding: 0 4px;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 10px;
    color: var(--text-muted);
  }
  .cta {
    flex: 0 0 auto;
    margin-left: auto;
    font-size: 12px;
    color: var(--attention);
  }
</style>
