import type { Snippet } from "svelte";

export type ActivityId =
  | "projects"
  | "agents"
  | "connections"
  | "files"
  | "git"
  | "search"
  | "settings"
  | "profile"
  | "home";

export type ConnectionStatus = "connected" | "connecting" | "disconnected" | "stale";

export interface WorkbenchConnection {
  name: string;
  kind: "local" | "ssh";
  status: ConnectionStatus;
  latencyMs?: number;
}

/** Layer shown in the center region. */
export type CenterView = "terminal" | "files" | "home";

/** Panel controls the Workbench hands to the frame snippets (responsive rules of spec 007). */
export interface FrameControls {
  readonly sidebarOpen: boolean;
  readonly agentsOpen: boolean;
  readonly narrow: boolean;
  toggleSidebar(): void;
  toggleAgents(): void;
  /** Activity bar selection of the projects or agents panel (`userSelectActivity`). */
  selectPanel(id: "projects" | "agents", current: ActivityId): void;
}

/**
 * Frame of the composed window (spec 010): top bar, activity bar, the five regions and the status
 * bar. Regions are named slots; specs 011–015 fill them through their region components without
 * editing the Workbench or App.svelte.
 */
export interface WorkbenchProps {
  sidebarOpen?: boolean;
  agentsOpen?: boolean;
  /** Center layer shown; hidden layers keep their size (visibility), never unmounted. */
  view?: CenterView;
  /** The files layer exists once opened (it is never mounted before). */
  filesMounted?: boolean;
  onToggleSidebar?: (open: boolean) => void;
  onToggleAgents?: (open: boolean) => void;
  /** Key events of the frame (element-scoped; the terminal target is excluded by the resolver). */
  onShellKeydown?: (event: KeyboardEvent) => void;
  /** Panel controls, bound out for the frame (menus, activity bar, region headers). */
  controls?: FrameControls | null;
  topBar?: Snippet;
  activityBar?: Snippet;
  /** Region slots. */
  projectsRegion?: Snippet;
  centerRegion?: Snippet;
  agentsRegion?: Snippet;
  filesRegion?: Snippet;
  homeRegion?: Snippet;
  statusBar?: Snippet;
  /** Window-level overlays (command palette, dialogs). */
  overlay?: Snippet;
}
