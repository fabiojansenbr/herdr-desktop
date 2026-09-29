<script lang="ts">
  // Files region slot (spec 010 foundation; filled by spec 015). Renders the window's current files
  // layer content passed by App.svelte and, while the review is on screen, the terminal dock that
  // keeps the active pane's surface receiving input and frames. The dock asks for the terminal
  // interest the hidden center layer had closed; it never resizes the surface (premise 6: one
  // surface, one geometry — the dock shows the focused pane's bottom rows of the same frame).
  import type { Snippet } from "svelte";
  import type { FrameContext } from "../../shell/frame-context";
  import type { GeometryDto } from "../../terminal/types";
  import TerminalDock from "./TerminalDock.svelte";

  let { ctx, children }: { ctx: FrameContext; children?: Snippet } = $props();

  let dockOpen = $state(true);

  function closeDock() {
    dockOpen = false;
    void ctx.controllers.surface.setInterest(false);
  }
  $effect(() => {
    // The visible dock is a visible terminal: App turns the interest off for the terminal layer
    // when Files is shown, so the review asks for it again — after the current flush, in case the
    // layer change and this effect run together. Repeated calls are dropped by the controller.
    if (ctx.view !== "files" || !dockOpen) return;
    const surface = ctx.controllers.surface;
    const frame = requestAnimationFrame(() => void surface.setInterest(true));
    return () => cancelAnimationFrame(frame);
  });
</script>

<div class="region" data-region="files" data-slot="files" data-endpoint={ctx.selectedEndpoint ?? ""}>
  <div class="region-body" data-review-body>{@render children?.()}</div>
  {#if ctx.view === "files" && dockOpen}
    <TerminalDock {ctx} onClose={closeDock} />
  {/if}
</div>

<style>
  .region {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .region-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
</style>
