import { describe, expect, it } from "vitest";
import type { AgentDto } from "../../agents/types";
import type { HostDto, HostWorkspaceDto } from "../../connections/types";
import { hostFixture } from "../../connections/fake-bridge";
import type { CollectionDto, ProjectDto } from "../../projects/types";
import {
  buildConnectionItems,
  buildProjectItem,
  buildTree,
  buildWorkspaceTree,
  workspaceMatchesRoot,
  formatAgentSummary,
  groupColor,
  mapAgentStatus,
} from "./tree-model";

const P1: ProjectDto = {
  id: "p1",
  label: "erp-api",
  endpoint_profile_id: "local",
  session_name: "hd-test",
  root: "/work/erp-api",
  binding: { project_id: "p1", connection_generation: 1, boot_id: "boot1", workspace_id: "w1" },
};

const P2: ProjectDto = {
  id: "p2",
  label: "herdr",
  endpoint_profile_id: "ssh-dev",
  session_name: "hd-test",
  root: "/home/user/herdr",
  binding: { project_id: "p2", connection_generation: 1, boot_id: "boot1", workspace_id: "w2" },
};

const P3: ProjectDto = {
  id: "p3",
  label: "empty-proj",
  endpoint_profile_id: "local",
  session_name: "hd-test",
  root: "/work/empty",
  binding: null,
};

const AGENTS: AgentDto[] = [
  { pane_id: "pane1", workspace_id: "w1", tab_id: "t1", name: "claude", kind: "claude", status: "working", launch_pending: false, ready: true, focused: true },
  { pane_id: "pane2", workspace_id: "w1", tab_id: "t1", name: "codex", kind: "codex", status: "blocked", launch_pending: false, ready: true, focused: false },
  { pane_id: "pane3", workspace_id: "w1", tab_id: "t1", name: "shell", kind: "shell", status: "idle", launch_pending: false, ready: true, focused: false },
  { pane_id: "pane4", workspace_id: "w1", tab_id: "t1", name: "extra", kind: "extra", status: "working", launch_pending: false, ready: true, focused: false },
  { pane_id: "pane5", workspace_id: "w2", tab_id: "t2", name: "pi", kind: "pi", status: "working", launch_pending: false, ready: true, focused: true },
];

