// Spec 002 — controller over a fake bridge that records every command it receives.
import { describe, expect, it } from "vitest";
import { createProjectsController } from "./controller";
import { createFakeProjectsBridge } from "./fake-bridge";
import { hostFixture } from "../connections/fake-bridge";
import type { HostDto } from "../connections/types";
import type { ProjectDto } from "./types";

async function seeded() {
  const bridge = createFakeProjectsBridge({ bootId: "boot-alpha" });
  const controller = createProjectsController(bridge);
  await controller.load();
  await controller.createCollection("Produto");
  await controller.createCollection("Pessoal");
  const [produto, pessoal] = controller.state.snapshot!.collections.map((c) => c.id);
  controller.openForm(produto!);
  controller.editForm("label", "Zeta API");
  controller.editForm("session_name", "hd-proj-a");
  controller.editForm("root", "/srv/zeta");
  await controller.submitProject();
  controller.openForm(produto!);
  controller.editForm("label", "Alfa Web");
  controller.editForm("session_name", "hd-proj-a");
  controller.editForm("root", "/srv/alfa");
  await controller.submitProject();
  const [a, b] = controller.state.snapshot!.projects.map((p) => p.id);
  await controller.addToCollection(pessoal!, a!);
  bridge.calls.length = 0;
  return { bridge, controller, produto: produto!, pessoal: pessoal!, a: a!, b: b! };
}

describe("projects controller", () => {
  it("opens a picked folder in Local default and reuses Meus projetos on repeated opens", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-016" });
    const controller = createProjectsController(bridge);
    const picker = async () => "/srv/my-project";
    await controller.openFolder(picker, "default");
    const snapshot = controller.state.snapshot!;
    expect(snapshot.collections).toHaveLength(1);
    expect(snapshot.collections[0]!.name).toBe("Meus projetos");
    expect(snapshot.projects[0]).toMatchObject({
      label: "my-project", root: "/srv/my-project", endpoint_profile_id: "local", session_name: "default",
    });
    expect(snapshot.collections[0]!.project_ids).toEqual([snapshot.projects[0]!.id]);
    expect(bridge.calls.map((c) => c.command)).toEqual([
      "projects_list", "group_create", "workspace_create", "group_assign",
    ]);
    expect(bridge.calls[2]).toEqual({
      command: "workspace_create",
      args: { endpoint: "local", cwd: "/srv/my-project", label: "my-project", focus: true },
    });
    await controller.openFolder(picker, "default");
    expect(controller.state.snapshot!.projects).toHaveLength(1);
    expect(controller.state.snapshot!.collections).toHaveLength(1);
    expect(bridge.workspaces()).toHaveLength(2);
  });

  it("cancelling the folder picker writes and opens nothing", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-016" });
    const controller = createProjectsController(bridge);
    await controller.openFolder(async () => null, "default");
    expect(bridge.calls).toEqual([]);
  });
  // Would catch: loading (render) that opens projects or starts processes.
  it("loads with a single list command and starts nothing", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-alpha" });
    const controller = createProjectsController(bridge);
    await controller.load();
    await controller.load();
    expect(bridge.calls.map((c) => c.command)).toEqual(["projects_list", "projects_list"]);
    expect(bridge.workspaces()).toEqual([]);
  });

  // AC-002-02 at the WebView seam. Would catch: "remove" wired to a runtime command, the
  // project removed from every collection, or its binding dropped.
  it("removes one association with exactly one store command and keeps the workspace", async () => {
    const { bridge, controller, produto, pessoal, a, b } = await seeded();
    await controller.open(a);
    const workspace = controller.state.snapshot!.projects.find((p) => p.id === a)!.binding!.workspace_id;
    bridge.calls.length = 0;

    await controller.removeFromCollection(produto, a);

    expect(bridge.calls).toEqual([{ command: "collection_remove_project", args: { collectionId: produto, projectId: a } }]);
    const snap = controller.state.snapshot!;
    expect(snap.collections.map((c) => [c.id, c.project_ids])).toEqual([
      [produto, [b]],
      [pessoal, [a]],
    ]);
    expect(snap.projects.find((p) => p.id === a)!.binding!.workspace_id).toBe(workspace);
    expect(bridge.workspaces()).toEqual([workspace]);
  });

  // AC-002-03 at the WebView seam. Would catch: double click creating two workspaces, or a
  // reopen that sends a second create.
  it("ignores a second open while one is in flight and reuses on reopen", async () => {
    const { bridge, controller, a } = await seeded();
    const first = controller.open(a);
    const second = controller.open(a);
    await Promise.all([first, second]);
    await controller.open(a);
    expect(bridge.calls.map((c) => c.command)).toEqual(["project_open", "project_open"]);
    expect(bridge.workspaces()).toHaveLength(1);
    expect(controller.state.lastOutcome[a]).toBe("reused");
  });

  // Would catch: an unavailable engine reported globally or erasing another project's state.
  it("keeps the open error on the failing project", async () => {
    const { bridge, controller, a, b } = await seeded();
    await controller.open(a);
    bridge.failOpen(b, { code: "server_unavailable", message: "API do Herdr indisponível", retryable: true, endpoint: "local" });
    await controller.open(b);
    expect(controller.state.projectErrors[b]?.code).toBe("server_unavailable");
    expect(controller.state.projectErrors[a]).toBeUndefined();
    expect(controller.state.snapshot!.projects.find((p) => p.id === a)!.binding).not.toBeNull();
    expect(controller.state.globalError).toBeNull();
  });

  // Keyboard and drag produce the store move command with the computed index; edges send nothing.
  it("maps keyboard and drag reordering to move commands", async () => {
    const { bridge, controller, produto, a, b } = await seeded();
    await controller.moveByKeyboard(produto, a, "up");
    expect(bridge.calls).toEqual([]);
    await controller.moveByKeyboard(produto, a, "down");
    await controller.moveByDrag(produto, a, b, "before");
    expect(bridge.calls).toEqual([
      { command: "collection_move_project", args: { collectionId: produto, projectId: a, toIndex: 1 } },
      { command: "collection_move_project", args: { collectionId: produto, projectId: a, toIndex: 0 } },
    ]);
    expect(controller.state.snapshot!.collections[0]!.project_ids).toEqual([a, b]);
  });

  // Would catch: submitting an incomplete form to the backend, or losing the typed buffer.
  it("does not submit an incomplete form and keeps the buffer", async () => {
    const { bridge, controller, produto } = await seeded();
    controller.openForm(produto);
    controller.editForm("label", "Sem raiz");
    await controller.submitProject();
    expect(bridge.calls).toEqual([]);
    expect(controller.state.form.open).toBe(true);
    expect(controller.state.form.missing).toEqual(["session_name", "root"]);
    expect(controller.state.form.draft.label).toBe("Sem raiz");
  });
});

