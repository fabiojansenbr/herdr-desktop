// Spec 007 (AC-007-04 frontend seam) — single selection driving the one terminal surface.
// Contract: .local/orchestration/surface-contract.md (planned selection_* / surface_* commands).
// Local and SSH deliberately share pane id "w1:p1" and differ in boot, generation and session,
// so any test that confuses hosts fails on the identity echoed to surface_input.
import { describe, expect, it } from "vitest";
import type { SurfaceBridge } from "./bridge";
import { createSurfaceController, startingHerdr, surfaceStatusText, workbenchConnection } from "./controller";
import type { FrameEvent, GeometryDto, InputDto, RuntimeError, SelectionDto, StatusDto, SurfaceIdentityDto } from "./types";

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

const HOSTS: Record<string, SelectionDto> = {
  local: { endpoint: "local", kind: "local", label: "Local", session: "s-local", online: true, identity: null },
  "ssh-dev": { endpoint: "ssh-dev", kind: "ssh", label: "dev-box", session: "s-remote", online: true, identity: null },
};

function status(session: string, over: Partial<StatusDto> = {}): StatusDto {
  return {
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
    ...over,
  };
}

const identityEvent = (boot: string, generation: number, pane: string | null): FrameEvent => ({
  type: "identity",
  boot_id: boot,
  generation: 1,
  connection_generation: generation,
  server_version: "0.9.0",
  pane_id: pane,
});
const live: FrameEvent = { type: "state", state: "live", reason: null, error: null };
const full = (marker: string): FrameEvent => ({
  type: "full",
  revision: 1,
  width: 1,
  height: 1,
  cells: [{ s: marker, fg: 0, bg: 0, m: 0 }],
  cursor: null,
  panes: [],
});
const text = (value: string): InputDto[] => [{ kind: "text", text: value }];

it("zero-config shows startup, the literal failure, retries and reports connection loss", async () => {
  const host = scripted({ autoAttach: false, selection: { ...HOSTS.local!, session: "default" } });
  const controller = createSurfaceController(host.bridge);
  const loading = controller.load();
  await tick();
  expect(surfaceStatusText(controller.state)).toBe("Iniciando o Herdr…");
  host.attaches[0]!.result.reject({ code: "herdr_not_found", message: "herdr não encontrado no PATH — instale o Herdr", retryable: true });
  await loading;
  expect(controller.state.error?.message).toBe("herdr não encontrado no PATH — instale o Herdr");
  const retry = controller.retry();
  await tick();
  expect(controller.state.error).toBeNull();
  expect(surfaceStatusText(controller.state)).toBe("Iniciando o Herdr…");
  host.attaches[1]!.emit(identityEvent("boot-016", 1, "w1:p1"));
  host.attaches[1]!.emit(live);
  host.attaches[1]!.result.resolve(status("default", { state: "live" }));
  await retry;
  expect(surfaceStatusText(controller.state)).toBe("Conectado");
  host.attaches[1]!.emit({ type: "state", state: "disconnected", reason: "socket_closed", error: null });
  expect(controller.state.error?.message).toBe("desconectado: socket_closed");
  expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toHaveLength(2);
  expect(host.calls).not.toContain("session_start");
});

interface Attach {
  geometry: GeometryDto;
  emit: (event: FrameEvent) => void;
  result: Deferred<StatusDto>;
}

