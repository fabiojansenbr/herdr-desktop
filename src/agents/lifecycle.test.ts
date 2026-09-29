// Spec 007 — lifecycle order of the Tauri agents bridge. The backend runs every command off the
// GUI thread, in the order its tasks first run; two lifecycle invokes sent together could race
// for that first run. The bridge therefore sends connect/detach one at a time, in JS call order:
// the next lifecycle invoke goes out only after the previous one settled (result or error).
// Other actions never wait for that queue, and nothing is ever re-sent.
import { describe, expect, it } from "vitest";
import { tauriAgentsBridge, type AgentsIpc } from "./bridge";
import type { AgentsEvent, Overview, QualifiedTarget } from "./types";

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
const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};
const geometry = { cols: 80, rows: 24, cell_width_px: 9, cell_height_px: 18 };

function overview(state: string, boot: string | null): Overview {
  return {
    state,
    session: boot ? "hd007q-session" : null,
    identity: boot ? { endpoint: "ssh-hd007q", session: "hd007q-session", connection_generation: 4, boot_id: boot } : null,
    server_version: null,
    capabilities: null,
    kinds: [],
    agents: [],
    tabs: [],
    topology: null,
    error: null,
  } as unknown as Overview;
}

interface Call {
  command: string;
  args: Record<string, unknown> | undefined;
  reply: Deferred<unknown>;
}

/** Fake IPC: every invoke is recorded when it is sent and answers only when the test says so. */
function fakeIpc() {
  const calls: Call[] = [];
  const channels: { handler: (event: AgentsEvent) => void }[] = [];
  const ipc: AgentsIpc = {
    invoke: <T>(command: string, args?: Record<string, unknown>) => {
      const reply = deferred<unknown>();
      calls.push({ command, args, reply });
      return reply.promise as Promise<T>;
    },
    channel: (handler) => {
      const channel = { handler };
      channels.push(channel);
      return channel;
    },
  };
  const sent = () => calls.map((call) => call.command);
  const call = (index: number): Call => {
    const found = calls[index];
    if (!found) throw new Error(`invoke #${index} was never sent: ${sent().join(", ")}`);
    return found;
  };
  const channel = (index: number) => {
    const found = channels[index];
    if (!found) throw new Error(`channel #${index} was never created`);
    return found;
  };
  return { ipc, calls, channels, sent, call, channel };
}

const target: QualifiedTarget = {
  endpoint: "ssh-hd007q",
  session: "hd007q-session",
  connection_generation: 4,
  boot_id: "boot-hd007q-1",
  workspace_id: null,
  pane_id: "w1:p1",
} as unknown as QualifiedTarget;

