// Command palette model (spec 010, AC-010-03): Projetos (the window's project catalog), Panes and
// Agentes (the engine's topology and agent list of the selected session) and Comandos (the enabled
// menu actions). Without a session only Comandos is listed.
//
// Spec 069 (AC-069-03) adds Idioma/Language as a group of the palette — the menu surface the app
// really renders. It is built by `languageSection()` rather than by `paletteSections()`, because it
// chooses a language instead of acting on one of the window's own sources.
import { statusLabel } from "../../agents/status";
import { LOCALES, localePreference, setLocalePreference, t, type Locale, type LocalePreference } from "../../i18n/index.svelte";
import type { AgentStatus } from "../../agents/types";
import { requestNewWorkspace } from "../../projects/new-workspace";

export type SectionId = "projects" | "panes" | "agents" | "commands" | "language";

/** The key each section's title reads; the language group uses the product's own word for it. */
const SECTION_KEY: Record<SectionId, string> = {
  projects: "frame.palette.section.projects",
  panes: "frame.palette.section.panes",
  agents: "frame.palette.section.agents",
  commands: "frame.palette.section.commands",
  language: "frame.language",
};

/** Title of each section, read on every call so the palette follows the current language. */
export function sectionTitle(id: SectionId): string {
  return t(SECTION_KEY[id]);
}


export interface PaletteSource {
  hasSession: boolean;
  projects: { id: string; label: string; root: string; host: string }[];
  panes: { pane_id: string; focused: boolean; width: number; height: number }[];
  agents: { pane_id: string; name: string | null; kind: string | null; status: AgentStatus }[];
  commands: { id: string; label: string; shortcut: string | null; run: () => void }[];
  openProject(id: string): void;
  focusPane(paneId: string): void;
  openAgent(paneId: string): void;
}

export interface PaletteEntry {
  id: string;
  label: string;
  detail: string;
  run: () => void;
  /** Set only on the options of a choice group: `true` on the one in force, which is marked. */
  checked?: boolean;
}

export interface PaletteSection {
  id: SectionId;
  title: string;
  entries: PaletteEntry[];
}

const fold = (text: string) => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

/** The palette's one filter: case- and accent-insensitive over the label and the detail. */
function matches(query: string): (entry: PaletteEntry) => boolean {
  const q = fold(query.trim());
  return (e) => q === "" || fold(`${e.label} ${e.detail}`).includes(q);
}

export function paletteSections(src: PaletteSource, query: string): PaletteSection[] {
  const keep = matches(query);
  const section = (id: SectionId, entries: PaletteEntry[]): PaletteSection => ({ id, title: sectionTitle(id), entries: entries.filter(keep) });
  const commands = section("commands", [
    // Spec 046: the palette opens the "Novo workspace" modal of the sidebar — the same one the
    // `+` of the WORKSPACES header opens, on the host the window is on.
    { id: "command:new-workspace", label: t("frame.palette.newWorkspace"), detail: t("frame.palette.newWorkspace.detail"), run: () => requestNewWorkspace() },
    ...src.commands.filter((c) => c.id !== "search").map((c) => ({ id: `command:${c.id}`, label: c.label, detail: c.shortcut ?? "", run: c.run })),
  ]);
  if (!src.hasSession) return [commands];
  return [
    section("projects", src.projects.map((p) => ({ id: `project:${p.id}`, label: p.label, detail: `${p.host} · ${p.root}`, run: () => src.openProject(p.id) }))),
    section("panes", src.panes.map((p) => ({ id: `pane:${p.pane_id}`, label: p.pane_id, detail: `${p.width}×${p.height}${p.focused ? ` · ${t("frame.palette.paneFocused")}` : ""}`, run: () => src.focusPane(p.pane_id) }))),
    section("agents", src.agents.map((a) => ({ id: `agent:${a.pane_id}`, label: a.name ?? a.kind ?? a.pane_id, detail: `${statusLabel(a.status)} · ${a.pane_id}`, run: () => src.openAgent(a.pane_id) }))),
    commands,
  ];
}

/**
 * Every language names itself in its own words (AC-069-03), so these three are written once and
 * read the same whatever the interface's language is.
 */
const LANGUAGE_NAMES: Record<Locale, string> = { en: "English", pt: "Português", es: "Español" };

/**
 * The language group of the palette: `Automatic` plus one option per language the product carries,
 * the preference in force marked. The group's own word is each option's detail, so searching
 * `Idioma`/`Language` reaches them — the filter reads the label and the detail, not the title.
 */
export function languageSection(query: string): PaletteSection {
  const group = t("frame.language");
  const current = localePreference();
  const option = (id: LocalePreference, label: string): PaletteEntry => ({
    id: `language:${id}`,
    label,
    detail: group,
    checked: current === id,
    run: () => setLocalePreference(id),
  });
  const entries = [option("auto", t("frame.language.auto")), ...LOCALES.map((language) => option(language, LANGUAGE_NAMES[language]))];
  return { id: "language", title: group, entries: entries.filter(matches(query)) };
}

export interface FocusTarget {
  readonly isConnected: boolean;
  focus(): void;
}

/** Remembers the element focused when the palette opened and gives it the focus back on close. */
export function createPaletteFocus<T extends FocusTarget>(active: () => T | null) {
  let opener: T | null = null;
  return {
    open() {
      opener = active();
    },
    close(): T | null {
      const target = opener;
      opener = null;
      if (!target?.isConnected) return null;
      target.focus();
      return target;
    },
  };
}
