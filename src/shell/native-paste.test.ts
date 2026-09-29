// Spec 007 — native Ctrl+Shift+V through the real IPC bridge and the same ordered input lane as
// typing (tauriSurfaceBridge with an injected invoke/channel and the real controller).
// Contract: .local/orchestration/native-paste-contract.md. The WebView never sees the clipboard
// text: the frontend sends only the identity captured at the gesture and receives a receipt
// without content (`surface_paste_clipboard`). A slow paste must not be overtaken by the next key,
// and earlier input must not be discarded or reordered.
import { describe, expect, it } from "vitest";
import {
  MAX_PENDING_INPUT_BATCHES,
  MAX_PENDING_INPUT_BYTES,
  tauriSurfaceBridge,
  type SurfaceBridge,
  type SurfaceIpc,
} from "./bridge";
import { NATIVE_PASTE_RESERVED_BYTES, createSurfaceController } from "./controller";
import type { FrameEvent, InputDto, PasteReceipt, SelectionDto, StatusDto, SurfaceIdentityDto } from "./types";

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
const receipt = (sent: boolean, pastedBytes: number): PasteReceipt => ({ pane_id: "w1:p1", sent, pasted_bytes: pastedBytes });

interface InputCall {
  expected: SurfaceIdentityDto;
  events: InputDto[];
  result: Deferred<void>;
}
interface PasteCall {
  expected: SurfaceIdentityDto;
  result: Deferred<PasteReceipt>;
}

/** Fake Tauri IPC: every command is recorded; input and paste answer only when the test says. */
function fakeIpc() {
  const calls: string[] = [];
  const inputs: InputCall[] = [];
  const pastes: PasteCall[] = [];
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
        case "surface_paste_clipboard": {
          const call: PasteCall = { expected: args.expected as SurfaceIdentityDto, result: deferred<PasteReceipt>() };
          pastes.push(call);
          return call.result.promise;
        }
        default:
          return undefined;
      }
    }) as SurfaceIpc["invoke"],
  };
  const count = (command: string) => calls.filter((call) => call === command).length;
  const order = () => calls.filter((call) => call === "surface_input" || call === "surface_paste_clipboard");
  return { ipc, calls, inputs, pastes, channels, count, order };
}

async function liveOn(
  ipc: ReturnType<typeof fakeIpc>,
  controller: ReturnType<typeof createSurfaceController>,
  endpoint: string,
  boot: string,
  generation: number,
) {
  await controller.select(endpoint);
  const emit = ipc.channels.at(-1)!;
  emit(identity(boot, generation));
  emit(live);
  return emit;
}

const CONFIRMED: SurfaceIdentityDto = {
  endpoint: "ssh-dev",
  session: "s-remote-hd007i",
  connection_generation: 7,
  boot_id: "boot-ssh-3",
  pane_id: "w1:p1",
};

function setup() {
  const ipc = fakeIpc();
  const controller = createSurfaceController(tauriSurfaceBridge(ipc.ipc));
  return { ipc, controller };
}

