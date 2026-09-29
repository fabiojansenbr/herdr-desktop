// Navigator state: the last snapshot from the backend plus UI-only state (in-flight opens,
// per-resource errors, the project form buffer). The backend stays the authority for the
// store; nothing here normalises session names or paths.

import { t } from "../i18n/index.svelte";
import type {
  OpenOutcome,
  OpenResponse,
  ProjectDraft,
  ProjectDto,
  ProjectsSnapshot,
  RuntimeBindingDto,
  RuntimeError,
} from "./types";

export interface FormState {
  open: boolean;
  collectionId: string | null;
  draft: ProjectDraft;
  missing: (keyof ProjectDraft)[];
  error: RuntimeError | null;
}

export interface NavigatorState {
  snapshot: ProjectsSnapshot | null;
  loading: boolean;
  globalError: RuntimeError | null;
  projectErrors: Record<string, RuntimeError>;
  collectionErrors: Record<string, RuntimeError>;
  opening: Record<string, true>;
  lastOutcome: Record<string, OpenOutcome>;
  invalidated: Record<string, RuntimeBindingDto>;
  form: FormState;
}

export type NavigatorAction =
  | { type: "load_started" }
  | { type: "loaded"; snapshot: ProjectsSnapshot }
  | { type: "load_failed"; error: RuntimeError }
  | { type: "snapshot"; snapshot: ProjectsSnapshot }
  | { type: "store_failed"; error: RuntimeError }
  | { type: "collection_failed"; collectionId: string; error: RuntimeError }
  | { type: "open_started"; projectId: string }
  | { type: "open_succeeded"; response: OpenResponse }
  | { type: "open_failed"; projectId: string; error: RuntimeError }
  | { type: "workspace_opened"; id: string }
  | { type: "form_open"; collectionId: string | null }
  | { type: "form_edit"; field: keyof ProjectDraft; value: string }
  | { type: "form_invalid"; missing: (keyof ProjectDraft)[] }
  | { type: "form_failed"; error: RuntimeError }
  | { type: "form_cancel" }
  | { type: "form_submitted"; snapshot: ProjectsSnapshot };

export function emptyDraft(): ProjectDraft {
  return { label: "", endpoint_profile_id: "local", session_name: "", root: "" };
}

export function initialState(): NavigatorState {
  return {
    snapshot: null,
    loading: false,
    globalError: null,
    projectErrors: {},
    collectionErrors: {},
    opening: {},
    lastOutcome: {},
    invalidated: {},
    form: { open: false, collectionId: null, draft: emptyDraft(), missing: [], error: null },
  };
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record };
  delete next[key];
  return next;
}

export function reduce(state: NavigatorState, action: NavigatorAction): NavigatorState {
  switch (action.type) {
    case "load_started":
      return { ...state, loading: true };
    case "loaded":
      return { ...state, loading: false, snapshot: action.snapshot, globalError: null };
    case "load_failed":
      return { ...state, loading: false, globalError: action.error };
    case "snapshot":
      return { ...state, snapshot: action.snapshot, globalError: null, collectionErrors: {} };
    case "store_failed":
      return { ...state, globalError: action.error };
    case "collection_failed":
      return { ...state, collectionErrors: { ...state.collectionErrors, [action.collectionId]: action.error } };
    case "open_started":
      return {
        ...state,
        opening: { ...state.opening, [action.projectId]: true },
        projectErrors: without(state.projectErrors, action.projectId),
      };
    case "open_succeeded": {
      const { result, snapshot } = action.response;
      const invalidated = result.invalidated
        ? { ...state.invalidated, [result.project_id]: result.invalidated }
        : without(state.invalidated, result.project_id);
      return {
        ...state,
        snapshot,
        opening: without(state.opening, result.project_id),
        projectErrors: without(state.projectErrors, result.project_id),
        lastOutcome: { ...state.lastOutcome, [result.project_id]: result.outcome },
        invalidated,
      };
    }
    case "open_failed":
      return {
        ...state,
        opening: without(state.opening, action.projectId),
        projectErrors: { ...state.projectErrors, [action.projectId]: action.error },
      };
    case "workspace_opened":
      return {
        ...state,
        opening: without(state.opening, action.id),
        projectErrors: without(state.projectErrors, action.id),
      };
    case "form_open":
      // Reopening keeps whatever was typed before a cancel.
      return { ...state, form: { ...state.form, open: true, collectionId: action.collectionId, error: null } };
    case "form_edit":
      return {
        ...state,
        form: {
          ...state.form,
          draft: { ...state.form.draft, [action.field]: action.value },
          missing: state.form.missing.filter((f) => f !== action.field || action.value.trim() === ""),
        },
      };
    case "form_invalid":
      return { ...state, form: { ...state.form, missing: action.missing } };
    case "form_failed":
      return { ...state, form: { ...state.form, error: action.error } };
    case "form_cancel":
      return { ...state, form: { ...state.form, open: false, missing: [], error: null } };
    case "form_submitted":
      return {
        ...state,
        snapshot: action.snapshot,
        form: { open: false, collectionId: null, draft: emptyDraft(), missing: [], error: null },
      };
  }
}

