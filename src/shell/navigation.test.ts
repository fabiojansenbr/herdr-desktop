// Spec 007 — navigation checkpoint: actions that need the terminal surface (focus/split/tabs and
// project open) issued while Files is shown bring the terminal back first and are sent once, to
// the target captured when the user acted, only after the backend acknowledged the interest AND a
// full frame of the current attach arrived. Agents start/prompt (JSON API) stay hidden. Seams: the
// App's product objects — tauriSurfaceBridge + createSurfaceController, createTerminalReveal,
// selectionScopedAgentsBridge + createAgentsController, selectingProjectsBridge.
// Local and SSH both expose "w1:p1" and differ in boot/generation/session.
import { describe, expect, it } from "vitest";
import type { AgentsBridge } from "../agents/bridge";
import { createAgentsController } from "../agents/controller";
import type { Overview, QualifiedTarget } from "../agents/types";
import type { ProjectsBridge } from "../projects/bridge";
import type { OpenResponse, ProjectDto, ProjectsSnapshot } from "../projects/types";
import { tauriSurfaceBridge, type SurfaceIpc } from "./bridge";
import { createSurfaceController, inputAllowed } from "./controller";
import { createTerminalReveal, selectingProjectsBridge, selectionScopedAgentsBridge } from "./routing";
import type { FrameEvent, SelectionDto, StatusDto } from "./types";

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(error: unknown): void;
}
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
const flush = async () => {
  for (let i = 0; i < 8; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const BOOT: Record<string, { boot: string; generation: number; session: string }> = {
  local: { boot: "boot-local-5", generation: 3, session: "s-local-hd007n" },
  "ssh-dev": { boot: "boot-ssh-8", generation: 4, session: "s-remote-hd007n" },
};
const HOSTS: Record<string, SelectionDto> = {
  local: { endpoint: "local", kind: "local", label: "Local", session: BOOT.local!.session, online: true, identity: null },
  "ssh-dev": { endpoint: "ssh-dev", kind: "ssh", label: "dev-box", session: BOOT["ssh-dev"]!.session, online: true, identity: null },
};
const status = (session: string): StatusDto => ({
  session,
  session_available: true,
  connected: true,
  state: "connecting",
  reason: null,
  generation: null,
  connection_generation: 0,
  boot_id: null,
  server_version: null,
  pane_id: null,
  last_error: null,
});
const identity = (endpoint: string): FrameEvent => ({
  type: "identity",
  boot_id: BOOT[endpoint]!.boot,
  generation: 1,
  connection_generation: BOOT[endpoint]!.generation,
  server_version: "",
  pane_id: "w1:p1",
});
const live: FrameEvent = { type: "state", state: "live", reason: null, error: null };
const full: FrameEvent = { type: "full", revision: 9, width: 2, height: 1, cells: [], cursor: null, panes: [] };

/** One ordered log of everything the backend received (surface IPC, agents and projects). */
function fakeWindow(options: { interestHeld?: boolean } = {}) {
  const log: string[] = [];
  const interest: Deferred<unknown>[] = [];
  const channels: ((event: FrameEvent) => void)[] = [];
  let selected: string | null = null;
  const ipc: SurfaceIpc = {
    channel: (onEvent) => ({ emit: onEvent }),
    invoke: (async (command: string, args: Record<string, unknown> = {}) => {
      switch (command) {
        case "selection_set":
          selected = args.endpoint as string;
          log.push(`selection_set:${selected}`);
          return HOSTS[selected];
        case "surface_attach":
          log.push(`surface_attach:${selected}`);
          channels.push((args.onEvent as { emit: (event: FrameEvent) => void }).emit);
          return status(HOSTS[selected ?? ""]?.session ?? "none");
        case "surface_status":
          return status("none");
        case "surface_interest": {
          log.push(`surface_interest:${selected}:${args.active}`);
          const reply = deferred<unknown>();
          interest.push(reply);
          if (!options.interestHeld) reply.resolve(null);
          return reply.promise;
        }
        default:
          return undefined;
      }
    }) as SurfaceIpc["invoke"],
  };
  const surface = createSurfaceController(tauriSurfaceBridge(ipc));
  const layers = { filesShown: false, documentVisible: true };
  const reveal = createTerminalReveal({
    surface,
    showTerminal: () => {
      log.push("show_terminal");
      layers.filesShown = false;
    },
    documentVisible: () => layers.documentVisible,
    timeoutMs: 200,
  });

  const overview = (endpoint: string): Overview => ({
    state: "live",
    session: BOOT[endpoint]!.session,
    identity: {
      endpoint,
      session: BOOT[endpoint]!.session,
      connection_generation: BOOT[endpoint]!.generation,
      boot_id: BOOT[endpoint]!.boot,
    },
    server_version: "",
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
    agents: [{ pane_id: "w1:p1", workspace_id: "w1", tab_id: "w1:t1", name: "a", kind: "shell", status: "idle", launch_pending: false, ready: true, focused: true }],
    tabs: [],
    topology: {
      revision: 1,
      width: 80,
      height: 24,
      focused_pane_id: "w1:p1",
      panes: [
        { pane_id: "w1:p1", x: 0, y: 0, width: 40, height: 24, focused: true },
        { pane_id: "w1:p2", x: 41, y: 0, width: 39, height: 24, focused: false },
      ],
      splits: [],
    },
    error: null,
  });
  const record = (name: string) => async (target: QualifiedTarget) =>
    void log.push(`${name}:${target.endpoint}:${target.boot_id}:${target.pane_id}`);
  const agentsBase: AgentsBridge = {
    connect: async () => overview(selected ?? "local"),
    overview: async () => overview(selected ?? "local"),
    detach: async () => overview(selected ?? "local"),
    startAgent: async (target, kind, name) => {
      log.push(`agent_start:${target.endpoint}:${target.pane_id}`);
      return { ...overview(target.endpoint).agents[0]!, pane_id: target.pane_id, kind, name };
    },
    prompt: async (target) => {
      log.push(`agent_prompt:${target.endpoint}:${target.pane_id}`);
      return { outcome: "sent", agent: overview(target.endpoint).agents[0]! };
    },
    openAttention: record("agent_open_attention"),
    split: record("pane_split"),
    focusPane: record("pane_focus"),
    setSplitRatio: record("pane_set_split_ratio"),
    input: record("pane_input"),
    createTab: record("tab_create"),
    focusTab: record("tab_focus"),
    zoomPane: async () => {},
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  };
  const agents = createAgentsController(
    selectionScopedAgentsBridge(agentsBase, () => surface.state.selection?.endpoint ?? null, { reveal: reveal.reveal }),
  );

  /** Selects and makes `endpoint` live (identity + full + live), agents attached to it. */
  const liveOn = async (endpoint: string) => {
    await surface.select(endpoint);
    const emit = channels.at(-1)!;
    emit(identity(endpoint));
    emit(full);
    emit(live);
    agents.invalidate();
    await agents.connect({ cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 });
    return emit;
  };
  /** Files layer shown (App: filesShown → setInterest(false)). */
  const showFiles = async () => {
    layers.filesShown = true;
    await surface.setInterest(false);
    log.length = 0;
  };
  const sent = (prefix: string) => log.filter((entry) => entry.startsWith(prefix));
  return { log, interest, channels, surface, reveal, agents, layers, liveOn, showFiles, sent };
}

const project = (id: string, endpoint: string): ProjectDto => ({
  id,
  label: id,
  endpoint_profile_id: endpoint,
  session_name: `s-${endpoint}`,
  root: "/srv/nav",
  binding: null,
});
const SNAPSHOT: ProjectsSnapshot = {
  version: 1,
  projects: [project("p-local", "local"), project("p-ssh", "ssh-dev")],
  collections: [],
};
function projectsBase(log: string[]): ProjectsBridge {
  const snapshot = async () => SNAPSHOT;
  return {
    list: snapshot,
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
          binding: { project_id: projectId, connection_generation: 4, boot_id: "boot-ssh-8", workspace_id: "w2" },
          outcome: "created",
          invalidated: null,
        },
        snapshot: SNAPSHOT,
      };
      return response;
    },
  };
}

