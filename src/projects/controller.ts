// Drives the navigator: every user intent becomes one bridge command; results replace the
// snapshot. Loading never opens a project. Errors stay on the resource that failed.

import type { HostDto } from "../connections/types";
import { workspaceMatchesRoot } from "../components/projects/tree-model";
import { t } from "../i18n/index.svelte";
import type { ProjectsBridge } from "./bridge";
import {
  dragMoveTarget,
  initialState,
  keyboardMoveTarget,
  missingDraftFields,
  reduce,
  type NavigatorAction,
  type NavigatorState,
} from "./reducer";
import type {
  GroupAssignInput,
  ProjectDraft,
  ProjectsSnapshot,
  RuntimeError,
  WorkspaceCreatedDto,
  WorkspaceCreateInput,
  WorkspacePrefInput,
} from "./types";

export interface ProjectsController {
  readonly state: NavigatorState;
  load(): Promise<void>;
  createCollection(name: string): Promise<void>;
  openForm(collectionId: string | null): void;
  editForm(field: keyof ProjectDraft, value: string): void;
  cancelForm(): void;
  submitProject(): Promise<void>;
  addToCollection(collectionId: string, projectId: string): Promise<void>;
  removeFromCollection(collectionId: string, projectId: string): Promise<void>;
  moveByKeyboard(collectionId: string, projectId: string, direction: "up" | "down"): Promise<void>;
  moveByDrag(collectionId: string, draggedId: string, overId: string, placement: "before" | "after"): Promise<void>;
  moveCollection(collectionId: string, direction: "up" | "down"): Promise<void>;
  open(projectId: string): Promise<void>;
  openFolder(pick: () => Promise<string | null>, session: string, endpoint?: string, groupName?: string): Promise<void>;
  /** Spec 025: focus the engine workspace of a host (never creates a tab/pane/workspace). */
  focusWorkspace(endpoint: string, workspaceId: string): Promise<void>;
  /** Spec 025: a closed project becomes a workspace with its saved cwd, focused. */
  openClosedProject(projectId: string): Promise<void>;
  renameWorkspace(endpoint: string, workspaceId: string, label: string): Promise<void>;
  closeWorkspace(endpoint: string, workspaceId: string): Promise<void>;
  /** Spec 025: local groups (the catalog formerly called collections). */
  createGroup(name: string): Promise<void>;
  assignToGroup(groupId: string, entry: GroupAssignInput): Promise<void>;
  /** Spec 044: colour / pinned / hidden of one workspace; the engine is never told. */
  setWorkspacePref(input: WorkspacePrefInput): Promise<void>;
  /**
   * Spec 046: the two steps whose refusal belongs to the "Novo workspace" modal — its own error
   * line replaces the row/collection error the sidebar shows for the other actions.
   */
  createWorkspace(input: WorkspaceCreateInput): Promise<WorkspaceCreatedDto>;
  assignToGroupChecked(groupId: string, entry: GroupAssignInput): Promise<void>;
  /** Spec 046: the folder a successful creation used, as the newest recent of its host. */
  recordRecentFolder(endpoint: string, path: string): Promise<void>;
  /** Spec 045: the collection menu and the persisted collapse. Nothing reaches the engine. */
  renameGroup(groupId: string, name: string): Promise<void>;
  setGroupColor(groupId: string, color: string): Promise<void>;
  setGroupCollapsed(groupId: string, collapsed: boolean): Promise<void>;
  deleteGroup(groupId: string): Promise<void>;
}

export function projectBasename(root: string): string {
  return root.split(/[\\/]/).filter(Boolean).at(-1) ?? root;
}

export interface ProjectsControllerOptions {
  /**
   * Live hosts of the window (spec 025 AC-025-05): a saved project whose cwd is an open
   * workspace is focused, never recreated. Defaults to no hosts.
   */
  hosts?: () => readonly HostDto[];
}

function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) return error as RuntimeError;
  return { code: "ipc_error", message: t("projects.error.ipc"), retryable: true };
}

