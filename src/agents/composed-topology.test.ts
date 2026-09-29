// Spec 007 — navigation checkpoint: the composed agents panel shows the topology/focus/geometry
// the selected connection confirmed (pushed by the backend feed), with no incidental action, and
// never keeps a confirmation after that connection ended. Seam: the App's agents controller over
// selectionScopedAgentsBridge. Local and SSH both expose "w1:p1" and differ in boot/generation.
import { describe, expect, it } from "vitest";
import { selectionScopedAgentsBridge } from "../shell/routing";
import type { AgentsBridge } from "./bridge";
import { createAgentsController } from "./controller";
import { viewModel } from "./reducer";
import type { AgentsEvent, Overview, QualifiedTarget, Topology } from "./types";

const geometry = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };

const single = (pane: string, width: number, height: number): Topology => ({
  revision: 1,
  width,
  height,
  focused_pane_id: pane,
  panes: [{ pane_id: pane, x: 0, y: 0, width, height, focused: true }],
  splits: [],
});

const overview = (endpoint: string, boot: string, generation: number, topology: Topology): Overview => ({
  state: "live",
  session: `s-${endpoint}`,
  identity: { endpoint, session: `s-${endpoint}`, connection_generation: generation, boot_id: boot },
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
  agents: [],
  tabs: [],
  topology,
  error: null,
});

function composedAgents(selected: { endpoint: string; boot: string; generation: number }) {
  const actions: string[] = [];
  const channels: ((event: AgentsEvent) => void)[] = [];
  const record = (name: string) => async (target: QualifiedTarget) => void actions.push(`${name}:${target.endpoint}:${target.pane_id}`);
  const base: AgentsBridge = {
    connect: async (_geometry, onEvent) => {
      channels.push(onEvent);
      return overview(selected.endpoint, selected.boot, selected.generation, single("w1:p1", 80, 24));
    },
    overview: async () => {
      actions.push("overview");
      return overview(selected.endpoint, selected.boot, selected.generation, single("w1:p1", 80, 24));
    },
    detach: async () => overview(selected.endpoint, selected.boot, selected.generation, single("w1:p1", 80, 24)),
    startAgent: async (target) => {
      actions.push(`start:${target.endpoint}`);
      throw { code: "unused", message: "", retryable: false };
    },
    prompt: async (target) => {
      actions.push(`prompt:${target.endpoint}`);
      throw { code: "unused", message: "", retryable: false };
    },
    openAttention: record("attention"),
    split: record("split"),
    focusPane: record("focus"),
    setSplitRatio: record("ratio"),
    input: record("input"),
    createTab: record("tab"),
    focusTab: record("tab_focus"),
    zoomPane: async () => {},
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  };
  const controller = createAgentsController(selectionScopedAgentsBridge(base, () => selected.endpoint));
  return { controller, actions, channels };
}