describe("navigation from Files waits for the terminal surface (spec 007)", () => {
  // Would catch: pane_split sent while the endpoint still has surface_interest false (engine
  // answers surface_inactive), sent before the ack, before the full frame, or sent twice.
  it("split from Files shows the terminal, waits for ack and full, then sends once to the captured target", async () => {
    const w = fakeWindow({ interestHeld: true });
    const emit = await w.liveOn("ssh-dev");
    void w.surface.setInterest(false);
    await flush();
    w.interest.at(-1)!.resolve(null);
    await flush();
    w.layers.filesShown = true;
    w.log.length = 0;

    const split = w.agents.split("right");
    await flush();
    expect(w.log).toEqual(["show_terminal", "surface_interest:ssh-dev:true"]);
    expect(w.layers.filesShown).toBe(false);

    w.interest.at(-1)!.resolve(null);
    await flush();
    expect(w.sent("pane_split")).toEqual([]);
    expect(w.surface.canInput()).toBe(false);

    emit(full);
    await split;
    expect(w.sent("pane_split")).toEqual(["pane_split:ssh-dev:boot-ssh-8:w1:p1"]);
    expect(w.agents.state.layoutError).toBeNull();
    await flush();
    expect(w.sent("pane_split")).toHaveLength(1);
  });

  // Would catch: the action going out although the backend never acknowledged the interest, or
  // an automatic retry of the mutation after the failure.
  it("without an ack the action fails with the true error and is never sent", async () => {
    const w = fakeWindow({ interestHeld: true });
    await w.liveOn("local");
    void w.surface.setInterest(false);
    await flush();
    w.interest.at(-1)!.resolve(null);
    await flush();
    w.log.length = 0;

    const focus = w.agents.focusPane("w1:p2");
    await flush();
    w.interest.at(-1)!.reject({ code: "surface_interest_failed", message: "ack recusado", retryable: false });
    await focus;
    expect(w.agents.state.layoutError?.code).toBe("surface_interest_failed");
    expect(w.sent("pane_focus")).toEqual([]);
    await flush();
    expect(w.log.filter((entry) => entry.startsWith("surface_interest"))).toHaveLength(1);
  });

  // Would catch: sending after the ack alone (no full frame of the current attach) instead of
  // failing bounded.
  it("with an ack but no full frame the action times out unsent", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();

    await w.agents.createTab();
    expect(w.sent("surface_interest")).toEqual(["surface_interest:local:true"]);
    expect(w.agents.state.layoutError?.code).toBe("surface_timeout");
    expect(w.sent("tab_create")).toEqual([]);
  });

  // Would catch: a split captured on Local being re-addressed to SSH (same pane id w1:p1) or sent
  // to Local after the user switched hosts during the wait.
  it("a host switch during the wait cancels the action with no Local fallback", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();

    const split = w.agents.split("down");
    await flush();
    const switched = w.surface.select("ssh-dev");
    await split;
    await switched;
    const emit = w.channels.at(-1)!;
    emit(identity("ssh-dev"));
    emit(full);
    emit(live);
    await flush();
    expect(w.agents.state.layoutError?.code).toBe("selection_changed");
    expect(w.sent("pane_split")).toEqual([]);
    expect(w.sent("surface_interest")).toEqual(["surface_interest:local:true"]);
  });

  // Would catch: the reveal waiter not rejecting when the target's connection is not the one the
  // surface confirmed — agents attached to an older generation of the same boot (reconnect), or to
  // an older boot with the same generation (reboot).
  it("refuses a target whose boot or generation differs from the confirmed surface", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    const bridge = selectionScopedAgentsBridge(
      {
        split: async (target: QualifiedTarget) => void w.log.push(`pane_split:${target.boot_id}:${target.connection_generation}`),
      } as unknown as AgentsBridge,
      () => w.surface.state.selection?.endpoint ?? null,
      { reveal: w.reveal.reveal },
    );
    const target = (boot: string, generation: number): QualifiedTarget => ({
      endpoint: "local",
      session: BOOT.local!.session,
      connection_generation: generation,
      boot_id: boot,
      pane_id: "w1:p1",
    });
    for (const stale of [target("boot-local-5", 2), target("boot-local-4", 3)]) {
      await w.showFiles();
      const result = bridge.split(stale, "right");
      await flush();
      w.channels.at(-1)!(full);
      await expect(result).rejects.toMatchObject({ code: "target_stale" });
    }
    expect(w.sent("pane_split")).toEqual([]);
    // Positive control: the confirmed boot/generation goes out once.
    await w.showFiles();
    const current = bridge.split(target("boot-local-5", 3), "right");
    await flush();
    w.channels.at(-1)!(full);
    await current;
    expect(w.sent("pane_split")).toEqual(["pane_split:boot-local-5:3"]);
  });

  // Would catch: faking document visibility to push an action while the window is hidden.
  it("with the document hidden nothing is revealed or sent", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();
    w.layers.documentVisible = false;

    await w.agents.split("right");
    expect(w.agents.state.layoutError?.code).toBe("surface_hidden");
    expect(w.log).toEqual([]);
    expect(w.layers.filesShown).toBe(true);
  });

  // Would catch: start/prompt (JSON API) needlessly bringing the terminal back or waiting for it.
  it("agents start and prompt stay available with Files shown and do not reveal the terminal", async () => {
    const w = fakeWindow({ interestHeld: true });
    await w.liveOn("local");
    void w.surface.setInterest(false);
    await flush();
    w.interest.at(-1)!.resolve(null);
    await flush();
    w.layers.filesShown = true;
    w.log.length = 0;

    w.agents.editStart("paneId", "w1:p2");
    w.agents.editStart("name", "revisor");
    await w.agents.startAgent();
    w.agents.editPrompt("w1:p1", "olá");
    await w.agents.sendPrompt("w1:p1");
    expect(w.log).toEqual(["agent_start:local:w1:p2", "agent_prompt:local:w1:p1"]);
    expect(w.layers.filesShown).toBe(true);
  });

  // Would catch: TerminalView input enabled from inputAllowed(state) alone while the shown surface
  // still waits for its full frame, and the state not changing when the gate opens.
  it("the input gate the view uses includes the interest and opens with a state change", async () => {
    const w = fakeWindow();
    const emit = await w.liveOn("local");
    await w.showFiles();
    const reveal = w.reveal.reveal("local");
    await flush();
    const before = w.surface.state;
    expect(inputAllowed(before)).toBe(true);
    expect(w.surface.canInput()).toBe(false);
    emit(full);
    const confirmed = await reveal;
    expect(confirmed).toMatchObject({ endpoint: "local", boot_id: "boot-local-5", connection_generation: 3, pane_id: "w1:p1" });
    expect(w.surface.canInput()).toBe(true);
    expect(w.surface.state).not.toBe(before);
  });
});

