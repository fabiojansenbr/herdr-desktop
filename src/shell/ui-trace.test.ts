// @vitest-environment happy-dom
// Spec 028 r3 — the WebView debug trace: pure line builders plus the one additive invoke. The
// command is a no-op in release (Rust side) and nothing is invoked outside the Tauri WebView.
// Would catch: a trace line built from the wrong fields, a trace driving behavior, an unbounded
// line, or an invoke of anything other than `ui_trace`.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  chordName,
  contextMenuLine,
  MAX_UI_TRACE_LINE_BYTES,
  menuRectLine,
  menuStateLine,
  pasteLine,
  pasteResultLine,
  rightClickLine,
  targetName,
  traceAvailable,
  uiTrace,
} from "./ui-trace";

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

function withTauri() {
  const invoke = vi.fn(async (_command: string, _args?: Record<string, unknown>) => undefined);
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = { invoke };
  return invoke;
}

describe("ui trace lines", () => {
  it("describes the contextmenu entry, the route, the menu state and the rendered menu", () => {
    expect(contextMenuLine({ clientX: 515, clientY: 31, target: "textarea.ime-target", paneId: "w1:p2" })).toBe(
      "contextmenu client=(515,31) target=textarea.ime-target pane=w1:p2",
    );
    expect(contextMenuLine({ clientX: 1, clientY: 2, target: "canvas", paneId: null })).toBe(
      "contextmenu client=(1,2) target=canvas pane=null",
    );
    expect(rightClickLine("w1:p2", "menu")).toBe("rightClickRoute pane=w1:p2 route=menu");
    expect(rightClickLine("w1:p1", "app")).toBe("rightClickRoute pane=w1:p1 route=app");
    expect(menuStateLine({ state: "open", paneId: "w1:p2", items: ["Zoom", "Fechar pane"] })).toBe(
      'pane-menu open pane=w1:p2 items=Zoom,Fechar pane',
    );
    expect(menuStateLine({ state: "ignored", paneId: "w1:p9", known: 0 })).toBe("pane-menu ignored pane=w1:p9 known=0");
    expect(menuStateLine({ state: "closed" })).toBe("pane-menu closed");
    expect(
      menuRectLine({ label: "Ações do pane w1:p1", rect: { left: 10.4, top: 20.6, width: 120, height: 30 }, zIndex: "8", items: 8 }),
    ).toBe('menu "Ações do pane w1:p1" left=10 top=21 width=120 height=30 z=8 items=8');
    expect(menuRectLine({ label: "menu", rect: null, zIndex: "none", items: 0 })).toBe('menu "menu" rect=null z=none items=0');
  });

  it("describes the paste chord, route and backend result without content", () => {
    expect(chordName({ key: "v", ctrlKey: true, shiftKey: true, altKey: false, metaKey: false })).toBe("Ctrl+Shift+V");
    expect(pasteLine({ chord: "Ctrl+V", route: "paste", paneId: "w1:p1" })).toBe("paste chord=Ctrl+V route=paste pane=w1:p1");
    expect(pasteResultLine({ kind: "forward_key", sent: true, bytes: 0 })).toBe("paste result kind=forward_key sent=true bytes=0");
    expect(pasteResultLine({ kind: "image", sent: true, bytes: 4096 })).toBe("paste result kind=image sent=true bytes=4096");
    expect(pasteResultLine(undefined)).toBe("paste result kind=unknown sent=false bytes=0");
  });

  it("names the event target as an element, never its content", () => {
    const element = document.createElement("textarea");
    element.id = "ime";
    element.className = "ime-target wide";
    expect(targetName(element)).toBe("textarea#ime.ime-target.wide");
    expect(targetName(null)).toBe("null");
    expect(targetName(window)).toBe("non-element");
  });
});

describe("ui trace dispatch", () => {
  it("does nothing outside the Tauri WebView", () => {
    expect(traceAvailable()).toBe(false);
    expect(() => uiTrace("contextmenu client=(0,0)")).not.toThrow();
  });

  it("invokes ui_trace with the bounded line inside the Tauri WebView", () => {
    const invoke = withTauri();
    expect(traceAvailable()).toBe(true);
    uiTrace("paste chord=Ctrl+V route=paste pane=w1:p1");
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke.mock.calls[0]![0]).toBe("ui_trace");
    expect(invoke.mock.calls[0]![1]).toEqual({ line: "paste chord=Ctrl+V route=paste pane=w1:p1" });

    uiTrace("x".repeat(MAX_UI_TRACE_LINE_BYTES + 10));
    expect(invoke).toHaveBeenCalledTimes(2);
    const line = (invoke.mock.calls[1]![1] as { line: string }).line;
    expect(line).toHaveLength(MAX_UI_TRACE_LINE_BYTES);
  });
});
