// Spec 007 (AC-007-04 frontend seam) — project open, agents and files follow the one selection.
import { describe, expect, it } from "vitest";
import type { AgentsBridge } from "../agents/bridge";
import { createAgentsController } from "../agents/controller";
import type { AgentsEvent, Overview, QualifiedTarget } from "../agents/types";
import type { ConnectionsBridge } from "../connections/bridge";
import type { ConnectionsView, HostDto } from "../connections/types";
import type { ProjectsBridge } from "../projects/bridge";
import { createProjectsController } from "../projects/controller";
import type { OpenResponse, ProjectDto, ProjectsSnapshot } from "../projects/types";
import { createTargetRegistry, ensureHostOnline, selectingProjectsBridge, selectionScopedAgentsBridge } from "./routing";

const project = (id: string, endpoint: string, root: string): ProjectDto => ({
  id,
  label: id,
  endpoint_profile_id: endpoint,
  session_name: `s-${endpoint}`,
  root,
  binding: null,
});
const SNAPSHOT: ProjectsSnapshot = {
  version: 1,
  projects: [project("p-local", "local", "/srv/a"), project("p-ssh", "ssh-dev", "/srv/a")],
  collections: [{ id: "c1", name: "Todos", project_ids: ["p-local", "p-ssh"] }],
};

function projectsBase(log: string[]): ProjectsBridge {
  const snapshot = async () => SNAPSHOT;
  return {
    list: async () => {
      log.push("projects_list");
      return SNAPSHOT;
    },
    createProject: snapshot,
    createCollection: snapshot,
    addToCollection: snapshot,
    removeFromCollection: snapshot,
    moveProject: snapshot,
    moveCollection: snapshot,
    groupCreate: snapshot,
    groupAssign: snapshot,
    workspacePrefSet: snapshot,
    groupRename: snapshot,
    groupSetColor: snapshot,
    groupSetCollapsed: snapshot,
    groupDelete: snapshot,
    recentFolderAdd: snapshot,
    workspaceFocus: async () => {},
    workspaceCreate: async () => ({ workspace_id: "w-new" }),
    workspaceRename: async () => {},
    workspaceClose: async () => {},
    open: async (projectId) => {
      log.push(`project_open:${projectId}`);
      const response: OpenResponse = {
        result: {
          project_id: projectId,
          binding: { project_id: projectId, connection_generation: 7, boot_id: "boot-ssh", workspace_id: "w1" },
          outcome: "created",
          invalidated: null,
        },
        snapshot: SNAPSHOT,
      };
      return response;
    },
  };
}

function host(endpoint: string, phase: HostDto["phase"]): HostDto {
  return {
    endpoint,
    label: endpoint,
    kind: endpoint === "local" ? "local" : "ssh",
    session: `s-${endpoint}`,
    target: null,
    visible: false,
    phase,
    phase_label: phase,
    attempt: 0,
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
  };
}

/**
 * Scripted hub: `list` returns the first snapshot, `connect` the next one, and each
 * `watch(revision)` the following one (revision + 1); with no snapshot left, watch never resolves.
 */
function connectionsBase(log: string[], ...snapshots: HostDto[][]): Pick<ConnectionsBridge, "list" | "connect" | "watch"> {
  let revision = 1;
  let index = 0;
  const view = (): ConnectionsView => ({ hub: { revision, hosts: snapshots[Math.min(index, snapshots.length - 1)]! }, profiles: [], store_error: null });
  const advance = () => {
    index += 1;
    revision += 1;
    return view();
  };
  return {
    list: async () => {
      log.push("connections_list");
      return view();
    },
    connect: async (endpoint) => {
      log.push(`connection_connect:${endpoint}`);
      return index + 1 < snapshots.length ? advance() : view();
    },
    watch: (since) => {
      log.push(`connections_watch:${since}`);
      if (index + 1 >= snapshots.length) return new Promise(() => {});
      return new Promise((resolve) => setTimeout(() => resolve(advance()), 5));
    },
  };
}

