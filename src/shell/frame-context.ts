// What the composed window hands to the region components (spec 010 slots). App.svelte owns the
// controllers and state; regions of specs 011–015 read them through this object (getters, so the
// reads stay reactive) and call the existing actions, without editing App.svelte or the Workbench.
import type { AgentsController } from "../agents/controller";
import type { AgentsState } from "../agents/reducer";
import type { MenuAction } from "../components/frame/menus";
import type { ConnectionsController, ConnectionsState } from "../connections/controller";
import type { ProjectsController } from "../projects/controller";
import type { NavigatorState } from "../projects/reducer";
import type { ProjectDto } from "../projects/types";
import type { SurfaceController, SurfaceState } from "./controller";
import type { ActivityId, CenterView } from "./Workbench.types";

export interface FrameContext {
  readonly surface: SurfaceState;
  readonly agents: AgentsState | null;
  readonly connections: ConnectionsState | null;
  readonly navigator: NavigatorState | null;
  readonly selectedEndpoint: string | null;
  /** Project opened on the selected host (null when none or on another host). */
  readonly activeProject: ProjectDto | null;
  readonly hostLabels: Record<string, string>;
  /** Branch of the focused tab's workspace reported by the engine (null when unknown). */
  readonly branch: string | null;
  readonly activity: ActivityId;
  readonly view: CenterView;
  readonly sidebarOpen: boolean;
  readonly agentsOpen: boolean;
  readonly controllers: {
    readonly surface: SurfaceController;
    readonly agents: AgentsController;
    readonly projects: ProjectsController;
    readonly connections: ConnectionsController;
  };
  /** Existing window actions (the same ones the menus call). */
  readonly actions: Readonly<Record<MenuAction, () => void>>;
  /** Availability of the host-dependent actions (null = enabled, otherwise the reason). */
  readonly unavailable: Readonly<Record<"split" | "newTab" | "newAgent", string | null>>;
}
