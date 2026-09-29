// Spec 007 (AC-007-04) — a prompt is offered only for an agent the engine can accept on the
// current connection. The engine refuses `agent.prompt` while a managed launch is pending
// ("agent … is not an active named agent"); readiness is the engine's `interactive_ready`
// (`AgentDto.ready`, managed launches) or, for detected agents, the engine-reported kind, both
// delivered by the existing reconciler/agents events, never guessed here.
import { describe, expect, it } from "vitest";
import type { AgentsBridge } from "./bridge";
import { createAgentsController } from "./controller";
import { initialState, reduce, viewModel, type AgentsState } from "./reducer";
import type { AgentDto, AgentsEvent, Capabilities, LiveIdentity, Overview, PromptOutcome, QualifiedTarget } from "./types";

const GEOMETRY = { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 };

const caps: Capabilities = {
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
};

const SSH: LiveIdentity = { endpoint: "ssh-dev", session: "hd007-remote", connection_generation: 2, boot_id: "boot-ssh" };

function agent(extra: Partial<AgentDto> = {}): AgentDto {
  return {
    pane_id: "w1:p1",
    workspace_id: "w1",
    tab_id: "w1:t1",
    name: "revisor",
    kind: "pi",
    status: "idle",
    launch_pending: false,
    ready: true,
    focused: true,
    ...extra,
  };
}

function overview(identity: LiveIdentity, agents: AgentDto[]): Overview {
  return {
    state: "live",
    session: identity.session,
    identity,
    server_version: "0.9.0",
    capabilities: caps,
    kinds: ["pi"],
    agents,
    tabs: [],
    topology: null,
    error: null,
  };
}

const PENDING = { launch_pending: true, ready: false };
// Planner premise correction: `interactive_ready` is true only for a managed launch in its Active
// phase, while `agent.prompt` accepts any effective known agent that is not launch-pending. "Not
// ready and unconfirmed" is therefore an agent without an engine-reported kind.
const UNCONFIRMED = { launch_pending: false, ready: false, kind: null };
/** Detected/manual agent (not a managed launch): kind known, never `interactive_ready`. */
const UNMANAGED = { launch_pending: false, ready: false, kind: "pi" };
const READY = { launch_pending: false, ready: true };

function typed(identity: LiveIdentity, readiness: Partial<AgentDto>, text = "gerar relatório"): AgentsState {
  let s = reduce(initialState(), { type: "connect_started" });
  s = reduce(s, { type: "connected", overview: overview(identity, [agent(readiness)]) });
  return reduce(s, { type: "prompt_edit", paneId: "w1:p1", text });
}

const row = (s: AgentsState) => viewModel(s).agents.find((a) => a.paneId === "w1:p1")!;
const event = (s: AgentsState, e: AgentsEvent) => reduce(s, { type: "event", event: e });

