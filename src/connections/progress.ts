// Progress lines of the connection dialog (spec 011, AC-011-03): derived only from the real
// connection already exposed by the backend — the latency and the server version of the
// negotiated welcome, the SSH user typed in the form and the workspace list read from that host.
// Nothing advances before the dialog asks to connect, and a failure marks the step that did not
// arrive (a link failure keeps the dialog open for a retry).

import { errorText, t } from "../i18n/index.svelte";
import type { ConnectionsState } from "./controller";
import { failureReason } from "./presentation";
import type { HostDto, RuntimeError, WorkspaceDto } from "./types";

export type StepStatus = "pending" | "ok" | "error" | "blocked";

export interface ProgressStep {
  step: number;
  status: StepStatus;
  label: string;
  /** Right-side detail of the step (handshake latency, compatibility); null hides it. */
  meta: string | null;
  compat: boolean;
}

/** SSH user of the form (`user@host` or `ssh://user@host`); empty when the target names none. */
export function sshUser(target: string): string {
  const authority = target.startsWith("ssh://") ? target.slice(6) : target;
  const at = authority.indexOf("@");
  return at > 0 ? authority.slice(0, at) : "";
}

/** Step text of the authentication: with the user when the target names one. */
function authenticatedText(user: string): string {
  return user ? t("progress.authenticatedAs", { user }) : t("progress.authenticated");
}

/** Host this dialog asked to connect; null before the request or without a matching host. */
export function connectingHost(state: ConnectionsState): HostDto | null {
  const endpoint = state.dialog.connecting;
  if (endpoint === null) return null;
  return state.view?.hub.hosts.find((h) => h.endpoint === endpoint) ?? null;
}

/** Workspace list read from the connecting host; null while it has not arrived. */
export function connectingWorkspaces(state: ConnectionsState): readonly WorkspaceDto[] | null {
  const endpoint = state.dialog.connecting;
  return endpoint === null ? null : (state.workspaces[endpoint] ?? null);
}

/** Error of the connection in progress: refusal of the command, host link or workspace read. */
export function connectingFailure(state: ConnectionsState): RuntimeError | null {
  const endpoint = state.dialog.connecting;
  if (endpoint === null) return null;
  return state.dialog.submitError ?? state.hostErrors[endpoint] ?? connectingHost(state)?.connection_error ?? null;
}

/** Which slot the failure came from; only the workspace read writes `hostErrors` in the dialog. */
type FailureSource = "submit" | "workspaces" | "link";

/**
 * Where `connectingFailure` picked the error from. It is the structure of the state, not the
 * wording of the message: since spec 071 the host composes messages in English and the user
 * reads them through `errorText`, so nothing here may depend on the text.
 */
export function connectingFailureSource(state: ConnectionsState): FailureSource | null {
  const endpoint = state.dialog.connecting;
  if (endpoint === null) return null;
  if (state.dialog.submitError !== null) return "submit";
  if (state.hostErrors[endpoint] !== undefined) return "workspaces";
  return connectingHost(state)?.connection_error ? "link" : null;
}

/**
 * The step (0-based) a failure marks, decided by its stable `code` and, for the workspace read,
 * by the slot it arrived in. Spec 071 (AC-071-03): the message is never inspected.
 */
const STEP_BY_CODE: Record<string, number> = {
  remote_herdr_missing: 2,
  remote_herdr_outdated: 2,
  remote_server_not_running: 2,
  remote_server_incompatible: 2,
  // Spec 031: the refused auth choice is an authentication step failure, never a fabricated
  // reachability one.
  ssh_agent_unavailable: 1,
  remote_api_unsupported: 3,
};

function failureStep(failure: RuntimeError, firstMissing: number, source: FailureSource | null): number {
  const byCode = STEP_BY_CODE[failure.code];
  if (byCode !== undefined) return byCode;
  // The workspace list is only read once the host is online, so a refusal of that read belongs
  // to step 4 whatever its code (it used to be recognized by "workspaces" in the message).
  if (source === "workspaces") return 3;
  return firstMissing;
}

export function connectionProgress(state: ConnectionsState): ProgressStep[] {
  const host = connectingHost(state);
  const workspaces = connectingWorkspaces(state);
  const failure = connectingFailure(state);

  const latency = host?.latency_ms ?? null;
  const version = host?.server_version ?? null;
  const generation = host?.generation ?? null;
  const reason = host ? failureReason(host) : null;
  // Spec 029 (AC-029-02): a host whose herdr was not found/updated never reads as found.
  const herdrFailed =
    reason !== null && (host?.attention === "herdr_missing" || host?.attention === "herdr_outdated");
  const reached = latency !== null;
  const welcomed = reached || version !== null;
  // Monotonic evidence: a later step is only ok when every earlier one arrived.
  const evidence = [reached, welcomed, welcomed && generation !== null, workspaces !== null];
  const firstMissing = evidence.findIndex((arrived) => !arrived);
  const failedAt =
    failure === null
      ? null
      : failureStep(failure, firstMissing < 0 ? 3 : firstMissing, connectingFailureSource(state));
  const status = (index: number): StepStatus => {
    if (evidence[index] === true) return "ok";
    if (failedAt === null) return "pending";
    if (index === failedAt) return "error";
    if (index > failedAt) return "blocked";
    return "ok";
  };
  // Spec 072 (AC-072-03): the step that failed says what the failure means in the user's
  // language. `failure.message` is the host's English text (071); `errorText` reads the key of
  // its `code` and keeps that message only for a code the product has no key for.
  const label = (index: number, fallback: string): string =>
    failure !== null && failedAt === index ? errorText(failure) : fallback;

  // Spec 071: the words of these four lines live in `src/i18n/areas/progress.ts`; the measured
  // facts (latency, user, version) are interpolated into them. The attention reason of step 3
  // still comes from `failureReason`, which belongs to the connections area.
  const herdrLine =
    version !== null ? t("progress.herdrFoundVersion", { version }) : t("progress.herdrFound");

  return [
    {
      step: 1,
      status: status(0),
      label: label(0, latency !== null ? t("progress.reachableLatency", { latency }) : t("progress.reachable")),
      meta: latency !== null ? `${latency} ms` : null,
      compat: false,
    },
    {
      step: 2,
      status: status(1),
      label: label(1, authenticatedText(sshUser(state.dialog.draft.target))),
      meta: null,
      compat: false,
    },
    {
      step: 3,
      status: status(2),
      label: label(2, evidence[2] === true ? herdrLine : herdrFailed ? reason : t("progress.herdrFound")),
      meta: evidence[2] === true ? t("progress.compatible") : null,
      compat: true,
    },
    { step: 4, status: status(3), label: label(3, t("progress.readingWorkspaces")), meta: null, compat: false },
  ];
}
