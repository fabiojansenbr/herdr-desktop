// Spec 067 (PRD i18n) — the i18n base: environment locale, `t()` with interpolation and
// `Intl.PluralRules`, relative time, the single table of agent status labels, the phase/error
// helpers and the untranslated detector the area specs (068–071) use.
//
// Would catch: a locale tag resolved to the wrong language, a saved preference ignored at boot,
// `document.documentElement.lang` left at the old fixed `pt-BR`, an area dictionary needing a
// manual registry, a missing key failing loudly instead of falling back to `en`, a status label
// duplicated again in one of the four places, or the vitest/E2E harnesses drifting off `pt`.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushSync, mount, unmount } from "svelte";
import { beforeEach, describe, expect, it } from "vitest";
import { normalizeStatus, statusPresentation } from "../agents/status";
import type { AgentStatus } from "../agents/types";
import { frameStatus } from "../components/center/model";
import { paletteSections, type PaletteSource } from "../components/frame/palette";
import { TAB_STATUS_TEXT } from "../components/sidebar/tab-rows";
import type { LinkPhase } from "../connections/types";
import {
  area,
  dictionaries,
  errorText,
  formatRelative,
  HTML_LANG,
  initLocale,
  locale,
  LOCALE_STORAGE_KEY,
  LOCALES,
  localePreference,
  phaseText,
  plural,
  resolveLocale,
  setLocalePreference,
  t,
  translate,
  type Locale,
  type Translated,
} from "./index.svelte";
import LocaleProbe from "./LocaleProbe.svelte";
import { untranslated } from "./testing";

const root = (path: string) => readFileSync(resolve(path), "utf8");

/**
 * Compile-time proof of AC-067-02: a `pt`/`es` that is missing a key of `en` is not a valid
 * translation, so `bun run check` rejects the area. If `Translated` ever became partial this
 * constant would stop compiling (the conditional would be `true`).
 */
type PartialTranslationIsRejected = { a: string } extends Translated<{ a: string; b: string }> ? true : false;
const _partialTranslationIsRejected: PartialTranslationIsRejected = false;
void _partialTranslationIsRejected;

// --- AC-067-04: the suite and the real-window harnesses are fixed on pt --------------------

describe("AC-067-04 testes e E2E fixos em pt", () => {
  // Would catch: the setup file dropped from one of the two vitest projects (the existing
  // assertions in Portuguese would then depend on the machine's locale).
  it("fixes pt through one setup file shared by the node and dom projects", () => {
    // First assertion of the file: nothing has changed the locale yet, so this is the setup's.
    expect(locale()).toBe("pt");
    const config = root("vite.config.ts");
    expect(config).toContain('setupFiles: ["./src/test-setup.ts"]');
    expect(config.match(/setupFiles/g)).toHaveLength(1);
    expect(root("src/test-setup.ts")).toContain('setLocalePreference("pt")');
  });

  // Would catch: a launcher that opens the real window without fixing the locale, which would
  // make the E2E selectors in Portuguese depend on the host's `LANG`.
  it("exports HERDR_DESKTOP_LOCALE=pt in the five real-window launchers", () => {
    for (const path of [
      "scripts/capture-window.sh",
      "scripts/measure-resources.sh",
      "src-tauri/tests/e2e_linux.rs",
      "src-tauri/tests/contracts.rs",
      "src-tauri/tests/bootstrap_zero_config.rs",
    ]) {
      expect(root(path), path).toMatch(/HERDR_DESKTOP_LOCALE[^\n]*pt/);
    }
  });
});

// --- AC-067-01: resolving the language ------------------------------------------------------

