// Spec 002 — navigator state (pure). Fixtures use distinct hosts/boots/labels so a value that
// leaks from one project to another, or a sort by name, changes the observed result.
import { describe, expect, it } from "vitest";
import {
  dragMoveTarget,
  endpointDisplay,
  initialState,
  keyboardMoveTarget,
  missingDraftFields,
  reduce,
  viewModel,
} from "./reducer";
import type { ProjectsSnapshot } from "./types";

const A = "0b6f8c62-1d7e-4c1a-9d53-7a1f2b3c4d5e";
const B = "5a1e2d3c-4b5a-4f69-8e7d-6c5b4a392817";
const C = "9f8e7d6c-5b4a-4392-8170-6f5e4d3c2b1a";
const PRODUTO = "c0ffee00-0000-4000-8000-000000000001";
const PESSOAL = "c0ffee00-0000-4000-8000-000000000002";

function snapshot(): ProjectsSnapshot {
  return {
    version: 1,
    projects: [
      {
        id: A,
        label: "Zeta API",
        endpoint_profile_id: "local",
        session_name: "hd-proj-a",
        root: "/srv/zeta",
        binding: { project_id: A, connection_generation: 3, boot_id: "boot-alpha", workspace_id: "w7" },
      },
      { id: B, label: "Alfa Web", endpoint_profile_id: "local", session_name: "hd-proj-b", root: "/srv/alfa", binding: null },
      { id: C, label: "Build", endpoint_profile_id: "ssh-build", session_name: "ci", root: "/home/ci/build", binding: null },
    ],
    collections: [
      { id: PRODUTO, name: "Produto", project_ids: [A, B] },
      { id: PESSOAL, name: "Pessoal", project_ids: [A] },
    ],
  };
}

const loaded = () => reduce(initialState(), { type: "loaded", snapshot: snapshot() });

describe("viewModel", () => {
  // Would catch: collections or projects sorted by name instead of the persisted order, a
  // project shown once when it belongs to two collections, or endpoint hidden for remote rows.
  it("keeps persisted order, repeats shared projects and labels the endpoint per project", () => {
    const vm = viewModel(loaded());
    expect(vm.empty).toBe(false);
    expect(vm.collections.map((c) => [c.name, c.projects.map((p) => p.label)])).toEqual([
      ["Produto", ["Zeta API", "Alfa Web"]],
      ["Pessoal", ["Zeta API"]],
    ]);
    expect(vm.unassigned.map((p) => [p.label, p.endpointLabel])).toEqual([["Build", "ssh-build"]]);
    const zeta = vm.collections[0]!.projects[0]!;
    expect(zeta.endpointLabel).toBe("Local");
    expect(zeta.status).toBe("open");
    expect(zeta.statusText).toBe("Workspace w7 aberto");
    expect([zeta.workspaceId, zeta.outcome]).toEqual(["w7", null]);
    expect(vm.collections[0]!.projects[1]!.status).toBe("closed");
    expect(vm.collections[0]!.projects[1]!.workspaceId).toBeNull();
  });

  // Would catch: an empty store rendered as a blank list with active actions (instead of an
  // explicit onboarding), or a loading state treated as empty.
  it("shows explicit onboarding only for an empty loaded store", () => {
    expect(viewModel(initialState()).empty).toBe(false);
    const empty = reduce(initialState(), { type: "loaded", snapshot: { version: 1, projects: [], collections: [] } });
    const vm = viewModel(empty);
    expect(vm.empty).toBe(true);
    expect(vm.collections).toEqual([]);
    expect(vm.canCreateProject).toBe(false);
  });
});

describe("per-resource errors", () => {
  // Would catch: an open failure displayed globally or on every row, or a later success of
  // another project clearing the failed project's error.
  it("keeps an open error on the affected project only", () => {
    let state = loaded();
    state = reduce(state, { type: "open_started", projectId: B });
    state = reduce(state, {
      type: "open_failed",
      projectId: B,
      error: { code: "server_unavailable", message: "API do Herdr indisponível", retryable: true, endpoint: "local" },
    });
    state = reduce(state, {
      type: "open_succeeded",
      response: {
        result: {
          project_id: A,
          binding: { project_id: A, connection_generation: 4, boot_id: "boot-alpha", workspace_id: "w7" },
          outcome: "reused",
          invalidated: null,
        },
        snapshot: snapshot(),
      },
    });
    const rows = viewModel(state).collections[0]!.projects;
    expect(rows.map((r) => [r.label, r.error?.code ?? null, r.opening])).toEqual([
      ["Zeta API", null, false],
      ["Alfa Web", "server_unavailable", false],
    ]);
    expect(state.globalError).toBeNull();
  });

  // Would catch: an invalidated binding (boot/generation changed) silently shown as the same
  // workspace instead of being reported.
  it("reports an invalidated binding on the project after reopening", () => {
    const fresh = snapshot();
    fresh.projects[0]!.binding = { project_id: A, connection_generation: 1, boot_id: "boot-beta", workspace_id: "w2" };
    const state = reduce(loaded(), {
      type: "open_succeeded",
      response: {
        result: {
          project_id: A,
          binding: fresh.projects[0]!.binding!,
          outcome: "created",
          invalidated: { project_id: A, connection_generation: 3, boot_id: "boot-alpha", workspace_id: "w7" },
        },
        snapshot: fresh,
      },
    });
    const zeta = viewModel(state).collections[0]!.projects[0]!;
    expect(zeta.statusText).toBe("Workspace w2 aberto");
    expect([zeta.workspaceId, zeta.outcome, zeta.outcomeText]).toEqual(["w2", "created", "criado agora"]);
    expect(zeta.notice).toBe("Vínculo anterior (w7) descartado: servidor ou conexão mudou");
  });
});