const failed = (h: HostDto, patch: Partial<HostDto>): HostDto => ({ ...h, ...patch });

describe("project open follows an explicit host selection", () => {
  // Would catch: project_open reaching the engine before the project's own endpoint is
  // connected and selected (the 002 handler opened everything on Local).
  it("connects and selects the project's endpoint before project_open", async () => {
    const log: string[] = [];
    const connections = connectionsBase(
      log,
      [host("local", "online"), host("ssh-dev", "offline")],
      [host("local", "online"), host("ssh-dev", "online")],
    );
    const bridge = selectingProjectsBridge(projectsBase(log), {
      ensureHost: (endpoint) => ensureHostOnline(connections, endpoint),
      select: async (endpoint) => {
        log.push(`select:${endpoint}`);
      },
    });
    await bridge.list();
    await bridge.open("p-ssh");
    expect(log).toEqual([
      "projects_list",
      "connections_list",
      "connection_connect:ssh-dev",
      "select:ssh-dev",
      "project_open:p-ssh",
    ]);
  });

  // Would catch: an already online host being reconnected (a second connection attempt).
  it("does not reconnect an online host", async () => {
    const log: string[] = [];
    const connections = connectionsBase(log, [host("local", "online")]);
    await ensureHostOnline(connections, "local");
    expect(log).toEqual(["connections_list"]);
  });

  // Would catch: an unknown endpoint silently opened on Local, or its error shown globally
  // instead of on the project row.
  it("keeps the failure on the project and never reaches project_open or another host", async () => {
    const log: string[] = [];
    const connections = connectionsBase(log, [host("local", "online")]);
    const bridge = selectingProjectsBridge(projectsBase(log), {
      ensureHost: (endpoint) => ensureHostOnline(connections, endpoint),
      select: async (endpoint) => {
        log.push(`select:${endpoint}`);
      },
    });
    const controller = createProjectsController(bridge);
    await controller.load();
    await controller.open("p-ssh");
    expect(log).toEqual(["projects_list", "connections_list"]);
    expect(controller.state.projectErrors["p-ssh"]?.code).toBe("unknown_endpoint");
    expect(controller.state.globalError).toBeNull();
    expect(controller.state.projectErrors["p-local"]).toBeUndefined();
  });

  // Would catch: project_open sent while the SSH host is still connecting (backend connect is
  // asynchronous), which made the user repeat the open.
  it("waits for a slow host to come online before selecting and opening", async () => {
    const log: string[] = [];
    const connections = connectionsBase(
      log,
      [host("ssh-dev", "offline")],
      [host("ssh-dev", "connecting")],
      [host("ssh-dev", "connecting")],
      [host("ssh-dev", "online")],
    );
    let releaseSelect!: () => void;
    const bridge = selectingProjectsBridge(projectsBase(log), {
      ensureHost: (endpoint) => ensureHostOnline(connections, endpoint, { timeoutMs: 1000 }),
      select: (endpoint) =>
        new Promise<void>((resolve) => {
          log.push(`select:${endpoint}`);
          releaseSelect = resolve;
        }),
    });
    const opening = bridge.open("p-ssh");
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(log).toEqual([
      "projects_list",
      "connections_list",
      "connection_connect:ssh-dev",
      "connections_watch:2",
      "connections_watch:3",
      "select:ssh-dev",
    ]);
    releaseSelect();
    await opening;
    expect(log.at(-1)).toBe("project_open:p-ssh");
    expect(log.filter((l) => l.startsWith("connection_connect"))).toHaveLength(1);
  });

  // Would catch: waiting forever (or opening anyway) when the host needs attention, fails and
  // retries, or is cancelled — and any automatic second connect or fallback.
  it("refuses with the host's own error when it does not come online", async () => {
    const attention = failed(host("ssh-dev", "attention"), {
      attention: "host_key_unknown",
      guidance: "confirme a chave do host fora do desktop",
    });
    const retrying = failed(host("ssh-dev", "reconnecting"), {
      connection_error: { code: "ssh_unavailable", message: "ssh falhou", retryable: true, endpoint: "ssh-dev" },
    });
    const cancelled = failed(host("ssh-dev", "offline"), { cancelled: true });
    for (const [end, code] of [
      [attention, "host_key_unknown"],
      [retrying, "ssh_unavailable"],
      [cancelled, "connection_cancelled"],
    ] as const) {
      const log: string[] = [];
      const connections = connectionsBase(log, [host("ssh-dev", "offline")], [host("ssh-dev", "connecting")], [end]);
      const bridge = selectingProjectsBridge(projectsBase(log), {
        ensureHost: (endpoint) => ensureHostOnline(connections, endpoint, { timeoutMs: 1000 }),
        select: async (endpoint) => void log.push(`select:${endpoint}`),
      });
      await expect(bridge.open("p-ssh")).rejects.toMatchObject({ code, endpoint: "ssh-dev" });
      expect(log.filter((l) => l.startsWith("connection_connect"))).toHaveLength(1);
      expect(log.some((l) => l.startsWith("select:") || l.startsWith("project_open"))).toBe(false);
    }
  });

  // Would catch: an unbounded wait, or a cancelled open still selecting/opening afterwards.
  it("stops waiting on timeout or cancellation", async () => {
    const log: string[] = [];
    const stuck = connectionsBase(log, [host("ssh-dev", "offline")], [host("ssh-dev", "connecting")]);
    await expect(ensureHostOnline(stuck, "ssh-dev", { timeoutMs: 20 })).rejects.toMatchObject({ code: "host_connect_timeout" });

    const abort = new AbortController();
    const waiting = ensureHostOnline(
      connectionsBase([], [host("ssh-dev", "offline")], [host("ssh-dev", "connecting")]),
      "ssh-dev",
      { timeoutMs: 1000, signal: abort.signal },
    );
    abort.abort();
    await expect(waiting).rejects.toMatchObject({ code: "open_cancelled" });
  });

  // Would catch: a selection refused by the backend still letting project_open run.
  it("stops when the selection is refused", async () => {
    const log: string[] = [];
    const bridge = selectingProjectsBridge(projectsBase(log), {
      ensureHost: async () => {},
      select: async () => {
        throw { code: "selection_changed", message: "outro host", retryable: false };
      },
    });
    await expect(bridge.open("p-local")).rejects.toMatchObject({ code: "selection_changed" });
    expect(log).toEqual(["projects_list"]);
  });

  // Would catch: the navigator not reporting which project/root became active.
  it("reports the opened project with its response", async () => {
    const log: string[] = [];
    const opened: string[] = [];
    const bridge = selectingProjectsBridge(projectsBase(log), {
      ensureHost: async () => {},
      select: async () => {},
      onOpened: (p, response) => opened.push(`${p.endpoint_profile_id}:${p.root}:${response.result.binding.workspace_id}`),
    });
    await bridge.open("p-ssh");
    expect(opened).toEqual(["ssh-dev:/srv/a:w1"]);
  });
});

