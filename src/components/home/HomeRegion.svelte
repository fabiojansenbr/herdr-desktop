<script lang="ts">
  // Home region slot (spec 010 foundation; filled by spec 012). It renders the designed home
  // screen ("Boa tarde, Fábio") from the window's tree/agents state; the Workbench mounts this
  // layer only while the center view is "home" (no active project on open, or the Início item).
  import HomeScreen from "./HomeScreen.svelte";
  import type { FrameContext } from "../../shell/frame-context";

  let { ctx }: { ctx: FrameContext } = $props();
</script>

<div class="region" data-region="home" data-slot="home" data-view={ctx.view}>
  <HomeScreen {ctx} />
</div>

<style>
  .region {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
</style>
