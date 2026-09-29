// Spec 041 — pure model of the new sidebar: collections regrouped across hosts (AC-041-02) and
// the per-row status summary that splits `done` out of idle (AC-041-03).
import { describe, expect, it } from "vitest";
import type { AgentDto, AgentStatus } from "../../agents/types";
import { hostFixture } from "../../connections/fake-bridge";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import type { CollectionDto, ProjectDto } from "../../projects/types";
import { groupColor } from "../projects/tree-model";
import { buildSidebar, effectiveAgents, sidebarStatus, UNGROUPED_ID } from "./sidebar-model";

function workspace(overrides: Partial<HostWorkspaceDto> & { workspace_id: string }): HostWorkspaceDto {
  return {
    number: 1,
    label: "workspace",
    focused: false,
    tab_count: 1,
    pane_count: 1,
    active_tab_id: "t1",
    agent_status: "unknown",
    cwd: null,
    branch: null,
    ...overrides,
  };
}

function agent(workspaceId: string, status: AgentStatus, paneId = `p-${workspaceId}-${status}`): AgentDto {
  return {
    pane_id: paneId,
    workspace_id: workspaceId,
    tab_id: "t1",
    name: paneId,
    kind: "codex",
    status,
    launch_pending: false,
    ready: true,
    focused: false,
  };
}

/**
 * AC-041-02 fixture: Local (`api` in G1, `web` with no collection) and the online SSH `dev-box`
 * (`lib` in G1), plus `G2` with no members anywhere.
 */
function fixture(): { hosts: HostDto[]; groups: CollectionDto[]; projects: ProjectDto[] } {
  const hosts: HostDto[] = [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "hd041",
      target: null,
      // Out of `number` order on purpose: the section must order by `number`, not by arrival.
      workspaces: [
        workspace({ workspace_id: "w-web", number: 2, label: "web", cwd: "/w/web", branch: "feat/login" }),
        workspace({ workspace_id: "w-api", number: 1, label: "api", cwd: "/w/api", branch: "main", focused: true }),
      ],
    }),
    hostFixture({
      endpoint: "ssh-dev",
      label: "dev-box",
      kind: "ssh",
      phase: "online",
      phase_label: "Online",
      session: "hd041",
      workspaces: [workspace({ workspace_id: "w-lib", number: 1, label: "lib", cwd: "/w/lib", branch: "master" })],
    }),
  ];
  const projects: ProjectDto[] = [
    { id: "p-api", label: "api", endpoint_profile_id: "local", session_name: "hd041", root: "/w/api", binding: null },
    { id: "p-web", label: "web", endpoint_profile_id: "local", session_name: "hd041", root: "/w/web", binding: null },
    { id: "p-lib", label: "lib", endpoint_profile_id: "ssh-dev", session_name: "hd041", root: "/w/lib", binding: null },
  ];
  const groups: CollectionDto[] = [
    { id: "g1", name: "G1", project_ids: ["p-api", "p-lib"] },
    { id: "g2", name: "G2", project_ids: [] },
  ];
  return { hosts, groups, projects };
}

describe("AC-041-02 collections regrouped across hosts", () => {
  // Would catch: one section per host (the 025 tree shown raw), SSH rows before Local ones inside
  // a collection, an empty collection dropped, or `Sem coleção` anywhere but last.
  it("orders G1 (api then lib), the empty G2 with count 0, and Sem coleção with web last", () => {
    const { hosts, groups, projects } = fixture();
    const sections = buildSidebar({ hosts, groups, projects, selectedEndpoint: "local" });

    expect(sections.map((s) => s.id)).toEqual(["g1", "g2", UNGROUPED_ID]);
    expect(sections.map((s) => s.name)).toEqual(["G1", "G2", "Sem coleção"]);
    expect(sections[0]!.rows.map((r) => r.name)).toEqual(["api", "lib"]);
    expect(sections[0]!.rows.map((r) => r.endpoint)).toEqual(["local", "ssh-dev"]);
    expect(sections[0]!.count).toBe(2);
    expect(sections[1]!.rows).toEqual([]);
    expect(sections[1]!.count).toBe(0);
    expect(sections[1]!.empty).toBe(true);
    expect(sections[2]!.rows.map((r) => r.name)).toEqual(["web"]);
    expect(sections[2]!.ungrouped).toBe(true);
  });

  // Would catch: a palette restarted per host, or `Sem coleção` reusing G1's colour.
  it("colours every header with groupColor of its position in the store", () => {
    const { hosts, groups, projects } = fixture();
    const sections = buildSidebar({ hosts, groups, projects, selectedEndpoint: "local" });
    expect(sections.map((s) => s.color)).toEqual([groupColor(0), groupColor(1), groupColor(2)]);
  });

  // Would catch: an empty `Sem coleção` header rendered when every workspace has a collection.
  it("omits Sem coleção when every row belongs to a collection", () => {
    const { hosts, groups, projects } = fixture();
    const all: CollectionDto[] = [{ id: "g1", name: "G1", project_ids: ["p-api", "p-lib", "p-web"] }];
    const sections = buildSidebar({ hosts, groups: all, projects, selectedEndpoint: "local" });
    expect(sections.map((s) => s.id)).toEqual(["g1"]);
    expect(sections[0]!.rows.map((r) => r.name)).toEqual(["api", "web", "lib"]);
    // The store order of the other fixture is untouched by this one.
    expect(groups.map((g) => g.id)).toEqual(["g1", "g2"]);
  });

  // Edge case of the spec: no host connected keeps the saved (empty) collections visible.
  it("keeps the store collections with no host connected and reports no rows", () => {
    const { groups, projects } = fixture();
    const sections = buildSidebar({ hosts: [], groups, projects, selectedEndpoint: null });
    expect(sections.map((s) => s.id)).toEqual(["g1", "g2"]);
    expect(sections.every((s) => s.rows.length === 0)).toBe(true);
  });

  // Edge case of the spec: a collection whose members live only on an offline host still shows.
  it("shows a collection whose only member sits on an offline host, marked offline", () => {
    const { groups, projects } = fixture();
    const hosts: HostDto[] = [
      hostFixture({
        endpoint: "ssh-dev",
        label: "dev-box",
        kind: "ssh",
        phase: "offline",
        phase_label: "Offline",
        session: "hd041",
        workspaces: [workspace({ workspace_id: "w-lib", number: 1, label: "lib", cwd: "/w/lib", branch: "master" })],
      }),
    ];
    const sections = buildSidebar({ hosts, groups, projects, selectedEndpoint: null });
    const g1 = sections.find((s) => s.id === "g1")!;
    expect(g1.rows.map((r) => r.name)).toEqual(["lib"]);
    expect(g1.rows[0]!.offline).toBe(true);
    expect(g1.rows[0]!.hostBadge).toBe("dev-box");
  });

  // Spec 036/041 P7: exactly one active row, the focused workspace of the selected host.
  it("marks exactly one row active across every collection", () => {
    const { hosts, groups, projects } = fixture();
    const sections = buildSidebar({ hosts, groups, projects, selectedEndpoint: "local" });
    const active = sections.flatMap((s) => s.rows).filter((r) => r.active);
    expect(active.map((r) => r.name)).toEqual(["api"]);
  });

  // Would catch: the local host's tag rendered like an SSH one.
  it("tags only SSH rows with the host name", () => {
    const { hosts, groups, projects } = fixture();
    const rows = buildSidebar({ hosts, groups, projects, selectedEndpoint: "local" }).flatMap((s) => s.rows);
    expect(rows.find((r) => r.name === "api")!.hostBadge).toBeNull();
    expect(rows.find((r) => r.name === "lib")!.hostBadge).toBe("dev-box");
  });
});

