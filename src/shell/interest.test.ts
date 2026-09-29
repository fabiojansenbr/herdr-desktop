// Spec 007 — surface interest of the composed window through the real IPC bridge and controller.
// Contract: .local/orchestration/interest-contract.md. Local and SSH share pane "w1:p1" and differ
// in boot/generation/session.
import { describe, expect, it } from "vitest";
import { tauriSurfaceBridge, type SurfaceBridge, type SurfaceIpc } from "./bridge";
import { createSurfaceController } from "./controller";
import { applyWindowSurfaceAction, INITIAL_WINDOW_SURFACE, surfaceActiveFromWindow } from "./interest";
import type { FrameEvent, GeometryDto, InputDto, SelectionDto, StatusDto } from "./types";

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
  local: { endpoint: "local", kind: "local", label: "Local", session: "s-local-hd007n", online: true, identity: null },
  "ssh-dev": { endpoint: "ssh-dev", kind: "ssh", label: "dev-box", session: "s-remote-hd007n", online: true, identity: null },
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
const full: FrameEvent = { type: "full", revision: 9, width: 2, height: 1, cells: [], cursor: null, panes: [] };
const text = (value: string): InputDto[] => [{ kind: "text", text: value }];

interface InterestCall {
  endpoint: string | null;
  active: boolean;
  result: Deferred<unknown>;
}

function fakeIpc(options: { held?: boolean; inputHeld?: Deferred<void> } = {}) {
  const calls: string[] = [];
  const interest: InterestCall[] = [];
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
        case "surface_status":
          return status("none");
        case "surface_interest": {
          const call: InterestCall = { endpoint: selected, active: args.active as boolean, result: deferred() };
          interest.push(call);
          if (!options.held) call.result.resolve(null);
          return call.result.promise;
        }
        case "surface_resize":
          return undefined;
        case "surface_input":
          return options.inputHeld?.promise;
        default:
          return undefined;
      }
    }) as SurfaceIpc["invoke"],
  };
  return { ipc, calls, interest, channels, inputs: () => calls.filter((c) => c === "surface_input").length };
}

async function liveOn(fake: ReturnType<typeof fakeIpc>, controller: ReturnType<typeof createSurfaceController>, endpoint: string, boot: string, generation: number) {
  await controller.select(endpoint);
  const emit = fake.channels.at(-1)!;
  emit(identity(boot, generation));
  emit(full);
  emit(live);
  return emit;
}

