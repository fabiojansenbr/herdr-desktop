// Tab containment of ConnectionDialog (pure seam; the DOM/native keyboard proof stays with the
// native a11y-navigation flow). Controls are plain tokens in the dialog's tabbable order.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { modalCloser, modalTrace, setModalTraceSink, wrapTarget, type ModalCloseParts } from "./modal-focus";

const [host, port, save, connect] = ["host", "port", "save", "connect"];
const order = [host, port, save, connect];

describe("wrapTarget", () => {
  // Would catch: Tab from the last control leaving the modal (window behind it), Shift+Tab from
  // the first leaving it, or the wrap sending both directions to the same end.
  it("wraps only at the ends: Tab last → first, Shift+Tab first → last", () => {
    expect(wrapTarget(order, connect, false)).toBe(host);
    expect(wrapTarget(order, host, true)).toBe(connect);
  });

  // Would catch: a trap that overrides every Tab (hiding real DOM order/skips from the native sweep).
  it("leaves inner moves to the native order", () => {
    expect(wrapTarget(order, host, false)).toBeNull();
    expect(wrapTarget(order, port, true)).toBeNull();
    expect(wrapTarget(order, save, false)).toBeNull();
    expect(wrapTarget(order, connect, true)).toBeNull();
  });

  // Would catch: focus lost to the body (e.g. a submit button disabled while saving) escaping the
  // modal on the next Tab instead of re-entering it at the end the key points to.
  it("brings focus outside/lost back inside at the end matching the direction", () => {
    expect(wrapTarget(order, null, false)).toBe(host);
    expect(wrapTarget(order, "opener", true)).toBe(connect);
  });

  it("no tabbable control: nothing to focus", () => {
    expect(wrapTarget([], null, false)).toBeNull();
  });
});

// GUI r7: after a native Escape the WebView lost document focus 9 ms after the dialog was removed,
// with focus restored to the opener only afterwards (teardown). Planner decision r8: the opener is
// focused synchronously while the dialog is still in the DOM, after the modal stops making the
// window inert, and only then the state change that removes the dialog runs.
function parts(opts: { open?: boolean; disabled?: boolean; connected?: boolean; noOpener?: boolean; focusHost?: () => Promise<void> } = {}) {
  const calls: string[] = [];
  let open = opts.open ?? true;
  const opener = {
    get isConnected() {
      calls.push("opener.isConnected");
      return opts.connected ?? true;
    },
    matches: (selector: string) => (calls.push(`opener.matches(${selector})`), selector === ":disabled" && opts.disabled === true),
    focus: () => void calls.push(open ? "opener.focus (dialog still modal)" : "opener.focus"),
  };
  const p: ModalCloseParts = {
    isOpen: () => open,
    release: () => void (calls.push("release"), (open = false)),
    opener: opts.noOpener ? null : opener,
    cancel: () => void calls.push("cancel"),
    ...(opts.focusHost ? { focusHost: () => (calls.push("focusHost"), opts.focusHost!()) } : {}),
  };
  const steps = () => calls.filter((c) => !c.startsWith("opener.isConnected") && !c.startsWith("opener.matches"));
  return { p, calls, steps };
}