/** Scripted bridge: every command is recorded; attach/selection results resolve on demand. */
function scripted(options: { autoSelect?: boolean; autoAttach?: boolean; interest?: boolean; selection?: SelectionDto; paste?: (expected: SurfaceIdentityDto) => Promise<import("./types").PasteReceipt> } = {}) {
  const autoSelect = options.autoSelect ?? true;
  const autoAttach = options.autoAttach ?? true;
  const calls: string[] = [];
  const attaches: Attach[] = [];
  const sets: { endpoint: string; result: Deferred<SelectionDto> }[] = [];
  const inputs: { expected: SurfaceIdentityDto; events: InputDto[] }[] = [];
  let inputError: RuntimeError | null = null;
  let current: string | null = options.selection?.endpoint ?? null;
  const bridge: SurfaceBridge = {
    selectionGet: async () => {
      calls.push("selection_get");
      return options.selection ?? { endpoint: null, kind: null, label: null, session: null, online: false, identity: null };
    },
    selectionSet: (endpoint) => {
      calls.push(`selection_set:${endpoint}`);
      const result = deferred<SelectionDto>();
      sets.push({ endpoint, result });
      if (autoSelect) {
        const host = HOSTS[endpoint];
        if (host) result.resolve(host);
        else result.reject({ code: "unknown_endpoint", message: "endpoint desconhecido", retryable: false });
      }
      return result.promise.then((selection) => {
        current = selection.endpoint;
        return selection;
      });
    },
    attach: (geometry, onEvent) => {
      calls.push(`surface_attach:${current}`);
      const result = deferred<StatusDto>();
      attaches.push({ geometry, emit: onEvent, result });
      if (autoAttach) result.resolve(status(HOSTS[current ?? ""]?.session ?? "none"));
      return result.promise;
    },
    input: async (expected, events) => {
      calls.push("surface_input");
      inputs.push({ expected, events });
      if (inputError) throw inputError;
    },
    pasteClipboard: options.paste
      ? (expected) => {
          calls.push("surface_paste_clipboard");
          return options.paste!(expected);
        }
      : undefined,
    interest: options.interest
      ? async () => {
          calls.push("surface_interest");
          return null;
        }
      : undefined,
    resize: async () => {
      calls.push("surface_resize");
    },
    focus: async () => {
      calls.push("surface_focus");
    },
    status: async () => {
      calls.push("surface_status");
      return status(HOSTS[current ?? ""]?.session ?? "none", { connected: false, state: "disconnected" });
    },
    detach: async () => {
      calls.push("surface_detach");
      return status("none", { connected: false, state: "disconnected" });
    },
    startSession: async () => {
      calls.push("session_start");
      return status("s-local");
    },
  };
  return {
    bridge,
    calls,
    attaches,
    sets,
    inputs,
    failInput(error: RuntimeError | null) {
      inputError = error;
    },
  };
}

async function liveOn(
  host: ReturnType<typeof scripted>,
  controller: ReturnType<typeof createSurfaceController>,
  endpoint: string,
  boot: string,
  generation: number,
) {
  await controller.select(endpoint);
  const attach = host.attaches.at(-1)!;
  attach.emit(identityEvent(boot, generation, "w1:p1"));
  attach.emit(live);
  return attach;
}

