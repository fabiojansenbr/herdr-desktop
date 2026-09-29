// The one i18n module (spec 067, PRD i18n). English is the product's source language; Portuguese
// and Spanish are translations of it, checked by type. The environment decides the language at
// boot (`app_locale` in the host, because the WebView has no environment) and a saved preference
// overrides it; nothing else reads `navigator.language`.
//
// Area dictionaries live in `src/i18n/areas/*.ts` and are discovered by `import.meta.glob`, so the
// specs that translate an area (068–071) add their own file and never edit a shared one. Keys
// carry the area prefix (`sidebar.newAgent`, `agent.status.working`, `error.<code>`).
//
// The current locale is `$state`: `t()` reads it on every call, so a mounted component re-renders
// when the language changes — no reload, no second source of truth.

/** The three languages the product carries; `en` is the source of every key. */
export type Locale = "en" | "pt" | "es";
/** What the user picked: a language, or `auto` (the environment decides). */
export type LocalePreference = "auto" | Locale;

export const LOCALES: readonly Locale[] = ["en", "pt", "es"];
export const DEFAULT_LOCALE: Locale = "en";
/** Where the preference is kept (P2 of the PRD); a missing/invalid value reads as `auto`. */
export const LOCALE_STORAGE_KEY = "herdr.locale";
/** `document.documentElement.lang` per locale: the tag the document announces. */
export const HTML_LANG: Record<Locale, string> = { en: "en", pt: "pt-BR", es: "es" };

/** Plural forms of one key; `other` is the only form every language has. */
export type PluralMessage = Partial<Record<Intl.LDMLPluralRule, string>> & { other: string };
export type Message = string | PluralMessage;
export type Dictionary = Record<string, Message>;
export type Params = Record<string, string | number>;

/**
 * A translation of `E`: every key of the English dictionary, with the same shape. A `pt`/`es`
 * missing a key of `en` is a type error, which is what makes `bun run check` the gate for the
 * completeness of a dictionary (AC-067-02).
 */
export type Translated<E extends Dictionary> = { [K in keyof E]: E[K] extends string ? string : PluralMessage };

export interface Area<E extends Dictionary> {
  readonly en: E;
  readonly pt: Translated<E>;
  readonly es: Translated<E>;
}

/** Declares one area's dictionaries; the default export of every `src/i18n/areas/*.ts`. */
export function area<E extends Dictionary>(dictionaries: Area<E>): Area<E> {
  return dictionaries;
}

type LoadedArea = { default: { en: Dictionary; pt: Dictionary; es: Dictionary } };

const AREAS = import.meta.glob<LoadedArea>("./areas/*.ts", { eager: true });

const DICTIONARIES: Record<Locale, Dictionary> = buildDictionaries();

function buildDictionaries(): Record<Locale, Dictionary> {
  const merged: Record<Locale, Dictionary> = { en: {}, pt: {}, es: {} };
  for (const path of Object.keys(AREAS).sort()) {
    const loaded = AREAS[path]!.default;
    for (const language of LOCALES) Object.assign(merged[language], loaded[language]);
  }
  return merged;
}

/** Every key of every area, by language. Read-only view for tests and the language selector. */
export function dictionaries(): Record<Locale, Dictionary> {
  return DICTIONARIES;
}

// --- current language ------------------------------------------------------------------------

let current = $state<Locale>(DEFAULT_LOCALE);
/** Language of the environment, kept so `auto` can be restored without another boot. */
let environment: Locale = DEFAULT_LOCALE;
let preference: LocalePreference = "auto";

/** The language in use. Reactive: a component that reads it re-renders when it changes. */
export function locale(): Locale {
  return current;
}

/** What the user picked (`auto` until they pick a language). */
export function localePreference(): LocalePreference {
  return preference;
}

/**
 * The language of an environment tag (`pt_BR.UTF-8`, `es-MX`, `C`, `""`): the primary subtag
 * decides, and anything the product does not carry is English (objective 1 of the PRD).
 */
export function resolveLocale(tag: string): Locale {
  const primary = tag.trim().toLowerCase().split(/[._@-]/)[0] ?? "";
  return (LOCALES as readonly string[]).includes(primary) ? (primary as Locale) : DEFAULT_LOCALE;
}