const overview = (endpoint: string, boot: string): Overview => ({
  state: "connected",
  session: `s-${endpoint}`,
  identity: { endpoint, session: `s-${endpoint}`, connection_generation: 7, boot_id: boot },
  server_version: "0.9.0",
  capabilities: {
    list_agents: true,
    start_agent: true,
    send_prompt: true,
    open_attention: true,
    split: true,
    focus: true,
    split_ratio: true,
    input: true,
    create_tab: true,
    focus_tab: true,
  },
  kinds: ["shell"],
  agents: [],
  tabs: [],
  topology: {
    revision: 1,
    width: 80,
    height: 24,
    focused_pane_id: "w1:p1",
    panes: [{ pane_id: "w1:p1", x: 0, y: 0, width: 80, height: 24, focused: true }],
    splits: [],
  },
  error: null,
});

function agentsBase(selected: { endpoint: string }) {
  const actions: QualifiedTarget[] = [];
  const handlers: ((event: AgentsEvent) => void)[] = [];
  const bridge: AgentsBridge = {
    connect: async (_geometry, onEvent) => {
      handlers.push(onEvent);
      return overview(selected.endpoint, `boot-${selected.endpoint}`);
    },
    overview: async () => overview(selected.endpoint, `boot-${selected.endpoint}`),
    detach: async () => overview(selected.endpoint, `boot-${selected.endpoint}`),
    startAgent: async (target) => {
      actions.push(target);
      throw { code: "unused", message: "", retryable: false };
    },
    prompt: async (target) => {
      actions.push(target);
      throw { code: "unused", message: "", retryable: false };
    },
    openAttention: async (target) => void actions.push(target),
    split: async (target) => void actions.push(target),
    focusPane: async (target) => void actions.push(target),
    setSplitRatio: async (target) => void actions.push(target),
    input: async (target) => void actions.push(target),
    createTab: async (target) => void actions.push(target),
    focusTab: async (target) => void actions.push(target),
    zoomPane: async () => {},
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  };
  return { bridge, actions, handlers };
}