describe("composed agents topology (spec 007 navigation)", () => {
  // Would catch: the panel keeping the attach-time w1:p1 80x24 after the server confirmed the
  // project's w2:p1 at 120x40 (the native defect), or needing an action/overview to resync.
  it("shows the pushed focus, pane and geometry without any action", async () => {
    const { controller, actions, channels } = composedAgents({ endpoint: "local", boot: "boot-local-5", generation: 3 });
    await controller.connect(geometry);
    expect(viewModel(controller.state).confirmed).toBe("w1:p1");

    channels[0]!({ type: "topology", topology: single("w2:p1", 120, 40) });
    const view = viewModel(controller.state);
    expect(view.confirmed).toBe("w2:p1");
    expect(view.panes.map((p) => [p.paneId, p.cells.width, p.cells.height, p.confirmed])).toEqual([["w2:p1", 120, 40, true]]);
    expect(controller.state.start.paneId).toBe("w2:p1");
    expect(actions).toEqual([]);

    await controller.split("right");
    expect(actions).toEqual(["split:local:w2:p1"]);
  });

  // Would catch: a lost/replaced connection leaving split/tab enabled on its old confirmed pane.
  it("drops the confirmation when the attached connection ends", async () => {
    const { controller, actions, channels } = composedAgents({ endpoint: "ssh-dev", boot: "boot-ssh-8", generation: 4 });
    await controller.connect(geometry);
    channels[0]!({ type: "topology", topology: single("w2:p1", 120, 40) });

    channels[0]!({ type: "state", state: "disconnected", error: { code: "connection_lost", message: "perdida", retryable: true } });
    const view = viewModel(controller.state);
    expect(controller.state.phase).toBe("disconnected");
    expect(view.confirmed).toBeNull();
    expect(view.panes).toEqual([]);
    expect(view.split.enabled).toBe(false);
    expect(view.canCreateTab).toBe(false);

    await controller.split("right");
    await controller.createTab();
    expect(actions).toEqual([]);
  });

  // Would catch: a late topology of the Local channel (same pane id family) landing on the panel
  // attached to SSH after the host switch.
  it("ignores topology of a previous host's channel", async () => {
    const selected = { endpoint: "local", boot: "boot-local-5", generation: 3 };
    const { controller, channels } = composedAgents(selected);
    await controller.connect(geometry);
    Object.assign(selected, { endpoint: "ssh-dev", boot: "boot-ssh-8", generation: 4 });
    controller.invalidate();
    await controller.connect(geometry);

    channels[0]!({ type: "topology", topology: single("w9:p1", 50, 10) });
    expect(viewModel(controller.state).confirmed).toBe("w1:p1");
    channels[1]!({ type: "topology", topology: single("w3:p2", 100, 30) });
    expect(viewModel(controller.state).confirmed).toBe("w3:p2");
    expect(controller.state.identity?.endpoint).toBe("ssh-dev");
  });
});

// R1 — an agents channel that ended (disconnected, or replaced by a newer connect) never confirms
// again: neither its late overview nor its later events. Only a new explicit connect does.
function pendingAgents() {
  const actions: string[] = [];
  const attaches: { emit: (event: AgentsEvent) => void; reply: (overview: Overview) => void; fail: (error: unknown) => void }[] = [];
  const record = (name: string) => async (target: QualifiedTarget) => void actions.push(`${name}:${target.endpoint}:${target.pane_id}`);
  const unused = async () => {
    throw { code: "unused", message: "", retryable: false };
  };
  const base: AgentsBridge = {
    connect: (_geometry, onEvent) =>
      new Promise<Overview>((reply, fail) => void attaches.push({ emit: onEvent, reply, fail })),
    overview: unused,
    detach: unused,
    startAgent: unused,
    prompt: unused,
    openAttention: record("attention"),
    split: record("split"),
    focusPane: record("focus"),
    setSplitRatio: record("ratio"),
    input: record("input"),
    createTab: record("tab"),
    focusTab: record("tab_focus"),
    zoomPane: async () => {},
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  };
  const controller = createAgentsController(selectionScopedAgentsBridge(base, () => "local"));
  return { controller, actions, attaches };
}

const lost = { type: "state", state: "disconnected", error: { code: "connection_lost", message: "perdida", retryable: true } } as const;

