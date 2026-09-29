<script lang="ts">
  // Side-by-side diff of spec 015: Snapshot anterior / Versão atual columns, removed lines with a
  // `-` prefix on `error` at 15 % and added lines with `+` on `working` at 15 %, numbered per side.
  // Above VIRTUAL_ROW_LIMIT rows only the visible window is rendered. The row markup keeps what
  // the spec 005/006 harnesses read from `[data-diff]`/`[data-remote-diff]`: `.diff-lines li[data-op]`
  // with the line text in a `code` child and `[data-diff-sources]`/`[data-diff-summary]`.
  import { t } from "../../i18n/index.svelte";
  import type { DiffLine } from "../../files/diff";
  import { sideBySide, virtualWindow, VIRTUAL_ROW_LIMIT } from "./review";

  /** Both view models (spec 005 local, spec 006 remote) provide this shape. */
  interface SideDiff {
    base?: string;
    baseLabel: string;
    currentLabel: string;
    lines: DiffLine[];
    added: number;
    removed: number;
    identical: boolean;
  }

  interface Props {
    diff: SideDiff;
    variant?: "local" | "remote";
  }

  let { diff, variant = "local" }: Props = $props();

  const ROW_HEIGHT = 18;
  const rows = $derived(sideBySide(diff.lines));
  const virtual = $derived(rows.length > VIRTUAL_ROW_LIMIT);
  let scrollTop = $state(0);
  let viewport = $state(0);
  const slice = $derived(
    virtual ? virtualWindow(rows.length, ROW_HEIGHT, scrollTop, viewport) : { start: 0, end: rows.length, top: 0, bottom: 0 },
  );
  const shown = $derived(rows.slice(slice.start, slice.end));
  const mark = (op: "same" | "removed" | "added") => (op === "added" ? "+" : op === "removed" ? "-" : " ");
</script>

<div class="diff" data-diff data-remote-diff={variant === "remote" ? "" : undefined} data-diff-base={diff.base}>
  <p class="sources" data-diff-sources>{diff.baseLabel} → {diff.currentLabel}</p>
  <div class="head">
    <span class="heading" data-diff-heading="base">{t("files.review.base")}</span>
    <span class="heading" data-diff-heading="current">{t("files.review.current")}</span>
  </div>
  <div class="scroll" bind:clientHeight={viewport} onscroll={(event) => (scrollTop = event.currentTarget.scrollTop)}>
    {#if slice.top > 0}<div class="spacer" style={`height:${slice.top}px`}></div>{/if}
    <ol class="diff-lines" style={`grid-template-rows:repeat(${shown.length}, ${ROW_HEIGHT}px)`}>
      {#each shown as row, index (slice.start + index)}
        {#if row.base}
          <li
            class="line"
            data-row={slice.start + index + 1}
            data-op={row.base.op}
            data-side="base"
            data-line={row.base.line}
            style={`grid-row:${index + 1};grid-column:1`}
          >
            <span class="mark" aria-hidden="true">{mark(row.base.op)}</span><code>{row.base.text}</code>
          </li>
        {/if}
        {#if row.current}
          <li
            class="line"
            data-row={slice.start + index + 1}
            data-op={row.current.op}
            data-side="current"
            data-line={row.current.line}
            style={`grid-row:${index + 1};grid-column:2`}
          >
            <span class="mark" aria-hidden="true">{mark(row.current.op)}</span><code>{row.current.text}</code>
          </li>
        {/if}
      {/each}
    </ol>
    {#if slice.bottom > 0}<div class="spacer" style={`height:${slice.bottom}px`}></div>{/if}
  </div>
  <p class="summary" data-diff-summary>{diff.identical ? t("files.review.identical") : `+${diff.added} −${diff.removed}`}</p>
</div>

<style>
  .diff {
    margin-top: 8px;
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    padding: 8px;
    background: var(--surface, #111318);
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .sources,
  .summary {
    margin: 0;
    font-size: 11px;
    color: var(--text-muted, #8c93a3);
  }
  .head {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 8px;
    margin: 6px 0 4px;
    border-bottom: 1px solid var(--border, #242833);
  }
  .heading {
    font-size: 11px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-muted, #8c93a3);
    padding: 2px 0;
  }
  .scroll {
    overflow: auto;
    min-height: 0;
  }
  .diff-lines {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    column-gap: 8px;
    font-family: var(--font-mono, "JetBrains Mono", "DejaVu Sans Mono", monospace);
    font-size: 11px;
  }
  .line {
    display: flex;
    gap: 6px;
    min-width: 0;
    height: 18px;
    line-height: 18px;
    padding: 0 4px;
    border-radius: 3px;
    overflow: hidden;
  }
  .line code {
    font: inherit;
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .line .mark {
    width: 8px;
    text-align: center;
  }
  .line[data-op="removed"] {
    background: rgba(242, 119, 122, 0.15);
    color: var(--error, #f2777a);
  }
  .line[data-op="added"] {
    background: rgba(91, 214, 138, 0.15);
    color: var(--working, #5bd68a);
  }
</style>
