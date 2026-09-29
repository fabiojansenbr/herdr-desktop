// Spec 007 — the composed window reattaches one AgentsController when the selected host (or its
// connection/boot) changes. Late results of the previous attachment — connect overview, connect
// error, channel events and pending actions — must never land on the new host's state, even when
// both hosts use the same pane ids. Nothing is retried. Harness defaults (no invalidate) unchanged.
import { describe, expect, it } from "vitest";
import type { AgentsBridge } from "./bridge";
import { createAgentsController } from "./controller";
import type { AgentDto, AgentsEvent, Overview, QualifiedTarget } from "./types";

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
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const geometry = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };

function overview(endpoint: string, boot: string): Overview {
  return {
    state: "connected",
    session: `s-${endpoint}`,
    identity: { endpoint, session: `s-${endpoint}`, connection_generation: endpoint === "local" ? 3 : 7, boot_id: boot },
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
    kinds: ["claude"],
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
  };
}
const agent = (name: string): AgentDto => ({
  pane_id: "w1:p1",
  workspace_id: "w1",
  tab_id: "t1",
  name,
  kind: "claude",
  status: "working",
  launch_pending: false,
  ready: true,
  focused: true,
});

/** Every command waits on a barrier the test releases. */
function barriers() {
  const connects: { emit: (event: AgentsEvent) => void; result: Deferred<Overview> }[] = [];
  const actions: { name: string; target: QualifiedTarget; result: Deferred<unknown> }[] = [];
  const pending = <T>(name: string, target: QualifiedTarget) => {
    const result = deferred<unknown>();
    actions.push({ name, target, result });
    return result.promise as Promise<T>;
  };
  const bridge: AgentsBridge = {
    connect: (_geometry, onEvent) => {
      const result = deferred<Overview>();
      connects.push({ emit: onEvent, result });
      return result.promise;
    },
    overview: () => Promise.reject(new Error("unused")),
    detach: () => Promise.reject(new Error("unused")),
    startAgent: (target) => pending("start", target),
    prompt: (target) => pending("prompt", target),
    openAttention: (target) => pending("attention", target),
    split: (target) => pending("split", target),
    focusPane: (target) => pending("focus", target),
    setSplitRatio: (target) => pending("ratio", target),
    input: (target) => pending("input", target),
    createTab: (target) => pending("tab", target),
    focusTab: (target) => pending("focus_tab", target),
    zoomPane: async () => {},
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  };
  return { bridge, connects, actions };
}

async function connected(host: ReturnType<typeof barriers>, controller: ReturnType<typeof createAgentsController>, endpoint: string, boot: string) {
  const done = controller.connect(geometry);
  host.connects.at(-1)!.result.resolve(overview(endpoint, boot));
  await done;
}

describe("agents attachment epochs (spec 007)", () => {
  // Would catch: a slow Local connect resolving after SSH was attached and overwriting its
  // identity/capabilities (both expose pane w1:p1).
  it("drops a late connect overview of the previous attachment", async () => {
    const host = barriers();
    const controller = createAgentsController(host.bridge);
    const first = controller.connect(geometry);
    controller.invalidate();
    const second = controller.connect(geometry);
    host.connects[1]!.result.resolve(overview("ssh-dev", "boot-ssh"));
    await second;
    host.connects[0]!.result.resolve(overview("local", "boot-local"));
    await first;
    expect(controller.state.identity).toEqual({ endpoint: "ssh-dev", session: "s-ssh-dev", connection_generation: 7, boot_id: "boot-ssh" });
    expect(controller.state.phase).toBe("connected");
  });

  // Would catch: the previous host's connect failure marking the new attachment as failed.
  it("drops a late connect error of the previous attachment", async () => {
    const host = barriers();
    const controller = createAgentsController(host.bridge);
    const first = controller.connect(geometry);
    controller.invalidate();
    await connected(host, controller, "ssh-dev", "boot-ssh");
    host.connects[0]!.result.reject({ code: "not_connected", message: "local indisponível", retryable: true });
    await first;
    expect(controller.state.phase).toBe("connected");
    expect(controller.state.connectionError).toBeNull();
  });

  // Would catch: events still flowing from the previous agents channel after invalidation,
  // before the new connect even starts.
  it("drops channel events of the previous attachment as soon as it is invalidated", async () => {
    const host = barriers();
    const controller = createAgentsController(host.bridge);
    await connected(host, controller, "local", "boot-local");
    controller.invalidate();
    host.connects[0]!.emit({ type: "agents", agents: [agent("local-agent")] });
    expect(controller.state.agents).toEqual([]);
    expect(controller.state.identity).toBeNull();
    await connected(host, controller, "ssh-dev", "boot-ssh");
    host.connects[0]!.emit({ type: "identity", identity: { endpoint: "local", session: "s-local", connection_generation: 3, boot_id: "boot-local" } });
    expect(controller.state.identity?.endpoint).toBe("ssh-dev");
  });

  // Would catch: a split or agent start pending on Local finishing after the switch and marking
  // the SSH panel (busy flag, error or agent list), or being retried on SSH.
  it("discards late success and error of actions started before the switch", async () => {
    const host = barriers();
    const controller = createAgentsController(host.bridge);
    await connected(host, controller, "local", "boot-local");
    const split = controller.split("right");
    controller.editStart("name", "revisor");
    const start = controller.startAgent();
    expect(host.actions.map((a) => [a.name, a.target.endpoint])).toEqual([
      ["split", "local"],
      ["start", "local"],
    ]);

    controller.invalidate();
    await connected(host, controller, "ssh-dev", "boot-ssh");
    host.actions[0]!.result.reject({ code: "stale_target", message: "boot anterior", retryable: false });
    host.actions[1]!.result.resolve(agent("revisor"));
    await split;
    await start;
    expect(controller.state.layoutError).toBeNull();
    expect(controller.state.layoutBusy).toBe(false);
    expect(controller.state.agents).toEqual([]);
    expect(controller.state.start.busy).toBe(false);
    expect(host.actions).toHaveLength(2);

    const sshSplit = controller.split("down");
    expect(host.actions.map((a) => [a.name, a.target.endpoint, a.target.boot_id])).toEqual([
      ["split", "local", "boot-local"],
      ["start", "local", "boot-local"],
      ["split", "ssh-dev", "boot-ssh"],
    ]);
    host.actions[2]!.result.resolve(undefined);
    await sshSplit;
    expect(controller.state.layoutBusy).toBe(false);
  });

  // Would catch: keystrokes queued for the previous host being delivered after the switch.
  it("does not deliver queued input of the previous attachment", async () => {
    const host = barriers();
    const controller = createAgentsController(host.bridge);
    await connected(host, controller, "local", "boot-local");
    const key = { key: "a", ctrlKey: false, altKey: false, shiftKey: false, metaKey: false };
    controller.key(key);
    controller.key({ ...key, key: "b" });
    await tick();
    expect(host.actions.filter((a) => a.name === "input")).toHaveLength(1);
    controller.invalidate();
    host.actions[0]!.result.resolve(undefined);
    await tick();
    expect(host.actions.filter((a) => a.name === "input")).toHaveLength(1);
  });

  // Would catch: harness behavior changing — without invalidate, a reconnect of the same
  // controller still applies its own result.
  it("keeps the standalone reconnect behavior", async () => {
    const host = barriers();
    const controller = createAgentsController(host.bridge);
    await connected(host, controller, "local", "boot-local");
    await connected(host, controller, "local", "boot-local-2");
    expect(controller.state.identity?.boot_id).toBe("boot-local-2");
  });
});
