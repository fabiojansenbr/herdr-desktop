// Spec 007 — terminal actions of the composed window (frontend seam of surface_pane_focus,
// surface_scroll, surface_copy_selection, surface_open_link). Local and SSH share pane ids
// "w1:p1"/"w1:p2" and differ in session, boot and generation, so any action that follows the
// next host instead of the captured one fails on the identity sent.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import {
  createTerminalActions,
  MAX_PENDING_ACTIONS,
  tauriTerminalActionsBridge,
  type ActionReceipt,
  type TerminalActionsBridge,
} from "./actions";
import type { SurfaceIdentityDto } from "./types";

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
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const SSH: SurfaceIdentityDto = { endpoint: "ssh-dev", session: "s-remote", connection_generation: 4, boot_id: "boot-ssh-3", pane_id: "w1:p1" };
const LOCAL: SurfaceIdentityDto = { endpoint: "local", session: "s-local", connection_generation: 9, boot_id: "boot-local-7", pane_id: "w1:p1" };

type Call = { kind: string; expected: SurfaceIdentityDto; request: unknown; reply: Deferred<ActionReceipt> };

function fakeBridge() {
  const calls: Call[] = [];
  const record = (kind: string) => (expected: SurfaceIdentityDto, request: unknown) => {
    const reply = deferred<ActionReceipt>();
    calls.push({ kind, expected: { ...expected }, request: structuredClone(request), reply });
    return reply.promise;
  };
  const bridge: TerminalActionsBridge = {
    focus: record("focus"),
    scroll: record("scroll"),
    copySelection: record("copy"),
    openLink: record("link"),
  };
  return { bridge, calls };
}

function nth(calls: Call[], index: number): Call {
  const call = calls[index];
  if (!call) throw new Error(`bridge call ${index} was not made`);
  return call;
}

const receipt = (pane: string, extra: Partial<ActionReceipt> = {}): ActionReceipt => ({ pane_id: pane, sent: true, ...extra });
const selection = { pane_id: "w1:p2", anchor: { row: 2, col: 1 }, cursor: { row: 3, col: 5 }, content_revision: 40 };
const link = { pane_id: "w1:p2", uri: "https://remote.example/b", viewport_row: 1, col: 1, content_revision: 40 };

