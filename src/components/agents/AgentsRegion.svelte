<script lang="ts">
  // Agents region (spec 014 panel; spec 017 AC-017-03 removes the 004 session-controls block).
  // Header AGENTES, counters, "Precisa da sua atenção" and "Em execução" only.
  import AgentPanel from "./AgentPanel.svelte";
  import type { FrameContext } from "../../shell/frame-context";

  let { ctx }: { ctx: FrameContext } = $props();
</script>

<div class="region" data-region="agents" data-slot="agents" data-agents-phase={ctx.agents?.phase ?? "idle"}>
  {#if ctx.agents}
    <AgentPanel {ctx} />
  {/if}
</div>

<style>
  .region {
    width: 256px;
    flex: none;
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
