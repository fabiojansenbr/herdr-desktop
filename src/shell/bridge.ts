// IPC wrapper for the composed window's single surface (spec 007). Command names follow the
// planned `selection` module in .local/orchestration/surface-contract.md; `session_start` is the
// existing explicit onboarding command of spec 001. No other command is invoked from here.

import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  FrameEvent,
  GeometryDto,
  InputDto,
  InterestOutcome,
  PasteReceipt,
  SelectionDto,
  StatusDto,
  SurfaceIdentityDto,
} from "./types";

/**
 * Input admitted by the surface and not answered yet (the batch in flight included); same limits
 * as `MAX_PENDING_INPUT_*` in src-tauri/src/bridge/composition.rs.
 * Contract: .local/orchestration/input-dispatch-contract.md.
 */
export const MAX_PENDING_INPUT_BATCHES = 256;
export const MAX_PENDING_INPUT_BYTES = 1024 * 1024;

export interface SurfaceBridge {
  selectionGet(): Promise<SelectionDto>;
  selectionSet(endpoint: string): Promise<SelectionDto>;
  /** Frames of the selected host only; the handler is bound to this attach. */
  attach(geometry: GeometryDto, onEvent: (event: FrameEvent) => void): Promise<StatusDto>;
  /**
   * Resolves once this batch was sent (or refused) by the backend; refused when
   * endpoint/boot/generation/pane differ or the surface changed before its turn. Never retried.
   */
  input(expected: SurfaceIdentityDto, events: InputDto[]): Promise<void>;
  resize(geometry: GeometryDto): Promise<void>;
  focus(focused: boolean): Promise<void>;
  status(): Promise<StatusDto>;
  /** Releases the channel only; hub and engine keep running. */
  detach(): Promise<StatusDto>;
  /**
   * Window interest in the selected host's surface (terminal hidden by the document or the files
   * layer). Optional only for legacy mocks; the product bridge always implements it.
   * Contract: .local/orchestration/interest-contract.md.
   */
  interest?(active: boolean): Promise<InterestOutcome | null>;
  /**
   * Native Ctrl+Shift+V: the backend reads the local clipboard at this paste's turn in the same
   * input lane and sends it once as one `Paste`; the text never reaches the WebView. Optional only
   * for legacy mocks; the product bridge always implements it.
   * Contract: .local/orchestration/native-paste-contract.md.
   */
  pasteClipboard?(expected: SurfaceIdentityDto): Promise<PasteReceipt>;
  /** Explicit user action ("Iniciar sessão"); never called on render. */
  startSession(): Promise<StatusDto>;
  /**
   * Returns keyboard focus to the WebView after a modal closed (`surface_focus_host`, no
   * arguments: the backend focuses only the invoking product window). Optional only for legacy
   * mocks; the product bridge always implements it.
   */
  focusHost?(): Promise<void>;
}

/** The product bridge: every optional member implemented. */
export type ProductSurfaceBridge = SurfaceBridge & { focusHost(): Promise<void> };

/** The Tauri primitives the bridge uses (injectable for contract tests). */
export interface SurfaceIpc {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** Channel object passed to `surface_attach`, delivering to `onEvent`. */
  channel(onEvent: (event: FrameEvent) => void): unknown;
}

const tauriIpc: SurfaceIpc = {
  invoke: (command, args) => invoke(command, args),
  channel: (onEvent) => {
    const channel = new Channel<FrameEvent>();
    channel.onmessage = onEvent;
    return channel;
  },
};

export function tauriSurfaceBridge(ipc: SurfaceIpc = tauriIpc): ProductSurfaceBridge {
  return {
    selectionGet: () => ipc.invoke<SelectionDto>("selection_get"),
    selectionSet: (endpoint) => ipc.invoke<SelectionDto>("selection_set", { endpoint }),
    attach: (geometry, onEvent) => ipc.invoke<StatusDto>("surface_attach", { geometry, onEvent: ipc.channel(onEvent) }),
    input: (expected, events) => ipc.invoke<void>("surface_input", { expected, events }),
    resize: (geometry) => ipc.invoke<void>("surface_resize", { geometry }),
    focus: (focused) => ipc.invoke<void>("surface_focus", { focused }),
    status: () => ipc.invoke<StatusDto>("surface_status"),
    detach: () => ipc.invoke<StatusDto>("surface_detach"),
    interest: (active) => ipc.invoke<InterestOutcome | null>("surface_interest", { active }),
    pasteClipboard: (expected) => ipc.invoke<PasteReceipt>("surface_paste_clipboard", { expected }),
    startSession: () => ipc.invoke<StatusDto>("session_start"),
    focusHost: () => ipc.invoke<void>("surface_focus_host"),
  };
}