describe("native paste dispatch (spec 007)", () => {
  // Would catch: the paste running outside the input lane (the next key overtaking a slow paste
  // or the paste overtaking earlier input), the identity echoed without the attach token captured
  // at the gesture, or the command invoked with anything but the confirmed identity.
  it("keeps one paste in flight and sends the following key in call order", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const held = controller.input(text("antes"));
    const pasted = controller.nativePaste();
    const following = controller.input(text("depois"));
    await flush();
    expect(ipc.count("surface_input")).toBe(1);
    expect(ipc.count("surface_paste_clipboard")).toBe(0);
    expect(ipc.inputs[0]!.expected).toEqual(CONFIRMED);
    ipc.inputs[0]!.result.resolve();
    expect(await held).toBe(true);
    await flush();

    expect(ipc.count("surface_paste_clipboard")).toBe(1);
    expect(ipc.count("surface_input")).toBe(1);
    expect(ipc.pastes[0]!.expected).toEqual(CONFIRMED);
    ipc.pastes[0]!.result.resolve(receipt(true, 42));
    expect(await pasted).toBe(true);
    await flush();

    expect(ipc.order()).toEqual(["surface_input", "surface_paste_clipboard", "surface_input"]);
    expect(ipc.inputs[1]!.events).toEqual(text("depois"));
    ipc.inputs[1]!.result.resolve();
    expect(await following).toBe(true);
  });

  // Would catch: an empty clipboard reported as an error or as a sent paste, and the lane stalling
  // after a receipt with nothing to send.
  it("answers an empty clipboard with false, no error and moves the lane on", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const pasted = controller.nativePaste();
    await flush();
    ipc.pastes[0]!.result.resolve(receipt(false, 0));
    expect(await pasted).toBe(false);
    expect(controller.state.error).toBeNull();
    const next = controller.input(text("segue"));
    await flush();
    expect(ipc.count("surface_input")).toBe(1);
    ipc.inputs[0]!.result.resolve();
    expect(await next).toBe(true);
    expect(ipc.order()).toEqual(["surface_paste_clipboard", "surface_input"]);
  });

  // Would catch: a failed read answered as success, retried, or stalling the lane.
  it("answers a failed paste with its error once and still sends the next key", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const pasted = controller.nativePaste();
    await flush();
    ipc.pastes[0]!.result.reject({ code: "clipboard_failed", message: "sem dono da área de transferência", retryable: true });
    expect(await pasted).toBe(false);
    expect(controller.state.error?.code).toBe("clipboard_failed");
    const next = controller.input(text("depois"));
    await flush();
    expect(ipc.order()).toEqual(["surface_paste_clipboard", "surface_input"]);
    ipc.inputs[0]!.result.resolve();
    expect(await next).toBe(true);
  });

  // Would catch: the queue keeping the exposed identity object by reference, so a caller mutation
  // retargets the waiting paste, and the paste being sent before its turn.
  it("sends the identity captured at the gesture, whatever the caller mutates afterwards", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const held = controller.input(text("retido"));
    const pasted = controller.nativePaste();
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(0);
    const exposed = controller.state.identity as SurfaceIdentityDto;
    exposed.boot_id = "boot-mutado";
    exposed.connection_generation = 99;
    exposed.pane_id = "w9:p9";
    ipc.inputs[0]!.result.resolve();
    expect(await held).toBe(true);
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(1);
    expect(ipc.pastes[0]!.expected).toEqual(CONFIRMED);
    ipc.pastes[0]!.result.resolve(receipt(true, 3));
    expect(await pasted).toBe(true);
  });

  // Would catch: a paste captured for one attach sent after a switch (to the old or the new
  // identity); the switch itself must not wait for the paste. The old attach's refusal is not
  // shown on the new selection (the same rule the typed input queue follows).
  it("refuses a queued paste on a host switch, without invoking the command", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const held = controller.input(text("voando"));
    const queued = controller.nativePaste();
    await flush();
    const switched = controller.select("local");
    expect(await queued).toBe(false);
    await switched;
    const emit = ipc.channels.at(-1)!;
    emit(identity("boot-local-7", 3));
    emit(live);
    ipc.inputs[0]!.result.resolve();
    expect(await held).toBe(true);
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(0);

    // A paste of the new attach goes normally.
    const after = controller.nativePaste();
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(1);
    expect(ipc.pastes[0]!.expected).toEqual({
      endpoint: "local",
      session: "s-local-hd007i",
      connection_generation: 3,
      boot_id: "boot-local-7",
      pane_id: "w1:p1",
    });
    ipc.pastes[0]!.result.resolve(receipt(true, 5));
    expect(await after).toBe(true);
  });

  // Would catch: a paste queued for a connection of the same attach sent after the connection
  // changed (reconnect/reboot), and the refusal not being reported to that attach.
  it("refuses a queued paste when the identity changes and reports it once", async () => {
    const { ipc, controller } = setup();
    const emit = await liveOn(ipc, controller, "ssh-dev", "boot-ssh-3", 7);
    const held = controller.input(text("voando"));
    const queued = controller.nativePaste();
    await flush();
    emit({ type: "state", state: "connecting", reason: "host_reconnecting", error: null });
    expect(controller.state.error?.code).toBe("input_cancelled");
    expect(await queued).toBe(false);
    ipc.inputs[0]!.result.resolve();
    expect(await held).toBe(true);
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(0);
  });

  // Would catch: queued or new pastes sent after the window is disposed.
  it("refuses a queued paste after dispose", async () => {
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const held = controller.input(text("voando"));
    const queued = controller.nativePaste();
    await flush();
    controller.dispose();
    expect(await queued).toBe(false);
    expect(await controller.nativePaste()).toBe(false);
    ipc.inputs[0]!.result.resolve();
    expect(await held).toBe(true);
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(0);
  });

  // Would catch: a paste before any live surface or without a confirmed pane sending anything.
  it("sends nothing without a confirmed identity", async () => {
    const { ipc, controller } = setup();
    expect(await controller.nativePaste()).toBe(false);
    expect(controller.state.error).toBeNull();
    await flush();
    expect(ipc.pastes).toHaveLength(0);
    expect(ipc.calls).not.toContain("surface_paste_clipboard");
  });

  // Would catch: the native paste escaping the lane's batch limit, or its reservation being
  // different from the 64 bytes the backend accounts at the call (Rust NATIVE_PASTE_RESERVED_BYTES).
  it("bounds pastes by batch count and by the 64-byte reservation", async () => {
    expect(NATIVE_PASTE_RESERVED_BYTES).toBe(64);
    const { ipc, controller } = setup();
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    for (let n = 0; n < MAX_PENDING_INPUT_BATCHES; n += 1) void controller.input(text(`${n}`));
    expect(await controller.nativePaste()).toBe(false);
    expect(controller.state.error?.code).toBe("input_overflow");
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(0);
    for (let n = 0; n < MAX_PENDING_INPUT_BATCHES; n += 1) {
      ipc.inputs[n]!.result.resolve();
      await flush();
    }

    // Bytes: a batch that leaves exactly 64 free admits the paste; the reservation is then spent,
    // so the next paste has no room and is refused before any invoke.
    const encoder = new TextEncoder();
    const base = encoder.encode(JSON.stringify(text(""))).length;
    const padding = MAX_PENDING_INPUT_BYTES - NATIVE_PASTE_RESERVED_BYTES - base;
    const held = controller.input(text("a".repeat(padding)));
    await flush();
    expect(encoder.encode(JSON.stringify(text("a".repeat(padding)))).length).toBe(MAX_PENDING_INPUT_BYTES - NATIVE_PASTE_RESERVED_BYTES);
    const exact = controller.nativePaste();
    const overflow = controller.nativePaste();
    expect(await overflow).toBe(false);
    expect(controller.state.error?.code).toBe("input_overflow");
    ipc.inputs.at(-1)!.result.resolve();
    expect(await held).toBe(true);
    await flush();
    expect(ipc.count("surface_paste_clipboard")).toBe(1);
    ipc.pastes[0]!.result.resolve(receipt(true, 1));
    expect(await exact).toBe(true);
  });

  // Would catch: a legacy mock without the native command fabricating a paste or wedging the lane
  // instead of refusing explicitly (Rust refuses the same way: `native_paste_unavailable`).
  it("refuses without a pasteClipboard bridge and keeps the lane usable", async () => {
    const ipc = fakeIpc();
    const legacy: SurfaceBridge = { ...tauriSurfaceBridge(ipc.ipc), pasteClipboard: undefined };
    const controller = createSurfaceController(legacy);
    await liveOn(ipc, controller, "local", "boot-local-7", 3);
    const pasted = controller.nativePaste();
    expect(await pasted).toBe(false);
    expect(controller.state.error?.code).toBe("native_paste_unavailable");
    expect(ipc.calls).not.toContain("surface_paste_clipboard");
    const next = controller.input(text("segue"));
    await flush();
    expect(ipc.count("surface_input")).toBe(1);
    ipc.inputs[0]!.result.resolve();
    expect(await next).toBe(true);
  });
});