describe("agents follow the selected host", () => {
  // Would catch: a split issued with the Local identity reaching the backend after SSH was
  // selected, or a split on SSH being sent twice.
  it("refuses actions addressed to a host that is no longer selected and sends the rest once", async () => {
    const selected = { endpoint: "local" };
    const base = agentsBase(selected);
    const controller = createAgentsController(selectionScopedAgentsBridge(base.bridge, () => selected.endpoint));
    const geometry = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };
    await controller.connect(geometry);

    selected.endpoint = "ssh-dev";
    await controller.split("right");
    expect(base.actions).toEqual([]);
    expect(controller.state.layoutError?.code).toBe("selection_changed");

    await controller.connect(geometry);
    await controller.split("right");
    expect(base.actions.map((t) => [t.endpoint, t.boot_id, t.pane_id])).toEqual([["ssh-dev", "boot-ssh-dev", "w1:p1"]]);
  });

  // Would catch: events of the previous host's agents channel updating the new host's panel.
  it("drops events delivered by a previous agents channel", async () => {
    const selected = { endpoint: "local" };
    const base = agentsBase(selected);
    const controller = createAgentsController(selectionScopedAgentsBridge(base.bridge, () => selected.endpoint));
    const geometry = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };
    await controller.connect(geometry);
    selected.endpoint = "ssh-dev";
    await controller.connect(geometry);
    base.handlers[0]!({ type: "identity", identity: { endpoint: "local", session: "s-local", connection_generation: 3, boot_id: "boot-local" } });
    expect(controller.state.identity?.endpoint).toBe("ssh-dev");
    base.handlers[1]!({ type: "identity", identity: { endpoint: "ssh-dev", session: "s-ssh-dev", connection_generation: 8, boot_id: "boot-ssh-2" } });
    expect(controller.state.identity?.boot_id).toBe("boot-ssh-2");
  });
});

describe("per-target state", () => {
  // Would catch: switching projects discarding the previous target's controller (and its dirty
  // buffers) or sharing one controller between different roots/hosts.
  it("keeps one instance per provider/host/root", () => {
    let created = 0;
    const registry = createTargetRegistry(() => ({ id: ++created }));
    const a = registry.get({ provider: "local", host: null, root: "/srv/a" });
    const b = registry.get({ provider: "sftp", host: "ssh-dev", root: "/srv/a" });
    expect(a).not.toBe(b);
    expect(registry.get({ provider: "local", host: null, root: "/srv/a" })).toBe(a);
    expect(created).toBe(2);
  });

  // Would catch: the render path creating controllers (and writing their state) while
  // evaluating the template, which Svelte rejects with state_unsafe_mutation.
  it("peeks without creating", () => {
    let created = 0;
    const registry = createTargetRegistry(() => ({ id: ++created }));
    const target = { provider: "local", host: null, root: "/srv/b" };
    expect(registry.peek(target)).toBeUndefined();
    expect(created).toBe(0);
    const item = registry.get(target);
    expect(registry.peek(target)).toBe(item);
    expect(created).toBe(1);
  });
});
