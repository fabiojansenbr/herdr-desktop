// Spec 024 AC-024-01 — yield order of the clean title bar. Search shrinks to 240 px first;
// only then the brand, project selector, host pill and Novo agente give way. Widths here are
// the preferred sizes the CSS assigns; compact project/host may still flex-shrink to fit.

import { t } from "../../i18n/index.svelte";

export type SearchMode = "full" | "min" | "icon";
export type BrandMode = "name" | "icon";
export type ProjectMode = "full" | "compact";
export type NewAgentMode = "label" | "icon";
export type HostMode = "full" | "short";

export interface TopbarMode {
  search: SearchMode;
  brand: BrandMode;
  project: ProjectMode;
  newAgent: NewAgentMode;
  host: HostMode;
}

export const SEARCH_MAX_PX = 460;
export const SEARCH_MIN_PX = 240;
export const SEARCH_ICON_BELOW_PX = 640;
export const PROJECT_MAX_PX = 280;
export const PROJECT_COMPACT_PX = 160;

const FULL: TopbarMode = {
  search: "full",
  brand: "name",
  project: "full",
  newAgent: "label",
  host: "full",
};

function preferred(mode: TopbarMode): number {
  const brand = mode.brand === "icon" ? 22 : 70;
  const project = mode.project === "compact" ? PROJECT_COMPACT_PX : PROJECT_MAX_PX;
  const search = mode.search === "icon" ? 30 : mode.search === "min" ? SEARCH_MIN_PX : SEARCH_MAX_PX;
  const neu = mode.newAgent === "icon" ? 30 : 110;
  const host = mode.host === "short" ? 72 : 220;
  const fixed = 30 + 30 + 90;
  const chrome = 22 + 60;
  return fixed + brand + project + search + neu + host + chrome;
}

/** Compact flags for a logical window width. Defaults wide so unmeasured mounts keep spec 018. */
export function topbarMode(width: number): TopbarMode {
  if (!(width > 0)) return { ...FULL };
  if (width < SEARCH_ICON_BELOW_PX) {
    return { search: "icon", brand: "icon", project: "compact", newAgent: "icon", host: "short" };
  }
  const mode: TopbarMode = { ...FULL };
  if (preferred(mode) <= width) return mode;
  mode.search = "min";
  if (preferred(mode) <= width) return mode;
  mode.brand = "icon";
  if (preferred(mode) <= width) return mode;
  mode.project = "compact";
  if (preferred(mode) <= width) return mode;
  mode.host = "short";
  if (preferred(mode) <= width) return mode;
  mode.newAgent = "icon";
  return mode;
}

export function shortHostLabel(label: string | null | undefined): string {
  const text = (label ?? "").trim();
  return text || t("frame.host.none");
}
