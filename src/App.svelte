<script lang="ts">
  // Composed window (spec 007) in the design frame (spec 010). One host selection drives the one
  // terminal surface, the agents panel and the files view; projects/collections on the left,
  // terminal in the center, agents on the right. The regions are the Workbench's named slots,
  // filled through their owners' region components (specs 011–015 edit those, not this file).
  // Feature controllers live here (not inside Workbench snippets, which unmount
  // when a panel collapses). Nothing on render creates a process: attaching only connects, and
  // starting a named session is the explicit "Iniciar sessão"; zero-config starts `default`.
  import { onDestroy, onMount, tick } from "svelte";
  import CenterRegion from "./components/center/CenterRegion.svelte";
  import { workspaceCounts } from "./components/center/model";
  import CommandPalette from "./components/frame/CommandPalette.svelte";
  import { buildMenus, type MenuAction } from "./components/frame/menus";
  import { createPaletteFocus, paletteSections, type PaletteEntry } from "./components/frame/palette";
  import { readSidebarOpen, writeSidebarOpen } from "./components/frame/sidebar";
  import { statusBarModel } from "./components/frame/status";
  import StatusBar from "./components/frame/StatusBar.svelte";
  import TitleBar from "./components/frame/TitleBar.svelte";
  import NewWorkspaceHost from "./components/sidebar/NewWorkspaceHost.svelte";
  import SidebarRail from "./components/sidebar/SidebarRail.svelte";
  import SidebarV2 from "./components/sidebar/SidebarV2.svelte";
  import ConnectionDialog from "./components/ConnectionDialog.svelte";
  import { pickProjectFolder } from "./projects/bridge";
  import { tauriAgentsBridge } from "./agents/bridge";
  import { createAgentsController } from "./agents/controller";
  import type { AgentsState } from "./agents/reducer";
  import { tauriConnectionsBridge } from "./connections/bridge";
  import { createConnectionsController, type ConnectionsState } from "./connections/controller";
  import { tauriProjectsBridge } from "./projects/bridge";
  import { createProjectsController } from "./projects/controller";
  import type { NavigatorState } from "./projects/reducer";
  import type { OpenResponse, ProjectDto } from "./projects/types";
  import { createTerminalActions, tauriTerminalActionsBridge, terminalActionLog } from "./shell/actions";
  import type { FrameContext } from "./shell/frame-context";
  import { platformOf, resolveShortcut, shortcutLabel } from "./shell/shortcuts";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { tauriSurfaceBridge } from "./shell/bridge";
  import { errorText, t } from "./i18n/index.svelte";
  import {
    createSurfaceController,
    startingHerdr,
    surfaceStatusText,
    type SurfaceState,
  } from "./shell/controller";
  import {
    applyWindowSurfaceAction,
    bindNativeWindowSurface,
    INITIAL_WINDOW_SURFACE,
    surfaceActiveFromWindow,
    type WindowSurfaceState,
  } from "./shell/interest";

  import {
    createTerminalReveal,
    ensureHostOnline,
    selectingProjectsBridge,
    selectionScopedAgentsBridge,
  } from "./shell/routing";
  import Workbench from "./shell/Workbench.svelte";
  import type { ActivityId, CenterView, FrameControls } from "./shell/Workbench.types";
  import TerminalView from "./terminal/TerminalView.svelte";
  import { defaultFrameCache } from "./terminal/frame-cache";
  import { DEFAULT_THEME, themeFromDto, type Theme } from "./terminal/colors";
  import { applyTheme, terminalThemeFrom, type ThemeDto } from "./theme/apply";

  // --- surface / selection -------------------------------------------------------------------
  let view: ReturnType<typeof TerminalView> | undefined = $state();
  let surface = $state<SurfaceState | null>(null);
  const surfaceBridge = tauriSurfaceBridge();
  const surfaceController = createSurfaceController(surfaceBridge, {
    onChange: (next) => (surface = next),
    geometry: () => view?.currentGeometry() ?? null,
    // The keyed TerminalView remounts on the new surfaceKey and subscribes before surface_attach,
    // so frames emitted during the attach call are not lost. One flush, no extra repaint.
    ready: () => tick(),
    // The previous host's agents attachment ends as soon as another host is chosen...
    onSelectionStart: () => {
      openAbort?.abort();
      agents.invalidate();
    },
    // ...and agents attach once per confirmed live connection (identity + live), including
    // a reconnect or reboot of the same host.
    onLive: () => {
      agents.invalidate();
      void agents.connect(agentGeometry());
    },
  });
  surface = surfaceController.state;
  const current = $derived(surface ?? surfaceController.state);
  // Terminal actions (focus/scroll/copy/link): the identity is captured when the user acts, only
  // while it belongs to the selected host; the backend refuses anything stale and never re-sends.
  const terminalActions = createTerminalActions(tauriTerminalActionsBridge(), () => {
    const state = surfaceController.state;
    return state.identity && state.selection?.endpoint === state.identity.endpoint ? state.identity : null;
  }, terminalActionLog()?.record);
  const selectedEndpoint = $derived(current.selection?.endpoint ?? null);
  let windowSurface = $state<WindowSurfaceState>({ ...INITIAL_WINDOW_SURFACE });
  // Actions that need the terminal surface (focus/split/tabs, project open) issued while Files is
  // shown bring the terminal back and wait for the backend ack + a full frame of the same host
  // first; a hidden document is never treated as visible (spec 007 navigation checkpoint).
  const terminalReveal = createTerminalReveal({
    surface: surfaceController,
    showTerminal: () => {
      // The clean casca has a single terminal layer (spec 018); files/home are not mounted.
    },
    documentVisible: () => surfaceActiveFromWindow(windowSurface),
  });
  // Same gate as the surface input queue (live identity AND interest acknowledged + full frame);
  // recomputed on every surface state change, which every gate change commits.
  const terminalInputEnabled = $derived(current !== null && surfaceController.canInput());

  // --- connections ---------------------------------------------------------------------------
  const connectionsBridge = tauriConnectionsBridge();
  let connectionsState = $state<ConnectionsState | null>(null);
  const connections = createConnectionsController(connectionsBridge, (next) => (connectionsState = next));
  connectionsState = connections.state;
  // Display names from the connections catalog the App already loaded; ids stay the identity.
  // The Local host is named by the front in the active language (spec 072), never by the backend label.
  const hostLabels = $derived(
    Object.fromEntries(
      (connectionsState?.view?.hub.hosts ?? []).map((h) => [h.endpoint, h.endpoint === "local" ? t("sidebar.host.thisComputer") : h.label]),
    ),
  );

  // --- agents --------------------------------------------------------------------------------
  let agentsState = $state<AgentsState | null>(null);
  const agents = createAgentsController(
    selectionScopedAgentsBridge(tauriAgentsBridge(), () => surfaceController.state.selection?.endpoint ?? null, {
      reveal: (endpoint) => terminalReveal.reveal(endpoint),
    }),
    (next) => (agentsState = next),
  );
  agentsState = agents.state;
  const agentGeometry = () => view?.currentGeometry() ?? { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 };

  // --- projects ------------------------------------------------------------------------------
  let active = $state<{ project: ProjectDto; response: OpenResponse } | null>(null);
  let openAbort: AbortController | null = null;
  let navState = $state<NavigatorState | null>(null);
  const projects = createProjectsController(
    selectingProjectsBridge(tauriProjectsBridge(), {
      ensureHost: (endpoint) => {
        openAbort?.abort();
        const abort = new AbortController();
        openAbort = abort;
        return ensureHostOnline(connectionsBridge, endpoint, { signal: abort.signal });
      },
      // Selects the project's host (when needed) and reactivates its terminal surface before the
      // endpoint open/focus; a switch, hide, failed show or timeout fails the open unsent.
      select: async (endpoint) => {
        await terminalReveal.selectAndReveal(endpoint);
      },
      onOpened: (project, response) => (active = { project, response }),
    }),
    (next) => (navState = next),
    // Spec 025 AC-025-05: clicking a closed project checks the live workspaces of its host.
    { hosts: () => connectionsState?.view?.hub.hosts ?? [] },
  );
  navState = projects.state;

  // Terminal interest (spec 019): the surface lease follows visibility/minimize, never keyboard
  // blur. `ClientShellFocus` still comes from the terminal view for cursor/input.
  const onVisibility = () => {
    windowSurface = applyWindowSurfaceAction(windowSurface, {
      kind: "visibility",
      hidden: typeof document !== "undefined" && document.visibilityState === "hidden",
    });
  };
  const centerView = $derived<CenterView>("terminal");
  const terminalShown = $derived(surfaceActiveFromWindow(windowSurface) && centerView === "terminal");
  $effect(() => {
    void surfaceController.setInterest(terminalShown);
  });
  let sidebarOpen = $state(readSidebarOpen(typeof localStorage === "undefined" ? null : localStorage));
  $effect(() => {
    writeSidebarOpen(sidebarOpen, typeof localStorage === "undefined" ? null : localStorage);
  });
  let agentsOpen = $state(false);
  // The sidebar owns the "Novo agente" popover; the window only reaches its entry point.
  let sidebar = $state<ReturnType<typeof SidebarV2> | undefined>();
  let activeActivity = $state<ActivityId>("projects");

  const activeHere = $derived(active && active.project.endpoint_profile_id === selectedEndpoint ? active : null);

  let unbindNative: (() => void) | undefined;
  let stopTheme: (() => void) | undefined;
  let termTheme = $state<Theme>(DEFAULT_THEME);
  function adoptTheme(dto: ThemeDto) {
    applyTheme(dto);
    termTheme = terminalThemeFrom(dto);
  }
  onMount(() => {
    document.addEventListener("visibilitychange", onVisibility);
    onVisibility();
    void bindNativeWindowSurface((action) => {
      windowSurface = applyWindowSurfaceAction(windowSurface, action);
    }).then((stop) => {
      unbindNative = stop;
    });
    void projects.load();
    void connections.load().then(() => connections.watch());
    void surfaceController.load();
    void (async () => {
      try {
        adoptTheme(await invoke<ThemeDto>("theme_current"));
      } catch {
        applyTheme(null);
        termTheme = themeFromDto(null);
      }
      try {
        stopTheme = await listen<ThemeDto>("theme_changed", (event) => adoptTheme(event.payload));
      } catch {
        /* not running inside Tauri */
      }
    })();
  });
  onDestroy(() => {
    if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onVisibility);
    unbindNative?.();
    stopTheme?.();
    connections.stop();
    surfaceController.dispose();
  });

  const caps = $derived(agentsState?.capabilities ?? null);
  const agentsReady = $derived(agentsState?.phase === "connected");
  const hasSession = $derived(Boolean(current.selection?.endpoint && (current.selection.session ?? current.status?.session)));
  const statusText = $derived(surfaceStatusText(current));
  const hostKindText = $derived(current.selection?.kind === "ssh" ? "SSH" : current.selection?.kind === "local" ? "Local" : "");
  const diagnostics = $derived(
    current.identity
      ? t("shell.app.diagnostics", {
          pane: current.identity.pane_id,
          generation: current.identity.connection_generation,
          boot: current.identity.boot_id.slice(0, 8),
        })
      : t("shell.app.noIdentity"),
  );

  // --- frame (spec 010) ------------------------------------------------------------------------
  let controls = $state<FrameControls | null>(null);
  const platform = platformOf(typeof navigator === "undefined" ? "" : navigator.userAgent);
  const searchHint = shortcutLabel("palette", platform);
  const liveCaps = $derived(agentsReady ? caps : null);
  const hostOnline = $derived(Boolean(current.selection?.endpoint) && current.phase === "live");
  const unavailableReason = (available: boolean | undefined) =>
    !current.selection?.endpoint
      ? t("shell.app.unavailableNoHost")
      : !hostOnline
        ? t("shell.app.unavailableOffline")
        : available
          ? null
          : t("shell.app.unavailableAction");

  const actions: Record<MenuAction, () => void> = {
    showProjects: () => selectActivity("projects"),
    openConnections: () => selectActivity("connections"),
    paste: () => void nativePasteWithReport(),
    toggleProjects: () => controls?.toggleSidebar(),
    toggleAgents: () => controls?.toggleAgents(),
    toggleFiles: () => {},
    openPalette: () => openPalette(),
    newAgent: () => {},
    split: () => void agents.split("right"),
    newTab: () => void agents.createTab(),
    reconnect: () => void surfaceController.retry(),
  };

  /** One native paste; answers what the backend pasted (spec 028) for the terminal's notice. */
  async function nativePasteWithReport() {
    await surfaceController.nativePaste();
    return surfaceController.state.paste;
  }

  function selectActivity(id: ActivityId) {
    if (id === "search") return openPalette();
    if (id === "projects" || id === "connections") {
      sidebarOpen = true;
      controls?.selectPanel("projects", activeActivity);
    }
    activeActivity = id;
  }

  const menus = $derived(
    buildMenus({
      hostSelected: Boolean(current.selection?.endpoint),
      hostOnline,
      canInput: terminalInputEnabled,
      canRetry: Boolean(current.selection?.endpoint) && current.phase === "disconnected",
      caps: liveCaps,
      platform,
      actions,
    }),
  );

  // Command palette: opener focus is returned synchronously before the palette leaves the DOM.
  let paletteOpen = $state(false);
  const paletteFocus = createPaletteFocus(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));
  function openPalette() {
    if (paletteOpen) return;
    paletteFocus.open();
    paletteOpen = true;
  }
  function closePalette() {
    paletteFocus.close();
    paletteOpen = false;
  }
  function runEntry(entry: PaletteEntry) {
    closePalette();
    entry.run();
  }
  const paletteFor = (query: string) =>
    paletteSections(
      {
        hasSession,
        projects: (navState?.snapshot?.projects ?? []).map((p) => ({ id: p.id, label: p.label, root: p.root, host: hostLabels[p.endpoint_profile_id] ?? p.endpoint_profile_id })),
        panes: agentsReady ? (agentsState?.topology?.panes ?? []) : [],
        agents: agentsReady ? (agentsState?.agents ?? []) : [],
        commands: menus.flatMap((m) => m.items).filter((i) => i.run).map((i) => ({ id: i.id, label: i.label, shortcut: i.shortcut, run: i.run! })),
        openProject: (id) => void projects.open(id),
        focusPane: (id) => void agents.focusPane(id),
        openAgent: (id) => void agents.openAttention(id),
      },
      query,
    );

  function onShellKeydown(event: KeyboardEvent) {
    const target = event.target instanceof Element ? event.target : null;
    const id = resolveShortcut({ key: event.key, ctrlKey: event.ctrlKey, metaKey: event.metaKey, shiftKey: event.shiftKey, altKey: event.altKey, isComposing: event.isComposing, repeat: event.repeat, target }, platform);
    if (id === "palette") {
      event.preventDefault();
      openPalette();
      return;
    }
    if (id === "sidebar") {
      event.preventDefault();
      sidebarOpen = !sidebarOpen;
      return;
    }
    if (id === "new-agent") {
      // Spec 041 AC-041-01 (P8): the chord opens the sidebar's own "Novo agente" popover, the
      // same one the button mounts; no second popover is created here.
      event.preventDefault();
      sidebar?.openNewAgent();
      return;
    }
    if (id === "new-tab") {
      // Spec 028 AC-028-01: one tab.create in the focused workspace, like the + button.
      event.preventDefault();
      void agents.createTab();
    }
  }

  const selectedHost = $derived(connectionsState?.view?.hub.hosts.find((h) => h.endpoint === selectedEndpoint) ?? null);
  const branch = $derived(selectedHost?.branch ?? null);
  const focusedCwd = $derived(
    agentsReady
      ? (agentsState?.topology?.panes.find((p) => p.focused || p.pane_id === current.identity?.pane_id)?.cwd ?? null)
      : null,
  );
  // Spec 025: the focused workspace of the selected host (the engine's own list); the selector
  // shows `<host> › <workspace> ⎇ <branch>` and the status line counts its panes/tabs.
  const focusedWorkspace = $derived((selectedHost?.workspaces ?? []).find((workspace) => workspace.focused) ?? null);
  const focusedCounts = $derived(
    selectedHost?.api === false || (agentsState?.tabs ?? []).length === 0
      ? {
          tabs: (selectedHost?.tabs ?? []).filter((t) =>
            focusedWorkspace ? t.workspace_id === focusedWorkspace.workspace_id : true,
          ).length,
          panes: (selectedHost?.panes ?? []).filter((p) =>
            focusedWorkspace ? p.workspace_id === focusedWorkspace.workspace_id : true,
          ).length,
        }
      : workspaceCounts({
          tabs: agentsState?.tabs ?? [],
          panes: agentsState?.topology?.panes ?? [],
          workspaceId: focusedWorkspace?.workspace_id ?? null,
        }),
  );
  const statusModel = $derived(
    statusBarModel({
      phase: current.phase,
      hasEndpoint: Boolean(current.selection?.endpoint),
      serverVersion: selectedHost?.server_version ?? current.status?.server_version ?? agentsState?.serverVersion ?? null,
      branch,
      panes: agentsReady || (selectedHost?.api === false && current.phase === "live") ? focusedCounts.panes : null,
      tabs: agentsReady || (selectedHost?.api === false && current.phase === "live") ? focusedCounts.tabs : null,
      agents: agentsReady
        ? (agentsState?.agents.length ?? null)
        : selectedHost?.api === false && current.phase === "live"
          ? (selectedHost?.agents?.length ?? null)
          : null,
      cwd: focusedCwd,
    }),
  );
  const waitingAgents = $derived((agentsState?.agents ?? []).filter((a) => a.status === "blocked"));
  const projectLabel = $derived(
    focusedWorkspace
      ? `${selectedHost?.kind === "local" ? t("connections.kind.localHint") : (selectedHost?.label ?? selectedEndpoint)} › ${focusedWorkspace.label}${
          focusedWorkspace.branch && focusedWorkspace.branch !== "—" ? ` ⎇ ${focusedWorkspace.branch}` : ""
        }`
      : t("connections.session", { name: current.selection?.session ?? current.status?.session ?? "default" }),
  );
  // Name of the selected host in the active language: the Local host is named by the front (spec 072),
  // an SSH host keeps its own label.
  const selectionName = $derived(
    current.selection?.endpoint
      ? current.selection.endpoint === "local"
        ? t("sidebar.host.thisComputer")
        : (current.selection.label ?? current.selection.endpoint)
      : null,
  );
  const topBarHost = $derived(
    current.selection?.endpoint
      ? {
          label: selectionName ?? current.selection.endpoint,
          kind: hostKindText,
          state: statusText,
          tone: (current.phase === "live" ? "live" : current.phase === "disconnected" ? "offline" : "busy") as "live" | "busy" | "offline",
        }
      : null,
  );

  const frame: FrameContext = {
    get surface() {
      return current;
    },
    get agents() {
      return agentsState;
    },
    get connections() {
      return connectionsState;
    },
    get navigator() {
      return navState;
    },
    get selectedEndpoint() {
      return selectedEndpoint;
    },
    get activeProject() {
      return activeHere?.project ?? null;
    },
    get hostLabels() {
      return hostLabels;
    },
    get branch() {
      return branch;
    },
    get activity() {
      return activeActivity;
    },
    get view() {
      return centerView;
    },
    get sidebarOpen() {
      return sidebarOpen;
    },
    get agentsOpen() {
      return agentsOpen;
    },
    controllers: { surface: surfaceController, agents, projects, connections },
    actions,
    get unavailable() {
      return {
        split: unavailableReason(liveCaps?.split),
        newTab: unavailableReason(liveCaps?.create_tab),
        newAgent: unavailableReason(liveCaps?.start_agent),
      };
    },
  };