export function createProjectsController(
  bridge: ProjectsBridge,
  onChange: (state: NavigatorState) => void = () => {},
  options: ProjectsControllerOptions = {},
): ProjectsController {
  let state = initialState();
  let picking = false;
  // One in-flight workspace action per target; a second click is ignored, never queued.
  const acting = new Set<string>();
  const once = async (key: string, run: () => Promise<void>) => {
    if (acting.has(key)) return;
    acting.add(key);
    try {
      await run();
    } finally {
      acting.delete(key);
    }
  };
  const dispatch = (action: NavigatorAction) => {
    state = reduce(state, action);
    onChange(state);
  };

  async function storeCommand(run: () => Promise<ProjectsSnapshot>, collectionId?: string) {
    try {
      dispatch({ type: "snapshot", snapshot: await run() });
    } catch (error) {
      const runtime = asRuntimeError(error);
      if (collectionId) dispatch({ type: "collection_failed", collectionId, error: runtime });
      else dispatch({ type: "store_failed", error: runtime });
    }
  }

  function collectionIds(collectionId: string): string[] {
    return state.snapshot?.collections.find((c) => c.id === collectionId)?.project_ids ?? [];
  }

  return {
    get state() {
      return state;
    },
    async load() {
      dispatch({ type: "load_started" });
      try {
        dispatch({ type: "loaded", snapshot: await bridge.list() });
      } catch (error) {
        dispatch({ type: "load_failed", error: asRuntimeError(error) });
      }
    },
    createCollection: (name) => storeCommand(() => bridge.createCollection(name)),
    openForm: (collectionId) => dispatch({ type: "form_open", collectionId }),
    editForm: (field, value) => dispatch({ type: "form_edit", field, value }),
    cancelForm: () => dispatch({ type: "form_cancel" }),
    async submitProject() {
      const missing = missingDraftFields(state.form.draft);
      if (missing.length > 0) {
        dispatch({ type: "form_invalid", missing });
        return;
      }
      try {
        const snapshot = await bridge.createProject(state.form.draft, state.form.collectionId);
        dispatch({ type: "form_submitted", snapshot });
      } catch (error) {
        dispatch({ type: "form_failed", error: asRuntimeError(error) });
      }
    },
    addToCollection: (collectionId, projectId) =>
      storeCommand(() => bridge.addToCollection(collectionId, projectId, null), collectionId),
    removeFromCollection: (collectionId, projectId) =>
      storeCommand(() => bridge.removeFromCollection(collectionId, projectId), collectionId),
    async moveByKeyboard(collectionId, projectId, direction) {
      const to = keyboardMoveTarget(collectionIds(collectionId), projectId, direction);
      if (to === null) return;
      await storeCommand(() => bridge.moveProject(collectionId, projectId, to), collectionId);
    },
    async moveByDrag(collectionId, draggedId, overId, placement) {
      const to = dragMoveTarget(collectionIds(collectionId), draggedId, overId, placement);
      if (to === null) return;
      await storeCommand(() => bridge.moveProject(collectionId, draggedId, to), collectionId);
    },
    async moveCollection(collectionId, direction) {
      const ids = state.snapshot?.collections.map((c) => c.id) ?? [];
      const to = keyboardMoveTarget(ids, collectionId, direction);
      if (to === null) return;
      await storeCommand(() => bridge.moveCollection(collectionId, to), collectionId);
    },
    async openFolder(pick, session, endpoint = "local", groupName = "Meus projetos") {
      if (picking) return;
      // AC-046-04: the native dialog only browses this computer, so it is never opened for an
      // SSH host — a folder picked here would be a Local path sent to another machine. The
      // remote path comes from the "Novo workspace" field instead; nothing falls back to Local.
      if (endpoint !== "local") {
        dispatch({
          type: "store_failed",
          error: {
            code: "remote_folder_picker_unavailable",
            message: t("projects.error.remotePicker"),
            retryable: false,
          },
        });
        return;
      }
      picking = true;
      let workspaceId: string | undefined;
      try {
        const cwd = await pick();
        if (cwd === null) return;
        let snapshot = await bridge.list();
        let group = snapshot.collections.find((c) => c.name === groupName);
        if (!group) {
          snapshot = await bridge.groupCreate(groupName);
          group = snapshot.collections.find((c) => c.name === groupName);
        }
        if (!group) throw new Error("group missing");
        const label = projectBasename(cwd);
        const created = await bridge.workspaceCreate({ endpoint_profile_id: endpoint, cwd, label, focus: true });
        workspaceId = created.workspace_id;
        dispatch({ type: "open_started", projectId: created.workspace_id });
        const after = await bridge.groupAssign(group.id, {
          endpoint_profile_id: endpoint,
          session_name: session,
          cwd,
          label,
        });
        dispatch({ type: "snapshot", snapshot: after });
        dispatch({ type: "workspace_opened", id: created.workspace_id });
      } catch (error) {
        const runtime = asRuntimeError(error);
        if (workspaceId) dispatch({ type: "open_failed", projectId: workspaceId, error: runtime });
        else dispatch({ type: "store_failed", error: runtime });
      } finally {
        picking = false;
      }
    },
    async open(projectId) {
      if (state.opening[projectId]) return;
      dispatch({ type: "open_started", projectId });
      try {
        dispatch({ type: "open_succeeded", response: await bridge.open(projectId) });
      } catch (error) {
        dispatch({ type: "open_failed", projectId, error: asRuntimeError(error) });
      }
    },
    async focusWorkspace(endpoint, workspaceId) {
      await once(`focus:${endpoint}:${workspaceId}`, async () => {
        try {
          await bridge.workspaceFocus(endpoint, workspaceId);
        } catch (error) {
          dispatch({ type: "store_failed", error: asRuntimeError(error) });
        }
      });
    },
    async openClosedProject(projectId) {
      await once(`open:${projectId}`, async () => {
        try {
          let project = state.snapshot?.projects.find((p) => p.id === projectId);
          if (!project) project = (await bridge.list()).projects.find((p) => p.id === projectId);
          if (!project) throw { code: "project_not_found", message: t("projects.error.projectNotFound"), retryable: false } satisfies RuntimeError;
          // AC-025-05: an open workspace on the saved cwd is focused, never duplicated by a
          // create — even if a stale tree still showed the project as closed.
          const live = (options.hosts?.() ?? [])
            .filter((host) => host.endpoint === project.endpoint_profile_id)
            .flatMap((host) => host.workspaces ?? [])
            .find((workspace) => workspaceMatchesRoot(workspace, project.root));
          if (live) {
            await bridge.workspaceFocus(project.endpoint_profile_id, live.workspace_id);
            return;
          }
          dispatch({ type: "open_started", projectId });
          await bridge.workspaceCreate({
            endpoint_profile_id: project.endpoint_profile_id,
            cwd: project.root,
            label: projectBasename(project.root),
            focus: true,
          });
          dispatch({ type: "workspace_opened", id: projectId });
        } catch (error) {
          dispatch({ type: "open_failed", projectId, error: asRuntimeError(error) });
        }
      });
    },
    async renameWorkspace(endpoint, workspaceId, label) {
      const trimmed = label.trim();
      if (!trimmed) return;
      await once(`rename:${endpoint}:${workspaceId}`, async () => {
        try {
          await bridge.workspaceRename(endpoint, workspaceId, trimmed);
        } catch (error) {
          dispatch({ type: "store_failed", error: asRuntimeError(error) });
        }
      });
    },
    async closeWorkspace(endpoint, workspaceId) {
      await once(`close:${endpoint}:${workspaceId}`, async () => {
        try {
          await bridge.workspaceClose(endpoint, workspaceId);
        } catch (error) {
          dispatch({ type: "store_failed", error: asRuntimeError(error) });
        }
      });
    },
    createGroup: (name) => storeCommand(() => bridge.groupCreate(name)),
    createWorkspace: (input) => bridge.workspaceCreate(input),
    async assignToGroupChecked(groupId, entry) {
      await storeCommand(() => bridge.groupAssign(groupId, entry), groupId);
      const failure = state.collectionErrors[groupId];
      if (failure) throw failure;
    },
    recordRecentFolder: (endpoint, path) => storeCommand(() => bridge.recentFolderAdd(endpoint, path)),
    assignToGroup: (groupId, entry) => storeCommand(() => bridge.groupAssign(groupId, entry), groupId),
    async renameGroup(groupId, name) {
      // AC-045-02: an empty name sends nothing; the backend bounds the rest.
      const trimmed = name.trim();
      if (!trimmed) return;
      await storeCommand(() => bridge.groupRename(groupId, trimmed), groupId);
    },
    setGroupColor: (groupId, color) => storeCommand(() => bridge.groupSetColor(groupId, color), groupId),
    async setGroupCollapsed(groupId, collapsed) {
      // One in-flight write per collection: a double click on the header is ignored, not queued.
      await once(`collapse:${groupId}`, () =>
        storeCommand(() => bridge.groupSetCollapsed(groupId, collapsed), groupId),
      );
    },
    deleteGroup: (groupId) => storeCommand(() => bridge.groupDelete(groupId), groupId),
    async setWorkspacePref(input) {
      // One in-flight write per workspace, like the other row actions: a double click on the
      // same swatch or on Fixar is ignored, never queued.
      await once(`pref:${input.endpoint_profile_id}:${input.root}`, () =>
        storeCommand(() => bridge.workspacePrefSet(input)),
      );
    },
  };
}