describe("agents attach ended before its reply (spec 007 navigation R1)", () => {
  // Would catch: the reproduced race — the pending connect's live overview flipping the panel back
  // to connected with w1:p1 confirmed and split enabled after the channel reported disconnected.
  it("ignores the late overview and later events of a channel that reported disconnected", async () => {
    const { controller, actions, attaches } = pendingAgents();
    const connecting = controller.connect(geometry);
    attaches[0]!.emit(lost);
    attaches[0]!.reply(overview("local", "boot-local-5", 3, single("w1:p1", 80, 24)));
    await connecting;
    expect(controller.state.phase).toBe("disconnected");
    expect(viewModel(controller.state).confirmed).toBeNull();
    expect(viewModel(controller.state).split.enabled).toBe(false);

    attaches[0]!.emit({ type: "topology", topology: single("w2:p1", 120, 40) });
    attaches[0]!.emit({ type: "state", state: "live", error: null });
    expect(viewModel(controller.state).confirmed).toBeNull();
    await controller.split("right");
    expect(actions).toEqual([]);

    // Only a new explicit attach confirms again (same pane id family, same identity).
    const again = controller.connect(geometry);
    attaches[1]!.reply(overview("local", "boot-local-5", 3, single("w1:p1", 80, 24)));
    await again;
    expect(controller.state.phase).toBe("connected");
    expect(viewModel(controller.state).confirmed).toBe("w1:p1");
    await controller.split("right");
    expect(actions).toEqual(["split:local:w1:p1"]);
  });

  // Would catch: a topology event of a channel that already ended (after its overview) confirming
  // a pane again without a new attach.
  it("ignores events of a connected channel after it reported disconnected", async () => {
    const { controller, actions, attaches } = pendingAgents();
    const connecting = controller.connect(geometry);
    attaches[0]!.reply(overview("local", "boot-local-5", 3, single("w1:p1", 80, 24)));
    await connecting;
    attaches[0]!.emit(lost);
    attaches[0]!.emit({ type: "topology", topology: single("w2:p1", 120, 40) });
    expect(controller.state.phase).toBe("disconnected");
    expect(viewModel(controller.state).confirmed).toBeNull();
    await controller.createTab();
    expect(actions).toEqual([]);
  });

  // Would catch: a late connect error of the ended channel replacing the disconnection it reported.
  it("keeps the reported disconnection when the ended attach fails afterwards", async () => {
    const { controller, attaches } = pendingAgents();
    const connecting = controller.connect(geometry);
    attaches[0]!.emit(lost);
    attaches[0]!.fail({ code: "late_failure", message: "tarde", retryable: true });
    await connecting;
    expect(controller.state.phase).toBe("disconnected");
    expect(controller.state.connectionError?.code).toBe("connection_lost");
  });

  // Would catch: over-fencing — topology/agents pushed before the overview of a channel that is still
  // alive being dropped instead of kept (they are newer than the overview snapshot).
  it("keeps events that arrive before the overview while the channel stays alive", async () => {
    const { controller, attaches } = pendingAgents();
    const connecting = controller.connect(geometry);
    attaches[0]!.emit({ type: "topology", topology: single("w2:p1", 120, 40) });
    attaches[0]!.reply(overview("local", "boot-local-5", 3, single("w1:p1", 80, 24)));
    await connecting;
    expect(controller.state.phase).toBe("connected");
    expect(viewModel(controller.state).confirmed).toBe("w2:p1");
  });

  // Would catch: a replaced attach (host switch while pending) landing its late overview, events or
  // disconnection on the newer attach — same pane ids, different connection.
  it("a newer attach is untouched by the late reply, events and disconnection of the replaced one", async () => {
    const { controller, actions, attaches } = pendingAgents();
    const first = controller.connect(geometry);
    controller.invalidate();
    const second = controller.connect(geometry);
    attaches[1]!.reply(overview("local", "boot-local-6", 4, single("w1:p1", 80, 24)));
    await second;

    attaches[0]!.reply(overview("local", "boot-local-5", 3, single("w1:p1", 80, 24)));
    await first;
    attaches[0]!.emit({ type: "topology", topology: single("w9:p9", 10, 10) });
    attaches[0]!.emit(lost);
    expect(controller.state.phase).toBe("connected");
    expect(controller.state.identity?.boot_id).toBe("boot-local-6");
    expect(viewModel(controller.state).confirmed).toBe("w1:p1");
    await controller.split("down");
    expect(actions).toEqual(["split:local:w1:p1"]);
  });
});