describe("tauriAgentsBridge lifecycle queue", () => {
  // Would catch: connect/detach invoked together (a delayed connect racing a later detach and
  // connect for the backend's first poll), an error that never releases the queue, a result
  // delivered to the wrong caller, or any lifecycle command sent twice.
  it("sends connect, detach and connect one at a time in call order; results and errors release the next", async () => {
    const fake = fakeIpc();
    const bridge = tauriAgentsBridge(fake.ipc);
    const eventsA: AgentsEvent[] = [];
    const eventsB: AgentsEvent[] = [];

    const connectA = bridge.connect(geometry, (event) => eventsA.push(event));
    const detach = bridge.detach();
    const connectB = bridge.connect(geometry, (event) => eventsB.push(event));
    const outcomes: string[] = [];
    connectA.then(() => outcomes.push("connectA"));
    detach.catch(() => outcomes.push("detach-error"));
    connectB.then(() => outcomes.push("connectB"));

    await settle();
    expect(fake.sent()).toEqual(["agents_connect"]);

    fake.call(0).reply.resolve(overview("live", "boot-hd007q-A"));
    await settle();
    expect(fake.sent()).toEqual(["agents_connect", "agents_detach"]);
    await expect(connectA).resolves.toEqual(overview("live", "boot-hd007q-A"));

    const lost = { code: "command_interrupted", message: "detach sem resultado", retryable: false };
    fake.call(1).reply.reject(lost);
    await expect(detach).rejects.toBe(lost);
    await settle();
    expect(fake.sent()).toEqual(["agents_connect", "agents_detach", "agents_connect"]);

    fake.call(2).reply.resolve(overview("live", "boot-hd007q-B"));
    await expect(connectB).resolves.toEqual(overview("live", "boot-hd007q-B"));
    await settle();
    expect(outcomes).toEqual(["connectA", "detach-error", "connectB"]);
    expect(fake.sent()).toEqual(["agents_connect", "agents_detach", "agents_connect"]);

    // Each connect carries its own channel; its events reach only its own handler.
    expect(fake.call(0).args?.geometry).toEqual(geometry);
    expect(fake.call(0).args?.onEvent).toBe(fake.channel(0));
    expect(fake.call(2).args?.onEvent).toBe(fake.channel(1));
    const event = { type: "state", state: "events_unavailable", error: null } as unknown as AgentsEvent;
    fake.channel(1).handler(event);
    expect(eventsB).toEqual([event]);
    expect(eventsA).toEqual([]);
    expect(fake.call(1).args).toBeUndefined();
  });

  // Would catch: a rejected connect leaving the queue stuck, or its error swallowed/retried.
  it("a failed connect releases the next detach and is not retried", async () => {
    const fake = fakeIpc();
    const bridge = tauriAgentsBridge(fake.ipc);
    const connect = bridge.connect(geometry, () => {});
    const detach = bridge.detach();
    await settle();
    expect(fake.sent()).toEqual(["agents_connect"]);
    const offline = { code: "host_offline_hd007q", message: "fora do ar", retryable: true };
    fake.call(0).reply.reject(offline);
    await expect(connect).rejects.toBe(offline);
    await settle();
    expect(fake.sent()).toEqual(["agents_connect", "agents_detach"]);
    fake.call(1).reply.resolve(overview("disconnected", null));
    await expect(detach).resolves.toEqual(overview("disconnected", null));
    await settle();
    expect(fake.sent()).toEqual(["agents_connect", "agents_detach"]);
  });

  // Would catch: overview or qualified actions (prompt, input, split...) put behind a pending
  // connect, which would stall the panel while a slow host answers the attach.
  it("overview and actions are sent immediately while a lifecycle command is pending", async () => {
    const fake = fakeIpc();
    const bridge = tauriAgentsBridge(fake.ipc);
    const connect = bridge.connect(geometry, () => {});
    const detach = bridge.detach();
    const prompt = bridge.prompt(target, "olá hd007q", false);
    const input = bridge.input(target, []);
    const seen = bridge.overview();
    await settle();
    expect(fake.sent()).toEqual(["agents_connect", "agent_prompt", "pane_input", "agents_overview"]);
    expect(fake.call(1).args).toEqual({ target, text: "olá hd007q", resendAfterUnknown: false });

    const outcome = { outcome: "unknown", error: { code: "timeout", message: "sem resposta", retryable: true } };
    fake.call(1).reply.resolve(outcome);
    fake.call(2).reply.resolve(undefined);
    fake.call(3).reply.resolve(overview("connecting", null));
    await expect(prompt).resolves.toEqual(outcome);
    await expect(input).resolves.toBeUndefined();
    await expect(seen).resolves.toEqual(overview("connecting", null));
    expect(fake.sent()).toEqual(["agents_connect", "agent_prompt", "pane_input", "agents_overview"]);

    fake.call(0).reply.resolve(overview("live", "boot-hd007q-C"));
    await expect(connect).resolves.toEqual(overview("live", "boot-hd007q-C"));
    await settle();
    expect(fake.sent()).toEqual(["agents_connect", "agent_prompt", "pane_input", "agents_overview", "agents_detach"]);
    fake.call(4).reply.resolve(overview("disconnected", null));
    await expect(detach).resolves.toEqual(overview("disconnected", null));
    expect(fake.calls.filter((call) => call.command === "agent_prompt")).toHaveLength(1);
  });
});
