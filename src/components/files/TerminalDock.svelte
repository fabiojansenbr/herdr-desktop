<script lang="ts">
  // Terminal dock of spec 015: the review screen shows the active pane's surface in a 190 px dock
  // under the diff. The surface stays the window's one surface (spec 007, premise 6): the dock
  // mounts the real TerminalView on the shared controller and paints the same frames, bottom
  // anchored so the prompt area of the focused pane is what the dock shows. It never sends a
  // resize: the PTY keeps the center terminal's geometry and input keeps flowing while the review
  // is open. FilesRegion keeps the interest open.
  import { t } from "../../i18n/index.svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import TerminalView from "../../terminal/TerminalView.svelte";
  import { activeAgent, dockLabel } from "./review";

  interface Props {
    ctx: FrameContext;
    onClose: () => void;
  }

  let { ctx, onClose }: Props = $props();

  const agent = $derived(activeAgent(ctx.agents?.agents ?? []));
  const label = $derived(dockLabel(agent, ctx.activeProject?.label ?? null));
  const surface = $derived(ctx.surface);
  const live = $derived(ctx.surface.selection !== null);
  const canInput = $derived(surface.selection !== null && ctx.controllers.surface.canInput());
</script>

<section class="dock" data-terminal-dock aria-label={t("files.review.dock")}>
  <header class="dock-head">
    <span class="dock-label" data-dock-label>{label}</span>
    <button type="button" class="dock-close" data-dock-close aria-label={t("files.review.dockClose")} onclick={onClose}>×</button>
  </header>
  <div class="dock-body" data-dock-surface>
    {#if live}
      <TerminalView
        subscribe={ctx.controllers.surface.subscribe}
        onInput={(events) => void ctx.controllers.surface.input(events)}
        onResize={() => {}}
        onFocus={(focused) => void ctx.controllers.surface.focus(focused)}
        inputEnabled={canInput}
        visible={true}
      />
    {:else}
      <p class="dock-empty">{t("files.review.dockEmpty")}</p>
    {/if}
  </div>
</section>

<style>
  .dock {
    flex: none;
    height: 190px;
    display: flex;
    flex-direction: column;
    margin: 0 12px 12px;
    border: 1px solid var(--border, #242833);
    border-radius: 6px;
    background: var(--surface, #111318);
    min-width: 0;
  }
  .dock-head {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border, #242833);
  }
  .dock-label {
    font-family: var(--font-mono, "JetBrains Mono", "DejaVu Sans Mono", monospace);
    font-size: 11px;
    letter-spacing: 0.04em;
    color: var(--text-muted, #8c93a3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dock-close {
    background: transparent;
    border: none;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
    padding: 0 6px;
    border-radius: 4px;
    font-size: 14px;
  }
  .dock-close:hover {
    color: var(--text, #e7e9ee);
    background: rgba(255, 255, 255, 0.1);
  }
  .dock-close:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: 1px;
  }
  .dock-body {
    flex: 1;
    min-height: 0;
    position: relative;
    overflow: hidden;
  }
  /* The dock is smaller than the surface it shows: anchor the canvas to the bottom rows (the
     prompt area) instead of the top-left of a clipped frame. */
  .dock-body :global(canvas) {
    position: absolute;
    left: 0;
    bottom: 0;
  }
  .dock-empty {
    margin: auto;
    color: var(--text-muted, #8c93a3);
    font-size: 12px;
  }
</style>