describe("surface selection (spec 007)", () => {
  // Would catch: reusing the Local identity (or the last global one) after selecting SSH, which
  // would address the SSH input to Local's boot/generation because both use pane w1:p1.
  it("routes input with the SSH identity after switching from Local with the same pane id", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    await liveOn(host, controller, "local", "boot-local", 3);
    await controller.input(text("a"));
    expect(host.inputs[0]!.expected).toEqual({
      endpoint: "local",
      session: "s-local",
      connection_generation: 3,
      boot_id: "boot-local",
      pane_id: "w1:p1",
    });

    await liveOn(host, controller, "ssh-dev", "boot-ssh", 7);
    await controller.input(text("b"));
    expect(host.inputs).toHaveLength(2);
    expect(host.inputs[1]!.expected).toEqual({
      endpoint: "ssh-dev",
      session: "s-remote",
      connection_generation: 7,
      boot_id: "boot-ssh",
      pane_id: "w1:p1",
    });
    expect(host.inputs[1]!.events).toEqual(text("b"));
  });

  // Would catch: keeping input enabled while the new host has not confirmed its identity, or
  // accepting identity/frames that the previous host's channel delivers late.
  it("blocks input until the new host is live and drops the previous host's late events", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    const seen: FrameEvent[] = [];
    controller.subscribe((event) => seen.push(event));
    const localAttach = await liveOn(host, controller, "local", "boot-local", 3);
    const keyBefore = controller.state.surfaceKey;

    const switching = controller.select("ssh-dev");
    expect(controller.canInput()).toBe(false);
    expect(controller.state.identity).toBeNull();
    expect(controller.state.surfaceKey).not.toBe(keyBefore);
    expect(await controller.input(text("lost"))).toBe(false);
    await switching;

    seen.length = 0;
    localAttach.emit(identityEvent("boot-local", 3, "w1:p1"));
    localAttach.emit(live);
    localAttach.emit(full("L"));
    expect(seen).toEqual([]);
    expect(controller.state.identity).toBeNull();
    expect(await controller.input(text("still-lost"))).toBe(false);

    const sshAttach = host.attaches.at(-1)!;
    sshAttach.emit(identityEvent("boot-ssh", 7, "w1:p1"));
    expect(controller.canInput()).toBe(false);
    sshAttach.emit(live);
    sshAttach.emit(full("S"));
    expect(seen.map((e) => e.type)).toEqual(["identity", "state", "full"]);
    expect(controller.canInput()).toBe(true);
    expect(host.inputs).toHaveLength(0);
  });

  // Would catch: a late selection_set of an earlier click attaching (or reporting) that host
  // after the user already chose another one.
  it("discards a delayed selection result of a superseded choice", async () => {
    const host = scripted({ autoSelect: false });
    const controller = createSurfaceController(host.bridge);
    const first = controller.select("local");
    await tick();
    expect(host.sets.map((set) => set.endpoint)).toEqual(["local"]);
    const second = controller.select("ssh-dev");
    host.sets[0]!.result.resolve(HOSTS.local!);
    await expect(first).rejects.toMatchObject({ code: "selection_changed" });
    await tick();
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual([]);
    host.sets[1]!.result.resolve(HOSTS["ssh-dev"]!);
    await second;
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual(["surface_attach:ssh-dev"]);
    expect(controller.state.selection?.endpoint).toBe("ssh-dev");
  });

  // Would catch: a slow attach of the previous host overwriting the status of the current one.
  it("ignores a late attach status of the previous host", async () => {
    const host = scripted({ autoAttach: false });
    const controller = createSurfaceController(host.bridge);
    const first = controller.select("local").catch((error: RuntimeError) => error);
    await tick();
    const second = controller.select("ssh-dev");
    host.attaches[0]!.result.resolve(status("s-local", { state: "live", boot_id: "boot-local" }));
    await tick();
    expect(controller.state.status?.boot_id ?? null).not.toBe("boot-local");
    await tick();
    host.attaches[1]!.result.resolve(status("s-remote", { state: "connecting" }));
    await second;
    expect(await first).toMatchObject({ code: "selection_changed" });
    expect(controller.state.status?.session).toBe("s-remote");
  });

  // Would catch: input replayed after reconnection or kept enabled while resynchronizing, and
  // the old boot used after the server rebooted.
  it("blocks while stale, never replays refused input and adopts the new boot after reconnect", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    const attach = await liveOn(host, controller, "ssh-dev", "boot-ssh", 7);
    attach.emit({ type: "state", state: "stale", reason: "surface_stale", error: null });
    expect(surfaceStatusText(controller.state)).toBe("Ressincronizando");
    expect(await controller.input(text("x"))).toBe(false);
    attach.emit(identityEvent("boot-ssh-2", 8, "w1:p1"));
    attach.emit(live);
    expect(await controller.input(text("y"))).toBe(true);
    expect(host.inputs.map((i) => [i.expected.boot_id, i.expected.connection_generation, i.events])).toEqual([
      ["boot-ssh-2", 8, text("y")],
    ]);
  });

  // Would catch: retrying a failed input (duplicate keystrokes) or queueing it for later.
  it("sends each input batch once and does not retry on error", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    await liveOn(host, controller, "ssh-dev", "boot-ssh", 7);
    host.failInput({ code: "result_unknown", message: "sem resposta", retryable: false });
    expect(await controller.input(text("once"))).toBe(false);
    host.failInput(null);
    await tick();
    expect(host.inputs).toHaveLength(1);
    expect(controller.state.error?.code).toBe("result_unknown");
  });

  // Would catch: no identity pane (null) still enabling input with an invented pane.
  it("keeps input blocked while the identity has no confirmed pane", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    await controller.select("local");
    const attach = host.attaches.at(-1)!;
    attach.emit(identityEvent("boot-local", 3, null));
    attach.emit(live);
    expect(controller.canInput()).toBe(false);
    expect(await controller.input(text("z"))).toBe(false);
    expect(host.inputs).toEqual([]);
  });

  // Would catch: an unavailable host silently falling back to Local, or "Tentar novamente"
  // re-selecting a different endpoint.
  it("keeps a failed host selected and retries only that host", async () => {
    const host = scripted({ autoAttach: false });
    const controller = createSurfaceController(host.bridge);
    const pending = controller.select("ssh-dev");
    await tick();
    host.attaches[0]!.result.reject({ code: "server_not_running", message: "servidor ausente", retryable: true });
    await pending;
    expect(controller.state.phase).toBe("disconnected");
    expect(controller.state.error?.code).toBe("server_not_running");
    expect(surfaceStatusText(controller.state)).toBe("Desconectado");

    const retry = controller.retry();
    await tick();
    host.attaches[1]!.result.resolve(status("s-remote"));
    await retry;
    expect(host.calls.filter((c) => c.startsWith("selection_set"))).toEqual(["selection_set:ssh-dev"]);
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual([
      "surface_attach:ssh-dev",
      "surface_attach:ssh-dev",
    ]);
    expect(host.calls).not.toContain("selection_set:local");
  });

  // Would catch: an unknown endpoint leaving the previous host's surface and input active.
  it("clears the surface and reports a refused selection without falling back", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    await liveOn(host, controller, "local", "boot-local", 3);
    await expect(controller.select("ssh-gone")).rejects.toMatchObject({ code: "unknown_endpoint" });
    expect(controller.canInput()).toBe(false);
    expect(controller.state.error?.code).toBe("unknown_endpoint");
    expect(host.calls.filter((c) => c.startsWith("selection_set"))).toEqual(["selection_set:local", "selection_set:ssh-gone"]);
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual(["surface_attach:local"]);
  });

  // Would catch: rendering creating engines — loading must never start a session, and only a
  // configured selection is attached.
  it("loads without starting a session and attaches only a configured selection", async () => {
    const empty = scripted();
    const idle = createSurfaceController(empty.bridge);
    await idle.load();
    expect(empty.calls).toEqual(["selection_get", "surface_status"]);
    expect(surfaceStatusText(idle.state)).toBe("Sem sessão");

    const configured = scripted({ selection: HOSTS.local });
    const controller = createSurfaceController(configured.bridge);
    await controller.load();
    expect(configured.calls.filter((c) => c === "session_start")).toEqual([]);
    expect(configured.calls.filter((c) => c.startsWith("surface_attach"))).toEqual(["surface_attach:local"]);
  });

  // Would catch: an unavailable desktop backend leaving the window blank (load rejecting) with
  // no way to try again, or the retry starting a session.
  it("keeps the window usable when the selection cannot be read and retries the read", async () => {
    const host = scripted({ selection: HOSTS.local });
    let failures = 1;
    const selectionGet = host.bridge.selectionGet;
    host.bridge.selectionGet = async () => {
      if (failures-- > 0) throw { code: "server_not_running", message: "servidor ausente", retryable: true };
      return selectionGet();
    };
    const controller = createSurfaceController(host.bridge);
    await controller.load();
    expect(controller.state.error?.code).toBe("server_not_running");
    expect(controller.state.selection).toBeNull();
    await controller.retry();
    expect(controller.state.error).toBeNull();
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual(["surface_attach:local"]);
    expect(host.calls).not.toContain("session_start");
  });

  // Would catch: a Full emitted synchronously by surface_attach reaching nobody (or the view of
  // the previous token) because attach ran before the remounted view subscribed.
  it("attaches only after the remounted view is subscribed, even with a synchronous emitter", async () => {
    const host = scripted();
    const attach = host.bridge.attach;
    host.bridge.attach = (geometry, onEvent) => {
      const marker = host.calls.filter((c) => c.startsWith("surface_attach")).length === 0 ? "L" : "S";
      const result = attach(geometry, onEvent);
      onEvent(identityEvent(`boot-${marker}`, 1, "w1:p1"));
      onEvent(live);
      onEvent(full(marker));
      return result;
    };
    const views: { key: number; frames: string[]; stop: () => void }[] = [];
    let gate = deferred<void>();
    const controller = createSurfaceController(host.bridge, {
      ready: async () => {
        await gate.promise;
        // Simulates the keyed remount: old view unsubscribes, the new one subscribes.
        views.at(-1)?.stop();
        const view = { key: controller.state.surfaceKey, frames: [] as string[], stop: () => {} };
        view.stop = controller.subscribe((event) => {
          if (event.type === "full") view.frames.push(event.cells[0]!.s);
        });
        views.push(view);
      },
    });

    const first = controller.select("local");
    await tick();
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual([]);
    gate.resolve();
    await first;
    gate = deferred<void>();
    const second = controller.select("ssh-dev");
    await tick();
    expect(host.calls.filter((c) => c.startsWith("surface_attach"))).toEqual(["surface_attach:local"]);
    gate.resolve();
    await second;
    expect(views.map((v) => v.frames)).toEqual([["L"], ["S"]]);
    expect(views[1]!.key).toBe(controller.state.surfaceKey);
    expect(views[0]!.key).not.toBe(views[1]!.key);
  });

  // Would catch: agents attached before the surface handshake (attach returning "connecting"),
  // attached twice for one connection, or not reattached after a reboot of the same host.
  it("reports live once per confirmed connection and selection start synchronously", async () => {
    const host = scripted();
    const events: string[] = [];
    const controller = createSurfaceController(host.bridge, {
      onSelectionStart: (endpoint) => events.push(`start:${endpoint}:${host.calls.length}`),
      onLive: (selection, identity) =>
        events.push(`live:${selection.endpoint}:${identity.boot_id}:${identity.connection_generation}`),
    });
    const pending = controller.select("ssh-dev");
    expect(events).toEqual(["start:ssh-dev:0"]);
    await pending;
    expect(controller.state.phase).toBe("connecting");
    const attach = host.attaches.at(-1)!;
    attach.emit(identityEvent("boot-ssh", 7, "w1:p1"));
    expect(events).toEqual(["start:ssh-dev:0"]);
    attach.emit(live);
    attach.emit(live);
    expect(events).toEqual(["start:ssh-dev:0", "live:ssh-dev:boot-ssh:7"]);
    attach.emit({ type: "state", state: "stale", reason: "surface_from_previous_boot", error: null });
    attach.emit(identityEvent("boot-ssh-2", 8, "w1:p1"));
    expect(events).toEqual(["start:ssh-dev:0", "live:ssh-dev:boot-ssh:7"]);
    attach.emit(live);
    expect(events).toEqual(["start:ssh-dev:0", "live:ssh-dev:boot-ssh:7", "live:ssh-dev:boot-ssh-2:8"]);
  });

  // Would catch: a live state without confirmed identity attaching agents.
  it("does not report live before the identity of the connection arrives", async () => {
    const host = scripted();
    const lives: string[] = [];
    const controller = createSurfaceController(host.bridge, { onLive: (s, id) => lives.push(`${s.endpoint}:${id.boot_id}`) });
    await controller.select("local");
    const attach = host.attaches.at(-1)!;
    attach.emit(live);
    expect(lives).toEqual([]);
    attach.emit(identityEvent("boot-local", 3, "w1:p1"));
    expect(lives).toEqual(["local:boot-local"]);
  });

  // Would catch: a late live of the previous host triggering agents for it after the switch.
  it("never reports live for a superseded attachment", async () => {
    const host = scripted();
    const lives: string[] = [];
    const controller = createSurfaceController(host.bridge, { onLive: (s) => lives.push(s.endpoint ?? "") });
    await controller.select("local");
    const localAttach = host.attaches.at(-1)!;
    await controller.select("ssh-dev");
    localAttach.emit(identityEvent("boot-local", 3, "w1:p1"));
    localAttach.emit(live);
    expect(lives).toEqual([]);
  });

  // Would catch: project_open proceeding before the selected surface is live, after another host
  // was chosen, after the attach failed, or waiting forever.
  it("waits for the selected host to be live and refuses otherwise", async () => {
    const host = scripted();
    const controller = createSurfaceController(host.bridge);
    await controller.select("ssh-dev");
    let ready = false;
    const waiting = controller.whenReady("ssh-dev").then(() => (ready = true));
    await tick();
    expect(ready).toBe(false);
    await expect(controller.whenReady("local")).rejects.toMatchObject({ code: "selection_changed" });
    const attach = host.attaches.at(-1)!;
    attach.emit(identityEvent("boot-ssh", 7, "w1:p1"));
    attach.emit(live);
    await waiting;
    expect(ready).toBe(true);

    await expect(controller.whenReady("ssh-dev", { timeoutMs: 20 })).resolves.toBeUndefined();
    const superseded = controller.whenReady("ssh-dev");
    await controller.select("local");
    await expect(superseded).resolves.toBeUndefined();
    await expect(controller.whenReady("ssh-dev")).rejects.toMatchObject({ code: "selection_changed" });
    await expect(controller.whenReady("local", { timeoutMs: 20 })).rejects.toMatchObject({ code: "surface_timeout" });

    const failing = scripted({ autoAttach: false });
    const other = createSurfaceController(failing.bridge);
    const selecting = other.select("ssh-dev");
    const failed = other.whenReady("ssh-dev");
    await tick();
    failing.attaches[0]!.result.reject({ code: "server_not_running", message: "servidor ausente", retryable: true });
    await selecting;
    await expect(failed).rejects.toMatchObject({ code: "server_not_running" });
  });

  // Would catch: status conveyed only by color.
  it("describes every phase with text", () => {
    const base = createSurfaceController(scripted().bridge).state;
    expect(surfaceStatusText({ ...base, phase: "live", selection: HOSTS.local! })).toBe("Conectado");
    expect(surfaceStatusText({ ...base, phase: "connecting", selection: HOSTS.local! })).toBe("Conectando");
    expect(surfaceStatusText({ ...base, phase: "switching", selection: HOSTS.local! })).toBe("Trocando de host");
  });

  // Would catch: the header pill carrying the state only in the dot color, or showing a host
  // when nothing is selected.
  it("labels the header connection with the host and the state in text", () => {
    const base = createSurfaceController(scripted().bridge).state;
    expect(workbenchConnection(base)).toBeUndefined();
    expect(workbenchConnection({ ...base, phase: "stale", selection: HOSTS["ssh-dev"]! })).toEqual({
      name: "dev-box — Ressincronizando",
      kind: "ssh",
      status: "stale",
    });
    expect(workbenchConnection({ ...base, phase: "disconnected", selection: HOSTS.local! })).toEqual({
      name: "Local — Desconectado",
      kind: "local",
      status: "disconnected",
    });
  });
});

