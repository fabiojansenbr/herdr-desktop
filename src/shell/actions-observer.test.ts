// Spec 010 r3 — optional result observer of the terminal actions (src/shell/actions.ts) and the
// harness-only action log the fidelity page reads. Replaces the r2 wrap of Tauri internals: the
// observation happens in the app's own action layer, the bridge call is unchanged.
import { describe, expect, it } from "vitest";

import {
  ACTION_LOG_LIMIT,
  actionLogFromSelection,
  createTerminalActions,
  type ActionEvent,
  type ActionReceipt,
  type TerminalActionsBridge,
} from "./actions";
import type { SurfaceIdentityDto } from "./types";

const LOCAL: SurfaceIdentityDto = { endpoint: "local", session: "s-local", connection_generation: 9, boot_id: "boot-local-7", pane_id: "w1:p1" };
const link = { pane_id: "w1:p1", uri: "https://hd.invalid/open?n=1", viewport_row: 1, col: 4, content_revision: 40 };
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function bridgeWith(openLink: TerminalActionsBridge["openLink"]): { bridge: TerminalActionsBridge; calls: string[] } {
  const calls: string[] = [];
  const ok = (kind: string) => async (_e: SurfaceIdentityDto, request: { pane_id: string }) => {
    calls.push(kind);
    return { pane_id: request.pane_id, sent: true } as ActionReceipt;
  };
  return {
    calls,
    bridge: {
      focus: ok("focus"),
      scroll: ok("scroll"),
      copySelection: ok("copy"),
      openLink: async (e, r) => {
        calls.push("link");
        return openLink(e, r);
      },
    },
  };
}

describe("terminal action result observer", () => {
  it("reports a link the bridge received with its real receipt (sent false stays false)", async () => {
    // Would catch: an observer that reports success without the receipt, or turns sent:false into sent.
    const seen: Array<[string, unknown, ActionEvent]> = [];
    const { bridge, calls } = bridgeWith(async () => ({ pane_id: "w1:p1", sent: false }));
    const actions = createTerminalActions(bridge, () => LOCAL, (kind, request, event) => seen.push([kind, request, event]));
    await expect(actions.onOpenLink(link)).resolves.toEqual({ ok: true });
    expect(calls).toEqual(["link"]);
    expect(seen).toEqual([
      ["link", link, { stage: "requested", waiting: 0, running: false }],
      ["link", link, { stage: "called" }],
      ["link", link, { stage: "settled", called: true, receipt: { pane_id: "w1:p1", sent: false }, result: { ok: true } }],
    ]);
  });

  it("reports a refusal with the backend code, and a refusal before any call as called:false", async () => {
    // Would catch: dropping the refusal code, or reporting "no identity" as if the host had been asked.
    const seen: Array<[string, ActionEvent]> = [];
    const { bridge, calls } = bridgeWith(async () => {
      throw { code: "focus_changed", message: "o pane focado mudou" };
    });
    let identity: SurfaceIdentityDto | null = LOCAL;
    const actions = createTerminalActions(bridge, () => identity, (kind, _r, event) => seen.push([kind, event]));
    await actions.onOpenLink(link);
    identity = null;
    await actions.onOpenLink(link);
    expect(calls).toEqual(["link"]);
    expect(seen).toEqual([
      ["link", { stage: "requested", waiting: 0, running: false }],
      ["link", { stage: "called" }],
      ["link", { stage: "settled", called: true, receipt: null, result: { ok: false, code: "focus_changed", message: "o pane focado mudou" } }],
      ["link", { stage: "requested", waiting: 0, running: false }],
      ["link", { stage: "settled", called: false, receipt: null, result: { ok: false, code: "no_identity", message: expect.any(String) } }],
    ]);
  });

  it("a link queued behind an action that never settles is visible as requested and never called", async () => {
    // Would catch: a log that cannot tell "the click never asked" from "asked, stuck in the lane".
    const seen: Array<[string, ActionEvent]> = [];
    const hung = new Promise<ActionReceipt>(() => {});
    const bridge: TerminalActionsBridge = {
      focus: async (_e, r) => ({ pane_id: r.pane_id, sent: true }),
      scroll: () => hung,
      copySelection: async (_e, r) => ({ pane_id: r.pane_id, sent: true }),
      openLink: async (_e, r) => ({ pane_id: r.pane_id, sent: false }),
    };
    const actions = createTerminalActions(bridge, () => LOCAL, (kind, _r, event) => seen.push([kind, event]));
    void actions.onScroll({ pane_id: "w1:p1", offset_from_bottom: 6 });
    void actions.onOpenLink(link);
    await flush();
    expect(seen).toEqual([
      ["scroll", { stage: "requested", waiting: 0, running: false }],
      ["scroll", { stage: "called" }],
      ["link", { stage: "requested", waiting: 0, running: true }],
    ]);
  });

  it("an observer that throws never changes the action result nor blocks the next action", async () => {
    // Would catch: observation failures leaking into the terminal (notice) or stalling the lane.
    const { bridge, calls } = bridgeWith(async () => ({ pane_id: "w1:p1", sent: true }));
    const actions = createTerminalActions(bridge, () => LOCAL, () => {
      throw new Error("observer broke");
    });
    await expect(actions.onOpenLink(link)).resolves.toEqual({ ok: true });
    await expect(actions.onScroll({ pane_id: "w1:p1", offset_from_bottom: 1 })).resolves.toEqual({ ok: true });
    await flush();
    expect(calls).toEqual(["link", "scroll"]);
  });

  it("without an observer the actions behave as before (no callback required)", async () => {
    const { bridge } = bridgeWith(async () => ({ pane_id: "w1:p1", sent: true }));
    await expect(createTerminalActions(bridge, () => LOCAL).onOpenLink(link)).resolves.toEqual({ ok: true });
  });
});

