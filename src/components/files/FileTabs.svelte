<script lang="ts">
  // Review tabs of spec 015: one entry per open file (`arquivo ×`) and, with its diff open, the
  // `arquivo · diff ×` entry right after it, then `+ Abrir arquivo`. The file entry keeps the
  // spec 005/006 classes and data attributes (the standalone harnesses drive them); the diff entry
  // is a distinct class so counting open files is unchanged there.
  import type { Snippet } from "svelte";
  import { t } from "../../i18n/index.svelte";
  import type { TabEntry } from "./review";

  interface Props {
    tabs: TabEntry[];
    /** `remote` renders the file entry with the spec 006 class and the caller's badge snippet. */
    variant?: "local" | "remote";
    badge?: Snippet<[TabEntry]>;
    dirtyOf?: (entry: TabEntry) => boolean;
    staleOf?: (entry: TabEntry) => boolean;
    loadingOf?: (entry: TabEntry) => boolean;
    onFocus: (id: string, kind: TabEntry["kind"]) => void;
    onClose: (id: string) => void;
    onOpenFile: () => void;
  }

  let {
    tabs,
    variant = "local",
    badge,
    dirtyOf = () => false,
    staleOf = () => false,
    loadingOf = () => false,
    onFocus,
    onClose,
    onOpenFile,
  }: Props = $props();
</script>

<div class="tabs" role="tablist" aria-label={t("files.tabs.label")} data-review-tabs>
  {#each tabs as entry (entry.id + ":" + entry.kind)}
    {#if entry.kind === "file"}
      <div
        class={variant === "remote" ? "remote-tab" : "tab"}
        class:active={entry.active}
        data-review-tab="file"
        data-tab={entry.id}
        data-file={entry.path}
        data-dirty={dirtyOf(entry)}
        data-stale={variant === "remote" ? staleOf(entry) : undefined}
        data-loading={variant === "remote" ? loadingOf(entry) : undefined}
      >
        <button class="tab-name" type="button" role="tab" aria-selected={entry.active} onclick={() => onFocus(entry.id, "file")}>
          {entry.label}
          {#if dirtyOf(entry)}<span class="dot" aria-label={t("files.tabs.dirty")}> •</span>{/if}
        </button>
        {#if badge}{@render badge(entry)}{/if}
        <button class="tab-close" type="button" aria-label={t("files.tabs.close", { name: entry.label })} onclick={() => onClose(entry.id)}>×</button>
      </div>
    {:else}
      <div class="diff-tab" class:active={entry.active} data-review-tab="diff" data-tab={entry.id} data-file={entry.path}>
        <button class="tab-name" type="button" role="tab" aria-selected={entry.active} onclick={() => onFocus(entry.id, "diff")}>{entry.label}</button>
        <button class="tab-close" type="button" aria-label={t("files.tabs.close", { name: entry.label })} onclick={() => onClose(entry.id)}>×</button>
      </div>
    {/if}
  {/each}
  <button class="open-file" type="button" data-open-file onclick={onOpenFile}>{t("files.tabs.openFile")}</button>
</div>

<style>
  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 8px;
    align-items: center;
  }
  .tab,
  .remote-tab,
  .diff-tab {
    display: flex;
    align-items: center;
    gap: 4px;
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    background: var(--surface-2, #171a21);
    padding: 2px 4px;
    transition: border-color 0.15s ease;
  }
  .tab.active,
  .remote-tab.active,
  .diff-tab.active {
    border-color: var(--accent, #8fa8ff);
    background: var(--surface-3, #1e222b);
  }
  .tab-name {
    background: transparent;
    border: none;
    color: inherit;
    cursor: pointer;
    padding: 2px 4px;
    font: inherit;
    font-size: 12px;
  }
  .tab-close {
    background: transparent;
    border: none;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
    padding: 0 6px;
    border-radius: 4px;
  }
  .tab-close:hover {
    color: var(--text, #e7e9ee);
    background: rgba(255, 255, 255, 0.1);
  }
  .tab-name:focus-visible,
  .tab-close:focus-visible,
  .open-file:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: 1px;
  }
  .dot {
    color: var(--attention, #f4b454);
  }
  .open-file {
    font: inherit;
    font-size: 12px;
    border-radius: 6px;
    background: var(--surface-2, #171a21);
    border: 1px dashed var(--border, #242833);
    color: var(--text-muted, #8c93a3);
    padding: 3px 10px;
    cursor: pointer;
  }
  .open-file:hover {
    border-color: var(--accent, #8fa8ff);
    color: var(--text, #e7e9ee);
  }
</style>
