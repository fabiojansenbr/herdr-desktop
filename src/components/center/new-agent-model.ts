// Spec 073 — what the "New agent" popover shows: the sections of its list, the display name and
// tile of each agent kind, and the recently started kinds.
//
// The engine publishes the kinds it knows, not which of them are installed, so the host answers
// that for the Local host (`agent_kinds_available`). Installed kinds come first; the rest are
// still offered, only dimmed, because the engine may launch an agent this lookup cannot see.
// `available === null` means "nobody answered" (a remote host, a failed or pending probe): every
// kind then counts as installed and no `Not installed` section exists at all.

import { AGENT_DISPLAY_TITLES } from "./model";

/** The shell entry: not an engine kind, so it never joins `Recent` or `Not installed`. */
export const SHELL_KIND = "Shell";

/** Where the recently started kinds are kept (per machine, not per session). */
export const RECENT_AGENTS_KEY = "herdr.recentAgents";

/** How many recent kinds the list shows. */
export const MAX_RECENT_AGENTS = 3;

/** Where the auto mode answer of the popup is kept (per machine, like the recent kinds). */
export const AGENT_AUTONOMY_KEY = "herdr.agentAutonomy";

export type AgentSection = "recent" | "all" | "notInstalled" | "shell";

export interface AgentChoice {
  /** Engine kind, or `Shell`. */
  kind: string;
  /** Name as the user reads it (`claude` → `Claude Code`). */
  label: string;
  /** Letter of the 20 px tile, like the workspace rows. */
  initial: string;
  section: AgentSection;
  /** `false` only when the host answered and found no binary. */
  installed: boolean;
}

/** Display name of one kind: the product's table, else the type with an upper-case initial. */
export function agentLabel(kind: string): string {
  const known = AGENT_DISPLAY_TITLES[kind];
  if (known) return known;
  return kind.charAt(0).toUpperCase() + kind.slice(1);
}

/** Tile letter of a display name: its first letter or digit, upper-cased. */
export function agentInitial(label: string): string {
  return (label.match(/[\p{L}\p{N}]/u)?.[0] ?? "").toUpperCase();
}

/** Does `query` match this kind? Both the display name and the type are searched. */
export function matchesAgentQuery(choice: { kind: string; label: string }, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  return choice.label.toLowerCase().includes(needle) || choice.kind.toLowerCase().includes(needle);
}

/**
 * The rows of the list, in the order they are drawn: the recent kinds as the user last used them,
 * then the installed ones by display name, then the ones with no binary found, and `Shell` last
 * (the component draws the divider before it). `query` filters every row, `Shell` included.
 */
export function agentChoices(input: {
  kinds: readonly string[];
  available: readonly string[] | null;
  recent: readonly string[];
  query?: string;
}): AgentChoice[] {
  const kinds = [...new Set(input.kinds.filter((kind) => kind !== SHELL_KIND))];
  const installed = (kind: string) => input.available === null || input.available.includes(kind);
  const choice = (kind: string, section: AgentSection): AgentChoice => {
    const label = section === "shell" ? SHELL_KIND : agentLabel(kind);
    // `Shell` is the engine's own tab, never a binary this lookup could miss.
    return { kind, label, initial: agentInitial(label), section, installed: section === "shell" || installed(kind) };
  };

  const recent = input.recent.filter((kind) => kinds.includes(kind)).slice(0, MAX_RECENT_AGENTS);
  const rest = kinds.filter((kind) => !recent.includes(kind));
  const byLabel = (a: AgentChoice, b: AgentChoice) => a.label.localeCompare(b.label);
  const rows = [
    ...recent.map((kind) => choice(kind, "recent")),
    ...rest.filter(installed).map((kind) => choice(kind, "all")).sort(byLabel),
    ...rest.filter((kind) => !installed(kind)).map((kind) => choice(kind, "notInstalled")).sort(byLabel),
    choice(SHELL_KIND, "shell"),
  ];
  const query = input.query ?? "";
  return rows.filter((row) => matchesAgentQuery(row, query));
}

/** `localStorage` may be absent or throw (private mode, disabled storage): then there are none. */
export function readRecentAgents(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(RECENT_AGENTS_KEY) ?? "[]");
    if (!Array.isArray(stored)) return [];
    return [...new Set(stored.filter((kind): kind is string => typeof kind === "string" && kind !== ""))].slice(
      0,
      MAX_RECENT_AGENTS,
    );
  } catch {
    return [];
  }
}

/** Puts `kind` first, without repeating it, and keeps at most three. Returns the new list. */
export function rememberRecentAgent(kind: string): string[] {
  const next = [kind, ...readRecentAgents().filter((entry) => entry !== kind)].slice(0, MAX_RECENT_AGENTS);
  if (typeof window !== "undefined") {
    try {
      localStorage.setItem(RECENT_AGENTS_KEY, JSON.stringify(next));
    } catch {
      // The choice is not kept across restarts; the list of this session still follows it.
    }
  }
  return next;
}

/**
 * Is auto mode on? It starts on and stays as the user left it; no storage at all (private mode,
 * disabled storage) also reads as on, which is the default the popup shows.
 */
export function readAgentAutonomy(): boolean {
  if (typeof window === "undefined") return true;
  try {
    return localStorage.getItem(AGENT_AUTONOMY_KEY) !== "off";
  } catch {
    return true;
  }
}

/** Keeps the answer for the next popup and returns it. */
export function rememberAgentAutonomy(on: boolean): boolean {
  if (typeof window !== "undefined") {
    try {
      localStorage.setItem(AGENT_AUTONOMY_KEY, on ? "on" : "off");
    } catch {
      // The choice is not kept across restarts; this popup still follows it.
    }
  }
  return on;
}

/** The flags of one kind as a row shows them (`--yolo --trust`), or null when it has none. */
export function autonomyHint(flags: readonly string[] | undefined): string | null {
  const named = (flags ?? []).filter((flag) => flag.trim() !== "");
  return named.length === 0 ? null : named.join(" ");
}
