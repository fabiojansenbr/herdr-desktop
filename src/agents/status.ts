// Presentation of the engine's agent states. Text and icon differ for every state (never colour
// alone); anything the engine did not publish as one of the five states is "unknown", which is
// never shown as done.

import { t } from "../i18n/index.svelte";
import type { AgentStatus } from "./types";

export const AGENT_STATUSES: readonly AgentStatus[] = ["working", "blocked", "idle", "done", "unknown"];

export interface StatusPresentation {
  label: string;
  icon: string;
  tone: "busy" | "attention" | "calm" | "success" | "muted";
  needsUser: boolean;
}

// Icon, tone and meaning are the product's; the words are the one i18n table (spec 067), read at
// call time so a language change reaches every reader without a reload.
const PRESENTATION: Record<AgentStatus, Omit<StatusPresentation, "label">> = {
  working: { icon: "⟳", tone: "busy", needsUser: false },
  blocked: { icon: "✋", tone: "attention", needsUser: true },
  idle: { icon: "○", tone: "calm", needsUser: false },
  done: { icon: "✓", tone: "success", needsUser: false },
  unknown: { icon: "?", tone: "muted", needsUser: false },
};

/** The one label of a state (`agent.status.*`), shared with the center, the palette and the sidebar. */
export function statusLabel(status: AgentStatus): string {
  return t(`agent.status.${normalizeStatus(status)}`);
}

export function normalizeStatus(value: unknown): AgentStatus {
  return typeof value === "string" && (AGENT_STATUSES as readonly string[]).includes(value) ? (value as AgentStatus) : "unknown";
}

export function statusPresentation(status: AgentStatus): StatusPresentation {
  const known = normalizeStatus(status);
  return { label: statusLabel(known), ...PRESENTATION[known] };
}