function isLocale(value: unknown): value is Locale {
  return typeof value === "string" && (LOCALES as readonly string[]).includes(value);
}

/** `localStorage` may be absent or throw (private mode, disabled storage, no document at all). */
function readStoredPreference(): LocalePreference | null {
  if (typeof window === "undefined") return null;
  try {
    const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
    if (stored === "auto") return "auto";
    return isLocale(stored) ? stored : null;
  } catch {
    return null;
  }
}

function writeStoredPreference(value: LocalePreference): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(LOCALE_STORAGE_KEY, value);
  } catch {
    // The preference is not kept across restarts; the session still follows the choice.
  }
}

function applyDocumentLang(language: Locale): void {
  if (typeof document !== "undefined") document.documentElement.lang = HTML_LANG[language];
}

function applyLocale(language: Locale): void {
  current = language;
  applyDocumentLang(language);
}

/** Records the user's choice (spec 069 shows the selector) and applies it at once. */
export function setLocalePreference(value: LocalePreference): void {
  preference = value;
  writeStoredPreference(value);
  applyLocale(value === "auto" ? environment : value);
}

/** The host's `app_locale`; dynamically imported so tests and the node project never load Tauri. */
async function appLocaleTag(): Promise<string> {
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<string>("app_locale");
}

/**
 * Boot: a saved `en|pt|es` wins; `auto` (or nothing saved) follows the environment the host
 * reports. A host that cannot answer is not fatal — the product falls back to English.
 */
export async function initLocale(readTag: () => Promise<string> = appLocaleTag): Promise<Locale> {
  let tag = "";
  try {
    tag = await readTag();
  } catch {
    tag = "";
  }
  environment = resolveLocale(tag);
  preference = readStoredPreference() ?? "auto";
  applyLocale(preference === "auto" ? environment : preference);
  return current;
}

// --- translation -----------------------------------------------------------------------------

const PLURAL_RULES: Partial<Record<Locale, Intl.PluralRules>> = {};

function pluralRules(language: Locale): Intl.PluralRules {
  return (PLURAL_RULES[language] ??= new Intl.PluralRules(HTML_LANG[language]));
}

/** The form of `forms` that `count` takes in `language` (`Intl.PluralRules`). */
export function plural(count: number, forms: PluralMessage, language: Locale = current): string {
  return forms[pluralRules(language).select(count)] ?? forms.other;
}

function interpolate(text: string, params?: Params): string {
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (match, name: string) => (name in params ? String(params[name]) : match));
}

/**
 * Pure lookup over explicit dictionaries: the language first, then English, then the key itself —
 * a missing translation must never render as an empty string.
 */
export function translate(dicts: Record<Locale, Dictionary>, language: Locale, key: string, params?: Params): string {
  const message = dicts[language][key] ?? dicts.en[key];
  if (message === undefined) return key;
  const text = typeof message === "string" ? message : plural(Number(params?.count ?? 0), message, language);
  return interpolate(text, params);
}

/** The translation of `key` in the current language. Every visible text of the product goes through it. */
export function t(key: string, params?: Params): string {
  return translate(DICTIONARIES, current, key, params);
}

function hasKey(key: string): boolean {
  return DICTIONARIES[current][key] !== undefined || DICTIONARIES.en[key] !== undefined;
}

// --- shared helpers ---------------------------------------------------------------------------

/**
 * Age of an observed moment, in the product's own words (the same thresholds and shape the agents
 * panel already shows). `ms` is how long ago it happened.
 */
export function formatRelative(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return t("time.now");
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return t("time.minutes", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("time.hours", { count: hours });
  return t("time.days", { count: Math.floor(hours / 24) });
}

/** Words of a connection phase; every phase has its own text, never a colour alone. */
export function phaseText(phase: "offline" | "connecting" | "online" | "reconnecting" | "attention"): string {
  return t(`phase.${phase}`);
}

/**
 * An engine error in the user's language when its `code` has a key (spec 071 adds them), and
 * otherwise the message the engine sent — never a fabricated generic text.
 */
export function errorText(error: { code: string; message: string }): string {
  const key = `error.${error.code}`;
  return error.code !== "" && hasKey(key) ? t(key) : error.message;
}