describe("project open from Files reactivates the selected host first (spec 007)", () => {
  // Would catch: project_open sent with the surface still inactive (surface_inactive), or before
  // the ack + full of the project's own host.
  it("same host: interest true, ack and full precede project_open", async () => {
    const w = fakeWindow();
    const emit = await w.liveOn("ssh-dev");
    await w.showFiles();
    const bridge = selectingProjectsBridge(projectsBase(w.log), {
      ensureHost: async () => void w.log.push("ensure_host"),
      select: (endpoint) => w.reveal.selectAndReveal(endpoint).then(() => undefined),
    });
    await bridge.list();
    const opened = bridge.open("p-ssh");
    await flush();
    expect(w.sent("project_open")).toEqual([]);
    emit(full);
    await opened;
    expect(w.log).toEqual(["ensure_host", "show_terminal", "surface_interest:ssh-dev:true", "project_open:p-ssh"]);
  });

  // Would catch: the whenReady deadlock — selecting another host while hidden attaches without
  // presentation and never becomes live — and project_open reaching the previous host.
  it("other host: selects it, attaches shown and opens once after its full frame", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();
    const bridge = selectingProjectsBridge(projectsBase(w.log), {
      ensureHost: async () => undefined,
      select: (endpoint) => w.reveal.selectAndReveal(endpoint).then(() => undefined),
    });
    await bridge.list();
    const opened = bridge.open("p-ssh");
    await flush();
    // Shown before the attach: the new host is attached presenting (its backend interest was never
    // lowered), and the hidden Local host is not re-shown on the way.
    expect(w.log).toEqual(["show_terminal", "selection_set:ssh-dev", "surface_attach:ssh-dev"]);
    const emit = w.channels.at(-1)!;
    emit(identity("ssh-dev"));
    emit(full);
    emit(live);
    await opened;
    expect(w.log).toEqual(["show_terminal", "selection_set:ssh-dev", "surface_attach:ssh-dev", "project_open:p-ssh"]);
  });

  // Would catch: a project open continuing on the newly clicked host after the user switched.
  it("a host switch during the wait fails the open without project_open", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();
    const bridge = selectingProjectsBridge(projectsBase(w.log), {
      ensureHost: async () => undefined,
      select: (endpoint) => w.reveal.selectAndReveal(endpoint).then(() => undefined),
    });
    await bridge.list();
    const opened = bridge.open("p-local");
    await flush();
    await w.surface.select("ssh-dev").catch(() => undefined);
    await expect(opened).rejects.toMatchObject({ code: "selection_changed" });
    expect(w.sent("project_open")).toEqual([]);
  });

  // Would catch: a hidden document faked visible to open the project.
  it("with the document hidden the open is refused before selecting", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();
    w.layers.documentVisible = false;
    const bridge = selectingProjectsBridge(projectsBase(w.log), {
      ensureHost: async () => undefined,
      select: (endpoint) => w.reveal.selectAndReveal(endpoint).then(() => undefined),
    });
    await bridge.list();
    await expect(bridge.open("p-ssh")).rejects.toMatchObject({ code: "surface_hidden" });
    expect(w.log).toEqual([]);
  });
});

