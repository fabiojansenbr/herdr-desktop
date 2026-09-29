<script lang="ts">
  // Review header of spec 015: screen title, the `projeto / caminho / arquivo` breadcrumb and, on
  // a remote host, the read-only snapshot banner in accent-soft. Presentation only: every string
  // comes from the caller (files state, host label), nothing is invented here.
  import { t } from "../../i18n/index.svelte";

  interface Props {
    segments: string[];
    banner?: string | null;
  }

  let { segments, banner = null }: Props = $props();
</script>

<header class="review-header" data-review-header>
  <h1 data-review-title>{t("files.review.title")}</h1>
  <nav class="breadcrumb" aria-label={t("files.review.breadcrumb")} data-review-breadcrumb>
    {#each segments as segment, index (index)}
      {#if index > 0}<span class="sep" aria-hidden="true">{" / "}</span>{/if}<span class="crumb">{segment}</span>
    {/each}
  </nav>
  {#if banner}
    <p class="banner" data-review-banner>{banner}</p>
  {/if}
</header>

<style>
  .review-header {
    display: grid;
    gap: 6px;
    padding: 2px 2px 8px;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 600;
    color: var(--text, #e7e9ee);
  }
  .breadcrumb {
    font-family: var(--font-mono, "JetBrains Mono", "DejaVu Sans Mono", monospace);
    font-size: 12px;
    color: var(--text-muted, #8c93a3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .banner {
    margin: 2px 0 0;
    padding: 4px 10px;
    border-radius: 6px;
    background: var(--accent-soft, #8fa8ff1f);
    color: var(--text, #e7e9ee);
    font-size: 12px;
  }
</style>