describe("tree-model", () => {
  it("maps agent status to dots correctly", () => {
    expect(mapAgentStatus("working")?.status).toBe("working");
    expect(mapAgentStatus("blocked")?.status).toBe("blocked");
    expect(mapAgentStatus("idle")?.status).toBe("idle");
    expect(mapAgentStatus("done")?.status).toBe("idle");
    expect(mapAgentStatus("unknown")).toBeNull();
  });

  it("formats agent summary accessibility text", () => {
    const dots = [
      { status: "working" as const, color: "green", label: "trabalhando" },
      { status: "blocked" as const, color: "amber", label: "aguardando" },
      { status: "idle" as const, color: "gray", label: "ocioso" },
    ];
    expect(formatAgentSummary(dots)).toBe("3 agentes: 1 trabalhando, 1 aguardando, 1 ocioso");
    expect(formatAgentSummary([])).toBe("Sem agentes ativos");
  });

  it("builds project item with branch, badge, active flag, and agent dots with overflow", () => {
    const hostLabels = { "ssh-dev": "dev-box" };

    // P1: Local, 4 agents in w1 -> 3 visible dots + 1 overflow
    const item1 = buildProjectItem(P1, "p1", hostLabels, AGENTS, "main");
    expect(item1.id).toBe("p1");
    expect(item1.name).toBe("erp-api");
    expect(item1.isSsh).toBe(false);
    expect(item1.hostBadge).toBeNull();
    expect(item1.branch).toBe("main");
    expect(item1.active).toBe(true);
    expect(item1.visibleDots.length).toBe(3);
    expect(item1.overflowCount).toBe(1);
    expect(item1.visibleDots.map((d) => d.status)).toEqual(["working", "blocked", "idle"]);

    // P2: SSH, 1 agent in w2 -> 1 dot, host badge dev-box
    const item2 = buildProjectItem(P2, "p1", hostLabels, AGENTS, null);
    expect(item2.id).toBe("p2");
    expect(item2.name).toBe("herdr");
    expect(item2.isSsh).toBe(true);
    expect(item2.hostBadge).toBe("dev-box");
    expect(item2.branch).toBe("—");
    expect(item2.active).toBe(false);
    expect(item2.visibleDots.length).toBe(1);
    expect(item2.overflowCount).toBe(0);

    // P3: No binding -> 0 dots
    const item3 = buildProjectItem(P3, null, hostLabels, AGENTS, "feat/login");
    expect(item3.visibleDots.length).toBe(0);
    expect(item3.overflowCount).toBe(0);
    expect(item3.branch).toBe("feat/login");
  });

  it("project with workspace without agents shows zero dots even when active and session has agents", () => {
    const P_EMPTY_WS: ProjectDto = {
      id: "p_empty_ws",
      label: "no-agents-proj",
      endpoint_profile_id: "local",
      session_name: "hd-test",
      root: "/work/empty-ws",
      binding: { project_id: "p_empty_ws", connection_generation: 1, boot_id: "boot1", workspace_id: "w3" },
    };
    // Active project with workspace w3 which has no agents in AGENTS list
    const item = buildProjectItem(P_EMPTY_WS, "p_empty_ws", {}, AGENTS, "main");
    expect(item.active).toBe(true);
    expect(item.allDots.length).toBe(0);
    expect(item.visibleDots.length).toBe(0);
    expect(item.overflowCount).toBe(0);
    expect(item.agentSummary).toBe("Sem agentes ativos");
  });

  it("builds groups with colors, counts, and projects", () => {
    const collections: CollectionDto[] = [
      { id: "c1", name: "Acme · Clientes", project_ids: ["p1", "p3"] },
      { id: "c2", name: "Open source", project_ids: ["p2"] },
      { id: "c3", name: "Vazio", project_ids: [] },
    ];
    const tree = buildTree(collections, [P1, P2, P3], "p1", { "ssh-dev": "dev-box" }, AGENTS, { p1: "main" });

    expect(tree.length).toBe(3);
    expect(tree[0]!.name).toBe("Acme · Clientes");
    expect(tree[0]!.count).toBe(2);
    expect(tree[0]!.empty).toBe(false);
    expect(tree[0]!.color).toBe(groupColor(0));
    expect(tree[0]!.projects.map((p) => p.name)).toEqual(["erp-api", "empty-proj"]);

    expect(tree[1]!.name).toBe("Open source");
    expect(tree[1]!.count).toBe(1);
    expect(tree[1]!.projects[0]!.hostBadge).toBe("dev-box");

    expect(tree[2]!.name).toBe("Vazio");
    expect(tree[2]!.count).toBe(0);
    expect(tree[2]!.empty).toBe(true);
  });

  it("without saved collections shows Meus projetos 0", () => {
    const tree = buildTree([], [], null, {}, []);
    expect(tree).toHaveLength(1);
    expect(tree[0]!.name).toBe("Meus projetos");
    expect(tree[0]!.count).toBe(0);
    expect(tree[0]!.empty).toBe(true);
  });

  it("builds connection items with Este computador, latency, and status", () => {
    const hosts: HostDto[] = [
      {
        endpoint: "local",
        label: "Local",
        kind: "local",
        session: "hd",
        target: null,
        visible: true,
        phase: "online",
        phase_label: "conectado",
        attempt: 0,
        retry_in_ms: null,
        cancelled: false,
        attention: null,
        guidance: null,
        connection_error: null,
        action_error: null,
        generation: 1,
        boot_id: "b1",
        cached: false,
        surface: null,
        screen: [],
        panes: [],
        actions: [],
      },
      {
        endpoint: "ssh-dev",
        label: "dev-box",
        kind: "ssh",
        session: "remote",
        target: "user@dev-box",
        visible: false,
        phase: "online",
        phase_label: "conectado",
        attempt: 0,
        retry_in_ms: null,
        cancelled: false,
        attention: null,
        guidance: null,
        connection_error: null,
        action_error: null,
        generation: 1,
        boot_id: "b2",
        cached: false,
        surface: null,
        screen: [],
        panes: [],
        actions: [],
        ...({ latency_ms: 32 } as any),
      },
      {
        endpoint: "ssh-build",
        label: "build-box",
        kind: "ssh",
        session: "ci",
        target: "ci@build-box",
        visible: false,
        phase: "offline",
        phase_label: "offline",
        attempt: 1,
        retry_in_ms: null,
        cancelled: false,
        attention: null,
        guidance: null,
        connection_error: null,
        action_error: null,
        generation: null,
        boot_id: null,
        cached: false,
        surface: null,
        screen: [],
        panes: [],
        actions: [],
      },
    ];

    const items = buildConnectionItems(hosts, "ssh-dev");
    expect(items.length).toBe(3);

    // Item 1: Este computador · Local
    expect(items[0]!.name).toBe("Este computador");
    expect(items[0]!.typeBadge).toBe("Local");
    expect(items[0]!.tone).toBe("ok");
    expect(items[0]!.active).toBe(false);

    // Item 2: dev-box · SSH · 32 ms
    expect(items[1]!.name).toBe("dev-box");
    expect(items[1]!.typeBadge).toBe("SSH");
    expect(items[1]!.latencyText).toBe("32 ms");
    expect(items[1]!.tone).toBe("ok");
    expect(items[1]!.active).toBe(true);

    // Item 3: build-box · SSH · offline
    expect(items[2]!.name).toBe("build-box");
    expect(items[2]!.typeBadge).toBe("SSH");
    expect(items[2]!.latencyText).toBe("offline");
    expect(items[2]!.tone).toBe("idle");
    expect(items[2]!.active).toBe(false);
  });
});