// R1 — readiness split: a project open needs the presented surface of the right connection (live
// identity frame + interest ack + full), not a focused pane; input/split/focus still need the pane
// the server confirmed. An empty session (no focused pane yet) must not stall the first project.
const identityWithoutPane = (endpoint: string): FrameEvent => ({ ...identity(endpoint), pane_id: null }) as FrameEvent;
const paneIdentity = (endpoint: string, pane: string): FrameEvent => ({ ...identity(endpoint), pane_id: pane }) as FrameEvent;

describe("first project in an empty session (spec 007 navigation R1)", () => {
  // Would catch: selectAndReveal waiting on whenInteractive (focused pane) so the first project of a
  // session without panes times out unsent, or opening before the ack + full of that host, or twice;
  // and — the other side — the presentation gate opening input/split without a confirmed pane.
  it.each([
    ["local", "local"],
    ["ssh-dev", "ssh-dev"],
    ["local", "ssh-dev"],
  ])("from %s with Files shown, the project on %s opens once after ack + full; input waits for a real pane", async (from, to) => {
    const w = fakeWindow({ interestHeld: true });
    const projectId = to === "local" ? "p-local" : "p-ssh";
    let emit: (event: FrameEvent) => void;
    if (from === to) {
      const selecting = w.surface.select(to);
      await flush();
      w.interest.forEach((reply) => reply.resolve(null));
      await selecting;
      emit = w.channels.at(-1)!;
      emit(identityWithoutPane(to));
      emit(full);
      emit(live);
    } else {
      emit = await w.liveOn(from);
    }
    void w.surface.setInterest(false);
    await flush();
    w.interest.at(-1)!.resolve(null);
    await flush();
    w.layers.filesShown = true;
    w.log.length = 0;

    const bridge = selectingProjectsBridge(projectsBase(w.log), {
      ensureHost: async () => undefined,
      select: (endpoint) => w.reveal.selectAndReveal(endpoint).then(() => undefined),
    });
    await bridge.list();
    const opened = bridge.open(projectId);
    await flush();
    if (from !== to) {
      emit = w.channels.at(-1)!;
      emit(identityWithoutPane(to));
      emit(live);
    }
    w.interest.at(-1)?.resolve(null);
    await flush();
    expect(w.sent("project_open")).toEqual([]);
    emit(full);
    await opened;
    await flush();
    expect(w.sent("project_open")).toEqual([`project_open:${projectId}`]);
    expect(w.layers.filesShown).toBe(false);

    // No pane confirmed: input and pane actions stay blocked, nothing faked.
    expect(w.surface.state.identity).toBeNull();
    expect(w.surface.canInput()).toBe(false);
    await expect(w.surface.input([{ type: "text", text: "x" } as never])).resolves.toBe(false);
    let revealed: unknown = null;
    const reveal = w.reveal.reveal(to).then((value) => (revealed = value));
    await flush();
    expect(revealed).toBeNull();

    // The server confirms the project's pane: input opens, once, for that pane.
    emit(paneIdentity(to, "w2:p1"));
    await reveal;
    expect(revealed).toMatchObject({ endpoint: to, boot_id: BOOT[to]!.boot, pane_id: "w2:p1" });
    expect(w.surface.canInput()).toBe(true);
    expect(w.sent("project_open")).toHaveLength(1);
  });
});