describe("terminal actions", () => {
  beforeEach(() => invoke.mockReset());

  it("captures the identity when the action is requested and never follows the next host", async () => {
    // Would catch: the identity read when the queued call starts (after the selection moved to
    // Local) instead of when the user acted, or an action re-targeted to the new host.
    const { bridge, calls } = fakeBridge();
    let current: SurfaceIdentityDto | null = SSH;
    const actions = createTerminalActions(bridge, () => current);

    const focus = actions.onSelectPane({ pane_id: "w1:p2", surface_revision: 7 });
    const scroll = actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: 12 });
    current = LOCAL;
    const copy = actions.onCopySelection(selection);
    await flush();
    expect(calls.map((c) => c.kind)).toEqual(["focus"]);

    nth(calls, 0).reply.resolve(receipt("w1:p2"));
    await flush();
    nth(calls, 1).reply.resolve(receipt("w1:p2", { offset_from_bottom: 12 }));
    await flush();
    nth(calls, 2).reply.resolve(receipt("w1:p2", { copied_bytes: 5 }));

    expect(await focus).toEqual({ ok: true });
    expect(await scroll).toEqual({ ok: true });
    expect(await copy).toEqual({ ok: true });
    expect(calls.map((c) => [c.kind, c.expected])).toEqual([
      ["focus", SSH],
      ["scroll", SSH],
      ["copy", LOCAL],
    ]);
    expect(nth(calls, 0).request).toEqual({ pane_id: "w1:p2" });
  });

  it("runs actions one at a time in request order, each sent once", async () => {
    // Would catch: concurrent invokes letting a later scroll land before an earlier focus, or a
    // failed action retried.
    const { bridge, calls } = fakeBridge();
    const actions = createTerminalActions(bridge, () => SSH);
    const focus = actions.onSelectPane({ pane_id: "w1:p2", surface_revision: 1 });
    const scroll = actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: 3 });
    const open = actions.onOpenLink(link);
    await flush();
    expect(calls).toHaveLength(1);
    nth(calls, 0).reply.reject({ code: "result_unknown", message: "o resultado de pane.focus é desconhecido", retryable: false });
    await flush();
    expect(calls.map((c) => c.kind)).toEqual(["focus", "scroll"]);
    nth(calls, 1).reply.resolve(receipt("w1:p2"));
    await flush();
    expect(calls.map((c) => c.kind)).toEqual(["focus", "scroll", "link"]);
    nth(calls, 2).reply.resolve({ pane_id: "w1:p2", sent: false });
    expect(await focus).toEqual({ ok: false, code: "result_unknown", message: "o resultado de pane.focus é desconhecido" });
    expect(await scroll).toEqual({ ok: true });
    expect(await open).toEqual({ ok: true });
    await flush();
    expect(calls).toHaveLength(3);
  });

  it("coalesces queued scrolls of the same pane and identity into the newest offset", async () => {
    // Would catch: every wheel tick queued (unbounded engine round trips), a coalesced scroll
    // reported without the real result, or scrolls of another pane/host merged.
    const { bridge, calls } = fakeBridge();
    let current: SurfaceIdentityDto = SSH;
    const actions = createTerminalActions(bridge, () => current);
    const focus = actions.onSelectPane({ pane_id: "w1:p2", surface_revision: 1 });
    const s1 = actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: 3 });
    const s2 = actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: 6 });
    const s3 = actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: 9 });
    const other = actions.onScroll({ pane_id: "w1:p1", offset_from_bottom: 2 });
    current = { ...SSH, connection_generation: 5 };
    const renewed = actions.onScroll({ pane_id: "w1:p1", offset_from_bottom: 4 });
    await flush();
    nth(calls, 0).reply.resolve(receipt("w1:p2"));
    await flush();
    nth(calls, 1).reply.reject({ code: "scroll_unavailable", message: "o pane não possui histórico rolável" });
    await flush();
    nth(calls, 2).reply.resolve(receipt("w1:p1"));
    await flush();
    nth(calls, 3).reply.resolve(receipt("w1:p1"));

    const failed = { ok: false, code: "scroll_unavailable", message: "o pane não possui histórico rolável" };
    expect(await focus).toEqual({ ok: true });
    expect([await s1, await s2, await s3]).toEqual([failed, failed, failed]);
    expect(await other).toEqual({ ok: true });
    expect(await renewed).toEqual({ ok: true });
    expect(calls.map((c) => [c.kind, c.request, c.expected.connection_generation])).toEqual([
      ["focus", { pane_id: "w1:p2" }, 4],
      ["scroll", { pane_id: "w1:p2", offset_from_bottom: 9 }, 4],
      ["scroll", { pane_id: "w1:p1", offset_from_bottom: 2 }, 4],
      ["scroll", { pane_id: "w1:p1", offset_from_bottom: 4 }, 5],
    ]);
  });

  it("denies only the unsupported action for that connection and refuses without identity", async () => {
    // Would catch: a missing pane.selection.read disabling focus/links, repeated invokes of a
    // method the connection does not announce, the denial leaking to a renewed connection, or an
    // action invoked with no confirmed identity.
    const { bridge, calls } = fakeBridge();
    let current: SurfaceIdentityDto | null = null;
    const actions = createTerminalActions(bridge, () => current);
    const none = await actions.onCopySelection(selection);
    expect(none).toMatchObject({ ok: false, code: "no_identity" });
    expect(calls).toHaveLength(0);

    current = SSH;
    const first = actions.onCopySelection(selection);
    await flush();
    nth(calls, 0).reply.reject({ code: "unsupported_method", message: "o método pane.selection.read não está disponível nesta máquina", retryable: false });
    expect(await first).toMatchObject({ ok: false, code: "unsupported_method" });
    const again = await actions.onCopySelection(selection);
    expect(again).toMatchObject({ ok: false, code: "unsupported_method" });
    expect(calls).toHaveLength(1);

    const open = actions.onOpenLink(link);
    await flush();
    expect(calls.map((c) => c.kind)).toEqual(["copy", "link"]);
    nth(calls, 1).reply.resolve({ pane_id: "w1:p2", sent: false });
    expect(await open).toEqual({ ok: true });

    current = { ...SSH, boot_id: "boot-ssh-4" };
    const renewed = actions.onCopySelection(selection);
    await flush();
    expect(calls.map((c) => c.kind)).toEqual(["copy", "link", "copy"]);
    nth(calls, 2).reply.resolve(receipt("w1:p2", { copied_bytes: 3 }));
    expect(await renewed).toEqual({ ok: true });
  });

  it("bounds the pending queue and maps unexpected failures to a visible error", async () => {
    // Would catch: an unbounded queue behind a stuck host, or a thrown non-RuntimeError turned
    // into success.
    const { bridge, calls } = fakeBridge();
    const actions = createTerminalActions(bridge, () => SSH);
    const running = actions.onOpenLink(link);
    const queued = Array.from({ length: MAX_PENDING_ACTIONS }, (_, i) =>
      actions.onSelectPane({ pane_id: `w1:p${i + 2}`, surface_revision: 1 }),
    );
    const refused = await actions.onSelectPane({ pane_id: "w1:p99", surface_revision: 1 });
    expect(refused).toMatchObject({ ok: false, code: "actions_busy" });
    await flush();
    expect(calls).toHaveLength(1);
    nth(calls, 0).reply.reject(new Error("ipc down"));
    expect(await running).toEqual({ ok: false, code: "action_failed", message: "ipc down" });
    for (let i = 0; i < queued.length; i += 1) {
      await flush();
      nth(calls, i + 1).reply.resolve(receipt(`w1:p${i + 2}`));
    }
    expect(await Promise.all(queued)).toEqual(queued.map(() => ({ ok: true })));
    expect(calls).toHaveLength(1 + MAX_PENDING_ACTIONS);
  });

  it("sends the selection points as they were when the copy was admitted", async () => {
    // Would catch: a shallow copy of the queued request, where TerminalView reusing/mutating its
    // anchor/cursor objects after the call changes what the backend reads.
    const { bridge, calls } = fakeBridge();
    const actions = createTerminalActions(bridge, () => SSH);
    const held = actions.onOpenLink(link);
    const request = { pane_id: "w1:p2", anchor: { row: 2, col: 1 }, cursor: { row: 3, col: 5 }, content_revision: 40 };
    const copy = actions.onCopySelection(request);
    request.anchor.row = 90;
    request.anchor.col = 7;
    request.cursor.row = 91;
    request.cursor.col = 8;
    request.content_revision = 41;
    await flush();
    nth(calls, 0).reply.resolve({ pane_id: "w1:p2", sent: false });
    await flush();
    nth(calls, 1).reply.resolve(receipt("w1:p2", { copied_bytes: 4 }));
    expect(await held).toEqual({ ok: true });
    expect(await copy).toEqual({ ok: true });
    expect(nth(calls, 1).request).toEqual({ pane_id: "w1:p2", anchor: { row: 2, col: 1 }, cursor: { row: 3, col: 5 }, content_revision: 40 });
    expect(calls).toHaveLength(2);
  });

  it("counts coalesced scroll waiters against the pending limit", async () => {
    // Would catch: coalescing that appends waiters to one queued entry without any bound, so a
    // stuck host retains unlimited pending promises.
    const { bridge, calls } = fakeBridge();
    const actions = createTerminalActions(bridge, () => SSH);
    const held = actions.onOpenLink(link);
    const scrolls = Array.from({ length: MAX_PENDING_ACTIONS }, (_, i) =>
      actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: i + 1 }),
    );
    const refused = await actions.onScroll({ pane_id: "w1:p2", offset_from_bottom: 99 });
    expect(refused).toMatchObject({ ok: false, code: "actions_busy" });
    await flush();
    expect(calls).toHaveLength(1);
    nth(calls, 0).reply.resolve({ pane_id: "w1:p2", sent: false });
    await flush();
    nth(calls, 1).reply.resolve(receipt("w1:p2", { offset_from_bottom: MAX_PENDING_ACTIONS }));
    expect(await held).toEqual({ ok: true });
    expect(await Promise.all(scrolls)).toEqual(scrolls.map(() => ({ ok: true })));
    await flush();
    expect(calls.map((c) => [c.kind, c.request])).toEqual([
      ["link", link],
      ["scroll", { pane_id: "w1:p2", offset_from_bottom: MAX_PENDING_ACTIONS }],
    ]);
  });

  it("invokes only the four bounded commands with the captured identity and concrete request", async () => {
    // Would catch: a renamed/generic command, the text or URI forwarded as a free argument, or
    // extra fields (surface_revision) sent to the backend DTO.
    invoke.mockResolvedValue({ pane_id: "w1:p2", sent: true });
    const bridge = tauriTerminalActionsBridge();
    await bridge.focus(SSH, { pane_id: "w1:p2" });
    await bridge.scroll(SSH, { pane_id: "w1:p2", offset_from_bottom: 7 });
    await bridge.copySelection(SSH, selection);
    await bridge.openLink(SSH, link);
    expect(invoke.mock.calls).toEqual([
      ["surface_pane_focus", { expected: SSH, request: { pane_id: "w1:p2" } }],
      ["surface_scroll", { expected: SSH, request: { pane_id: "w1:p2", offset_from_bottom: 7 } }],
      ["surface_copy_selection", { expected: SSH, request: selection }],
      ["surface_open_link", { expected: SSH, request: link }],
    ]);
  });
});