describe("AC-067-01 resolução do idioma", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  // Would catch: a region subtag or an encoding suffix leaking into the decision, or a language
  // the product does not carry resolving to anything but English.
  it("maps every environment tag to one of the three locales", () => {
    const tags = ["pt-BR", "pt_PT.UTF-8", "es-MX", "es", "en_US.UTF-8", "fr-FR", "C", ""];
    expect(tags.map(resolveLocale)).toEqual(["pt", "pt", "es", "es", "en", "en", "en", "en"]);
  });

  // Would catch: the saved preference ignored, `auto` not falling back to the environment, or
  // `<html lang>` left at the value `index.html` used to pin.
  it("boots from the saved preference and otherwise from app_locale, writing <html lang>", async () => {
    localStorage.setItem(LOCALE_STORAGE_KEY, "es");
    await initLocale(async () => "pt-BR");
    expect(locale()).toBe("es");
    expect(localePreference()).toBe("es");
    expect(document.documentElement.lang).toBe("es");

    localStorage.setItem(LOCALE_STORAGE_KEY, "auto");
    await initLocale(async () => "pt_BR.UTF-8");
    expect(locale()).toBe("pt");
    expect(localePreference()).toBe("auto");
    expect(document.documentElement.lang).toBe("pt-BR");

    localStorage.removeItem(LOCALE_STORAGE_KEY);
    await initLocale(async () => "en_US.UTF-8");
    expect(locale()).toBe("en");
    expect(document.documentElement.lang).toBe("en");

    localStorage.setItem(LOCALE_STORAGE_KEY, "klingon");
    await initLocale(async () => "es-MX");
    expect(locale()).toBe("es");
  });

  // Edge of the spec: `localStorage` unavailable must not break the boot.
  it("follows the environment when localStorage throws", async () => {
    const original = Object.getOwnPropertyDescriptor(window, "localStorage");
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      get() {
        throw new Error("storage disabled");
      },
    });
    try {
      await initLocale(async () => "pt-BR");
      expect(locale()).toBe("pt");
      setLocalePreference("es");
      expect(locale()).toBe("es");
    } finally {
      if (original) Object.defineProperty(window, "localStorage", original);
    }
  });

  // Would catch: `app_locale` unavailable (no Tauri host) turning the boot into a rejected promise.
  it("falls back to English when app_locale fails", async () => {
    await initLocale(async () => {
      throw new Error("no host");
    });
    expect(locale()).toBe("en");
  });

  // Would catch: `index.html` pinning a language again, so the document would lie until the boot.
  it("index.html no longer pins pt-BR", () => {
    expect(root("index.html")).toContain('<html lang="en">');
    expect(root("src/main.ts")).toContain("initLocale");
  });

  it("maps each locale to its document language", () => {
    expect(HTML_LANG).toEqual({ en: "en", pt: "pt-BR", es: "es" });
    expect(LOCALES).toEqual(["en", "pt", "es"]);
  });
});

// --- AC-067-02: t(), areas and plurals ------------------------------------------------------