// R1 — every surface wait is fenced by the attach episode captured when the user acted: a quick
// A→B→A switch (B cancelled before completing) reattaches A with the SAME boot, generation and pane
// ids, and the old intent must fail unsent instead of waiting for the new attach.
describe("surface waits are fenced by episode (spec 007 navigation R1)", () => {
  // Would catch: whenInteractive keyed only by endpoint — split captured on the first attach of
  // Local sent on its re-attach after local→ssh-dev→local (identical identity, so target_stale
  // cannot tell them apart).
  it("a split waiting on Local fails unsent after local→ssh-dev→local with identical identity", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();
    const split = w.agents.split("right");
    await flush();
    expect(w.log).toEqual(["show_terminal", "surface_interest:local:true"]);

    const toB = w.surface.select("ssh-dev").catch((error) => error);
    const backToA = w.surface.select("local");
    await backToA;
    expect((await toB).code).toBe("selection_changed");
    const emit = w.channels.at(-1)!;
    emit(identity("local"));
    emit(full);
    emit(live);
    await split;
    await flush();
    expect(w.surface.canInput()).toBe(true);
    expect(w.surface.state.identity).toMatchObject({ endpoint: "local", boot_id: "boot-local-5", connection_generation: 3, pane_id: "w1:p1" });
    expect(w.agents.state.layoutError?.code).toBe("selection_changed");
    expect(w.sent("pane_split")).toEqual([]);
  });

  // Would catch: the same for a project open waiting for the full frame of the selected host.
  it("a project open waiting on Local fails unsent after local→ssh-dev→local", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    await w.showFiles();
    const bridge = selectingProjectsBridge(projectsBase(w.log), {
      ensureHost: async () => undefined,
      select: (endpoint) => w.reveal.selectAndReveal(endpoint).then(() => undefined),
    });
    await bridge.list();
    const opened = bridge.open("p-local");
    opened.catch(() => undefined);
    await flush();
    void w.surface.select("ssh-dev").catch(() => undefined);
    await w.surface.select("local");
    const emit = w.channels.at(-1)!;
    emit(identity("local"));
    emit(full);
    emit(live);
    await expect(opened).rejects.toMatchObject({ code: "selection_changed" });
    await flush();
    expect(w.sent("project_open")).toEqual([]);
  });

  // Would catch: an intent issued after the re-attach being refused (over-fencing): the new episode
  // captures its own epoch and is sent once.
  it("an action issued on the new attach is sent once", async () => {
    const w = fakeWindow();
    await w.liveOn("local");
    void w.surface.select("ssh-dev").catch(() => undefined);
    await w.surface.select("local");
    const emit = w.channels.at(-1)!;
    emit(identity("local"));
    emit(full);
    emit(live);
    await w.showFiles();
    const split = w.agents.split("down");
    await flush();
    emit(full);
    await split;
    expect(w.sent("pane_split")).toEqual(["pane_split:local:boot-local-5:w1:p1"]);
  });

  // Would catch: the scoped bridge holding the caller's target object by reference, so a mutation
  // while the reveal waits re-addresses the action (another pane / boot) at send time.
  it("the target is captured by copy when the user acts", async () => {
    const w = fakeWindow();
    const emit = await w.liveOn("local");
    await w.showFiles();
    const bridge = selectionScopedAgentsBridge(
      {
        split: async (target: QualifiedTarget) => void w.log.push(`pane_split:${target.boot_id}:${target.pane_id}`),
      } as unknown as AgentsBridge,
      () => w.surface.state.selection?.endpoint ?? null,
      { reveal: w.reveal.reveal },
    );
    const target: QualifiedTarget = {
      endpoint: "local",
      session: BOOT.local!.session,
      connection_generation: 3,
      boot_id: "boot-local-5",
      workspace_id: "w1",
      pane_id: "w1:p1",
    };
    const split = bridge.split(target, "right");
    await flush();
    target.pane_id = "w1:p2";
    target.workspace_id = "w9";
    emit(full);
    await split;
    expect(w.sent("pane_split")).toEqual(["pane_split:boot-local-5:w1:p1"]);
  });
});