describe("surface interest (spec 007)", () => {
  // Would catch: input still flowing while hidden, interest not sent or sent repeatedly, or a
  // window blur treated as hiding.
  it("hiding closes input at once, sends active:false once and ignores focus changes", async () => {
    const fake = fakeIpc();
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc));
    await liveOn(fake, controller, "ssh-dev", "boot-ssh-3", 7);
    expect(await controller.input(text("control"))).toBe(true);
    const sent = fake.inputs();

    await controller.focus(false);
    expect(fake.interest).toHaveLength(0);
    const hiding = controller.setInterest(false);
    expect(controller.canInput()).toBe(false);
    expect(await controller.input(text("hidden"))).toBe(false);
    await hiding;
    await controller.setInterest(false);
    expect(fake.interest.map((c) => [c.endpoint, c.active])).toEqual([["ssh-dev", false]]);
    expect(fake.inputs()).toBe(sent);
  });

  // Would catch: reopening on the acknowledgement alone or on a full frame alone.
  it.each(["ack-first", "full-first"])("showing reopens only after acknowledgement and a full frame (%s)", async (order) => {
    const fake = fakeIpc({ held: true });
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc));
    const emit = await liveOn(fake, controller, "local", "boot-local-4", 2);
    const hiding = controller.setInterest(false);
    await flush();
    fake.interest[0]!.result.resolve(null);
    await hiding;
    const showing = controller.setInterest(true);
    await flush();
    expect(fake.interest.map((c) => c.active)).toEqual([false, true]);
    if (order === "ack-first") {
      fake.interest[1]!.result.resolve({ endpoint: "local", active: true, sent: true, supported: true, floor: 5 });
      await showing;
      expect(controller.canInput()).toBe(false);
      emit(full);
      emit(live);
    } else {
      emit(full);
      emit(live);
      expect(controller.canInput()).toBe(false);
      fake.interest[1]!.result.resolve(null);
      await showing;
    }
    expect(controller.canInput()).toBe(true);
    expect(await controller.input(text("back"))).toBe(true);
  });

  // Would catch: a late acknowledgement or full of an older show reopening a hidden surface, or
  // interest changes reaching the backend out of order.
  it("hide-show-hide ends hidden even when the show resolves late", async () => {
    const fake = fakeIpc({ held: true });
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc));
    const emit = await liveOn(fake, controller, "local", "boot-local-4", 2);
    const a = controller.setInterest(false);
    await flush();
    fake.interest[0]!.result.resolve(null);
    await a;
    const b = controller.setInterest(true);
    await flush();
    const c = controller.setInterest(false);
    emit(full);
    emit(live);
    fake.interest[1]!.result.resolve(null);
    await flush();
    expect(controller.canInput()).toBe(false);
    fake.interest[2]!.result.resolve(null);
    await Promise.all([b, c]);
    emit(full);
    emit(live);
    expect(fake.interest.map((call) => call.active)).toEqual([false, true, false]);
    expect(controller.canInput()).toBe(false);

    // Changes superseded before reaching the backend are not sent (no floor advanced for them).
    const d = controller.setInterest(true);
    const e = controller.setInterest(false);
    await Promise.all([d, e]);
    expect(fake.interest).toHaveLength(3);
  });

  // Would catch: queued input sent after hiding, or a show collapsed with an undone hide
  // reopening before any full frame of the activation it waits for.
  it("hiding refuses queued input and a collapsed show still waits for its full frame", async () => {
    const inputHeld = deferred<void>();
    const fake = fakeIpc({ held: true, inputHeld });
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc));
    const emit = await liveOn(fake, controller, "local", "boot-local-4", 2);
    const first = controller.input(text("in-flight"));
    const queued = controller.input(text("queued"));
    await flush();
    const hiding = controller.setInterest(false);
    expect(await queued).toBe(false);
    inputHeld.resolve();
    expect(await first).toBe(true);
    expect(fake.inputs()).toBe(1);
    fake.interest[0]!.result.resolve(null);
    await hiding;

    const show1 = controller.setInterest(true);
    await flush();
    const hide = controller.setInterest(false);
    const show2 = controller.setInterest(true);
    fake.interest[1]!.result.resolve(null);
    await Promise.all([show1, hide, show2]);
    emit(live);
    expect(fake.interest.map((c) => c.active)).toEqual([false, true]);
    expect(controller.canInput()).toBe(false);
    emit(full);
    expect(controller.canInput()).toBe(true);
  });

  // Would catch: a host selected while hidden presenting frames, or a host hidden earlier never
  // being shown again when reselected.
  it("reattach sends the current interest before surface_attach only when needed", async () => {
    const fake = fakeIpc();
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc));
    await liveOn(fake, controller, "local", "boot-local-4", 2);
    await controller.setInterest(false);
    await controller.select("ssh-dev");
    const attach = fake.calls.lastIndexOf("surface_attach");
    expect(fake.calls.lastIndexOf("surface_interest")).toBeLessThan(attach);
    expect(fake.interest.map((c) => [c.endpoint, c.active])).toEqual([
      ["local", false],
      ["ssh-dev", false],
    ]);
    await controller.setInterest(true);
    const emit = fake.channels.at(-1)!;
    emit(identity("boot-ssh-3", 7));
    emit(full);
    emit(live);
    expect(controller.canInput()).toBe(true);
    await controller.select("local");
    expect(fake.interest.map((c) => [c.endpoint, c.active]).slice(2)).toEqual([
      ["ssh-dev", true],
      ["local", true],
    ]);

    // Never hidden: attach sends no interest at all.
    const fresh = fakeIpc();
    const other = createSurfaceController(tauriSurfaceBridge(fresh.ipc));
    await liveOn(fresh, other, "ssh-dev", "boot-ssh-3", 7);
    await other.select("local");
    expect(fresh.interest).toHaveLength(0);
  });

  // Would catch: an interest error reopening input or being retried.
  it("an interest error is shown once, keeps input closed and is not retried", async () => {
    const fake = fakeIpc({ held: true });
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc));
    const emit = await liveOn(fake, controller, "local", "boot-local-4", 2);
    const hiding = controller.setInterest(false);
    await flush();
    fake.interest[0]!.result.resolve(null);
    await hiding;
    const showing = controller.setInterest(true);
    await flush();
    fake.interest[1]!.result.reject({ code: "timeout", message: "sem resposta", retryable: true });
    await showing;
    emit(full);
    emit(live);
    expect(controller.state.error?.code).toBe("timeout");
    expect(controller.canInput()).toBe(false);
    await flush();
    expect(fake.interest).toHaveLength(2);
  });

  // Would catch: legacy bridges without the command breaking hide/show.
  it("a bridge without interest suspends locally and reopens on the next full frame", async () => {
    const fake = fakeIpc();
    const real = tauriSurfaceBridge(fake.ipc);
    const legacy: SurfaceBridge = { ...real };
    delete legacy.interest;
    const controller = createSurfaceController(legacy);
    const emit = await liveOn(fake, controller, "local", "boot-local-4", 2);
    await controller.setInterest(false);
    expect(controller.canInput()).toBe(false);
    await controller.setInterest(true);
    expect(controller.canInput()).toBe(false);
    emit(full);
    emit(live);
    expect(controller.canInput()).toBe(true);
    expect(fake.calls).not.toContain("surface_interest");
  });
});