describe("AC-067-02 t(), áreas e plurais", () => {
  // Would catch: interpolation dropped, the plural category taken from the wrong language, or a
  // locale switch not reaching `t()`.
  it("translates, interpolates and pluralises in the three languages", () => {
    const expected = [
      ["en", "Hello Ana", "1 item", "2 items"],
      ["pt", "Olá Ana", "1 item", "2 itens"],
      ["es", "Hola Ana", "1 elemento", "2 elementos"],
    ] as const;
    for (const [language, hello, one, other] of expected) {
      setLocalePreference(language);
      expect(t("demo.hello", { name: "Ana" })).toBe(hello);
      expect(t("demo.items", { count: 1 })).toBe(one);
      expect(t("demo.items", { count: 2 })).toBe(other);
    }
  });

  // Would catch: a missing translation rendering as an empty string, or a typo in a key throwing.
  it("falls back to en and then to the key itself", () => {
    const dicts = { en: { "x.only": "English only" }, pt: {}, es: {} };
    expect(translate(dicts, "es", "x.only")).toBe("English only");
    expect(translate(dicts, "pt", "x.only")).toBe("English only");
    expect(translate(dicts, "en", "x.only")).toBe("English only");
    expect(translate(dicts, "es", "x.missing")).toBe("x.missing");
    expect(translate(dicts, "en", "x.missing")).toBe("x.missing");
    setLocalePreference("es");
    expect(t("no.such.key")).toBe("no.such.key");
  });

  // Would catch: an area needing a manual registration (the 068–071 specs add areas in parallel
  // and must not edit a shared file).
  it("discovers areas by import.meta.glob, with pt/es holding every en key", () => {
    expect(root("src/i18n/index.svelte.ts")).toContain('import.meta.glob');
    expect(root("src/i18n/index.svelte.ts")).toContain('"./areas/*.ts"');
    const dicts = dictionaries();
    const english = Object.keys(dicts.en).sort();
    expect(english.length).toBeGreaterThan(0);
    for (const language of ["pt", "es"] as const) {
      expect(Object.keys(dicts[language]).sort(), language).toEqual(english);
    }
  });

  // Would catch: `plural` picking `other` for a language whose rules differ from English.
  it("selects the plural category with Intl.PluralRules", () => {
    const forms = { one: "um", other: "muitos" };
    expect(plural(1, forms, "pt")).toBe("um");
    expect(plural(2, forms, "pt")).toBe("muitos");
    // The category is the language's, not a count test: CLDR counts 0 as `one` in pt, `other` in en.
    expect(plural(0, forms, "pt")).toBe("um");
    expect(plural(0, forms, "en")).toBe("muitos");
    expect(plural(0, forms, "es")).toBe("muitos");
    expect(plural(2, forms, "es")).toBe("muitos");
    expect(plural(3, { other: "só other" }, "en")).toBe("só other");
  });

  // Would catch: `t()` reading a non-reactive copy of the locale, which would leave a mounted
  // window in the old language until a reload.
  it("re-renders a Svelte component that uses t() when the locale changes", () => {
    setLocalePreference("pt");
    const target = document.createElement("div");
    document.body.appendChild(target);
    const component = mount(LocaleProbe, { target });
    try {
      flushSync();
      expect(target.querySelector('[data-probe="status"]')!.textContent).toBe("Trabalhando");
      expect(target.querySelector('[data-probe="items"]')!.textContent).toBe("2 itens");
      setLocalePreference("es");
      flushSync();
      expect(target.querySelector('[data-probe="status"]')!.textContent).toBe("Trabajando");
      expect(target.querySelector('[data-probe="items"]')!.textContent).toBe("2 elementos");
    } finally {
      void unmount(component);
      target.remove();
    }
  });

  // Would catch: `area()` losing the `{ en, pt, es }` shape the glob expects.
  it("area() keeps the three dictionaries it was given", () => {
    const sample = area({ en: { "x.a": "A" }, pt: { "x.a": "a" }, es: { "x.a": "á" } });
    expect(Object.keys(sample)).toEqual(["en", "pt", "es"]);
  });
});

// --- AC-067-03: relative time, status table and helpers -------------------------------------