// ---------------------------------------------------------------------------------------
// Spec 029 (AC-029-02) — a host whose connection failed is never shown as connected: error dot,
// the reason in words and the explicit Tentar novamente.
// ---------------------------------------------------------------------------------------

describe("connection items on failure (AC-029-02)", () => {
  const failure = (endpoint: string, attention: HostDto["attention"], code: string, message: string): HostDto =>
    hostFixture({
      endpoint,
      label: endpoint,
      phase: "attention",
      phase_label: "Precisa de atenção",
      attention,
      guidance: "Configure o host em um terminal e use Tentar novamente.",
      connection_error: { code, message, retryable: false, endpoint },
    });

  it("shows the reason, the error dot and Tentar novamente for every failure", () => {
    const hosts = [
      hostFixture({ endpoint: "local", kind: "local", label: "Este computador", phase: "online", phase_label: "Online", latency_ms: 4 }),
      failure("ssh-mac", "herdr_missing", "remote_herdr_missing", "o Herdr não foi encontrado no host remoto"),
      failure("ssh-auth", "authentication_required", "ssh_authentication_required", "o host exige autenticação interativa"),
      failure("ssh-old", "herdr_outdated", "remote_herdr_outdated", "o Herdr do host está desatualizado (versão 0.8.0)"),
      hostFixture({
        endpoint: "ssh-slow",
        label: "dev-lento",
        phase: "offline",
        phase_label: "Offline",
        attempt: 1,
        retry_in_ms: 2000,
        connection_error: { code: "ssh_unreachable", message: "host SSH inacessível", retryable: true, endpoint: "ssh-slow" },
      }),
    ];
    const items = buildConnectionItems(hosts, "ssh-mac");
    expect(items[1]!.statusText).toBe("Herdr não encontrado");
    expect(items[2]!.statusText).toBe("autenticação recusada");
    expect(items[3]!.statusText).toBe("Herdr desatualizado no host");
    expect(items[4]!.statusText).toBe("sem resposta");
    for (const item of items.slice(1)) {
      expect(item.tone).toBe("attention");
      expect(item.retryable).toBe(true);
      expect(item.latencyText).not.toContain("ms");
    }
    // Local is connected and never gets the failure treatment nor a retry.
    expect(items[0]!.tone).toBe("ok");
    expect(items[0]!.retryable).toBe(false);
    expect(items[0]!.statusText).toBe("Online");
  });

  it("keeps an offline host without a failure as idle (no fabricated reason)", () => {
    const items = buildConnectionItems([hostFixture({ endpoint: "ssh-idle", phase: "offline", phase_label: "Offline" })], null);
    expect(items[1]!.tone).toBe("idle");
    expect(items[1]!.statusText).toBe("Offline");
    expect(items[1]!.retryable).toBe(false);
  });

  // Spec 029 AC-029-01: the chosen binary path and version travel to the host tooltip.
  it("builds the herdr tooltip from the discovered binary and the negotiated version", () => {
    const discovered = hostFixture({
      endpoint: "ssh-mac",
      phase: "online",
      phase_label: "Online",
      latency_ms: 42,
      server_version: "0.9.1",
      herdr_binary: { path: "/Users/ec2-user/.local/bin/herdr", version: "0.9.1" },
    });
    const pathHost = hostFixture({
      endpoint: "ssh-path",
      phase: "online",
      phase_label: "Online",
      latency_ms: 8,
      server_version: "0.9.0",
      herdr_binary: { path: "herdr", version: null },
    });
    const items = buildConnectionItems(
      [hostFixture({ endpoint: "local", kind: "local", label: "Este computador", phase: "online", phase_label: "Online" }), discovered, pathHost],
      null,
    );
    expect(items[1]!.tooltip).toBe("herdr 0.9.1 · /Users/ec2-user/.local/bin/herdr");
    expect(items[2]!.tooltip).toBe("herdr 0.9.0 · herdr");
    expect(items[0]!.tooltip).toBe("Online");
  });
});

