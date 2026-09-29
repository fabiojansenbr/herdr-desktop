// Spec 067 (AC-067-04) — one setup file for both vitest projects. The suite is fixed on `pt`:
// the assertions written before this spec compare the Portuguese wording, so the tests must not
// follow the machine's language (`LANG=en_US.UTF-8` here). Each test that exercises another
// language sets it explicitly.
import { setLocalePreference } from "./i18n/index.svelte";

setLocalePreference("pt");
