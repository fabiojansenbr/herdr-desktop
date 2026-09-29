<script lang="ts">
  // Collapsible projects sidebar of the clean casca (spec 018 AC-018-02), reframed by spec 047
  // (AC-047-01): 288 px open, 56 px collapsed — the rail of `design/v2-04-lateral-recolhida.png`,
  // never 0 px. Collapsing swaps the children for the `rail` slot instead of emptying the region,
  // so the marks, the "Precisa de você" count and the workspace status stay on screen.
  // Persistence lives in `herdr.sidebar.open`.
  import type { Snippet } from "svelte";
  import { sidebarVisuallyOpen } from "./sidebar";
  import { t } from "../../i18n/index.svelte";

  let {
    open = true,
    viewportWidth = null,
    children,
    rail,
  }: {
    open?: boolean;
    /** Logical window width for tests; `window.innerWidth` when omitted. */
    viewportWidth?: number | null;
    children?: Snippet;
    /** What the 56 px rail paints while the sidebar is collapsed (spec 047). */
    rail?: Snippet;
  } = $props();

  let measured = $state(0);
  const width = $derived(viewportWidth ?? measured);
  const shown = $derived(sidebarVisuallyOpen(open, width));

  $effect(() => {
    if (viewportWidth != null) return;
    const update = () => {
      measured = window.innerWidth;
    };
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  });
</script>

<!-- svelte-ignore a11y_role_supports_aria_props -->
<div
  class="sidebar"
  class:rail={!shown}
  data-region="sidebar"
  data-rail={shown ? undefined : "true"}
  role="region"
  aria-label={t("frame.sidebar.label")}
  aria-expanded={shown}
>
  {#if shown}
    {@render children?.()}
  {:else}
    {@render rail?.()}
  {/if}
</div>

<style>
  .sidebar {
    flex: none;
    width: 288px;
    min-width: 0;
    background-color: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    z-index: 4;
  }
  /* The rail of spec 047: the popovers of Novo agente and of the host reach past its 56 px. */
  .sidebar.rail {
    width: 56px;
    overflow: visible;
  }
</style>