describe("blur does not change surface_active (AC-019-03)", () => {
  const geometry: GeometryDto = { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18 };
  const sized: FrameEvent = { type: "full", revision: 3, width: 120, height: 40, cells: [], cursor: null, panes: [] };

  // Would catch: window blur mapped to surface_interest false, or a resize/Full of another size
  // applied because the TUI took the geometry controller.
  it("blur sends keyboard focus only; hidden drops the lease; geometry of Full stays 120×40", async () => {
    const fake = fakeIpc();
    const controller = createSurfaceController(tauriSurfaceBridge(fake.ipc), { geometry: () => geometry });
    const emit = await liveOn(fake, controller, "local", "boot-local-4", 2);
    emit(sized);
    const before = { cols: 120, rows: 40, width: sized.width, height: sized.height };
    const frames: FrameEvent[] = [];
    const stop = controller.subscribe((event) => frames.push(event));

    let windowState = INITIAL_WINDOW_SURFACE;
    windowState = applyWindowSurfaceAction(windowState, { kind: "keyboard-focus", focused: false });
    expect(surfaceActiveFromWindow(windowState)).toBe(true);
    await controller.focus(windowState.keyboardFocus);
    await controller.setInterest(surfaceActiveFromWindow(windowState));
    expect(fake.interest).toHaveLength(0);
    expect(fake.calls.filter((c) => c === "terminal_focus" || c === "surface_focus")).toContain("surface_focus");
    expect(fake.calls.filter((c) => c === "surface_resize")).toHaveLength(0);

    windowState = applyWindowSurfaceAction(windowState, { kind: "visibility", hidden: true });
    expect(surfaceActiveFromWindow(windowState)).toBe(false);
    await controller.setInterest(surfaceActiveFromWindow(windowState));
    expect(fake.interest.map((c) => c.active)).toEqual([false]);

    windowState = applyWindowSurfaceAction(windowState, { kind: "visibility", hidden: false });
    windowState = applyWindowSurfaceAction(windowState, { kind: "keyboard-focus", focused: true });
    expect(surfaceActiveFromWindow(windowState)).toBe(true);
    const showing = controller.setInterest(true);
    await showing;
    emit(sized);
    emit(live);
    await controller.focus(true);
    stop();

    const afterFull = [...frames].reverse().find((e) => e.type === "full");
    expect(afterFull).toMatchObject({ width: before.width, height: before.height });
    expect(fake.calls.filter((c) => c === "surface_resize")).toHaveLength(0);
  });
});
