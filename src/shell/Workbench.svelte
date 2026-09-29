<script lang="ts">
  // Frame of the clean window (spec 018), reframed by spec 047 (AC-047-04) to the approved
  // design: the sidebar (288 px open, 56 px as the rail) runs the full height from the top of
  // the window, and the 44 px top bar sits only over the main area. Status bar 26 px.
  // Activity/agents/files/home stay in the repo as isolated components, not mounted here.
  import type { Snippet } from "svelte";
  import { t } from "../i18n/index.svelte";
  import Sidebar from "../components/frame/Sidebar.svelte";
  import type { FrameControls } from "./Workbench.types";

  let {
    sidebarOpen = $bindable(true),
    agentsOpen = $bindable(false),
    onToggleSidebar,
    onToggleAgents,
    onShellKeydown,
    controls = $bindable(null),
    topBar,
    projectsRegion,
    railRegion,
    centerRegion,
    statusBar,
    overlay,
  }: {
    sidebarOpen?: boolean;
    agentsOpen?: boolean;
    onToggleSidebar?: (open: boolean) => void;
    onToggleAgents?: (open: boolean) => void;
    onShellKeydown?: (event: KeyboardEvent) => void;
    controls?: FrameControls | null;
    topBar?: Snippet;
    projectsRegion?: Snippet;
    /** The 56 px rail the sidebar paints while it is collapsed (spec 047). */
    railRegion?: Snippet;
    centerRegion?: Snippet;
    statusBar?: Snippet;
    overlay?: Snippet;
  } = $props();

  const frameControls: FrameControls = {
    get sidebarOpen() {
      return sidebarOpen;
    },
    get agentsOpen() {
      return agentsOpen;
    },
    get narrow() {
      return false;
    },
    toggleSidebar: () => {
      sidebarOpen = !sidebarOpen;
      onToggleSidebar?.(sidebarOpen);
    },
    toggleAgents: () => {
      agentsOpen = !agentsOpen;
      onToggleAgents?.(agentsOpen);
    },
    selectPanel: () => {
      sidebarOpen = true;
      onToggleSidebar?.(true);
    },
  };
  controls = frameControls;

  function frameKeys(node: HTMLElement) {
    const handler = (event: KeyboardEvent) => onShellKeydown?.(event);
    node.addEventListener("keydown", handler);
    return { destroy: () => node.removeEventListener("keydown", handler) };
  }
</script>

<div class="wb-shell" use:frameKeys>
  <div class="wb-body">
    <Sidebar open={sidebarOpen}>
      {@render projectsRegion?.()}
      {#snippet rail()}
        {@render railRegion?.()}
      {/snippet}
    </Sidebar>

    <div class="wb-column">
      {@render topBar?.()}

      <main class="wb-main" data-region="main" aria-label={t("shell.workbench.main")}>
        {@render centerRegion?.()}
      </main>
    </div>
  </div>

  {@render statusBar?.()}
  {@render overlay?.()}
</div>

<style>
  .wb-shell {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    background-color: var(--bg);
    color: var(--text);
    font-family: var(--font-ui);
    font-size: 13px;
    line-height: 1.4;
    overflow: hidden;
    position: relative;
  }
  .wb-body {
    flex: 1;
    min-height: 0;
    min-width: 0;
    display: flex;
    overflow: hidden;
    position: relative;
  }
  .wb-column {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .wb-main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    background-color: var(--bg);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
</style>