describe("modalCloser", () => {
  // Would catch: cancel (dialog removed from the DOM) before the opener focus (r7 order), focus
  // while the modal still makes the opener inert, or release skipped.
  it("releases the modal, focuses the opener, then cancels — in that order, synchronously", () => {
    const w = parts();
    expect(modalCloser(w.p)()).toBe(true);
    expect(w.steps()).toEqual(["release", "opener.focus", "cancel"]);
  });

  // Would catch: the opener checked when the dialog opened instead of at close time.
  it("reads the opener state at close time", () => {
    const w = parts();
    const close = modalCloser(w.p);
    expect(w.calls).toEqual([]);
    close();
    expect(w.calls).toContain("opener.isConnected");
  });

  // Would catch: focusing a disabled or detached opener (focus would go to the body and the
  // dialog would never close because the focus threw or returned early).
  it("disabled opener: no focus, still releases and cancels", () => {
    const w = parts({ disabled: true });
    modalCloser(w.p)();
    expect(w.steps()).toEqual(["release", "cancel"]);
    expect(w.calls).toContain("opener.matches(:disabled)");
  });

  it("detached opener: no focus, still releases and cancels", () => {
    const w = parts({ connected: false });
    modalCloser(w.p)();
    expect(w.steps()).toEqual(["release", "cancel"]);
  });

  it("no opener recorded: releases and cancels only", () => {
    const w = parts({ noOpener: true });
    modalCloser(w.p)();
    expect(w.steps()).toEqual(["release", "cancel"]);
  });

  // Would catch: close() called again on a dialog the platform already closed (native close event).
  it("dialog already closed: no second release; opener focused, then cancel", () => {
    const w = parts({ open: false });
    modalCloser(w.p)();
    expect(w.steps()).toEqual(["opener.focus", "cancel"]);
  });

  // Would catch: the close event queued by release() running the sequence a second time
  // (double cancel / refocus after the dialog left the DOM).
  it("runs once: later requests (e.g. the queued close event) do nothing", () => {
    const w = parts();
    const close = modalCloser(w.p);
    close();
    const before = w.calls.length;
    expect(close()).toBe(false);
    expect(w.calls.length).toBe(before);
  });
});

// No DOM in this environment: the native a11y-close-dialog step is the real proof. This guards the
// wiring. Would catch: Escape, Cancelar or the native close event still calling cancelDialog
// directly (dialog removed before the opener is focused), or a closer not built from the modal.
// GUI r8: window blur 4 ms after Escape. The fidelity page needs to place close()/focus()/cancel()
// in time; production installs no sink, so the trace must change nothing.
describe("modalCloser trace", () => {
  // Would catch: marks at the wrong moment (e.g. all after cancel) or a missing step.
  it("marks each step right after it runs, in order, with whether the opener took focus", () => {
    const w = parts();
    const marks: string[] = [];
    modalCloser({ ...w.p, trace: (step, focused) => void marks.push(`${step}:${focused ?? "-"}@${w.steps().length}`) })();
    expect(marks).toEqual(["release:true@1", "focus:true@2", "cancel:-@3"]);
  });

  // Would catch: a "focus" mark claiming focus for a disabled opener, or release marked when skipped.
  it("skipped steps are marked as not done", () => {
    const w = parts({ disabled: true, open: false });
    const marks: string[] = [];
    modalCloser({ ...w.p, trace: (step, done) => void marks.push(`${step}:${done ?? "-"}`) })();
    expect(marks).toEqual(["release:false", "focus:false", "cancel:-"]);
  });

  // Would catch: the trace changing the close order or throwing when no sink is installed.
  it("modalTrace forwards to the installed sink only; without one it is a no-op", () => {
    const w = parts();
    modalCloser({ ...w.p, trace: modalTrace })();
    expect(w.steps()).toEqual(["release", "opener.focus", "cancel"]);
    const seen: string[] = [];
    const remove = setModalTraceSink((step, done) => void seen.push(`${step}:${done ?? "-"}`));
    modalTrace("focus", true);
    remove();
    modalTrace("cancel");
    expect(seen).toEqual(["focus:true"]);
  });
});