</script>

<Workbench
  bind:sidebarOpen
  bind:agentsOpen
  bind:controls
  {onShellKeydown}
>
  {#snippet topBar()}
    <TitleBar
      host={topBarHost}
      {searchHint}
      onOpenPalette={openPalette}
      onHost={() => connections.openDialog()}
      {sidebarOpen}
      onToggleSidebar={() => (sidebarOpen = !sidebarOpen)}
      waitingCount={waitingAgents.length}
      onBell={() => {
        const first = waitingAgents[0];
        if (first) void agents.openAttention(first.pane_id);
      }}
      {projectLabel}
      ctx={frame}
      pickFolder={pickProjectFolder}
    />
  {/snippet}

  {#snippet projectsRegion()}
    <SidebarV2 bind:this={sidebar} ctx={frame} />
  {/snippet}

  <!-- Spec 047: the 56 px rail the sidebar paints while it is collapsed. -->
  {#snippet railRegion()}
    <SidebarRail ctx={frame} />
  {/snippet}

  {#snippet centerRegion()}
    <CenterRegion ctx={frame}>
      {#if current.error}
        <div class="banner" role="alert" data-error={current.error.code}>
          <span>{errorText(current.error)}</span>
          {#if current.error.retryable && (current.selection === null || current.selection.endpoint)}
            <button type="button" onclick={() => surfaceController.retry()} disabled={current.phase === "connecting"}>
              {t("connections.action.retry")}
            </button>
          {/if}
        </div>
      {/if}

      {#if statusText === startingHerdr() && !defaultFrameCache.has(current.selection?.endpoint ?? "")}
        <div class="onboarding" data-startup>
          <h1>{startingHerdr()}</h1>
        </div>
      {:else if current.selection?.endpoint && hasSession}
        {#if current.phase === "disconnected" && current.status && !current.error}
          <div class="banner">
            {#if current.selection.kind === "local" && current.status.session && !current.status.session_available}
              <span>{t("shell.app.sessionNotRunning", { session: current.status.session })}</span>
              <button type="button" onclick={() => surfaceController.startSession()} disabled={current.busy}>{t("shell.app.startSession")}</button>
            {:else}
              <span>{t("shell.app.disconnectedFrom", { host: selectionName ?? current.selection.endpoint })}</span>
              <button type="button" onclick={() => surfaceController.retry()}>{t("connections.action.connect")}</button>
            {/if}
          </div>
        {/if}
        <div class="terminal">
          <TerminalView
            bind:this={view}
            endpoint={selectedEndpoint}
            hostLabel={topBarHost?.label}
            subscribe={surfaceController.subscribe}
            onInput={(events) => void surfaceController.input(events)}
            onResize={(geometry) => surfaceController.resize(geometry)}
            onFocus={(focused) => surfaceController.focus(focused)}
            onSelectPane={terminalActions.onSelectPane}
            onScroll={terminalActions.onScroll}
            onCopySelection={terminalActions.onCopySelection}
            onNativePaste={nativePasteWithReport}
            onOpenLink={terminalActions.onOpenLink}
            inputEnabled={terminalInputEnabled}
            visible={true}
            theme={termTheme}
          />
        </div>
      {/if}
      {#if connectionsState}
        <ConnectionDialog controller={connections} state={connectionsState} focusHost={() => surfaceBridge.focusHost()} />
      {/if}
    </CenterRegion>
  {/snippet}

  {#snippet statusBar()}
    <StatusBar
      phase={current.phase}
      {diagnostics}
      model={statusModel}
      host={current.selection?.endpoint ? `${selectionName} · ${hostKindText}` : null}
      session={current.selection?.session ?? current.status?.session ?? null}
      blocked={current.phase === "live" && !current.identity ? "entrada bloqueada: aguardando pane confirmado" : null}
      readOnly={current.selection?.kind === "ssh"}
      notice={agentsState?.notice ?? null}
    />
  {/snippet}

  {#snippet overlay()}
    {#if paletteOpen}
      <CommandPalette sections={paletteFor} onRun={runEntry} onClose={closePalette} />
    {/if}
    <!-- Spec 046: the window owns the "Novo workspace" modal. It cannot live in the sidebar: the
         047 rail replaces it while it is collapsed, and the palette command and "Abrir projeto…"
         on an SSH host must reach the modal in the three states of the sidebar. -->
    <NewWorkspaceHost ctx={frame} />
  {/snippet}
</Workbench>

<style>
  .terminal {
    flex: 1;
    min-height: 0;
    display: flex;
    padding: 8px;
    background: var(--bg);
    font-family: var(--font-mono);
  }
  .terminal > :global(*) {
    flex: 1;
    min-width: 0;
  }
  /* The terminal's action toolbar is UI text over row 0: the inherited mono font widened it over
     the link cell and swallowed the Ctrl+click (010 r4). */
  .terminal :global([role="toolbar"]) {
    font-family: var(--font-ui);
  }
  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text);
  }
  .banner[role="alert"] {
    color: var(--error);
  }
  .onboarding {
    margin: auto;
    max-width: 560px;
    padding: 24px;
    color: var(--text-muted);
  }
  .onboarding h1 {
    font-size: 16px;
    color: var(--text);
  }
</style>
