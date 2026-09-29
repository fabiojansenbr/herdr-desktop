<script lang="ts">
  // Spec 014 AC-014-01 — the three counters of the design plus "?" for the states the engine
  // did not publish as one of its four. Unknown is never added to the idle (idle + done) box.
  import { t } from "../../i18n/index.svelte";
  import type { Counters } from "./panel";

  let { counters }: { counters: Counters } = $props();
</script>

<div class="counters" data-counters>
  <div class="box" data-counter="active">
    <strong>{counters.active}</strong>
    <span>{t("agents.counters.active")}</span>
  </div>
  <div class="box" data-counter="waiting">
    <strong class="waiting">{counters.waiting}</strong>
    <span>{t("agents.counters.waiting")}</span>
  </div>
  <div class="box" data-counter="idle">
    <strong>{counters.idle}</strong>
    <span>{t("agents.counters.idle")}</span>
  </div>
  {#if counters.unknown > 0}
    <div class="box" data-counter="unknown" title={t("agents.counters.unknownTitle")}>
      <strong>{counters.unknown}</strong>
      <span>{t("agents.counters.unknown")}</span>
    </div>
  {/if}
</div>

<style>
  .counters {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
    padding: 8px 12px 4px;
  }
  .box {
    display: grid;
    gap: 2px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface-2);
    min-width: 0;
  }
  .box[data-counter="unknown"] {
    grid-column: 1 / -1;
    grid-template-columns: auto 1fr;
    align-items: baseline;
    gap: 6px;
  }
  strong {
    font-size: 20px;
    font-weight: 600;
    color: var(--text);
    line-height: 1.1;
  }
  strong.waiting {
    color: var(--attention);
  }
  span {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