describe("AC-041-03 sidebarStatus splits done out of idle", () => {
  // Would catch: `done` folded into idle like mapAgentStatus does, or `unknown` counted as work.
  it("maps working/blocked/done and leaves the rest idle", () => {
    expect(sidebarStatus(["working"]).kind).toBe("working");
    expect(sidebarStatus(["blocked"]).kind).toBe("waiting");
    expect(sidebarStatus(["done"]).kind).toBe("done");
    expect(sidebarStatus(["idle"]).kind).toBe("idle");
    expect(sidebarStatus(["unknown"]).kind).toBe("idle");
    expect(sidebarStatus([]).kind).toBe("idle");
    expect(sidebarStatus(["working"]).label).toBe("1 trabalhando");
    expect(sidebarStatus(["blocked"]).label).toBe("1 aguardando");
    expect(sidebarStatus(["done"]).label).toBe("1 concluído");
    expect(sidebarStatus(["idle", "unknown"]).label).toBe("");
  });

  // Would catch: the badge counting every agent instead of the working ones.
  it("counts the working agents for the badge and keeps the other flags", () => {
    const status = sidebarStatus(["working", "working", "idle", "blocked", "done"]);
    expect(status.working).toBe(2);
    expect(status.waiting).toBe(1);
    expect(status.done).toBe(1);
    expect(status.idle).toBe(1);
    expect(status.kind).toBe("working");
    expect(status.label).toBe("2 trabalhando");
  });

  // Would catch: a row summarising the agents of another workspace.
  it("summarises each row from the agents of its own workspace", () => {
    const { hosts, groups, projects } = fixture();
    const agents = [agent("w-api", "working"), agent("w-api", "working", "p2"), agent("w-web", "blocked"), agent("w-lib", "done")];
    const rows = buildSidebar({ hosts, groups, projects, agents, selectedEndpoint: "local" }).flatMap((s) => s.rows);
    const of = (name: string) => rows.find((r) => r.name === name)!.status;
    expect(of("api").kind).toBe("working");
    expect(of("api").working).toBe(2);
    expect(of("web").kind).toBe("waiting");
    expect(of("lib").kind).toBe("done");
  });

  // Would catch: a closed row (025) losing its agents because it has no workspace id.
  it("summarises a closed project row by its project id, like the 025 tree", () => {
    const hosts: HostDto[] = [
      hostFixture({ endpoint: "local", label: "Este computador", kind: "local", phase: "online", phase_label: "Online", session: "hd041", target: null, workspaces: [] }),
    ];
    const projects: ProjectDto[] = [
      { id: "p-api", label: "api", endpoint_profile_id: "local", session_name: "hd041", root: "/w/api", binding: null },
    ];
    const sections = buildSidebar({ hosts, groups: [], projects, agents: [agent("p-api", "blocked")], selectedEndpoint: "local" });
    const row = sections[0]!.rows[0]!;
    expect(row.kind).toBe("closed");
    expect(row.status.kind).toBe("waiting");
  });
});

describe("agents fallback shared with the 011 region", () => {
  // Would catch: the sidebar going blank on a host whose API never listed agents (the tree used
  // the host's own agent/pane snapshot instead).
  it("derives agents from the hosts only when the live list is empty", () => {
    const hosts: HostDto[] = [
      hostFixture({
        endpoint: "local",
        kind: "local",
        phase: "online",
        agents: [
          { pane_id: "p1", workspace_id: "w-api", tab_id: "t1", name: "codex", agent: "codex", agent_status: "working", focused: true } as never,
        ],
      }),
    ];
    expect(effectiveAgents([], hosts).map((a) => [a.workspace_id, a.status])).toEqual([["w-api", "working"]]);
    const live = [agent("w-api", "blocked")];
    expect(effectiveAgents(live, hosts)).toBe(live);
  });
});
