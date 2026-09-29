// In-memory ProjectsBridge for the isolated preview and the controller tests. Records every
// command with the exact argument object the Tauri bridge would send. It simulates one local
// engine (workspaces tagged per project); it is not the store implementation.

import { GROUP_PALETTE } from "../components/projects/tree-model";
import type { ProjectsBridge } from "./bridge";
import type {
  CollectionDto,
  OpenResponse,
  ProjectDraft,
  ProjectDto,
  ProjectsSnapshot,
  RecentFolderDto,
  RuntimeError,
  WorkspaceCreateInput,
  WorkspaceCreatedDto,
  WorkspacePrefDto,
} from "./types";

/** Same normalization the store and the 025 tree apply before comparing roots. */
function normalizeRoot(root: string): string {
  return root.trim().replace(/[\\/]+$/, "") || root.trim();
}

export interface RecordedCall {
  command: string;
  args: Record<string, unknown>;
}

export interface FakeProjectsBridge extends ProjectsBridge {
  calls: RecordedCall[];
  /** Workspace ids alive in the simulated engine. */
  workspaces(): string[];
  failOpen(projectId: string, error: RuntimeError): void;
  /**
   * Makes the named command (the wire name, as recorded) refuse with `error` until it is cleared
   * with `null`. The call is still recorded: it left the WebView and came back refused, which is
   * what a caller that must undo an optimistic state has to survive.
   */
  failCommand(command: string, error: RuntimeError | null): void;
  reboot(bootId: string): void;
}

