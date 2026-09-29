<script lang="ts">
  // Spec 046 (round 2) — owner of the "Novo workspace" modal, mounted by the window beside the
  // frame and never inside the sidebar.
  //
  // The sidebar is not always on screen: spec 047 replaces it with the 56 px rail whenever it is
  // collapsed, by the user's choice or by a window too narrow for a 400 px center. A subscriber
  // living in the WORKSPACES section is therefore unmounted exactly then, and the requests of the
  // palette command and of "Abrir projeto…" on an SSH host (AC-046-04) would be dropped in
  // silence. Here the request always has a listener, in the three states of the sidebar.
  import type { FrameContext } from "../../shell/frame-context";
  import { onNewWorkspaceRequest } from "../../projects/new-workspace";
  import NewWorkspaceDialog from "./NewWorkspaceDialog.svelte";

  let { ctx }: { ctx: FrameContext } = $props();

  /** Open modal: `null` when closed, otherwise the host it opened marking (null = the selected). */
  let creating = $state<{ endpoint: string | null } | null>(null);

  $effect(() => onNewWorkspaceRequest((endpoint) => (creating = { endpoint })));
</script>

{#if creating}
  <NewWorkspaceDialog {ctx} endpoint={creating.endpoint} onclose={() => (creating = null)} />
{/if}
