// Spec 046 — "Novo workspace": the host choice, the recents of that host and the ordered chain
// of existing commands a creation runs (P4). Nothing here is new engine surface: the chain is
// the 037 host switch, `workspace_create`, `agent_start` on the pane of the workspace the host
// confirmed and `group_assign`; the recents are a client-only list of the store (v4).
//
// The dialog lives in `src/components/sidebar/NewWorkspaceDialog.svelte`; this module keeps the
// decisions testable without a window, and carries the request other regions raise (the palette
// command and the title bar's "Abrir projeto…" on an SSH host, AC-046-04).
import type { HostDto } from "../connections/types";
import { t } from "../i18n/index.svelte";
import { projectBasename } from "./controller";
import type { GroupAssignInput, RecentFolderDto, RuntimeError, WorkspaceCreateInput, WorkspaceCreatedDto } from "./types";

/** "Iniciar com" entry that starts no agent: the workspace opens with its shell (P4). */
export const SHELL_START = "shell";

export interface HostChoice {
  endpoint: string;
  name: string;
  kind: "local" | "ssh";
  online: boolean;
  /** Why this host cannot receive a workspace; `null` when it can. */
  reason: "offline" | null;
}

/**
 * One choice per visible host, Local first as in the footer. A host that is not online is listed
 * and disabled with its motive — never hidden, and never replaced by Local.
 */
export function hostChoices(hosts: readonly HostDto[]): HostChoice[] {
  return hosts
    .filter((host) => host.visible !== false)
    .map((host) => ({
      endpoint: host.endpoint,
      name: host.kind === "local" ? t("sidebar.host.thisComputer") : host.label || host.endpoint,
      kind: host.kind,
      online: host.phase === "online",
      reason: host.phase === "online" ? null : ("offline" as const),
    }));
}

/** Folders already opened on `endpoint`, newest first (the store keeps at most 8 per host). */
export function recentFoldersFor(recents: readonly RecentFolderDto[] | undefined, endpoint: string): string[] {
  return (recents ?? []).filter((recent) => recent.endpoint_profile_id === endpoint).map((recent) => recent.path);
}

/** Label of the new workspace: the last segment of the folder, as the 016 open does. */
export function workspaceLabel(folder: string): string {
  return projectBasename(folder.trim());
}

function asRuntimeError(error: unknown): RuntimeError {
  if (error && typeof error === "object" && "code" in error && "message" in error) return error as RuntimeError;
  return { code: "ipc_error", message: t("projects.error.ipc"), retryable: true };
}

export interface NewWorkspaceDeps {
  /** Host of the window right now; the dialog may be marking another one. */
  selectedEndpoint(): string | null;
  /** The 037 switch (`controllers.surface.select`): no reconnection, no fallback to Local. */
  selectHost(endpoint: string): Promise<unknown>;
  createWorkspace(input: WorkspaceCreateInput): Promise<WorkspaceCreatedDto>;
  /**
   * Pane of the workspace the host published, once its metadata arrived; `null` when the host
   * never confirmed it, and then no agent is started anywhere.
   */
  confirmPane(endpoint: string, workspaceId: string): Promise<string | null>;
  startAgent(paneId: string, kind: string): Promise<void>;
  assignToCollection(collectionId: string, entry: GroupAssignInput): Promise<void>;
  recordRecent(endpoint: string, path: string): Promise<void>;
  sessionOf(endpoint: string): string;
}

export interface NewWorkspaceInput {
  endpoint: string;
  folder: string;
  collectionId: string | null;
  /** `SHELL_START` or one of the engine's agent kinds. */
  start: string;
}

/**
 * Runs the creation in the order of AC-046-03 and answers `null` on success or the error of the
 * step that failed. Every step targets the chosen host: a refusal stops the chain there and
 * nothing is retried on another host. The folder is recorded as a recent of that host as soon as
 * the workspace exists, so the next dialog offers it even if a later step failed.
 */
export async function createWorkspaceFlow(deps: NewWorkspaceDeps, input: NewWorkspaceInput): Promise<RuntimeError | null> {
  const cwd = input.folder.trim();
  if (!cwd) return { code: "invalid_project_root", message: t("projects.error.folderRequired"), retryable: false };
  const label = workspaceLabel(cwd);
  let created: WorkspaceCreatedDto | null = null;
  try {
    if (deps.selectedEndpoint() !== input.endpoint) await deps.selectHost(input.endpoint);
    created = await deps.createWorkspace({ endpoint_profile_id: input.endpoint, cwd, label, focus: true });
    if (input.start !== SHELL_START) {
      const paneId = await deps.confirmPane(input.endpoint, created.workspace_id);
      if (!paneId) {
        throw { code: "workspace_not_confirmed", message: t("projects.error.notConfirmed"), retryable: true } satisfies RuntimeError;
      }
      await deps.startAgent(paneId, input.start);
    }
    if (input.collectionId) {
      await deps.assignToCollection(input.collectionId, {
        endpoint_profile_id: input.endpoint,
        session_name: deps.sessionOf(input.endpoint),
        cwd,
        label,
      });
    }
    return null;
  } catch (error) {
    return asRuntimeError(error);
  } finally {
    // The folder was used on this host; a failure to remember it never fails the creation.
    if (created) await deps.recordRecent(input.endpoint, cwd).catch(() => {});
  }
}

export interface PollOptions {
  tries?: number;
  delayMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

/** Reads `value` until it answers something, at most `tries` times. Sends nothing itself. */
export async function pollFor<T>(value: () => T | null, options: PollOptions = {}): Promise<T | null> {
  const tries = options.tries ?? 20;
  const delayMs = options.delayMs ?? 60;
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  for (let attempt = 0; attempt < tries; attempt += 1) {
    const current = value();
    if (current !== null && current !== undefined) return current;
    await sleep(delayMs);
  }
  return null;
}

type RequestListener = (endpoint: string | null) => void;

const listeners = new Set<RequestListener>();

/**
 * Asks the sidebar to open the dialog, optionally already marking `endpoint`. Used by the
 * palette command and by "Abrir projeto…" when the selected host is not this computer.
 */
export function requestNewWorkspace(endpoint: string | null = null): void {
  for (const listener of [...listeners]) listener(endpoint);
}

export function onNewWorkspaceRequest(listener: RequestListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
