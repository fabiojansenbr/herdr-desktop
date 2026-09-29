// Spec 045 (AC-045-01) — dragging a workspace by its handle.
//
// The gesture is decided here, away from the DOM: `planDrop` turns a source row and the spot it
// was dropped on into the store commands that must run, in order, and nothing else. Every step
// is a command the catalog already had (025/002): `collection_move_project` to reorder inside a
// collection, `collection_remove_project` to leave one and `group_assign` to enter one — the
// latter upserting the ProjectRef by root, so a live workspace that never had one gets it before
// the move. No engine command is ever part of a plan: dragging organizes the desktop catalog and
// never opens, closes or moves a workspace between hosts.
//
// The drag itself is process-wide state (the source row is grabbed on one component and dropped
// on another), so it lives in this module behind `beginDrag`/`activeDrag`/`endDrag`. Esc cancels
// it: the window listener is installed with the drag and removed with it.
import { t } from "../../i18n/index.svelte";
import type { ProjectDto } from "../../projects/types";
import { UNGROUPED_ID } from "./sidebar-model";

/** Payload type of the drag, for the browsers that carry one. */
export const DRAG_MIME = "application/x-herdr-workspace";

/** Refusal shown when the gesture would take a workspace to another host (out of scope of 045). */
export function crossHostHint(): string {
  return t("sidebar.drag.crossHost");
}

/** What is being dragged: one sidebar row, with everything a plan needs. */
export interface DragRow {
  readonly rowId: string;
  /** `null` while the live workspace has no ProjectRef yet (it is created by `assign`). */
  readonly projectId: string | null;
  readonly endpoint: string;
  readonly session: string;
  readonly cwd: string | null;
  readonly label: string;
  /** The collection the row is shown in, or [`UNGROUPED_ID`]. */
  readonly collectionId: string;
}

/** Where the pointer released: over another row, or over a collection header. */
export type DropSpot =
  | {
      readonly kind: "row";
      readonly collectionId: string;
      readonly projectId: string | null;
      readonly endpoint: string;
      readonly placement: "before" | "after";
    }
  | { readonly kind: "collection"; readonly collectionId: string };

export type DragStep =
  | { readonly kind: "assign"; readonly groupId: string }
  | { readonly kind: "remove"; readonly collectionId: string }
  | {
      readonly kind: "move";
      readonly collectionId: string;
      readonly overProjectId: string;
      readonly placement: "before" | "after";
    };

export interface DragPlan {
  /** Store commands to run in order; empty when the gesture changes nothing. */
  readonly steps: readonly DragStep[];
  /** Hint to show instead of an insertion point, or `null` when the drop is allowed. */
  readonly refused: string | null;
}

const NOTHING: DragPlan = { steps: [], refused: null };

/**
 * The plan of one drop (AC-045-01). Reordering inside a collection is a single move; entering
 * another collection means leaving the current one first, because the 025 tree shows a project
 * under the first collection that lists it — assigning alone would leave the row where it was.
 */
export function planDrop(source: DragRow, spot: DropSpot): DragPlan {
  // A workspace the engine reported without a cwd cannot be keyed by root, so it never moves.
  if (!source.cwd) return NOTHING;

  if (spot.kind === "row") {
    // Out of scope of 045: a workspace belongs to its host and is never recreated elsewhere.
    if (spot.endpoint !== source.endpoint) return { steps: [], refused: crossHostHint() };
    if (spot.projectId === null || spot.projectId === source.projectId) return NOTHING;
    if (spot.collectionId === source.collectionId) {
      if (source.projectId === null || spot.collectionId === UNGROUPED_ID) return NOTHING;
      return {
        steps: [
          { kind: "move", collectionId: spot.collectionId, overProjectId: spot.projectId, placement: spot.placement },
        ],
        refused: null,
      };
    }
    const enter = planDrop(source, { kind: "collection", collectionId: spot.collectionId });
    if (enter.steps.length === 0 || spot.collectionId === UNGROUPED_ID) return enter;
    return {
      steps: [
        ...enter.steps,
        { kind: "move", collectionId: spot.collectionId, overProjectId: spot.projectId, placement: spot.placement },
      ],
      refused: null,
    };
  }

  if (spot.collectionId === source.collectionId) return NOTHING;

  const leave: DragStep[] =
    source.collectionId === UNGROUPED_ID || source.projectId === null
      ? []
      : [{ kind: "remove", collectionId: source.collectionId }];

  // "Sem coleção" is not a stored collection: landing there is only leaving the current one.
  if (spot.collectionId === UNGROUPED_ID) return { steps: leave, refused: null };
  return { steps: [...leave, { kind: "assign", groupId: spot.collectionId }], refused: null };
}

/**
 * The ProjectRef of the dragged row in `projects`, by the key `group_assign` upserts on
 * (endpoint, session, root — the 025 key). A row that had none before an `assign` step has one
 * after it, and the plan's `move` needs that id: without this the row would land at the end of
 * the collection instead of where the insertion point promised.
 */
export function projectIdOf(projects: readonly ProjectDto[], source: DragRow): string | null {
  if (!source.cwd) return null;
  const root = normalizeRoot(source.cwd);
  const found = projects.find(
    (project) =>
      project.endpoint_profile_id === source.endpoint &&
      project.session_name === source.session &&
      normalizeRoot(project.root) === root,
  );
  return found?.id ?? null;
}

/** Same normalization the store and the 025 tree apply before comparing roots. */
function normalizeRoot(root: string): string {
  return root.trim().replace(/[\\/]+$/, "") || root.trim();
}

/** Which edge of `rect` the pointer at `clientY` is closest to (the insertion point). */
export function placementOf(rect: { top: number; height: number }, clientY: number): "before" | "after" {
  return clientY < rect.top + rect.height / 2 ? "before" : "after";
}

// ---------------------------------------------------------------------------------------
// Drag in flight
// ---------------------------------------------------------------------------------------

let active: DragRow | null = null;
let onEscape: ((event: KeyboardEvent) => void) | null = null;
const listeners = new Set<() => void>();

function announce() {
  for (const listener of [...listeners]) listener();
}

/** Called by the components that draw an insertion point, so Esc clears theirs too. */
export function onDragChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function activeDrag(): DragRow | null {
  return active;
}

export function beginDrag(row: DragRow): void {
  active = row;
  if (typeof window === "undefined") return;
  onEscape = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    endDrag();
  };
  window.addEventListener("keydown", onEscape);
}

/** Ends the drag (dropped, cancelled by Esc or `dragend`); the next drop plans nothing. */
export function endDrag(): void {
  active = null;
  if (onEscape && typeof window !== "undefined") window.removeEventListener("keydown", onEscape);
  onEscape = null;
  announce();
}
