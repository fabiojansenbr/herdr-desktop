// Spec 004 — controller over a fake bridge that records every command it receives.
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAgentsController } from "./controller";
import { createFakeAgentsBridge, type FakeAgentsOptions } from "./fake-bridge";
import type { TabDto } from "./types";
import type { KeyLike } from "../terminal/actions";

const GEOMETRY = { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 };

function key(k: string, mods: Partial<KeyLike> = {}): KeyLike {
  return { key: k, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods };
}

async function connected(options: Partial<FakeAgentsOptions> = {}) {
  const bridge = createFakeAgentsBridge({ bootId: "boot-lumen", generation: 3, session: "hd004-alpha", ...options });
  const controller = createAgentsController(bridge);
  await controller.connect(GEOMETRY);
  bridge.calls.length = 0;
  return { bridge, controller };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function target(pane: string, boot = "boot-lumen") {
  return { endpoint: "local", session: "hd004-alpha", connection_generation: 3, boot_id: boot, workspace_id: "w1", pane_id: pane };
}

function tab(tabId: string, focused = false): TabDto {
  const number = Number(tabId.split(":")[1]!.slice(1));
  return { tab_id: tabId, workspace_id: tabId.split(":")[0]!, label: String(number), number, pane_count: 1, focused };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("agents controller", () => {
  // Would catch: rendering/connecting that starts agents or sends prompts.
  it("connects with one command and starts nothing", async () => {
    const bridge = createFakeAgentsBridge({ bootId: "boot-lumen", generation: 3, session: "hd004-alpha" });
    const controller = createAgentsController(bridge);
    await controller.connect(GEOMETRY);
    await settle();
    expect(bridge.calls.map((c) => c.command)).toEqual(["agents_connect"]);
    expect(controller.state.phase).toBe("connected");
  });

  // AC-004-01. Would catch: start sent to another pane/host or with a kind the user did not pick.
  it("starts the chosen kind on the chosen pane with the qualified target", async () => {
    const { bridge, controller } = await connected();
    controller.editStart("paneId", "w1:p2");
    controller.editStart("kind", "claude");
    controller.editStart("name", "revisor");
    await controller.startAgent();
    await controller.startAgent(); // agent now exists: no duplicate start on the same pane
    expect(bridge.calls).toEqual([
      { command: "agent_start", args: { target: target("w1:p2"), kind: "claude", name: "revisor", autonomous: false } },
    ]);
  });

  // AC-076-03. Would catch: the autonomy the user chose lost on the way to the bridge, or one
  // autonomous start leaking into the next one (the old form never asks for it).
  it("carries the chosen autonomy to the bridge, for that start only", async () => {
    const { bridge, controller } = await connected();
    controller.editStart("paneId", "w1:p2");
    controller.editStart("kind", "claude");
    controller.editStart("name", "auto");
    controller.setStartAutonomy(true);
    await controller.startAgent();
    controller.editStart("paneId", "w1:p3");
    controller.editStart("name", "manual");
    await controller.startAgent();
    expect(bridge.calls).toEqual([
      { command: "agent_start", args: { target: target("w1:p2"), kind: "claude", name: "auto", autonomous: true } },
      { command: "agent_start", args: { target: target("w1:p3"), kind: "claude", name: "manual", autonomous: false } },
    ]);
  });

  // AC-004-02. Would catch: the GUI answering an approval (input/prompt) when an agent blocks,
  // or "Abrir pane" doing more than focusing that pane.
  it("surfaces blocked agents without sending anything and opens the pane only on request", async () => {
    const { bridge, controller } = await connected();
    bridge.emit({ type: "agents", agents: [bridge.agent("w1:p1", "blocked")] });
    await settle();
    expect(bridge.calls).toEqual([]);
    await controller.openAttention("w1:p1");
    expect(bridge.calls).toEqual([{ command: "agent_open_attention", args: { target: target("w1:p1") } }]);
  });

  // AC-004-01. Would catch: automatic retry on timeout, the typed text lost, or a resend that
  // does not carry the user's acknowledgement.
  it("never repeats a prompt with an unknown outcome on its own", async () => {
    const { bridge, controller } = await connected();
    bridge.failPrompt("w1:p1", { code: "timeout", message: "o endpoint não respondeu a tempo", retryable: true, endpoint: "local" }, true);
    controller.editPrompt("w1:p1", "Gerar relatório");
    await Promise.all([controller.sendPrompt("w1:p1"), controller.sendPrompt("w1:p1")]);
    await settle();
    expect(bridge.calls).toEqual([
      { command: "agent_prompt", args: { target: target("w1:p1"), text: "Gerar relatório", resendAfterUnknown: false } },
    ]);
    expect(controller.state.prompts["w1:p1"]!.text).toBe("Gerar relatório");
    expect(controller.state.prompts["w1:p1"]!.outcome).toBe("unknown");

    bridge.clearPromptFailure("w1:p1");
    await controller.sendPrompt("w1:p1");
    expect(bridge.calls[1]).toEqual({
      command: "agent_prompt",
      args: { target: target("w1:p1"), text: "Gerar relatório", resendAfterUnknown: true },
    });
    expect(bridge.calls).toHaveLength(2);
    expect(controller.state.prompts["w1:p1"]!.text).toBe("");
  });

  // AC-004-01. Would catch: a disabled action still invoking its command, or other actions
  // disabled along with it.
  it("does not invoke an action whose method is absent and keeps the others", async () => {
    const { bridge, controller } = await connected({ missing: ["send_prompt"] });
    controller.editPrompt("w1:p1", "oi");
    await controller.sendPrompt("w1:p1");
    expect(bridge.calls).toEqual([]);
    await controller.split("right");
    expect(bridge.calls.map((c) => c.command)).toEqual(["pane_split"]);
  });

  // AC-004-03. Would catch: a GUI shortcut leaking to the PTY in addition to its action.
  it("runs GUI shortcuts as actions and sends no input for them", async () => {
    const { bridge, controller } = await connected();
    expect(controller.key(key("D", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(controller.key(key("ArrowLeft", { altKey: true, shiftKey: true }))).toBe(true);
    await settle();
    expect(bridge.calls).toEqual([
      { command: "pane_split", args: { target: target("w1:p1"), direction: "right" } },
      { command: "pane_set_split_ratio", args: { target: target("w1:p1"), path: [], ratio: 0.4 } },
    ]);
  });

  // AC-004-03. Would catch: a terminal shortcut sent twice (keydown + keypress) or to the
  // pane under the mouse instead of the confirmed one.
  it("sends a terminal shortcut once to the confirmed pane, in typing order", async () => {
    const { bridge, controller } = await connected();
    expect(controller.key(key("e", { ctrlKey: true }))).toBe(true);
    controller.key(key("l"));
    controller.key(key("s"));
    controller.key(key("Enter"));
    await settle();
    await settle();
    expect(bridge.calls).toEqual([
      { command: "pane_input", args: { target: target("w1:p1"), events: [{ kind: "key", code: "Char", modifiers: 2, ch: "e" }] } },
      { command: "pane_input", args: { target: target("w1:p1"), events: [{ kind: "text", text: "l" }] } },
      { command: "pane_input", args: { target: target("w1:p1"), events: [{ kind: "text", text: "s" }] } },
      { command: "pane_input", args: { target: target("w1:p1"), events: [{ kind: "key", code: "Enter", modifiers: 0 }] } },
    ]);
  });

  // AC-004-03. Would catch: input routed to a pane whose focus the server has not confirmed.
  it("holds input while a focus change is unconfirmed and follows the server confirmation", async () => {
    const { bridge, controller } = await connected();
    await controller.focusPane("w1:p2");
    controller.key(key("x"));
    await settle();
    expect(bridge.calls).toEqual([{ command: "pane_focus", args: { target: target("w1:p2") } }]);
    expect(controller.state.notice).toMatch(/confirm/);
    bridge.emit({ type: "topology", topology: bridge.topology("w1:p2", 0.3) });
    controller.key(key("x"));
    await settle();
    expect(bridge.calls[1]).toEqual({ command: "pane_input", args: { target: target("w1:p2"), events: [{ kind: "text", text: "x" }] } });
  });

  // Edge case: identity. Would catch: targets kept on the old boot after the server rebooted.
  it("uses the new boot after an identity event", async () => {
    const { bridge, controller } = await connected();
    bridge.emit({ type: "identity", identity: { endpoint: "local", session: "hd004-alpha", connection_generation: 3, boot_id: "boot-nova" } });
    await controller.split("down");
    expect(bridge.calls).toEqual([{ command: "pane_split", args: { target: target("w1:p1", "boot-nova"), direction: "down" } }]);
  });

  // AC-027-01. Would catch: a closed tab staying in the bar until another focus event, a
  // reconciliation per forwarded event instead of one per tick, or a structural event with no
  // effect at all.
  it("reconciles the tab list once per forwarded event tick and drops the closed tab", async () => {
    const { bridge, controller } = await connected({ tabs: [tab("w1:t1", true), tab("w1:t2")] });
    bridge.setTabs([tab("w1:t1", true)]);
    bridge.emit({ type: "structure", events: ["tab_closed"] });
    await settle();
    expect(bridge.calls).toEqual([{ command: "agents_overview", args: { reconcile: true } }]);
    expect(controller.state.tabs.map((t) => t.tab_id)).toEqual(["w1:t1"]);

    bridge.setTabs([tab("w1:t1", true), tab("w1:t2"), tab("w1:t3", true)]);
    bridge.emit({ type: "structure", events: ["tab_created"] });
    bridge.emit({ type: "structure", events: ["tab_renamed"] });
    bridge.emit({ type: "structure", events: ["tab_focused"] });
    await settle();
    expect(bridge.calls.filter((c) => c.command === "agents_overview")).toHaveLength(2);
    expect(controller.state.tabs.map((t) => t.tab_id)).toEqual(["w1:t1", "w1:t2", "w1:t3"]);
  });

  // AC-027-03. Would catch: a workspace/pane event that never reaches the React reconciliation.
  it("reconciles on workspace and pane lifecycle events too", async () => {
    const { bridge, controller } = await connected();
    bridge.emit({ type: "structure", events: ["workspace_closed", "pane_closed"] });
    await settle();
    expect(bridge.calls.map((c) => c.command)).toEqual(["agents_overview"]);
    expect(controller.state.tabs).toHaveLength(1);
  });

  // Edge case: an event before the initial list waits for the connection and applies once after.
  it("waits for the connect list when a structural event arrives while connecting", async () => {
    const bridge = createFakeAgentsBridge({ bootId: "boot-lumen", generation: 3, session: "hd004-alpha" });
    const controller = createAgentsController(bridge);
    const connecting = controller.connect(GEOMETRY);
    bridge.emit({ type: "structure", events: ["tab_closed"] });
    await connecting;
    await settle();
    expect(bridge.calls.filter((c) => c.command === "agents_overview")).toHaveLength(1);
  });

  // AC-027-02. Would catch: a click on a tab the engine no longer has doing nothing (no read, no
  // warning) while the bar keeps the dead tab.
  it("reconciles and warns when the engine no longer has the clicked tab", async () => {
    const { bridge, controller } = await connected({ tabs: [tab("w1:t1", true), tab("w1:t2")] });
    bridge.failFocusTab("w1:t2", { code: "tab_not_found", message: "tab w1:t2 not found", retryable: false });
    bridge.setTabs([tab("w1:t1", true)]);
    await controller.focusTab("w1:t2");
    await settle();
    expect(bridge.calls.filter((c) => c.command === "agents_overview")).toHaveLength(1);
    expect(controller.state.tabs.map((t) => t.tab_id)).toEqual(["w1:t1"]);
    expect(controller.state.notice).toBe("Aba fechada em outro cliente");
  });

  // AC-027-02. Would catch: the warning becoming permanent, or clearing a newer notice.
  it("clears the closed-tab warning after three seconds", async () => {
    vi.useFakeTimers();
    const bridge = createFakeAgentsBridge({ bootId: "boot-lumen", generation: 3, session: "hd004-alpha", tabs: [tab("w1:t1", true), tab("w1:t2")] });
    const controller = createAgentsController(bridge);
    await controller.connect(GEOMETRY);
    bridge.failFocusTab("w1:t2", { code: "tab_not_found", message: "gone", retryable: false });
    const acting = controller.focusTab("w1:t2");
    await vi.advanceTimersByTimeAsync(0);
    await acting;
    expect(controller.state.notice).toBe("Aba fechada em outro cliente");
    vi.advanceTimersByTime(3000);
    expect(controller.state.notice).toBeNull();
  });

  // AC-027-02. Would catch: a click with no confirmed pane sending nothing and saying nothing.
  it("answers a tab click with a notice when no pane is confirmed", async () => {
    const { bridge, controller } = await connected();
    bridge.emit({ type: "state", state: "disconnected", error: null });
    await controller.focusTab("w1:t1");
    expect(bridge.calls).toEqual([]);
    expect(controller.state.notice).toMatch(/pane confirmado/);
  });

  // AC-027-02. Would catch: a refusal other than the tab being gone also hiding the tab.
  it("keeps the tab and only reports other focus failures", async () => {
    const { bridge, controller } = await connected({ tabs: [tab("w1:t1", true), tab("w1:t2")] });
    bridge.failFocusTab("w1:t2", { code: "timeout", message: "sem resposta", retryable: true });
    await controller.focusTab("w1:t2");
    await settle();
    expect(bridge.calls.map((c) => c.command)).toEqual(["tab_focus"]);
    expect(controller.state.tabs.map((t) => t.tab_id)).toEqual(["w1:t1", "w1:t2"]);
    expect(controller.state.notice).toBeNull();
  });

  // Edge case: unavailability. Would catch: a failed action on one agent clearing the others'
  // prompts or the panel state.
  it("keeps the error on the failed agent and preserves other buffers", async () => {
    const { bridge, controller } = await connected({ agents: [["w1:p1", "idle"], ["w1:p2", "idle"]] });
    bridge.failPrompt("w1:p1", { code: "server_unavailable", message: "API do Herdr indisponível", retryable: true, endpoint: "local" }, false);
    controller.editPrompt("w1:p1", "primeiro");
    controller.editPrompt("w1:p2", "segundo");
    await controller.sendPrompt("w1:p1");
    expect(controller.state.prompts["w1:p1"]!.error?.code).toBe("server_unavailable");
    expect(controller.state.prompts["w1:p1"]!.text).toBe("primeiro");
    expect(controller.state.prompts["w1:p2"]!.text).toBe("segundo");
    expect(controller.state.prompts["w1:p2"]!.error).toBeNull();
    expect(controller.state.phase).toBe("connected");
  });
});
