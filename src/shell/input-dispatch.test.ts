// Spec 007 — ordered, bounded input of the composed surface through the real IPC bridge
// (tauriSurfaceBridge with an injected invoke/channel) and the real controller.
// Contract: .local/orchestration/input-dispatch-contract.md.
// Local and SSH share pane "w1:p1" and differ in boot, generation and session, so input echoed
// with the wrong identity fails the assertions.
import { describe, expect, it } from "vitest";
import { MAX_PENDING_INPUT_BATCHES, MAX_PENDING_INPUT_BYTES, tauriSurfaceBridge, type SurfaceIpc } from "./bridge";
import { createSurfaceController } from "./controller";
import type { FrameEvent, InputDto, SelectionDto, StatusDto, SurfaceIdentityDto } from "./types";

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
  for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const HOSTS: Record<string, SelectionDto> = {
  local: { endpoint: "local", kind: "local", label: "Local", session: "s-local-hd007i", online: true, identity: null },
  "ssh-dev": { endpoint: "ssh-dev", kind: "ssh", label: "dev-box", session: "s-remote-hd007i", online: true, identity: null },
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
const identity = (boot: string, generation: number): FrameEvent => ({
  type: "identity",
  boot_id: boot,
  generation: 1,
  connection_generation: generation,
  server_version: "",
  pane_id: "w1:p1",
});
const live: FrameEvent = { type: "state", state: "live", reason: null, error: null };
const text = (value: string): InputDto[] => [{ kind: "text", text: value }];

interface InputCall {
  expected: SurfaceIdentityDto;
  events: InputDto[];
  result: Deferred<void>;
}

/** Fake Tauri IPC: every command is recorded; surface_input answers only when the test says. */
function fakeIpc() {
  const calls: string[] = [];
  const inputs: InputCall[] = [];
  const channels: ((event: FrameEvent) => void)[] = [];
  let selected: string | null = null;
  const ipc: SurfaceIpc = {
    channel: (onEvent) => ({ emit: onEvent }),
    invoke: (async (command: string, args: Record<string, unknown> = {}) => {
      calls.push(command);
      switch (command) {
        case "selection_set":
          selected = args.endpoint as string;
          return HOSTS[selected];
        case "surface_attach":
          channels.push((args.onEvent as { emit: (event: FrameEvent) => void }).emit);
          return status(HOSTS[selected ?? ""]?.session ?? "none");
        case "surface_detach":
        case "surface_status":
          return status("none");
        case "surface_input": {
          const call: InputCall = {
            expected: args.expected as SurfaceIdentityDto,
            events: args.events as InputDto[],
            result: deferred<void>(),
          };
          inputs.push(call);
          return call.result.promise;
        }
        default:
          return undefined;
      }
    }) as SurfaceIpc["invoke"],
  };
  return { ipc, calls, inputs, channels, count: () => calls.filter((c) => c === "surface_input").length };
}

async function liveOn(ipc: ReturnType<typeof fakeIpc>, controller: ReturnType<typeof createSurfaceController>, endpoint: string, boot: string, generation: number) {
  await controller.select(endpoint);
  const emit = ipc.channels.at(-1)!;
  emit(identity(boot, generation));
  emit(live);
  return emit;
}

function setup() {
  const ipc = fakeIpc();
  const controller = createSurfaceController(tauriSurfaceBridge(ipc.ipc));
  return { ipc, controller };
}

describe("surface input dispatch (spec 007)", () => {
  // Would catch: concurrent surface_input invokes (the backend may run them in any order),
  // batches sent out of call order, or a result delivered to the wrong call.
  it("keeps one surface_input in flight and sends batches in call order", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const results = ["a", "b", "c"].map((t) => controller.input(text(t)));
    await flush();
    expect(ipc.count()).toBe(1);
    expect(ipc.inputs[0]!.events).toEqual(text("a"));
    expect(ipc.inputs[0]!.expected).toEqual({
      endpoint: "ssh-dev",
      session: "s-remote-hd007i",
      connection_generation: 7,
      boot_id: "boot-ssh-3",
      pane_id: "w1:p1",
    });
    ipc.inputs[0]!.result.resolve();
    await flush();
    expect(ipc.count()).toBe(2);
    expect(ipc.inputs[1]!.events).toEqual(text("b"));
    ipc.inputs[1]!.result.resolve();
    await flush();
    ipc.inputs[2]!.result.resolve();
    expect(await Promise.all(results)).toEqual([true, true, true]);
    expect(ipc.inputs.map((i) => i.events[0])).toEqual([text("a")[0], text("b")[0], text("c")[0]]);
  });

  // Would catch: a failed batch stalling the queue, being retried, or its unknown result
  // silently reported as success.
  it("answers a failed batch with its error once and still sends the next batch", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const first = controller.input(text("perdido"));
    const second = controller.input(text("depois"));
    await flush();
    ipc.inputs[0]!.result.reject({ code: "input_interrupted", message: "sem resultado", retryable: false });
    expect(await first).toBe(false);
    expect(controller.state.error?.code).toBe("input_interrupted");
    await flush();
    expect(ipc.count()).toBe(2);
    ipc.inputs[1]!.result.resolve();
    expect(await second).toBe(true);
    expect(ipc.inputs.map((i) => (i.events[0] as { text: string }).text)).toEqual(["perdido", "depois"]);
  });

  // Would catch: an unbounded queue, the in-flight batch not counted, or an excess batch sent
  // (or queued) instead of refused before any invoke.
  it("bounds pending batches and bytes, in-flight included, and refuses the excess before sending", async () => {
    expect(MAX_PENDING_INPUT_BATCHES).toBe(256);
    expect(MAX_PENDING_INPUT_BYTES).toBe(1024 * 1024);
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const accepted = Array.from({ length: MAX_PENDING_INPUT_BATCHES }, (_, n) => controller.input(text(`${n}`)));
    expect(await controller.input(text("excesso"))).toBe(false);
    expect(controller.state.error?.code).toBe("input_overflow");
    await flush();
    expect(ipc.count()).toBe(1);
    for (let n = 0; n < MAX_PENDING_INPUT_BATCHES; n += 1) {
      ipc.inputs[n]!.result.resolve();
      await flush();
    }
    expect((await Promise.all(accepted)).every(Boolean)).toBe(true);
    expect(ipc.count()).toBe(MAX_PENDING_INPUT_BATCHES);
    expect(ipc.inputs.some((i) => (i.events[0] as { text: string }).text === "excesso")).toBe(false);

    // Bytes: a held half-megabyte batch plus another one exceed the limit; capacity returns.
    const half = "a".repeat(MAX_PENDING_INPUT_BYTES / 2);
    const big = controller.input(text(half));
    await flush();
    expect(await controller.input(text("b".repeat(MAX_PENDING_INPUT_BYTES / 2)))).toBe(false);
    expect(controller.state.error?.code).toBe("input_overflow");
    expect(await controller.input(text("x".repeat(MAX_PENDING_INPUT_BYTES + 1)))).toBe(false);
    expect(controller.state.error?.code).toBe("input_too_large");
    ipc.inputs.at(-1)!.result.resolve();
    expect(await big).toBe(true);
    const again = controller.input(text("c".repeat(MAX_PENDING_INPUT_BYTES / 2)));
    await flush();
    ipc.inputs.at(-1)!.result.resolve();
    expect(await again).toBe(true);
    expect(ipc.count()).toBe(MAX_PENDING_INPUT_BATCHES + 2);
  });

  // Would catch: queued batches of the previous host sent after switching (to the new host's
  // identity, or to the old one), the switch waiting for a slow write before showing the
  // blocked state, or a late error of the old write overwriting the new selection's state.
  it("refuses queued batches on a host switch without waiting for the write in flight", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const flying = controller.input(text("voando"));
    const queued = [controller.input(text("fila-1")), controller.input(text("fila-2"))];
    await flush();
    const switched = controller.select("ssh-dev");
    expect(controller.state.phase).toBe("switching");
    expect(controller.canInput()).toBe(false);
    expect(await Promise.all(queued)).toEqual([false, false]);
    await switched;
    const emit = ipc.channels.at(-1)!;
    emit(identity("boot-ssh-3", 7));
    emit(live);
    expect(controller.state.phase).toBe("live");
    expect(ipc.count()).toBe(1);

    ipc.inputs[0]!.result.reject({ code: "target_boot_stale", message: "antigo", retryable: false });
    expect(await flying).toBe(false);
    expect(controller.state.error).toBeNull();
    const remote = controller.input(text("remoto"));
    await flush();
    expect(ipc.count()).toBe(2);
    expect(ipc.inputs[1]!.expected.boot_id).toBe("boot-ssh-3");
    ipc.inputs[1]!.result.resolve();
    expect(await remote).toBe(true);
    expect(ipc.inputs.map((i) => (i.events[0] as { text: string }).text)).toEqual(["voando", "remoto"]);
  });

  // Would catch: queued batches captured for one connection sent after a reconnection to another
  // boot, or after the surface stops being live, and a silent refusal.
  it("refuses queued batches when the identity changes or the surface leaves live", async () => {
    const { ipc, controller } = setup();
    const emit = await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const flying = controller.input(text("voando"));
    const queued = controller.input(text("boot-antigo"));
    await flush();
    emit({ type: "state", state: "connecting", reason: "host_reconnecting", error: null });
    expect(controller.state.error?.code).toBe("input_cancelled");
    emit(identity("boot-ssh-9", 8));
    emit(live);
    expect(await queued).toBe(false);
    ipc.inputs[0]!.result.resolve();
    expect(await flying).toBe(true);
    await flush();
    expect(ipc.count()).toBe(1);

    const fresh = controller.input(text("novo"));
    const stale = controller.input(text("antes-do-stale"));
    await flush();
    emit({ type: "state", state: "stale", reason: "surface_stale", error: null });
    expect(await stale).toBe(false);
    ipc.inputs[1]!.result.resolve();
    expect(await fresh).toBe(true);
    await flush();
    expect(ipc.inputs.map((i) => [(i.events[0] as { text: string }).text, i.expected.boot_id])).toEqual([
      ["voando", "boot-ssh-3"],
      ["novo", "boot-ssh-9"],
    ]);
  });

  // Would catch: the queue keeping the caller's arrays/events (or the exposed identity object) by
  // reference, so mutations after admission change the bytes or target of a waiting batch and
  // bypass the budget computed at admission.
  it("sends a waiting batch exactly as admitted, whatever the caller mutates afterwards", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const held = controller.input(text("retido"));
    await flush();
    expect(ipc.count()).toBe(1);

    const pixel = { x: 9, y: 18 };
    const geometry = { cols: 80, rows: 24, width_px: 720, height_px: 432 };
    const batch: InputDto[] = [
      { kind: "mouse", action: "down", button: "left", column: 1, row: 2, pixel, geometry, modifiers: 0, lines: 3 },
      { kind: "text", text: "ok" },
    ];
    const admitted = structuredClone(batch);
    const admittedBytes = new TextEncoder().encode(JSON.stringify(admitted)).length;
    const admittedIdentity = structuredClone(controller.state.identity);
    const waiting = controller.input(batch);

    // Caller mutates everything it can reach after admission.
    pixel.x = 999;
    geometry.cols = 1;
    (batch[1] as { text: string }).text = "z".repeat(MAX_PENDING_INPUT_BYTES + 1);
    batch.push({ kind: "text", text: "intruso" });
    const exposed = controller.state.identity as SurfaceIdentityDto;
    exposed.boot_id = "boot-mutado";
    exposed.connection_generation = 99;
    exposed.pane_id = "w9:p9";

    ipc.inputs[0]!.result.resolve();
    expect(await held).toBe(true);
    await flush();
    expect(ipc.count()).toBe(2);
    const sent = ipc.inputs[1]!;
    expect(sent.events).toEqual(admitted);
    expect(new TextEncoder().encode(JSON.stringify(sent.events)).length).toBe(admittedBytes);
    expect(sent.expected).toEqual(admittedIdentity);
    expect(sent.events).not.toBe(batch);
    expect((sent.events[0] as { pixel: unknown }).pixel).not.toBe(pixel);
    expect((sent.events[0] as { geometry: unknown }).geometry).not.toBe(geometry);
    sent.result.resolve();
    expect(await waiting).toBe(true);

    // The next batch still targets the confirmed identity, not the mutated exposed object.
    const next = controller.input(text("depois"));
    await flush();
    expect(ipc.count()).toBe(3);
    expect(ipc.inputs[2]!.expected).toEqual(admittedIdentity);
    ipc.inputs[2]!.result.resolve();
    expect(await next).toBe(true);
  });

  // Would catch: batches still sent after the controller was disposed (window closing).
  it("refuses queued and new batches after dispose", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const flying = controller.input(text("voando"));
    const queued = controller.input(text("fila"));
    await flush();
    controller.dispose();
    expect(await queued).toBe(false);
    expect(await controller.input(text("depois"))).toBe(false);
    ipc.inputs[0]!.result.resolve();
    expect(await flying).toBe(true);
    await flush();
    expect(ipc.count()).toBe(1);
  });
});
