<script lang="ts">
  // Center region (spec 010 slot, filled by spec 013): over the single surface App.svelte renders
  // below, the frames of the panes. The stage reserves one cell row above the terminal so the
  // frame of a pane at row 0 — and the terminal's own action toolbar — live in a band of their
  // own instead of covering content cells.
  //
  // Spec 055 (AC-055-01): the workspace tabs left this region. The strip is drawn by the top bar
  // (`components/frame/TitleBar.svelte`), so the stage is the region's only row and starts at the
  // top of the main area.
  import type { Snippet } from "svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import PaneFrames from "./PaneFrames.svelte";

  let { ctx, children }: { ctx: FrameContext; children?: Snippet } = $props();

  let stage: HTMLDivElement | undefined = $state();
</script>

<div class="region" data-slot="center">
  <div class="stage" data-center-stage bind:this={stage}>
    <PaneFrames {ctx} {stage} />
    {@render children?.()}
  </div>
</div>

<style>
  .region {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 8px;
    gap: 8px;
    background: var(--bg);
  }
</style>
