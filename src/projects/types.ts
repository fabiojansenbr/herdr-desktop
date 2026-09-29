// DTOs shared with src-tauri/src/project_store.rs (ProjectsSnapshot / OpenResponse).

import type { RuntimeError } from "../terminal/types";

export type { RuntimeError };

/** Ephemeral link to a running workspace; valid only for this boot and connection generation. */
export interface RuntimeBindingDto {
  project_id: string;
  connection_generation: number;
  boot_id: string;
  workspace_id: string;
}

/** Durable ProjectRef (UUID + endpoint/session/root) plus the current binding, if any. */
export interface ProjectDto {
  id: string;
  label: string;
  endpoint_profile_id: string;
  session_name: string;
  root: string;
  binding: RuntimeBindingDto | null;
  /** Git branch of the project repository (spec 011); null when unknown or not local. */
  branch?: string | null;
}

export interface CollectionDto {
  id: string;
  name: string;
  project_ids: string[];
  /**
   * Spec 045: one of `GROUP_PALETTE`. Optional on the wire — the store omits it while the
   * collection has none, so the sidebar falls back to the colour of its position.
   */
  color?: string | null;
  /** Spec 045: collapsed in the sidebar; omitted by the store while it is open. */
  collapsed?: boolean;
}

/**
 * Client-only preference of one workspace (spec 044), keyed by `(endpoint_profile_id, raiz
 * normalizada)` — the same key spec 025 uses for a project. It never reaches the engine.
 */
export interface WorkspacePrefDto {
  endpoint_profile_id: string;
  root: string;
  /** One of `GROUP_PALETTE`, or null for the default folder colour. */
  color: string | null;
  pinned: boolean;
  hidden: boolean;
}

/** `workspace_pref_set`: the fields to change; the ones left out keep their current value. */
export interface WorkspacePrefInput {
  endpoint_profile_id: string;
  root: string;
  color?: string;
  pinned?: boolean;
  hidden?: boolean;
}

/**
 * One folder already opened as a workspace on a host (spec 046), newest first. Client-only: the
 * engine is never asked for it and nothing here opens anything.
 */
export interface RecentFolderDto {
  endpoint_profile_id: string;
  path: string;
}

export interface ProjectsSnapshot {
  version: number;
  projects: ProjectDto[];
  collections: CollectionDto[];
  /** Spec 044; optional so the pre-v3 fixtures of the other specs stay valid. */
  workspace_prefs?: WorkspacePrefDto[];
  /** Spec 046 (store v4); optional so the pre-v4 fixtures of the other specs stay valid. */
  recent_folders?: RecentFolderDto[];
}

/** One workspace moved into a local group, identified by its root cwd (spec 025, AC-025-02). */
export interface GroupAssignInput {
  endpoint_profile_id: string;
  session_name: string;
  cwd: string;
  label: string;
}

/** `workspace.create` on a host (spec 025, AC-025-03). */
export interface WorkspaceCreateInput {
  endpoint_profile_id: string;
  cwd: string;
  label: string;
  focus: boolean;
}

export interface WorkspaceCreatedDto {
  workspace_id: string;
}

export interface ProjectDraft {
  label: string;
  endpoint_profile_id: string;
  session_name: string;
  root: string;
}

export type OpenOutcome = "created" | "rediscovered" | "reused";

export interface OpenResult {
  project_id: string;
  binding: RuntimeBindingDto;
  outcome: OpenOutcome;
  invalidated: RuntimeBindingDto | null;
}

export interface OpenResponse {
  result: OpenResult;
  snapshot: ProjectsSnapshot;
}
