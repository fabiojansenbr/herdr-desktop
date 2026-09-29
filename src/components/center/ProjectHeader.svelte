<script lang="ts">
  // Spec 013 name kept as a mount of the 017 workspace header.
  import type { FrameContext } from "../../shell/frame-context";
  import WorkspaceHeader from "./WorkspaceHeader.svelte";

  let { ctx }: { ctx: FrameContext } = $props();
</script>

<WorkspaceHeader {ctx} />
