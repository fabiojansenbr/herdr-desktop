<script lang="ts">
  // The status glyph of a sidebar line, in one place. The nested tabs of a workspace (spec 042)
  // and the "Precisa de você" box (spec 051) are the same kind of line, so they must not drift
  // into two drawings of the same state: one component owns the icon and its colour per state.
  import type { SidebarStatusKind } from "./sidebar-model";

  let { status }: { status: SidebarStatusKind } = $props();
</script>

<span class="status" data-status={status} aria-hidden="true">
  {#if status === "working"}
    <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M21 12a9 9 0 1 1-6.2-8.55" /></svg>
  {:else if status === "waiting"}
    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9" /><path d="M12 7.5v5" /><path d="M12 16.2v.2" /></svg>
  {:else if status === "done"}
    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="m8.5 12.2 2.4 2.4 4.6-4.9" /></svg>
  {:else}
    <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="9" /></svg>
  {/if}
</span>

<style>
  .status {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    flex: none;
    color: var(--idle, #8c93a3);
  }
  .status[data-status="working"],
  .status[data-status="done"] {
    color: var(--working, #5bd68a);
  }
  .status[data-status="waiting"] {
    color: var(--attention, #f4b454);
  }
</style>