// Spec 035 AC-035-01/02 — a switch between two hosts that stay online is a surface handover:
// the leaving channel goes quiet and the entered one goes live without any disconnected phase or
// error/banner (`desconectado: host_reconnecting` was the measured false banner).
describe("host switch (spec 035)", () => {
  // Would catch: the controller turning the switch into `disconnected` (or showing a retryable
  // error) for a host that is online, or a late renegotiation event of the superseded channel
  // turning the newly entered host off.
  it("switches between online hosts without any disconnected phase or banner", async () => {
    const host = scripted();
    const phases: string[] = [];
    const errors: (string | null)[] = [];
    const controller = createSurfaceController(host.bridge, {
      onChange: (state) => {
        phases.push(state.phase);
        errors.push(state.error?.code ?? null);
      },
    });
    await liveOn(host, controller, "local", "boot-local", 3);
    const leaving = host.attaches.at(-1)!;

    // The entered channel goes live on the same connection of its host.
    await liveOn(host, controller, "ssh-dev", "boot-ssh", 7);
    // The old channel reports a failed renegotiation after the switch: it belongs to the
    // superseded attach and never reaches the new host.
    leaving.emit({ type: "state", state: "disconnected", reason: "host_reconnecting", error: null });
    await tick();

    expect(controller.state.phase).toBe("live");
    expect(controller.state.error).toBeNull();
    expect(surfaceStatusText(controller.state)).toBe("Conectado");
    expect(phases).not.toContain("disconnected");
    expect(errors.filter((code) => code !== null)).toEqual([]);
    expect(host.calls.filter((call) => call.startsWith("surface_attach"))).toEqual([
      "surface_attach:local",
      "surface_attach:ssh-dev",
    ]);
  });
});