describe("harness action log", () => {
  const selection = { feature: "fidelity", phase: "flow", params: {} };

  it("exists only for a native harness selection; the product gets null", () => {
    // Would catch: a log (and its memory) installed in the product window.
    expect(actionLogFromSelection(undefined)).toBeNull();
    expect(actionLogFromSelection({ feature: "fidelity" })).toBeNull();
    expect(actionLogFromSelection(selection)).not.toBeNull();
  });

  it("records kind, uri, called, receipt and result with a time, and take() consumes them", () => {
    // Would catch: losing the uri of a link, or entries that survive take() and pollute the next step.
    let now = 10;
    const log = actionLogFromSelection(selection, () => now)!;
    log.record("link", link, { stage: "requested", waiting: 1, running: true });
    now = 12;
    log.record("link", link, { stage: "called" });
    log.record("link", link, { stage: "settled", called: true, receipt: { pane_id: "w1:p1", sent: true }, result: { ok: true } });
    now = 25;
    log.record("focus", { pane_id: "w1:p2" }, { stage: "settled", called: true, receipt: null, result: { ok: false, code: "stale_surface", message: "m" } });
    expect(log.take()).toEqual([
      { kind: "link", pane_id: "w1:p1", uri: link.uri, at_ms: 10, stage: "requested", waiting: 1, running: true },
      { kind: "link", pane_id: "w1:p1", uri: link.uri, at_ms: 12, stage: "called" },
      { kind: "link", pane_id: "w1:p1", uri: link.uri, at_ms: 12, stage: "settled", called: true, receipt: { pane_id: "w1:p1", sent: true }, result: { ok: true } },
      { kind: "focus", pane_id: "w1:p2", uri: null, at_ms: 25, stage: "settled", called: true, receipt: null, result: { ok: false, code: "stale_surface", message: "m" } },
    ]);
    expect(log.take()).toEqual([]);
  });

  it("is bounded: only the newest ACTION_LOG_LIMIT entries are kept", () => {
    // Would catch: an unbounded buffer in a long flow.
    const log = actionLogFromSelection(selection, () => 0)!;
    for (let i = 0; i < ACTION_LOG_LIMIT + 5; i++) log.record("scroll", { pane_id: `w1:p${i}` }, { stage: "called" });
    const entries = log.take();
    expect(entries).toHaveLength(ACTION_LOG_LIMIT);
    expect(entries[0]?.pane_id).toBe("w1:p5");
  });
});