describe("project form", () => {
  // Would catch: cancel discarding what the user typed (buffer lost) or submit keeping it.
  it("preserves the draft buffer on cancel and clears it after a successful submit", () => {
    let state = reduce(loaded(), { type: "form_open", collectionId: PESSOAL });
    state = reduce(state, { type: "form_edit", field: "label", value: "Novo" });
    state = reduce(state, { type: "form_edit", field: "root", value: "/srv/novo" });
    state = reduce(state, { type: "form_cancel" });
    expect(state.form.open).toBe(false);
    state = reduce(state, { type: "form_open", collectionId: PRODUTO });
    expect(state.form.draft.label).toBe("Novo");
    expect(state.form.draft.root).toBe("/srv/novo");
    expect(state.form.collectionId).toBe(PRODUTO);
    state = reduce(state, { type: "form_submitted", snapshot: snapshot() });
    expect(state.form.open).toBe(false);
    expect(state.form.draft).toEqual({ label: "", endpoint_profile_id: "local", session_name: "", root: "" });
  });

  // Only presence is checked in the WebView; session/root rules belong to the backend.
  it("lists missing required fields", () => {
    expect(missingDraftFields({ label: " ", endpoint_profile_id: "local", session_name: "", root: "/x" })).toEqual([
      "label",
      "session_name",
    ]);
    expect(missingDraftFields({ label: "A", endpoint_profile_id: "local", session_name: "default", root: "x" })).toEqual([]);
  });
});

describe("reordering", () => {
  // Would catch: moving past the edges (index -1 / length), or emitting a no-op move.
  it("computes one-step keyboard targets and refuses edges", () => {
    const ids = [A, B, C];
    expect(keyboardMoveTarget(ids, B, "up")).toBe(0);
    expect(keyboardMoveTarget(ids, B, "down")).toBe(2);
    expect(keyboardMoveTarget(ids, A, "up")).toBeNull();
    expect(keyboardMoveTarget(ids, C, "down")).toBeNull();
    expect(keyboardMoveTarget(ids, "missing", "down")).toBeNull();
  });

  // The backend removes then inserts at the target index; drag targets must use the same
  // convention. Would catch: an off-by-one when dragging downwards.
  it("computes drag targets in remove-then-insert indices", () => {
    const ids = [A, B, C];
    expect(dragMoveTarget(ids, A, C, "after")).toBe(2);
    expect(dragMoveTarget(ids, A, C, "before")).toBe(1);
    expect(dragMoveTarget(ids, C, A, "before")).toBe(0);
    expect(dragMoveTarget(ids, C, B, "after")).toBeNull();
    expect(dragMoveTarget(ids, B, B, "before")).toBeNull();
  });
});

describe("friendly endpoint labels (spec 007)", () => {
  // Would catch: the navigator badge showing the raw SSH profile id when the App already loaded a
  // display name for it, a label leaking to another endpoint, or the id stopping being the
  // identity (row id / fallback when no name is known).
  it("uses the catalog display name per endpoint and falls back to the id", () => {
    const labels = { "ssh-build": "Servidor de Build", "ssh-other": "Outro Host" };
    const vm = viewModel(loaded(), labels);
    expect(vm.unassigned.map((p) => [p.id, p.endpointLabel])).toEqual([[C, "Servidor de Build"]]);
    expect(vm.collections[0]!.projects[0]!.endpointLabel).toBe("Local");
    expect(viewModel(loaded()).unassigned[0]!.endpointLabel).toBe("ssh-build");
    expect(endpointDisplay("ssh-build", labels)).toBe("Servidor de Build");
    expect(endpointDisplay("ssh-unknown", labels)).toBe("ssh-unknown");
    expect(endpointDisplay("local", { local: "Nome indevido" })).toBe("Local");
    expect(endpointDisplay("ssh-build")).toBe("ssh-build");
  });
});