// Planner decision r10 (GUI r9: after dialog.close() the window blur came with no relatedTarget and
// the WebView lost GTK widget focus while the compositor kept the window). The composed window asks
// the host (surface_focus_host) to return keyboard focus to the WebView after the dialog is gone.
describe("modalCloser host focus", () => {
  const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

  // Would catch: the host focus requested before cancel (dialog still in the DOM) or before the
  // opener focus, not requested at all, or requested asynchronously after another task.
  it("release → opener focus → cancel → host focus, synchronously and in that order", () => {
    const w = parts({ focusHost: () => Promise.resolve() });
    expect(modalCloser(w.p)()).toBe(true);
    expect(w.steps()).toEqual(["release", "opener.focus", "cancel", "focusHost"]);
  });

  // Would catch: the host focus tied to the opener (the WebView loses focus either way).
  it("disabled, detached or missing opener: host focus still requested after cancel", () => {
    for (const opts of [{ disabled: true }, { connected: false }, { noOpener: true }]) {
      const w = parts({ ...opts, focusHost: () => Promise.resolve() });
      modalCloser(w.p)();
      expect(w.steps()).toEqual(["release", "cancel", "focusHost"]);
    }
  });

  // Would catch: a second host focus from the queued native close event.
  it("runs once: the host focus is requested once per modal", () => {
    const w = parts({ focusHost: () => Promise.resolve() });
    const close = modalCloser(w.p);
    close();
    close();
    expect(w.calls.filter((c) => c === "focusHost")).toHaveLength(1);
  });

  // Would catch: the outcome marked before the command answered, a refusal marked as done, or a
  // rejection left unhandled.
  it("marks focus-host with the command outcome once it settles", async () => {
    for (const [result, want] of [[() => Promise.resolve(), "focus-host:true"], [() => Promise.reject(new Error("focus_host_refused")), "focus-host:false"]] as const) {
      const w = parts({ focusHost: result });
      const marks: string[] = [];
      modalCloser({ ...w.p, trace: (step, done) => void marks.push(`${step}:${done ?? "-"}`) })();
      expect(marks).toEqual(["release:true", "focus:true", "cancel:-"]);
      await flush();
      expect(marks).toEqual(["release:true", "focus:true", "cancel:-", want]);
    }
  });
});

describe("ConnectionDialog close wiring", () => {
  const view = readFileSync(new URL("./ConnectionDialog.svelte", import.meta.url), "utf8");
  const script = view.slice(0, view.indexOf("</script>"));
  const markup = view.slice(view.indexOf("</script>"), view.indexOf("<style>"));

  it("builds one closer per modal from the dialog, its opener and cancelDialog", () => {
    expect(script).toMatch(/import \{[^}]*\bmodalCloser\b[^}]*\} from "\.\/modal-focus"/);
    expect(script).toMatch(/modalCloser\(\{\s*isOpen: \(\) => el\.open,\s*release: \(\) => el\.close\(\),\s*opener,\s*cancel: \(\) => controller\.cancelDialog\(\),\s*focusHost,\s*trace: modalTrace,?\s*\}\)/);
  });

  // Would catch: the composed window not handing the host focus command to the dialog, the dialog
  // requiring it (the standalone connections harness has none), or another command bound to it.
  it("the composed window passes the surface bridge host focus to the dialog", () => {
    expect(script).toMatch(/focusHost\?: \(\) => Promise<void>;/);
    expect(script).toMatch(/let \{ controller, state: connState, focusHost \}: Props = \$props\(\);/);
    const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
    expect(app).toMatch(/const surfaceBridge = tauriSurfaceBridge\(\);/);
    expect(app).toMatch(/createSurfaceController\(surfaceBridge,/);
    expect(app).toMatch(/<ConnectionDialog controller=\{connections\} state=\{connectionsState\} focusHost=\{\(\) => surfaceBridge\.focusHost\(\)\} \/>/);
  });

  it("Escape, the native close event and Cancelar go through the closer", () => {
    const escape = /if \(e\.key === "Escape"\) \{([^}]*)\}/.exec(script)?.[1] ?? "";
    expect(escape).toMatch(/requestClose\(\)/);
    expect(escape).not.toMatch(/cancelDialog/);
    expect(script).toMatch(/const onClose = \(\) => (void )?requestClose\(\);/);
    const cancelButton = /<button type="button" onclick=\{([^}]*)\}>\{t\("connections\.action\.cancel"\)\}<\/button>/.exec(markup)?.[1] ?? "";
    expect(cancelButton).toMatch(/requestClose\(\)/);
    expect(cancelButton).not.toMatch(/cancelDialog/);
  });
});
