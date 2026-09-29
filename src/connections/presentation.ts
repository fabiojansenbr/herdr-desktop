// Text for connection states and explicit actions. Every state has its own words (not only a
// colour); attention always carries configuration guidance and is retried only on request.
//
// Spec 070: every word here comes from `t()`. The five phases reuse `phaseText` of the core area
// (spec 067), so a phase is written in exactly one place; only the tone stays local.

import { phaseText, t } from "../i18n/index.svelte";
import type { DraftForm, HostDto, InputBlock, LinkPhase } from "./types";

export type Tone = "idle" | "progress" | "ok" | "warn" | "attention";

export interface HostStatus {
  text: string;
  tone: Tone;
  detail: string;
  guidance: string | null;
}

const PHASE_TONE: Record<LinkPhase, Tone> = {
  offline: "idle",
  connecting: "progress",
  online: "ok",
  reconnecting: "warn",
  attention: "attention",
};

export function hostStatus(host: HostDto): HostStatus {
  let detail = "";
  if (host.phase === "offline" && host.cancelled) {
    detail = t("connections.detail.cancelled");
  } else if (host.phase === "attention") {
    detail = host.connection_error?.message ?? "";
  } else if ((host.phase === "reconnecting" || host.phase === "offline") && host.retry_in_ms !== null) {
    detail = t("connections.detail.retry", { attempt: host.attempt, seconds: Math.ceil(host.retry_in_ms / 1000) });
  }
  return {
    text: phaseText(host.phase),
    tone: PHASE_TONE[host.phase],
    detail,
    guidance: host.phase === "attention" ? host.guidance : null,
  };
}

/**
 * Why a host is not connected, in words (spec 029, AC-029-02). Never a fabricated default: a
 * host without a recorded failure has no reason, and a failed host never reads as connected.
 */
export function failureReason(host: HostDto): string | null {
  if (!host.connection_error) return null;
  switch (host.attention) {
    case "herdr_missing":
    case "herdr_outdated":
    case "authentication_required":
    case "host_key_unknown":
    case "host_key_changed":
    case "ssh_unavailable":
    case "server_not_running":
    case "server_incompatible":
      return t(`connections.failure.${host.attention}`);
    default:
      return t("connections.failure.unknown");
  }
}

export type HostAction = "connect" | "cancel" | "retry";

export function hostActions(host: HostDto): HostAction[] {
  switch (host.phase) {
    case "offline":
      return ["connect"];
    case "attention":
      return ["retry"];
    default:
      return ["cancel"];
  }
}

/**
 * Words of each explicit action. Getters, not a frozen table: the text is read at render time, so
 * a language change reaches a button that is already on screen.
 */
export const ACTION_TEXT: Record<HostAction, string> = {
  get connect() {
    return t("connections.action.connect");
  },
  get cancel() {
    return t("connections.action.cancel");
  },
  get retry() {
    return t("connections.action.retry");
  },
};

export function inputBlockText(block: InputBlock | null): string {
  return block ? t(`connections.block.${block}`) : "";
}

const SESSION = /^[A-Za-z0-9._-]+$/;

/** Mirrors ssh_options.rs / profiles.rs validation. Returns only the failing fields. */
export function validateDraft(draft: Omit<DraftForm, "id">): Partial<Record<keyof DraftForm, string>> {
  const errors: Partial<Record<keyof DraftForm, string>> = {};
  const label = draft.label.trim();
  if (label === "" || new TextEncoder().encode(label).length > 128) errors.label = t("connections.validation.label");

  const target = draft.target.trim();
  if (target === "") errors.target = t("connections.validation.target");
  else if (target.startsWith("-")) errors.target = t("connections.validation.targetDash");
  else if (/[\s\p{Cc}]/u.test(target) || target.length > 1024) errors.target = t("connections.validation.targetInvalid");
  else {
    const authority = target.startsWith("ssh://") ? target.slice(6) : target;
    const at = authority.lastIndexOf("@");
    if (at >= 0 && authority.slice(0, at).includes(":")) errors.target = t("connections.validation.targetPassword");
  }

  const port = draft.port.trim();
  if (port !== "" && (!/^\d+$/.test(port) || Number(port) < 1 || Number(port) > 65535)) {
    errors.port = t("connections.validation.port");
  }

  const session = draft.session.trim();
  if (session === "") errors.session = t("connections.validation.session");
  else if (session.length > 64 || session === "." || session === ".." || !SESSION.test(session)) {
    errors.session = t("connections.validation.sessionChars");
  }
  return errors;
}