describe("prompt readiness (view model)", () => {
  // Would catch: canPrompt ignoring launch_pending (r3: prompt sent while the engine still
  // launched the agent), or ignoring ready=false after the launch settled without confirmation.
  it("offers the prompt only when the engine reports the agent ready", () => {
    const pending = row(typed(SSH, PENDING));
    expect(pending.canPrompt).toBe(false);
    expect(pending.promptDisabledReason).toBe("o agente está iniciando; aguarde o servidor confirmar que está pronto");
    expect(pending.readiness).toBe("iniciando…");

    const unconfirmed = row(typed(SSH, UNCONFIRMED));
    expect(unconfirmed.canPrompt).toBe(false);
    expect(unconfirmed.promptDisabledReason).toBe("aguardando o servidor confirmar que o agente está pronto");

    const ready = row(typed(SSH, READY));
    expect(ready.canPrompt).toBe(true);
    expect(ready.promptDisabledReason).toBeNull();
    expect(ready.readiness).toBe("pronto");
  });

  // Would catch: a detected/manual agent (kind reported by the engine, no managed launch, so never
  // interactive_ready) regressing to a disabled prompt although the engine's agent.prompt accepts it.
  it("offers the prompt to a detected agent with a known kind that is not a managed launch", () => {
    const unmanaged = row(typed(SSH, UNMANAGED));
    expect(unmanaged.canPrompt).toBe(true);
    expect(unmanaged.promptDisabledReason).toBeNull();
    const emptyKind = row(typed(SSH, { ...UNMANAGED, kind: "" }));
    expect(emptyKind.canPrompt).toBe(false);
    expect(emptyKind.promptDisabledReason).toBe("aguardando o servidor confirmar que o agente está pronto");
  });

  // Would catch: readiness latched from the start result, or the typed text lost while waiting.
  it("follows the engine's agents events both ways and keeps the typed text", () => {
    let s = typed(SSH, PENDING);
    s = event(s, { type: "agents", agents: [agent(READY)] });
    expect(row(s).canPrompt).toBe(true);
    s = event(s, { type: "agents", agents: [agent(PENDING)] });
    expect(row(s).canPrompt).toBe(false);
    expect(row(s).promptText).toBe("gerar relatório");
  });

  // Would catch: the readiness gate hiding the blocked/sending reasons or the empty-text reason.
  it("keeps the blocked, sending and empty-text reasons", () => {
    const blocked = row(typed(SSH, { ...UNCONFIRMED, status: "blocked" }));
    expect(blocked.promptDisabledReason).toBe("o agente aguarda sua resposta no terminal");
    const sending = row(reduce(typed(SSH, READY), { type: "prompt_started", paneId: "w1:p1" }));
    expect(sending.promptDisabledReason).toBe("enviando…");
    const empty = row(typed(SSH, READY, "  "));
    expect(empty.promptDisabledReason).toBe("escreva o prompt");
  });

  // Would catch: an unknown outcome resend offered while the agent is not ready, or the unknown
  // outcome/resend label discarded by the readiness change.
  it("keeps an unknown outcome and its manual resend label across readiness changes", () => {
    let s = reduce(typed(SSH, READY), { type: "prompt_started", paneId: "w1:p1" });
    s = reduce(s, { type: "prompt_unknown", paneId: "w1:p1", error: { code: "result_unknown", message: "sem resposta", retryable: false } });
    s = event(s, { type: "agents", agents: [agent(PENDING)] });
    expect(row(s).canPrompt).toBe(false);
    expect(row(s).promptLabel).toBe("Reenviar mesmo assim");
    expect(row(s).outcome).toBe("Resultado desconhecido: verifique o pane antes de reenviar");
    s = event(s, { type: "agents", agents: [agent(READY)] });
    expect(row(s).canPrompt).toBe(true);
    expect(row(s).promptLabel).toBe("Reenviar mesmo assim");
  });

  // Would catch: readiness observed on one host/session/boot/generation enabling the prompt after
  // the live identity changed (same pane id w1:p1 on the new connection).
  it("does not trust readiness observed on another endpoint, session, boot or generation", () => {
    const others: LiveIdentity[] = [
      { ...SSH, endpoint: "local" },
      { ...SSH, session: "hd007-other" },
      { ...SSH, boot_id: "boot-ssh-9" },
      { ...SSH, connection_generation: 3 },
    ];
    for (const other of others) {
      let s = typed(SSH, READY);
      expect(row(s).canPrompt).toBe(true);
      s = event(s, { type: "identity", identity: other });
      expect(row(s).canPrompt).toBe(false);
      expect(row(s).promptDisabledReason).toBe("aguardando o servidor confirmar o agente nesta conexão");
      s = event(s, { type: "agents", agents: [agent(READY)] });
      expect(row(s).canPrompt).toBe(true);
    }
    // Same identity reported again: nothing becomes stale.
    const same = event(typed(SSH, READY), { type: "identity", identity: { ...SSH } });
    expect(row(same).canPrompt).toBe(true);
  });

  // Would catch: agents events that arrived on the new channel before its overview being bound
  // to the previous identity (prompt wrongly left disabled) or the overview's older list winning.
  it("binds agents events received while connecting to the identity the connect confirms", () => {
    let s = typed(SSH, PENDING);
    s = reduce(s, { type: "connect_started" });
    const next: LiveIdentity = { ...SSH, boot_id: "boot-ssh-9" };
    s = event(s, { type: "agents", agents: [agent(READY)] });
    s = reduce(s, { type: "connected", overview: overview(next, [agent(PENDING)]) });
    s = reduce(s, { type: "prompt_edit", paneId: "w1:p1", text: "oi" });
    expect(row(s).canPrompt).toBe(true);
  });
});