// ---------------------------------------------------------------------------------------
// Spec 025 — PROJETOS lists the engine workspaces of each host (AC-025-01/02).
// ---------------------------------------------------------------------------------------

function workspaceFixture(overrides: Partial<HostWorkspaceDto> & { workspace_id: string }): HostWorkspaceDto {
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

const SAVED_HERDR: ProjectDto = {
  id: "p1",
  label: "herdr",
  endpoint_profile_id: "local",
  session_name: "default",
  root: "/srv/herdr",
  binding: null,
  branch: "master",
};

const SAVED_ZETA: ProjectDto = {
  id: "p2",
  label: "zeta",
  endpoint_profile_id: "ssh-dev",
  session_name: "default",
  root: "/srv/zeta",
  binding: null,
};

const GROUP_MEUS: CollectionDto = { id: "g1", name: "Meus projetos", project_ids: ["p1"] };

const LOCAL_WORKSPACES = [
  workspaceFixture({ workspace_id: "w3", number: 3, label: "backend", branch: "feat/backend" }),
  workspaceFixture({ workspace_id: "w1", number: 1, label: "herdr", branch: "master", focused: true, cwd: "/srv/herdr" }),
  workspaceFixture({ workspace_id: "w2", number: 2, label: "frontend", branch: "feat/front", cwd: "/work/front" }),
];

function engineHosts(): HostDto[] {
  return [
    hostFixture({
      endpoint: "local",
      label: "Este computador",
      kind: "local",
      phase: "online",
      phase_label: "Online",
      session: "default",
      target: null,
      workspaces: LOCAL_WORKSPACES,
    }),
    hostFixture({
      endpoint: "ssh-dev",
      label: "mac-mini",
      kind: "ssh",
      phase: "online",
      phase_label: "Online",
      session: "default",
      workspaces: [workspaceFixture({ workspace_id: "r1", number: 1, label: "~", branch: null, cwd: "/home/user" })],
    }),
  ];
}

describe("spec 025 workspace tree", () => {
  it("lists exactly the workspaces of each host, ordered by number", () => {
    const agents: AgentDto[] = [
      { pane_id: "w1:p1", workspace_id: "w1", tab_id: "t1", name: "claude", kind: "claude", status: "working", launch_pending: false, ready: true, focused: true },
      { pane_id: "w9:p1", workspace_id: "w9", tab_id: "t9", name: "other", kind: "codex", status: "blocked", launch_pending: false, ready: true, focused: false },
    ];
    const tree = buildWorkspaceTree({ hosts: engineHosts(), groups: [], projects: [], agents });

    expect(tree.map((host) => host.header)).toEqual(["Este computador · Local", "mac-mini · SSH"]);
    expect(tree[0]!.rows.map((row) => row.name)).toEqual(["herdr", "frontend", "backend"]);
    expect(tree[0]!.rows.map((row) => row.branch)).toEqual(["master", "feat/front", "feat/backend"]);
    expect(tree[0]!.rows[0]!.active).toBe(true);
    expect(tree[0]!.rows[0]!.workspaceId).toBe("w1");
    expect(tree[0]!.rows[0]!.visibleDots).toHaveLength(1);
    expect(tree[0]!.rows[1]!.visibleDots).toHaveLength(0);
    expect(tree[1]!.rows).toHaveLength(1);
    expect(tree[1]!.rows[0]!.name).toBe("~");
    expect(tree[1]!.rows[0]!.branch).toBeNull();
    expect(tree[1]!.rows[0]!.active).toBe(false);
    expect(tree[0]!.rows.length + tree[1]!.rows.length).toBe(4);
  });

  it("reconciles a closed workspace without restarting (new host snapshot)", () => {
    const closed = engineHosts().map((host) =>
      host.endpoint === "local"
        ? { ...host, workspaces: (host.workspaces ?? []).filter((w) => w.workspace_id !== "w2") }
        : host,
    );
    const tree = buildWorkspaceTree({ hosts: closed, groups: [], projects: [] });
    expect(tree[0]!.rows.map((row) => row.name)).toEqual(["herdr", "backend"]);
    expect(tree[0]!.rows.length + tree[1]!.rows.length).toBe(3);
  });

  it("places a live workspace under the group of its root cwd and keeps the live row", () => {
    const tree = buildWorkspaceTree({
      hosts: engineHosts(),
      groups: [GROUP_MEUS],
      projects: [SAVED_HERDR, SAVED_ZETA],
    });
    const local = tree[0]!;
    expect(local.groups).toHaveLength(1);
    expect(local.groups[0]!.name).toBe("Meus projetos");
    expect(local.groups[0]!.count).toBe(1);
    expect(local.groups[0]!.rows[0]!.kind).toBe("workspace");
    expect(local.groups[0]!.rows[0]!.workspaceId).toBe("w1");
    expect(local.groups[0]!.rows[0]!.projectId).toBe("p1");
    expect(local.rows.map((row) => row.name)).toEqual(["frontend", "backend"]);
    // The saved project in the group is not repeated as a closed row while its workspace is open.
    expect(local.rows.some((row) => row.kind === "closed")).toBe(false);
  });

  it("shows a closed project in its group with `fechado` and opens with its cwd", () => {
    const withoutHerdr = engineHosts().map((host) =>
      host.endpoint === "local"
        ? { ...host, workspaces: (host.workspaces ?? []).filter((w) => w.workspace_id !== "w1") }
        : host,
    );
    const tree = buildWorkspaceTree({ hosts: withoutHerdr, groups: [GROUP_MEUS], projects: [SAVED_HERDR, SAVED_ZETA] });
    const row = tree[0]!.groups[0]!.rows[0]!;
    expect(row.kind).toBe("closed");
    expect(row.name).toBe("herdr");
    expect(row.branch).toBe("fechado");
    expect(row.projectId).toBe("p1");
    expect(row.cwd).toBe("/srv/herdr");
    expect(row.workspaceId).toBeNull();
    expect(row.active).toBe(false);
    // A saved project without a group is loose under its host, like a workspace without a group.
    expect(tree[1]!.rows.map((r) => r.name)).toContain("zeta");
  });

  it("creates an empty group visible under the first host (`Novo grupo`)", () => {
    const tree = buildWorkspaceTree({
      hosts: engineHosts(),
      groups: [{ id: "g-empty", name: "Personal", project_ids: [] }],
      projects: [],
    });
    expect(tree[0]!.groups).toHaveLength(1);
    expect(tree[0]!.groups[0]!.name).toBe("Personal");
    expect(tree[0]!.groups[0]!.count).toBe(0);
    expect(tree[0]!.groups[0]!.empty).toBe(true);
    expect(tree[1]!.groups).toHaveLength(0);
  });

  it("marks an offline host's rows disabled with `offline`, without dropping them (cached)", () => {
    const hosts = engineHosts().map((host) => (host.endpoint === "ssh-dev" ? { ...host, phase: "offline" as const } : host));
    const tree = buildWorkspaceTree({ hosts, groups: [], projects: [] });
    expect(tree[1]!.offline).toBe(true);
    expect(tree[1]!.rows).toHaveLength(1);
    expect(tree[1]!.rows[0]!.disabled).toBe(true);
    expect(tree[1]!.rows[0]!.name).toBe("~");
  });

  it("keeps two workspaces with the same cwd as two rows in the same group", () => {
    const hosts = engineHosts().map((host) =>
      host.endpoint === "local"
        ? {
            ...host,
            workspaces: [
              workspaceFixture({ workspace_id: "wa", number: 1, label: "herdr", cwd: "/srv/herdr" }),
              workspaceFixture({ workspace_id: "wb", number: 2, label: "herdr-2", cwd: "/srv/herdr" }),
            ],
          }
        : host,
    );
    const tree = buildWorkspaceTree({ hosts, groups: [GROUP_MEUS], projects: [SAVED_HERDR] });
    expect(tree[0]!.groups[0]!.rows.map((row) => row.workspaceId)).toEqual(["wa", "wb"]);
    expect(tree[0]!.rows).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------------------
// Spec 025 AC-025-05 — a live workspace is NEVER shown as a closed project when its cwd (any
// pane/root cwd, not only the focused pane) matches the saved root. Would catch the measured
// bug: Listing-multisite-backend appeared as `fechado` and the click created a second
// workspace while wM was open with the same cwd.
// ---------------------------------------------------------------------------------------

describe("spec 025 live workspace vs saved project (AC-025-05)", () => {
  const SAVED_BACKEND: ProjectDto = {
    id: "p-backend",
    label: "Listing-multisite-backend",
    endpoint_profile_id: "local",
    session_name: "default",
    root: "/srv/backend",
    binding: null,
  };
  const GROUP: CollectionDto = { id: "g1", name: "Meus projetos", project_ids: ["p-backend"] };

  it("a live workspace matching any of its cwds is the group's live row, with no closed duplicate", () => {
    const hosts = engineHosts().map((host) =>
      host.endpoint === "local"
        ? {
            ...host,
            workspaces: [
              workspaceFixture({
                workspace_id: "wb",
                number: 2,
                label: "Listing-multisite-backend",
                branch: "feat/search-suggest-by-name",
                // The focused pane cd'ed into a subdirectory; the root lives in the pane cwds.
                cwd: "/srv/backend/work",
                cwds: ["/srv/backend", "/srv/backend/work"],
              }),
            ],
          }
        : host,
    );
    const tree = buildWorkspaceTree({ hosts, groups: [GROUP], projects: [SAVED_BACKEND] });
    const local = tree[0]!;
    expect(local.groups).toHaveLength(1);
    expect(local.groups[0]!.rows).toHaveLength(1);
    expect(local.groups[0]!.rows[0]!.kind).toBe("workspace");
    expect(local.groups[0]!.rows[0]!.workspaceId).toBe("wb");
    expect(local.groups[0]!.rows[0]!.branch).toBe("feat/search-suggest-by-name");
    expect(local.rows.some((row) => row.workspaceId === "wb")).toBe(false);
    expect(local.rows.some((row) => row.kind === "closed")).toBe(false);
    expect(tree.flatMap((host) => [...host.rows, ...host.groups.flatMap((group) => group.rows)]).filter((row) => row.name === "Listing-multisite-backend")).toHaveLength(1);
  });

  it("workspaceMatchesRoot normalizes slashes and reads both cwd and cwds", () => {
    expect(workspaceMatchesRoot({ cwd: "/srv/backend", cwds: [] }, "/srv/backend/")).toBe(true);
    expect(workspaceMatchesRoot({ cwd: "/srv/other", cwds: ["/srv/backend"] }, "/srv/backend")).toBe(true);
    expect(workspaceMatchesRoot({ cwd: "/srv/other", cwds: [] }, "/srv/backend")).toBe(false);
  });

  it("with no live workspace for the root the project is still a closed row", () => {
    const tree = buildWorkspaceTree({ hosts: engineHosts(), groups: [GROUP], projects: [SAVED_BACKEND] });
    const closed = tree[0]!.groups[0]!.rows[0]!;
    expect(closed.kind).toBe("closed");
    expect(closed.name).toBe("Listing-multisite-backend");
    expect(closed.branch).toBe("fechado");
  });
});

describe("spec 036 single active row (AC-036-01)", () => {
  it("only the focused workspace of the selected host is active when multiple hosts are connected", () => {
    const hosts: HostDto[] = [
      hostFixture({
        endpoint: "local",
        label: "Este computador",
        kind: "local",
        phase: "online",
        session: "default",
        workspaces: [
          workspaceFixture({ workspace_id: "w1", number: 1, label: "herdr", branch: "master", focused: true }),
          workspaceFixture({ workspace_id: "w2", number: 2, label: "frontend", branch: "feat/front", focused: false }),
        ],
      }),
      hostFixture({
        endpoint: "ssh-dev",
        label: "mac-mini",
        kind: "ssh",
        phase: "online",
        session: "default",
        workspaces: [
          workspaceFixture({ workspace_id: "r1", number: 1, label: "~", branch: null, focused: true }),
        ],
      }),
    ];

    // selectedEndpoint = local -> only herdr is active; ~ is not active
    const localTree = buildWorkspaceTree({
      hosts,
      groups: [],
      projects: [],
      selectedEndpoint: "local",
    });
    const localAllRows = localTree.flatMap((h) => [...h.rows, ...h.groups.flatMap((g) => g.rows)]);
    const localActiveRows = localAllRows.filter((r) => r.active);
    expect(localActiveRows).toHaveLength(1);
    expect(localActiveRows[0]!.name).toBe("herdr");
    expect(localActiveRows[0]!.endpoint).toBe("local");

    const sshRowWhenLocal = localTree[1]!.rows[0]!;
    expect(sshRowWhenLocal.name).toBe("~");
    expect(sshRowWhenLocal.active).toBe(false);

    // selectedEndpoint = ssh-dev -> only ~ is active; herdr is not active
    const sshTree = buildWorkspaceTree({
      hosts,
      groups: [],
      projects: [],
      selectedEndpoint: "ssh-dev",
    });
    const sshAllRows = sshTree.flatMap((h) => [...h.rows, ...h.groups.flatMap((g) => g.rows)]);
    const sshActiveRows = sshAllRows.filter((r) => r.active);
    expect(sshActiveRows).toHaveLength(1);
    expect(sshActiveRows[0]!.name).toBe("~");
    expect(sshActiveRows[0]!.endpoint).toBe("ssh-dev");

    const localRowWhenSsh = sshTree[0]!.rows.find((r) => r.name === "herdr")!;
    expect(localRowWhenSsh.active).toBe(false);
  });
});