// ---------------------------------------------------------------------------------------
// Spec 025 — the projects side bar acts on the engine workspaces (AC-025-01/02/03).
// Would catch: a click wired to tab.create/pane.split, an open that creates twice, a grouped
// workspace that is not persisted by its cwd, or a migration that fabricates empty groups.
// ---------------------------------------------------------------------------------------

const SAVED: ProjectDto = {
  id: "00000000-0000-4000-8000-0000000000aa",
  label: "herdr",
  endpoint_profile_id: "local",
  session_name: "default",
  root: "/srv/herdr",
  binding: null,
};

describe("projects controller — workspaces (spec 025)", () => {
  it("focuses a clicked workspace exactly once and never creates a tab or splits a pane", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-025" });
    const controller = createProjectsController(bridge);
    await controller.load();
    bridge.calls.length = 0;
    const first = controller.focusWorkspace("local", "w1");
    const second = controller.focusWorkspace("local", "w1");
    await Promise.all([first, second]);
    expect(bridge.calls).toEqual([{ command: "workspace_focus", args: { endpoint: "local", workspaceId: "w1" } }]);
    expect(bridge.calls.some((c) => c.command === "tab_create" || c.command === "pane_split")).toBe(false);
  });

  it("opens a closed project with one workspace.create carrying its cwd, basename and focus", async () => {
    const renamed: ProjectDto = { ...SAVED, label: "Zeta API", root: "/srv/zeta" };
    const bridge = createFakeProjectsBridge({
      bootId: "boot-025",
      seed: { version: 2, projects: [renamed], collections: [{ id: "g1", name: "Meus projetos", project_ids: [renamed.id] }] },
    });
    const controller = createProjectsController(bridge);
    await controller.load();
    bridge.calls.length = 0;
    await controller.openClosedProject(renamed.id);
    expect(bridge.calls).toEqual([
      { command: "workspace_create", args: { endpoint: "local", cwd: "/srv/zeta", label: "zeta", focus: true } },
    ]);
    expect(bridge.workspaces()).toHaveLength(1);
  });

  it("maps rename and close to workspace_rename/workspace_close after confirmation", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-025" });
    const controller = createProjectsController(bridge);
    await controller.load();
    bridge.calls.length = 0;
    await controller.renameWorkspace("local", "w1", "renomeado");
    await controller.closeWorkspace("local", "w1");
    expect(bridge.calls).toEqual([
      { command: "workspace_rename", args: { endpoint: "local", workspaceId: "w1", label: "renomeado" } },
      { command: "workspace_close", args: { endpoint: "local", workspaceId: "w1" } },
    ]);
  });

  it("persists a workspace in a group by its cwd and reopens it in the same group", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-025" });
    const controller = createProjectsController(bridge);
    await controller.load();
    await controller.createGroup("Meus projetos");
    const groupId = controller.state.snapshot!.collections[0]!.id;
    await controller.assignToGroup(groupId, { endpoint_profile_id: "local", session_name: "default", cwd: "/srv/herdr", label: "herdr" });

    const reloaded = createProjectsController(bridge);
    await reloaded.load();
    const group = reloaded.state.snapshot!.collections[0]!;
    expect(group.project_ids).toHaveLength(1);
    const project = reloaded.state.snapshot!.projects.find((p) => p.id === group.project_ids[0])!;
    expect(project).toMatchObject({ root: "/srv/herdr", label: "herdr", endpoint_profile_id: "local" });
    await reloaded.assignToGroup(groupId, { endpoint_profile_id: "local", session_name: "default", cwd: "/srv/herdr", label: "herdr" });
    expect(reloaded.state.snapshot!.collections[0]!.project_ids).toHaveLength(1);
    expect(reloaded.state.snapshot!.projects).toHaveLength(1);
  });

  it("loads the old catalog as closed projects in their groups without inventing groups", async () => {
    const bridge = createFakeProjectsBridge({
      bootId: "boot-025",
      seed: {
        version: 2,
        projects: [SAVED],
        collections: [
          { id: "g1", name: "Produto", project_ids: [SAVED.id] },
          { id: "g2", name: "Pessoal", project_ids: [] },
        ],
      },
    });
    const controller = createProjectsController(bridge);
    await controller.load();
    expect(controller.state.snapshot!.collections).toHaveLength(2);
    expect(controller.state.snapshot!.collections.map((c) => c.name)).toEqual(["Produto", "Pessoal"]);
    expect(bridge.calls.map((c) => c.command)).toEqual(["projects_list"]);
  });
});