// Spec 028 AC-028-04 — the paste gesture (Ctrl+Shift+V / Cmd+V) invokes the native command once
// and the backend's answer (text or image) is reported on the state without clipboard content.
describe("native paste result (AC-028-04)", () => {
  // Would catch: an image paste not reported (or reported as text), more than one invoke per
  // gesture, or the receipt's bytes lost on the state.
  it("reports the image result of one native_paste", async () => {
    const host = scripted({
      paste: async (expected) => {
        expect(expected.pane_id).toBe("w1:p1");
        return { pane_id: "w1:p1", sent: true, pasted_bytes: 4096, kind: "image" };
      },
    });
    const controller = createSurfaceController(host.bridge);
    await liveOn(host, controller, "local", "boot-local", 3);

    expect(await controller.nativePaste()).toBe(true);
    expect(host.calls.filter((call) => call === "surface_paste_clipboard")).toHaveLength(1);
    expect(controller.state.paste).toEqual({ pane_id: "w1:p1", sent: true, bytes: 4096, kind: "image" });
    expect(controller.state.error).toBeNull();
  });

  // Would catch: an empty clipboard reported as text/image or raising an error.
  it("reports an empty clipboard as empty without an error", async () => {
    const host = scripted({
      paste: async () => ({ pane_id: "w1:p1", sent: false, pasted_bytes: 0 }),
    });
    const controller = createSurfaceController(host.bridge);
    await liveOn(host, controller, "local", "boot-local", 3);

    expect(await controller.nativePaste()).toBe(false);
    expect(controller.state.paste).toEqual({ pane_id: "w1:p1", sent: false, bytes: 0, kind: "empty" });
    expect(controller.state.error).toBeNull();
  });

  // Spec 028 r3 — a Local image paste is not a bridged image: the backend answers `forward_key`
  // (one Ctrl+V sent; the app reads the clipboard). Would catch: that answer reported as an
  // error, as an empty clipboard or as a pasted image.
  it("reports the forward_key answer of a local image paste", async () => {
    const host = scripted({
      paste: async () => ({ pane_id: "w1:p1", sent: true, pasted_bytes: 0, kind: "forward_key" }),
    });
    const controller = createSurfaceController(host.bridge);
    await liveOn(host, controller, "local", "boot-local", 3);

    expect(await controller.nativePaste()).toBe(true);
    expect(host.calls.filter((call) => call === "surface_paste_clipboard")).toHaveLength(1);
    expect(controller.state.paste).toEqual({ pane_id: "w1:p1", sent: true, bytes: 0, kind: "forward_key" });
    expect(controller.state.error).toBeNull();
  });
});