describe("AC-067-03 tempo relativo, status e ajudantes", () => {
  // Would catch: a changed threshold or a language keeping the Portuguese words (the pt values are
  // the ones the product already shows).
  it("formats relative time per locale", () => {
    const cases: [number, string, string, string][] = [
      [30_000, "agora", "now", "ahora"],
      [5 * 60_000, "5 min", "5 min", "5 min"],
      [3 * 3_600_000, "3 h", "3 h", "3 h"],
      [2 * 86_400_000, "2 d", "2 d", "2 d"],
    ];
    for (const [ms, pt, en, es] of cases) {
      setLocalePreference("pt");
      expect(formatRelative(ms), `pt ${ms}`).toBe(pt);
      setLocalePreference("en");
      expect(formatRelative(ms), `en ${ms}`).toBe(en);
      setLocalePreference("es");
      expect(formatRelative(ms), `es ${ms}`).toBe(es);
    }
  });

  // Would catch: one of the four places keeping its own copy of the labels — the duplication this
  // spec removes — or a language missing a state.
  it("has one table of agent status labels, read by the four places", () => {
    const table: Record<Locale, Record<AgentStatus, string>> = {
      pt: { working: "Trabalhando", blocked: "Aguardando você", idle: "Ocioso", done: "Concluído", unknown: "Desconhecido" },
      en: { working: "Working", blocked: "Waiting for you", idle: "Idle", done: "Done", unknown: "Unknown" },
      es: { working: "Trabajando", blocked: "Esperándote", idle: "Inactivo", done: "Terminado", unknown: "Desconocido" },
    };
    const statuses: AgentStatus[] = ["working", "blocked", "idle", "done", "unknown"];
    for (const language of LOCALES) {
      setLocalePreference(language);
      for (const status of statuses) {
        const want = table[language][status];
        expect(t(`agent.status.${status}`), `${language}/${status}`).toBe(want);
        expect(statusPresentation(status).label, `status.ts ${language}/${status}`).toBe(want);
        expect(frameStatus(status).label, `center ${language}/${status}`).toBe(want);
        expect(paletteDetail(status), `palette ${language}/${status}`).toContain(want);
      }
      // The sidebar's four kinds map onto the same table (`waiting` is the engine's `blocked`).
      expect(TAB_STATUS_TEXT.working).toBe(table[language].working);
      expect(TAB_STATUS_TEXT.waiting).toBe(table[language].blocked);
      expect(TAB_STATUS_TEXT.done).toBe(table[language].done);
      expect(TAB_STATUS_TEXT.idle).toBe(table[language].idle);
    }
    // The four modules read the table; none of them holds the words any more.
    for (const path of [
      "src/agents/status.ts",
      "src/components/center/model.ts",
      "src/components/frame/palette.ts",
      "src/components/sidebar/tab-rows.ts",
    ]) {
      expect(root(path), path).not.toMatch(/Trabalhando|Aguardando você|Ocioso|Concluído/);
    }
    expect(normalizeStatus("nonsense")).toBe("unknown");
  });

  // Would catch: a connection phase without words of its own (the state would then read only as a
  // colour), or a phase dropped from the helper.
  it("phaseText covers the five link phases in the three languages", () => {
    const phases: LinkPhase[] = ["offline", "connecting", "online", "reconnecting", "attention"];
    setLocalePreference("pt");
    expect(phases.map((phase) => phaseText(phase))).toEqual([
      "Offline",
      "Conectando",
      "Online",
      "Reconectando",
      "Precisa de atenção",
    ]);
    setLocalePreference("en");
    expect(phases.map((phase) => phaseText(phase))).toEqual([
      "Offline",
      "Connecting",
      "Online",
      "Reconnecting",
      "Needs attention",
    ]);
    setLocalePreference("es");
    expect(phaseText("attention")).toBe("Necesita atención");
  });

  // Would catch: an engine message being hidden behind a generic text when no key exists for its
  // code (the user would lose the only explanation available).
  it("errorText prefers the key of the code and otherwise shows the message", () => {
    setLocalePreference("pt");
    expect(errorText({ code: "demo_code", message: "raw message" })).toBe("Erro de demonstração");
    setLocalePreference("en");
    expect(errorText({ code: "demo_code", message: "raw message" })).toBe("Demo error");
    expect(errorText({ code: "no_such_code", message: "raw message" })).toBe("raw message");
    expect(errorText({ code: "", message: "raw message" })).toBe("raw message");
  });
});

// --- AC-067-05: the detector the area specs use ---------------------------------------------

describe("AC-067-05 detector para as áreas", () => {
  const fixture = (html: string): Element => {
    const host = document.createElement("div");
    host.innerHTML = html;
    return host.firstElementChild!;
  };

  // Would catch: the detector reading only text (missing the accessible names), or reporting a
  // translated tree as untranslated.
  it("reports Portuguese text and accessible names, and nothing in an English tree", () => {
    expect(untranslated(fixture('<div aria-label="Fechar aba">Workspaces <span title="Close">agora</span></div>'))).toEqual([
      "Fechar aba",
      "agora",
    ]);
    expect(untranslated(fixture("<div>Close tab <span>now</span></div>"))).toEqual([]);
  });

  // Would catch: the word list matching inside longer words (`sem` inside `assembly`) or the
  // accent marks being case-sensitive.
  it("matches whole words and accents in either case, including placeholders", () => {
    expect(untranslated(fixture('<div><input placeholder="Buscar arquivos" /><p>assembly semantics</p></div>'))).toEqual([
      "Buscar arquivos",
    ]);
    expect(untranslated(fixture("<div>AÇÕES</div>"))).toEqual(["AÇÕES"]);
    expect(untranslated(fixture("<div>Nova coleção</div>"))).toEqual(["Nova coleção"]);
    expect(untranslated(fixture("<div><style>.a{}</style><script></script>Fine</div>"))).toEqual([]);
  });
});

/** One palette entry's detail for `status` (the palette lists the agent state next to the pane). */
function paletteDetail(status: AgentStatus): string {
  const source: PaletteSource = {
    hasSession: true,
    projects: [],
    panes: [],
    agents: [{ pane_id: "w1:p1", name: "claude", kind: "claude", status }],
    commands: [],
    openProject: () => {},
    focusPane: () => {},
    openAgent: () => {},
  };
  return paletteSections(source, "")
    .find((section) => section.id === "agents")!
    .entries[0]!.detail;
}