export function createFakeProjectsBridge(options: { bootId: string; seed?: ProjectsSnapshot }): FakeProjectsBridge {
  let bootId = options.bootId;
  let generation = 1;
  let nextId = 1;
  let nextWorkspace = 1;
  const projects: ProjectDto[] = structuredClone(options.seed?.projects ?? []);
  const collections: CollectionDto[] = structuredClone(options.seed?.collections ?? []);
  const prefs: WorkspacePrefDto[] = structuredClone(options.seed?.workspace_prefs ?? []);
  const recents: RecentFolderDto[] = structuredClone(options.seed?.recent_folders ?? []);
  let engine: { id: string; project: string }[] = [];
  const failures = new Map<string, RuntimeError>();
  const refusals = new Map<string, RuntimeError>();
  const calls: RecordedCall[] = [];

  const id = () => `00000000-0000-4000-8000-${String(nextId++).padStart(12, "0")}`;
  const snapshot = (): ProjectsSnapshot =>
    structuredClone({ version: 4, projects, collections, workspace_prefs: prefs, recent_folders: recents });
  const fail = (code: string, message: string): never => {
    throw { code, message, retryable: false } satisfies RuntimeError;
  };
  const collection = (collectionId: string) =>
    collections.find((c) => c.id === collectionId) ?? fail("collection_not_found", "coleção não encontrada");
  const record = (command: string, args: Record<string, unknown> = {}) => {
    calls.push({ command, args });
    const refusal = refusals.get(command);
    if (refusal) throw refusal;
  };

  return {
    calls,
    workspaces: () => engine.map((w) => w.id),
    failOpen: (projectId, error) => failures.set(projectId, error),
    failCommand: (command, error) => {
      if (error) refusals.set(command, error);
      else refusals.delete(command);
    },
    reboot(next) {
      bootId = next;
      generation += 1;
      engine = [];
    },
    async list() {
      record("projects_list");
      return snapshot();
    },
    async createProject(draft: ProjectDraft, collectionId) {
      record("project_create", { draft, collectionId });
      const target = collectionId ? collection(collectionId) : null;
      const project: ProjectDto = { id: id(), ...structuredClone(draft), label: draft.label.trim(), binding: null };
      projects.push(project);
      target?.project_ids.push(project.id);
      return snapshot();
    },
    async createCollection(name) {
      record("collection_create", { name });
      if (!name.trim()) fail("invalid_collection_name", "nome de coleção vazio");
      collections.push({ id: id(), name: name.trim(), project_ids: [] });
      return snapshot();
    },
    async addToCollection(collectionId, projectId, index) {
      record("collection_add_project", { collectionId, projectId, index });
      const target = collection(collectionId);
      if (target.project_ids.includes(projectId)) fail("association_exists", "o projeto já está nesta coleção");
      const at = Math.min(index ?? target.project_ids.length, target.project_ids.length);
      target.project_ids.splice(at, 0, projectId);
      return snapshot();
    },
    async removeFromCollection(collectionId, projectId) {
      record("collection_remove_project", { collectionId, projectId });
      const target = collection(collectionId);
      const at = target.project_ids.indexOf(projectId);
      if (at < 0) fail("association_not_found", "o projeto não pertence a esta coleção");
      target.project_ids.splice(at, 1);
      return snapshot();
    },
    async moveProject(collectionId, projectId, toIndex) {
      record("collection_move_project", { collectionId, projectId, toIndex });
      const target = collection(collectionId);
      const from = target.project_ids.indexOf(projectId);
      if (from < 0) fail("association_not_found", "o projeto não pertence a esta coleção");
      target.project_ids.splice(from, 1);
      target.project_ids.splice(Math.min(toIndex, target.project_ids.length), 0, projectId);
      return snapshot();
    },
    async moveCollection(collectionId, toIndex) {
      record("collection_move", { collectionId, toIndex });
      const from = collections.findIndex((c) => c.id === collectionId);
      if (from < 0) fail("collection_not_found", "coleção não encontrada");
      const [moved] = collections.splice(from, 1);
      collections.splice(Math.min(toIndex, collections.length), 0, moved!);
      return snapshot();
    },
    async open(projectId): Promise<OpenResponse> {
      record("project_open", { projectId });
      const failure = failures.get(projectId);
      if (failure) throw failure;
      const project = projects.find((p) => p.id === projectId) ?? fail("project_not_found", "projeto não encontrado");
      const previous = project.binding;
      const valid = previous !== null && previous.boot_id === bootId && previous.connection_generation === generation;
      let outcome: OpenResponse["result"]["outcome"];
      let workspaceId: string;
      if (previous !== null && valid && engine.some((w) => w.id === previous.workspace_id)) {
        outcome = "reused";
        workspaceId = previous.workspace_id;
      } else {
        const tagged = engine.find((w) => w.project === projectId);
        if (tagged) {
          outcome = "rediscovered";
          workspaceId = tagged.id;
        } else {
          outcome = "created";
          workspaceId = `w${nextWorkspace++}`;
          engine.push({ id: workspaceId, project: projectId });
        }
      }
      project.binding = { project_id: projectId, connection_generation: generation, boot_id: bootId, workspace_id: workspaceId };
      const invalidated = previous && outcome !== "reused" ? previous : null;
      return { result: { project_id: projectId, binding: project.binding, outcome, invalidated }, snapshot: snapshot() };
    },
    async groupCreate(name) {
      record("group_create", { name });
      if (!name.trim()) fail("invalid_collection_name", "nome de grupo vazio");
      collections.push({ id: id(), name: name.trim(), project_ids: [] });
      return snapshot();
    },
    async groupAssign(groupId, entry) {
      record("group_assign", { groupId, ...entry });
      const target = collection(groupId);
      let project = projects.find(
        (p) =>
          p.endpoint_profile_id === entry.endpoint_profile_id &&
          p.session_name === entry.session_name &&
          p.root === entry.cwd,
      );
      if (!project) {
        project = {
          id: id(),
          label: entry.label.trim() || entry.cwd.split(/[\\/]/).filter(Boolean).at(-1) || entry.cwd,
          endpoint_profile_id: entry.endpoint_profile_id,
          session_name: entry.session_name,
          root: entry.cwd,
          binding: null,
        };
        projects.push(project);
      }
      if (!target.project_ids.includes(project.id)) target.project_ids.push(project.id);
      return snapshot();
    },
    async groupRename(groupId, name) {
      record("group_rename", { groupId, name });
      const target = collection(groupId);
      const trimmed = name.trim();
      if (!trimmed || [...trimmed].length > 80) fail("invalid_collection_name", "nome de grupo inválido");
      target.name = trimmed;
      return snapshot();
    },
    async groupSetColor(groupId, color) {
      record("group_set_color", { groupId, color });
      const target = collection(groupId);
      if (!GROUP_PALETTE.some((known) => known.toLowerCase() === color.toLowerCase())) {
        fail("invalid_color", "esta cor não pertence à paleta do desktop");
      }
      target.color = color;
      return snapshot();
    },
    async groupSetCollapsed(groupId, collapsed) {
      record("group_set_collapsed", { groupId, collapsed });
      const target = collection(groupId);
      // The store omits `false`, like `skip_serializing_if` does on the Rust side.
      if (collapsed) target.collapsed = true;
      else delete target.collapsed;
      return snapshot();
    },
    async groupDelete(groupId) {
      record("group_delete", { groupId });
      const at = collections.findIndex((c) => c.id === groupId);
      if (at < 0) fail("collection_not_found", "coleção não encontrada");
      // Only the collection goes: its projects stay in the catalog, now without a collection.
      collections.splice(at, 1);
      return snapshot();
    },
    async workspacePrefSet(input) {
      // The arguments the Tauri bridge sends, camelCase included (spec 044).
      record("workspace_pref_set", {
        endpointProfileId: input.endpoint_profile_id,
        root: input.root,
        color: input.color,
        pinned: input.pinned,
        hidden: input.hidden,
      });
      const endpoint = input.endpoint_profile_id.trim();
      if (!endpoint) fail("unknown_endpoint", "o host desta preferência não está configurado");
      const root = normalizeRoot(input.root);
      if (!root || root.length > 4096) fail("invalid_root", "raiz de workspace inválida");
      if (input.color !== undefined && !GROUP_PALETTE.some((known) => known.toLowerCase() === input.color!.toLowerCase())) {
        fail("invalid_color", "esta cor não pertence à paleta do desktop");
      }
      const at = prefs.findIndex((pref) => pref.endpoint_profile_id === endpoint && normalizeRoot(pref.root) === root);
      const entry: WorkspacePrefDto =
        at >= 0 ? { ...prefs[at]! } : { endpoint_profile_id: endpoint, root, color: null, pinned: false, hidden: false };
      if (input.color !== undefined) entry.color = input.color;
      if (input.pinned !== undefined) entry.pinned = input.pinned;
      if (input.hidden !== undefined) entry.hidden = input.hidden;
      const meaningful = entry.color !== null || entry.pinned || entry.hidden;
      if (at >= 0) {
        if (meaningful) prefs[at] = entry;
        else prefs.splice(at, 1);
      } else if (meaningful) {
        prefs.push(entry);
      }
      return snapshot();
    },
    async recentFolderAdd(endpointProfileId, path) {
      record("recent_folder_add", { endpointProfileId, path });
      const endpoint = endpointProfileId.trim();
      if (!endpoint) fail("unknown_endpoint", "o host desta pasta não está configurado");
      const normalized = normalizeRoot(path);
      if (!normalized || normalized.length > 4096) fail("invalid_root", "raiz de workspace inválida");
      // Same rule as the store (spec 046): top of the list, no duplicate, 8 per host.
      const at = recents.findIndex((recent) => recent.endpoint_profile_id === endpoint && normalizeRoot(recent.path) === normalized);
      if (at >= 0) recents.splice(at, 1);
      recents.unshift({ endpoint_profile_id: endpoint, path: normalized });
      let seen = 0;
      for (let i = 0; i < recents.length; i += 1) {
        if (recents[i]!.endpoint_profile_id !== endpoint) continue;
        seen += 1;
        if (seen > 8) recents.splice(i--, 1);
      }
      return snapshot();
    },
    async workspaceFocus(endpoint, workspaceId) {
      record("workspace_focus", { endpoint, workspaceId });
    },
    async workspaceCreate(input: WorkspaceCreateInput): Promise<WorkspaceCreatedDto> {
      record("workspace_create", {
        endpoint: input.endpoint_profile_id,
        cwd: input.cwd,
        label: input.label,
        focus: input.focus,
      });
      if (!input.cwd.trim()) fail("invalid_project_root", "a raiz do workspace é obrigatória");
      const workspaceId = `w${nextWorkspace++}`;
      engine.push({ id: workspaceId, project: "" });
      return { workspace_id: workspaceId };
    },
    async workspaceRename(endpoint, workspaceId, label) {
      record("workspace_rename", { endpoint, workspaceId, label });
      if (!engine.some((w) => w.id === workspaceId)) fail("workspace_not_found", "workspace não encontrado");
    },
    async workspaceClose(endpoint, workspaceId) {
      record("workspace_close", { endpoint, workspaceId });
      if (!engine.some((w) => w.id === workspaceId)) fail("workspace_not_found", "workspace não encontrado");
      engine = engine.filter((w) => w.id !== workspaceId);
    },
  };
}
