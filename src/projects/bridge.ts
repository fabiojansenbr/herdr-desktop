// IPC wrapper for the projects module. Command names mirror `project_store::COMMANDS`
// (checked by src-tauri/tests/projects.rs). The commands are registered in the window by
// spec 007; the preview uses the fake bridge.

import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n/index.svelte";
import type { GroupAssignInput, OpenResponse, ProjectDraft, ProjectsSnapshot, WorkspacePrefInput } from "./types";
import { tauriWorkspaceBridge, type WorkspaceCommandsBridge } from "./workspace-bridge";

export interface ProjectsBridge extends WorkspaceCommandsBridge {
  list(): Promise<ProjectsSnapshot>;
  createProject(draft: ProjectDraft, collectionId: string | null): Promise<ProjectsSnapshot>;
  createCollection(name: string): Promise<ProjectsSnapshot>;
  addToCollection(collectionId: string, projectId: string, index: number | null): Promise<ProjectsSnapshot>;
  removeFromCollection(collectionId: string, projectId: string): Promise<ProjectsSnapshot>;
  moveProject(collectionId: string, projectId: string, toIndex: number): Promise<ProjectsSnapshot>;
  moveCollection(collectionId: string, toIndex: number): Promise<ProjectsSnapshot>;
  open(projectId: string): Promise<OpenResponse>;
  /** Spec 025: groups of the desktop catalog (collections renamed in the UI). */
  groupCreate(name: string): Promise<ProjectsSnapshot>;
  groupAssign(groupId: string, entry: GroupAssignInput): Promise<ProjectsSnapshot>;
  /** Spec 044: colour / pinned / hidden of one workspace; never reaches the engine. */
  workspacePrefSet(input: WorkspacePrefInput): Promise<ProjectsSnapshot>;
  /** Spec 045: the collection menu. Deleting one keeps its projects (they lose the collection). */
  groupRename(groupId: string, name: string): Promise<ProjectsSnapshot>;
  groupSetColor(groupId: string, color: string): Promise<ProjectsSnapshot>;
  groupSetCollapsed(groupId: string, collapsed: boolean): Promise<ProjectsSnapshot>;
  groupDelete(groupId: string): Promise<ProjectsSnapshot>;
  /** Spec 046: the folder a successful creation used, as the newest recent of its host. */
  recentFolderAdd(endpointProfileId: string, path: string): Promise<ProjectsSnapshot>;
}

/**
 * Native folder picker. Spec 071 (AC-071-03): the title travels already translated — the host
 * only validates it (non-empty, at most 80 characters) and never carries a text of its own.
 */
export function pickProjectFolder(): Promise<string | null> {
  return invoke<string | null>("project_pick_folder", { title: t("dialog.openProject") });
}

export function tauriProjectsBridge(): ProjectsBridge {
  return {
    list: () => invoke<ProjectsSnapshot>("projects_list"),
    createProject: (draft, collectionId) => invoke<ProjectsSnapshot>("project_create", { draft, collectionId }),
    createCollection: (name) => invoke<ProjectsSnapshot>("collection_create", { name }),
    addToCollection: (collectionId, projectId, index) =>
      invoke<ProjectsSnapshot>("collection_add_project", { collectionId, projectId, index }),
    removeFromCollection: (collectionId, projectId) =>
      invoke<ProjectsSnapshot>("collection_remove_project", { collectionId, projectId }),
    moveProject: (collectionId, projectId, toIndex) =>
      invoke<ProjectsSnapshot>("collection_move_project", { collectionId, projectId, toIndex }),
    moveCollection: (collectionId, toIndex) => invoke<ProjectsSnapshot>("collection_move", { collectionId, toIndex }),
    open: (projectId) => invoke<OpenResponse>("project_open", { projectId }),
    groupCreate: (name) => invoke<ProjectsSnapshot>("group_create", { name }),
    groupAssign: (groupId, entry) => invoke<ProjectsSnapshot>("group_assign", { groupId, entry }),
    workspacePrefSet: (input) =>
      invoke<ProjectsSnapshot>("workspace_pref_set", {
        endpointProfileId: input.endpoint_profile_id,
        root: input.root,
        color: input.color,
        pinned: input.pinned,
        hidden: input.hidden,
      }),
    groupRename: (groupId, name) => invoke<ProjectsSnapshot>("group_rename", { groupId, name }),
    groupSetColor: (groupId, color) => invoke<ProjectsSnapshot>("group_set_color", { groupId, color }),
    groupSetCollapsed: (groupId, collapsed) =>
      invoke<ProjectsSnapshot>("group_set_collapsed", { groupId, collapsed }),
    groupDelete: (groupId) => invoke<ProjectsSnapshot>("group_delete", { groupId }),
    recentFolderAdd: (endpointProfileId, path) =>
      invoke<ProjectsSnapshot>("recent_folder_add", { endpointProfileId, path }),
    ...tauriWorkspaceBridge(),
  };
}
