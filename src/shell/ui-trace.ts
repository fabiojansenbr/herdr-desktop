// Spec 028 r3 — debug trace of the window to the Tauri host's stderr. Every call is additive and
// never drives behavior: the host command (`ui_trace`) is a no-op in release builds and outside
// the Tauri WebView (unit tests, a plain browser) nothing is invoked. The line builders are pure
// so the exact content can be asserted; a trace never carries clipboard content or credentials.

import { invoke } from "@tauri-apps/api/core";

/** Prefix the host writes before every line; mirrors the Rust `UI_TRACE_PREFIX`. */
export const UI_TRACE_PREFIX = "[ui] ";

/** Longest line the host accepts; mirrors `MAX_UI_TRACE_LINE_BYTES` in src-tauri. */
export const MAX_UI_TRACE_LINE_BYTES = 4096;

/** True inside the Tauri WebView (false in unit tests and the plain browser). */
export function traceAvailable(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Sends one debug line to the host once; missing host or a failed invoke changes nothing. */
export function uiTrace(line: string): void {
  if (!traceAvailable()) return;
  const bounded = line.length > MAX_UI_TRACE_LINE_BYTES ? line.slice(0, MAX_UI_TRACE_LINE_BYTES) : line;
  void invoke("ui_trace", { line: bounded }).catch(() => {});
}

/** Element under the event, as `tag#id.class`, never the full outer HTML. */
export function targetName(target: EventTarget | null): string {
  if (target instanceof Element) {
    const id = target.id ? `#${target.id}` : "";
    const className = typeof target.className === "string" ? target.className.trim().replace(/\s+/g, ".") : "";
    const classes = className ? `.${className}` : "";
    return `${target.tagName.toLowerCase()}${id}${classes}`;
  }
  return target ? "non-element" : "null";
}

/** Chord as `Ctrl+Shift+V`, for the paste route trace. */
export function chordName(e: { key: string; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; metaKey: boolean }): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");
  if (e.metaKey) parts.push("Cmd");
  parts.push(e.key.length === 1 ? e.key.toUpperCase() : e.key);
  return parts.join("+");
}

/** Entry of the browser `contextmenu` handler: click point, event target and the pane hit. */
export function contextMenuLine(input: { clientX: number; clientY: number; target: string; paneId: string | null }): string {
  return `contextmenu client=(${input.clientX},${input.clientY}) target=${input.target} pane=${input.paneId ?? "null"}`;
}

/** Answer of `rightClickRoute` for the clicked pane. */
export function rightClickLine(paneId: string, route: string): string {
  return `rightClickRoute pane=${paneId} route=${route}`;
}

/** Pane menu state after the center region handled (or ignored) the event. */
export function menuStateLine(input: { state: "open" | "closed" | "ignored"; paneId?: string; known?: number; items?: readonly string[] }): string {
  if (input.state === "ignored") return `pane-menu ignored pane=${input.paneId ?? "null"} known=${input.known ?? 0}`;
  if (input.state === "closed") return "pane-menu closed";
  return `pane-menu open pane=${input.paneId ?? "null"} items=${(input.items ?? []).join(",")}`;
}

/** Position and stacking of the rendered menu (`ContextMenu` onMount). */
export function menuRectLine(input: {
  label: string;
  rect: { left: number; top: number; width: number; height: number } | null;
  zIndex: string;
  items: number;
}): string {
  const rect = input.rect
    ? `left=${Math.round(input.rect.left)} top=${Math.round(input.rect.top)} width=${Math.round(input.rect.width)} height=${Math.round(input.rect.height)}`
    : "rect=null";
  return `menu "${input.label}" ${rect} z=${input.zIndex} items=${input.items}`;
}

/** Paste shortcut: chord, route of `nativePasteRoute` and the focused pane it was routed for. */
export function pasteLine(input: { chord: string; route: string; paneId: string | null }): string {
  return `paste chord=${input.chord} route=${input.route} pane=${input.paneId ?? "null"}`;
}

/** Result reported by the backend for one native paste (never its content). */
export function pasteResultLine(result: { kind?: unknown; sent?: unknown; bytes?: unknown } | null | undefined): string {
  const kind = typeof result?.kind === "string" ? result.kind : "unknown";
  return `paste result kind=${kind} sent=${result?.sent === true} bytes=${typeof result?.bytes === "number" ? result.bytes : 0}`;
}
