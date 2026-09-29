// Tab containment of a modal dialog (ConnectionDialog). Native showModal() makes the window behind
// inert, but what Tab does past the last control is engine-specific (browser chrome, WebView host
// widget); this keeps it inside. Inner moves stay native so real DOM order is what users get.

/** Control to focus instead of the native Tab move, or null to let the native move happen. */
export function wrapTarget<T>(tabbables: readonly T[], active: T | null, backward: boolean): T | null {
  if (tabbables.length === 0) return null;
  const first = tabbables[0]!;
  const last = tabbables[tabbables.length - 1]!;
  const at = active === null ? -1 : tabbables.indexOf(active);
  if (at === -1) return backward ? last : first;
  if (backward) return at === 0 ? last : null;
  return at === tabbables.length - 1 ? first : null;
}

/** Control that opened the modal (an HTMLElement satisfies it); its state is read at close time. */
export interface ModalOpener {
  readonly isConnected: boolean;
  matches(selector: string): boolean;
  focus(): void;
}

export interface ModalCloseParts {
  /** The dialog element is still open (showModal). */
  isOpen(): boolean;
  /** dialog.close(): leaves the top layer, so the window behind is no longer inert. */
  release(): void;
  opener: ModalOpener | null;
  /** State change that removes the dialog from the DOM (controller.cancelDialog). */
  cancel(): void;
  /**
   * Composed window only: asks the host to return keyboard focus to the WebView after the dialog
   * is gone (`surface_focus_host`; GUI r9 lost the WebView widget focus when the modal left).
   */
  focusHost?: (() => Promise<void>) | undefined;
  /** Optional mark right after each step (`done` = the step acted; absent for cancel; for focus-host, the command outcome once it settles). */
  trace?: ModalTrace;
}

export type ModalTraceStep = "release" | "focus" | "cancel" | "focus-host";
export type ModalTrace = (step: ModalTraceStep, done?: boolean) => void;

let traceSink: ModalTrace | null = null;

/** Forwards to the sink installed by the fidelity page; without one (production) it does nothing. */
export const modalTrace: ModalTrace = (step, done) => traceSink?.(step, done);

/** Installs the trace sink (fidelity observation only); returns its removal. */
export function setModalTraceSink(sink: ModalTrace): () => void {
  traceSink = sink;
  return () => {
    if (traceSink === sink) traceSink = null;
  };
}

/**
 * Close request of one open modal (Escape, Cancelar, native close). GUI r7: removing the dialog
 * while focus was inside it made the WebView lose document focus before the opener was focused.
 * So, synchronously: release the inert window, focus the opener (if still attached and enabled),
 * and only then cancel; then, when given, ask the host for keyboard focus (decision r10). Runs once
 * per modal; later requests return false and do nothing.
 */
export function modalCloser(parts: ModalCloseParts): () => boolean {
  let done = false;
  return () => {
    if (done) return false;
    done = true;
    const open = parts.isOpen();
    if (open) parts.release();
    parts.trace?.("release", open);
    const opener = parts.opener;
    const usable = opener !== null && opener.isConnected && !opener.matches(":disabled");
    if (usable) opener.focus();
    parts.trace?.("focus", usable);
    parts.cancel();
    parts.trace?.("cancel");
    const host = parts.focusHost;
    if (host) {
      let pending: Promise<void>;
      try {
        pending = host();
      } catch (error) {
        pending = Promise.reject(error);
      }
      void pending.then(
        () => parts.trace?.("focus-host", true),
        () => parts.trace?.("focus-host", false),
      );
    }
    return true;
  };
}