interface Call {
  target: QualifiedTarget;
  text: string;
  resend: boolean;
}

function scriptedBridge(first: Overview) {
  const calls: Call[] = [];
  const listeners: ((event: AgentsEvent) => void)[] = [];
  let next = first;
  let outcome: PromptOutcome | null = null;
  const unused = async () => {
    throw new Error("not used");
  };
  const bridge: AgentsBridge = {
    connect: async (_geometry, onEvent) => {
      listeners.push(onEvent);
      return next;
    },
    overview: async () => next,
    detach: async () => next,
    startAgent: unused,
    prompt: async (target, text, resend) => {
      calls.push({ target, text, resend });
      return outcome ?? { outcome: "sent", agent: agent(READY) };
    },
    openAttention: unused,
    split: unused,
    focusPane: unused,
    setSplitRatio: unused,
    input: unused,
    createTab: unused,
    focusTab: unused,
    zoomPane: async () => {},
    closeTab: async () => {},
    renameTab: async () => {},
    closePane: async () => {},
  };
  return {
    bridge,
    calls,
    emit: (event: AgentsEvent, channel = listeners.length - 1) => listeners[channel]!(event),
    setNext: (o: Overview) => (next = o),
    setOutcome: (o: PromptOutcome | null) => (outcome = o),
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("prompt readiness (controller)", () => {
  // Would catch: the public sendPrompt bypassing the disabled state (keyboard shortcut / driver
  // calling it directly) and the engine answering agent_not_ready; or a prompt retried by itself
  // once the agent became ready.
  it("sends nothing before readiness and exactly once after the user sends when ready", async () => {
    const script = scriptedBridge(overview(SSH, [agent(PENDING)]));
    const controller = createAgentsController(script.bridge);
    await controller.connect(GEOMETRY);
    controller.editPrompt("w1:p1", "gerar relatório");
    await controller.sendPrompt("w1:p1");
    script.emit({ type: "agents", agents: [agent(UNCONFIRMED)] });
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toEqual([]);
    expect(controller.state.prompts["w1:p1"]).toEqual({ text: "gerar relatório", sending: false, outcome: null, error: null });

    script.emit({ type: "agents", agents: [agent(READY)] });
    await settle();
    expect(script.calls).toEqual([]); // readiness alone never sends
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toEqual([
      {
        target: { endpoint: "ssh-dev", session: "hd007-remote", connection_generation: 2, boot_id: "boot-ssh", workspace_id: "w1", pane_id: "w1:p1" },
        text: "gerar relatório",
        resend: false,
      },
    ]);
    expect(controller.state.prompts["w1:p1"]!.outcome).toBe("sent");
  });

  // Would catch: the controller refusing a detected (non-managed) agent with a known kind, or
  // sending it more than once.
  it("sends once to a detected agent with a known kind that the engine never marks ready", async () => {
    const script = scriptedBridge(overview(SSH, [agent(UNMANAGED)]));
    const controller = createAgentsController(script.bridge);
    await controller.connect(GEOMETRY);
    controller.editPrompt("w1:p1", "revisar");
    await controller.sendPrompt("w1:p1");
    await settle();
    expect(script.calls.map((c) => [c.target.pane_id, c.text, c.resend])).toEqual([["w1:p1", "revisar", false]]);
    expect(controller.state.prompts["w1:p1"]!.outcome).toBe("sent");
  });

  // Would catch: a manual resend after an unknown outcome bypassing readiness, or losing the
  // resend flag once the agent is ready again.
  it("keeps the manual resend after an unknown outcome behind readiness", async () => {
    const script = scriptedBridge(overview(SSH, [agent(READY)]));
    const controller = createAgentsController(script.bridge);
    await controller.connect(GEOMETRY);
    script.setOutcome({ outcome: "unknown", error: { code: "result_unknown", message: "sem resposta", retryable: false } });
    controller.editPrompt("w1:p1", "olá");
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toHaveLength(1);
    script.emit({ type: "agents", agents: [agent(PENDING)] });
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toHaveLength(1);
    script.setOutcome(null);
    script.emit({ type: "agents", agents: [agent(READY)] });
    await controller.sendPrompt("w1:p1");
    expect(script.calls.map((c) => [c.text, c.resend])).toEqual([
      ["olá", false],
      ["olá", true],
    ]);
  });

  // Would catch: readiness confirmed before the connection ended still enabling the prompt (and a
  // direct/manual resend reaching the bridge) after the channel reported `disconnected`, or the
  // typed text / unknown outcome being discarded by the disconnect.
  it("drops readiness when the connection ends, keeping text and unknown outcome", async () => {
    const script = scriptedBridge(overview(SSH, [agent(READY)]));
    const controller = createAgentsController(script.bridge);
    await controller.connect(GEOMETRY);
    script.setOutcome({ outcome: "unknown", error: { code: "result_unknown", message: "sem resposta", retryable: false } });
    controller.editPrompt("w1:p1", "olá");
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toHaveLength(1);
    expect(viewModel(controller.state).agents[0]!.canPrompt).toBe(true);

    script.emit({ type: "state", state: "disconnected", error: null });
    const rowAfter = viewModel(controller.state).agents[0]!;
    expect(rowAfter.canPrompt).toBe(false);
    expect(rowAfter.promptDisabledReason).toBe("sem conexão com o servidor; aguarde reconectar");
    expect(rowAfter.promptLabel).toBe("Reenviar mesmo assim");
    expect(rowAfter.promptText).toBe("olá");
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toHaveLength(1);
    expect(controller.state.prompts["w1:p1"]!.outcome).toBe("unknown");

    // A late ready list on the ended channel is dropped by the controller; nothing re-enables.
    script.emit({ type: "agents", agents: [agent(READY)] });
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toHaveLength(1);
  });

  // Would catch: readiness of the previous boot (same pane id) used after the identity changed,
  // and a late ready event of a replaced attachment enabling the new host's pending agent.
  it("refuses readiness of a stale boot or a replaced attachment", async () => {
    const script = scriptedBridge(overview(SSH, [agent(READY)]));
    const controller = createAgentsController(script.bridge);
    await controller.connect(GEOMETRY);
    controller.editPrompt("w1:p1", "um");
    script.emit({ type: "identity", identity: { ...SSH, boot_id: "boot-ssh-9" } });
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toEqual([]);

    controller.invalidate();
    script.setNext(overview({ ...SSH, endpoint: "local", session: "hd007-local" }, [agent(PENDING)]));
    await controller.connect(GEOMETRY);
    script.emit({ type: "agents", agents: [agent(READY)] }, 0); // late event of the SSH attachment
    controller.editPrompt("w1:p1", "dois");
    await controller.sendPrompt("w1:p1");
    expect(script.calls).toEqual([]);
    expect(viewModel(controller.state).agents[0]!.promptDisabledReason).toBe(
      "o agente está iniciando; aguarde o servidor confirmar que está pronto",
    );
  });
});