/** Presence only; the backend validates session names, endpoint ids and roots. */
export function missingDraftFields(draft: ProjectDraft): (keyof ProjectDraft)[] {
  const required: (keyof ProjectDraft)[] = ["label", "endpoint_profile_id", "session_name", "root"];
  return required.filter((field) => draft[field].trim() === "");
}

/** Target index for a one-step keyboard move, or null when nothing would move. */
export function keyboardMoveTarget(ids: string[], id: string, direction: "up" | "down"): number | null {
  const from = ids.indexOf(id);
  if (from < 0) return null;
  const to = direction === "up" ? from - 1 : from + 1;
  return to < 0 || to >= ids.length ? null : to;
}

/**
 * Target index for dropping `draggedId` before/after `overId`, in the backend convention
 * (remove the dragged item, then insert at the index). Null when nothing would move.
 */
export function dragMoveTarget(
  ids: string[],
  draggedId: string,
  overId: string,
  placement: "before" | "after",
): number | null {
  const from = ids.indexOf(draggedId);
  if (from < 0 || draggedId === overId) return null;
  const rest = ids.filter((id) => id !== draggedId);
  const over = rest.indexOf(overId);
  if (over < 0) return null;
  const to = placement === "before" ? over : over + 1;
  return to === from ? null : to;
}

/** Read at call time (spec 068), so a language change reaches a view model rebuilt afterwards. */
function outcomeText(outcome: OpenOutcome): string {
  return t(`projects.outcome.${outcome}`);
}

export interface ProjectRowView {
  id: string;
  label: string;
  endpointLabel: string;
  session: string;
  root: string;
  status: "open" | "closed" | "opening";
  statusText: string;
  workspaceId: string | null;
  /** Result of the last open in this GUI session (created/rediscovered/reused), if any. */
  outcome: OpenOutcome | null;
  outcomeText: string | null;
  notice: string | null;
  error: RuntimeError | null;
  opening: boolean;
}

export interface CollectionView {
  id: string;
  name: string;
  projects: ProjectRowView[];
  error: RuntimeError | null;
}

export interface NavigatorView {
  /** Loaded store without collections or projects: explicit onboarding, no row actions. */
  empty: boolean;
  canCreateProject: boolean;
  collections: CollectionView[];
  /** Projects that belong to no collection (e.g. after removing their last association). */
  unassigned: ProjectRowView[];
}

/** Endpoint display names the App already loaded (connections catalog), keyed by the stable id. */
export type HostLabels = Readonly<Record<string, string>>;

/** Friendly name of an endpoint; the id stays the identity and is shown only when no name is known. */
export function endpointDisplay(endpoint: string, labels?: HostLabels): string {
  if (endpoint === "local") return "Local";
  const label = labels && Object.hasOwn(labels, endpoint) ? labels[endpoint]?.trim() : "";
  return label ? label : endpoint;
}

function row(state: NavigatorState, project: ProjectDto, labels?: HostLabels): ProjectRowView {
  const opening = state.opening[project.id] === true;
  const invalidated = state.invalidated[project.id];
  const outcome = project.binding ? (state.lastOutcome[project.id] ?? null) : null;
  return {
    id: project.id,
    label: project.label,
    endpointLabel: endpointDisplay(project.endpoint_profile_id, labels),
    session: project.session_name,
    root: project.root,
    status: opening ? "opening" : project.binding ? "open" : "closed",
    statusText: opening
      ? t("projects.status.opening")
      : project.binding
        ? t("projects.status.open", { id: project.binding.workspace_id })
        : t("projects.status.none"),
    workspaceId: project.binding?.workspace_id ?? null,
    outcome,
    outcomeText: outcome ? outcomeText(outcome) : null,
    notice: invalidated ? t("projects.status.invalidated", { id: invalidated.workspace_id }) : null,
    error: state.projectErrors[project.id] ?? null,
    opening,
  };
}

export function viewModel(state: NavigatorState, labels?: HostLabels): NavigatorView {
  const snapshot = state.snapshot;
  if (!snapshot) return { empty: false, canCreateProject: false, collections: [], unassigned: [] };
  const byId = new Map(snapshot.projects.map((p) => [p.id, p]));
  const assigned = new Set<string>();
  const collections = snapshot.collections.map((collection) => ({
    id: collection.id,
    name: collection.name,
    error: state.collectionErrors[collection.id] ?? null,
    projects: collection.project_ids.flatMap((id) => {
      const project = byId.get(id);
      if (!project) return [];
      assigned.add(id);
      return [row(state, project, labels)];
    }),
  }));
  const unassigned = snapshot.projects.filter((p) => !assigned.has(p.id)).map((p) => row(state, p, labels));
  return {
    empty: snapshot.collections.length === 0 && snapshot.projects.length === 0,
    canCreateProject: snapshot.collections.length > 0,
    collections,
    unassigned,
  };
}