// Spec 037 AC-037-02 — "Iniciando o Herdr…" só aparece quando o desktop está de fato iniciando
// o servidor Local (016). Para host SSH Online na primeira visita, o status é "Conectando", nunca startingHerdr().
describe("instant host switch and status messages (spec 037)", () => {
  it("host SSH Online na primeira visita has status text != startingHerdr() (AC-037-02)", async () => {
    const host = scripted({
      selection: { endpoint: "ssh-dev", kind: "ssh", label: "dev-box", session: "default", online: true, identity: null },
      autoAttach: false,
    });
    const controller = createSurfaceController(host.bridge);
    const loading = controller.load();
    await tick();

    // Connecting to an SSH host with session "default": must NOT show startingHerdr()!
    expect(controller.state.phase).toBe("connecting");
    expect(surfaceStatusText(controller.state)).not.toBe(startingHerdr());
    expect(surfaceStatusText(controller.state)).toBe("Conectando");

    // Switching phase also does not show startingHerdr()
    const switchingState = { ...controller.state, phase: "switching" as const };
    expect(surfaceStatusText(switchingState)).toBe("Trocando de host");
    expect(surfaceStatusText(switchingState)).not.toBe(startingHerdr());

    // Only Local with session "default" in connecting shows startingHerdr()
    const localStartingState = {
      ...controller.state,
      phase: "connecting" as const,
      selection: { endpoint: "local", kind: "local" as const, label: "Local", session: "default", online: true, identity: null },
    };
    expect(surfaceStatusText(localStartingState)).toBe(startingHerdr());

    // Local with named session in connecting shows "Conectando"
    const localNamedState = {
      ...controller.state,
      phase: "connecting" as const,
      selection: { endpoint: "local", kind: "local" as const, label: "Local", session: "named-session", online: true, identity: null },
    };
    expect(surfaceStatusText(localNamedState)).toBe("Conectando");

    // Resolve attach to finish loading cleanly
    host.attaches[0]!.emit(identityEvent("boot-ssh", 1, "w1:p1"));
    host.attaches[0]!.emit(live);
    host.attaches[0]!.result.resolve(status("default", { state: "live" }));
    await loading;
    expect(surfaceStatusText(controller.state)).toBe("Conectado");
  });

  it("blocks input when switching to another host and keeps it blocked during switching (AC-037-01)", async () => {
    const host = scripted({ autoAttach: false });
    const controller = createSurfaceController(host.bridge);

    // Make local live
    const localLoading = controller.select("local");
    await tick();
    const localAttach = host.attaches[0]!;
    localAttach.emit(identityEvent("boot-local", 1, "w1:p1"));
    localAttach.emit(live);
    localAttach.result.resolve(status("s-local", { state: "live" }));
    await localLoading;
    expect(controller.canInput()).toBe(true);

    // Switch to ssh-dev: input is immediately blocked
    const switching = controller.select("ssh-dev");
    expect(controller.canInput()).toBe(false);
    expect(await controller.input(text("lost"))).toBe(false);
    await tick();

    // Attach for ssh-dev is running, input remains blocked
    const sshAttach = host.attaches[1]!;
    expect(controller.canInput()).toBe(false);

    // Finish ssh-dev attach
    sshAttach.emit(identityEvent("boot-ssh", 2, "w1:p1"));
    sshAttach.emit(live);
    sshAttach.result.resolve(status("s-remote", { state: "live" }));
    await switching;
    expect(controller.canInput()).toBe(true);
  });
});