// ---------------------------------------------------------------------------------------
// Spec 025 AC-025-05 — with a live workspace on the saved cwd, clicking the closed project
// focuses that workspace and never creates another one. Would catch the measured wK → wM bug.
// ---------------------------------------------------------------------------------------

function onlineHost(workspaces: HostDto["workspaces"]) {
  return hostFixture({
    endpoint: "local",
    label: "Este computador",
    kind: "local",
    phase: "online",
    phase_label: "Online",
    session: "default",
    target: null,
    workspaces,
  });
}

describe("projects controller — live workspace wins over the closed project (spec 025)", () => {
  it("focuses the open workspace with the saved cwd instead of creating a second one", async () => {
    const bridge = createFakeProjectsBridge({
      bootId: "boot-025",
      seed: { version: 2, projects: [SAVED], collections: [{ id: "g1", name: "Meus projetos", project_ids: [SAVED.id] }] },
    });
    let hosts: HostDto[] = [
      onlineHost([
        {
          workspace_id: "wM",
          number: 2,
          label: "herdr",
          focused: true,
          tab_count: 1,
          pane_count: 1,
          active_tab_id: "wM:t1",
          agent_status: "idle",
          cwd: "/srv/herdr/work",
          cwds: ["/srv/herdr", "/srv/herdr/work"],
          branch: "master",
        },
      ]),
    ];
    const controller = createProjectsController(bridge, () => {}, { hosts: () => hosts });
    await controller.load();
    bridge.calls.length = 0;
    await controller.openClosedProject(SAVED.id);
    expect(bridge.calls).toEqual([{ command: "workspace_focus", args: { endpoint: "local", workspaceId: "wM" } }]);
    expect(bridge.workspaces()).toHaveLength(0);
  });

  it("creates exactly one workspace when no live workspace has that cwd", async () => {
    const bridge = createFakeProjectsBridge({ bootId: "boot-025" });
    let hosts: HostDto[] = [onlineHost([])];
    const controller = createProjectsController(bridge, () => {}, { hosts: () => hosts });
    await controller.load();
    bridge.calls.length = 0;
    await controller.createGroup("Meus projetos");
    const groupId = controller.state.snapshot!.collections[0]!.id;
    await controller.assignToGroup(groupId, { endpoint_profile_id: "local", session_name: "default", cwd: "/srv/herdr", label: "herdr" });
    bridge.calls.length = 0;
    const project = controller.state.snapshot!.projects[0]!;
    await controller.openClosedProject(project.id);
    expect(bridge.calls.filter((c) => c.command === "workspace_create")).toHaveLength(1);
    expect(bridge.calls.some((c) => c.command === "workspace_focus")).toBe(false);
  });
});
