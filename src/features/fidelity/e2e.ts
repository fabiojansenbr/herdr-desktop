// Native window scenario of spec 007 (driven by src-tauri/tests/fidelity_native.rs). Observes
// the composed App like a user and reports raw observations; the parent test computes the checks
// (tests/fidelity-native/plan.rs). Native keys/IME come from the parent on the private display,
// never from synthetic DOM events here. Phases not listed as implemented below throw, so a
// placeholder can never be reported as a result.

import { invoke } from "@tauri-apps/api/core";
import { byText, type, waitFor } from "../../harness/dom";
import type { Scenario } from "../../harness/native";
import { terminalProbe } from "../../terminal/probe";
import { domPage, runResourceBench } from "./resource-bench";
import { runMouseScrollLinks } from "./mouse-flow";
import { runPasteSelection, type AwaitParent } from "./paste-flow";
import { returnToLocal, runSshPhase } from "./ssh-flow";
import { domA11yPage, domResizePage, runA11yNavigation, runResizeDpi, windowDeps } from "./view-flow";
import { domVisualAgentsPage, runVisualAgents } from "./visual-agents";
import { domVisualFilesPage, runVisualFiles } from "./visual-files";
import { domVisualCenterPage, runVisualCenter } from "./visual-center";
import { domVisualFramePage, runVisualFrame } from "./visual-frame";
import { domVisualProjectsPage, runVisualProjects } from "./visual-projects";

// Must equal plan::FLOW, in order (asserted by fidelity_native.rs).
// phases:begin
export const PHASES = [
  "compose-mount",
  "lazy-editor",
  "hosts-identity",
  "ssh-agent-actions",
  "ssh-files-readonly",
  "legacy-server",
  "host-switch-dirty",
  "native-keys",
  "native-ime",
  "paste-selection",
  "mouse-scroll-links",
  "resize-dpi",
  "a11y-navigation",
  "visual-frame",
  "visual-agents",
  "visual-projects",
  "visual-files",
  "visual-center",
] as const;
// phases:end

/**
 * Scripts the page has fetched: resource timing entries (buffer enlarged at scenario start),
 * entries seen by a buffered PerformanceObserver, and script/modulepreload elements. Pathnames
 * only; the parent maps them through dist/.vite/module-chunks.json. Reads no editor code.
 */
const observed = new Set<string>();
export function observeResources(): void {
  performance.setResourceTimingBufferSize(4096);
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) observed.add(entry.name);
  }).observe({ type: "resource", buffered: true });
}

export function loadedScripts(): { at_ms: number; scripts: string[]; editor_dom: boolean } {
  const urls = new Set<string>(observed);
  for (const entry of performance.getEntriesByType("resource")) urls.add(entry.name);
  for (const s of Array.from(document.querySelectorAll<HTMLScriptElement>("script[src]"))) urls.add(s.src);
  for (const l of Array.from(document.querySelectorAll<HTMLLinkElement>('link[rel="modulepreload"]'))) urls.add(l.href);
  const scripts = Array.from(urls)
    .map((url) => new URL(url, location.href).pathname)
    .filter((path) => path.endsWith(".js"))
    .sort();
  return { at_ms: performance.now(), scripts, editor_dom: document.querySelector(".cm-editor") !== null };
}

/** Trusted input events seen by the composed terminal target (complementary evidence only). */
type EventRecord = { type: string; trusted: boolean; t: number; key?: string; data?: string | null; input?: string };
function recordTrustedEvents(target: HTMLElement): { take: () => EventRecord[] } {
  let events: EventRecord[] = [];
  const push = (e: Event) => {
    const k = e as KeyboardEvent & CompositionEvent & InputEvent;
    // Epoch ms, same clock as the parent's measurement windows.
    const t = performance.timeOrigin + performance.now();
    events.push({ type: e.type, trusted: e.isTrusted, t, key: k.key, data: k.data ?? undefined, input: k.inputType });
  };
  for (const type of ["keydown", "compositionstart", "compositionupdate", "compositionend", "beforeinput"]) {
    target.addEventListener(type, push, { capture: true });
  }
  return {
    take: () => {
      const out = events;
      events = [];
      return out;
    },
  };
}

/** Pointer steps carry the client viewport read at the step (the parent observes the window). */
export function withClientViewport(parent: AwaitParent, win: { innerWidth: number; innerHeight: number; devicePixelRatio: number; screenX: number; screenY: number }): AwaitParent {
  return (step, detail) => parent(step, { ...detail, client: { coordinate_space: "client", width: win.innerWidth, height: win.innerHeight, dpr: win.devicePixelRatio, screen_x: win.screenX, screen_y: win.screenY } });
}

function awaitParent(step: string, detail: Record<string, unknown>, timeoutMs = 110000): Promise<Record<string, unknown>> {
  return invoke<Record<string, unknown>>("harness_await", { step, detail, timeoutMs });
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function composeMount(root: HTMLElement): Promise<Record<string, unknown>> {
  const status = await waitFor("status bar", () => root.querySelector<HTMLElement>(".status-bar [data-phase]"));
  await waitFor("live phase", () => status.dataset.phase === "live", 60000);
  const canvas = root.querySelector<HTMLCanvasElement>("canvas");
  const items = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item"));
  return {
    app_mounted: root.querySelector(".status-bar") !== null,
    fake_bridge: false,
    status_phase: status.dataset.phase ?? "",
    status_text: status.innerText,
    session_text: items.map((i) => i.innerText).find((t) => t.startsWith("sessão")) ?? "",
    terminal_canvas: canvas !== null,
    terminal_visible: canvas !== null && canvas.getClientRects().length > 0,
  };
}

function field(form: HTMLElement, label: string): HTMLInputElement {
  const found = Array.from(form.querySelectorAll("label")).find((l) => l.querySelector("span")?.textContent?.trim() === label);
  const input = found?.querySelector("input");
  if (!input) throw new Error(`field ${label} not found in ${form.innerText}`);
  return input;
}

/** Opens the Local fixture project through the navigator and a file through the explorer. */
async function lazyEditor(root: HTMLElement, params: Record<string, unknown>): Promise<Record<string, unknown>> {
  await waitFor("live terminal", () => root.querySelector<HTMLElement>(".status-bar [data-phase=live]"), 60000);
  await waitFor("terminal canvas", () => root.querySelector("canvas"));
  const mounted = loadedScripts();
  await sleep(2100);
  const idle = loadedScripts();
  const project = params.project as { label: string; endpoint: string; session: string; root: string; file: string };
  const projects = await waitFor("projects activity", () => root.querySelector<HTMLButtonElement>('button[aria-label="Projetos e coleções"]'));
  if (projects.getAttribute("aria-pressed") !== "true") projects.click();
  root.querySelector<HTMLButtonElement>("[data-new-group]")?.click();
  const nameInput = await waitFor("new collection input", () => root.querySelector<HTMLInputElement>('input[placeholder="Nova coleção"]'));
  type(nameInput, "Fidelidade");
  (await waitFor("create collection", () => { const b = byText<HTMLButtonElement>(root, "button", "Criar coleção"); return b && !b.disabled ? b : null; })).click();
  (await waitFor("group menu", () => root.querySelector<HTMLButtonElement>("[data-group-menu]"))).click();
  (await waitFor("new project button", () => byText<HTMLButtonElement>(root, "button", "Novo projeto aqui"))).click();
  const form = await waitFor("project form", () => root.querySelector<HTMLElement>('form[aria-label="Novo projeto"]'));
  type(field(form, "Nome"), project.label);
  type(field(form, "Endpoint"), project.endpoint);
  type(field(form, "Sessão Herdr"), project.session);
  type(field(form, "Raiz do projeto"), project.root);
  (await waitFor("save project", () => byText<HTMLButtonElement>(form, "button", "Salvar projeto"))).click();
  const row = await waitFor("project row", () => root.querySelector<HTMLElement>(`[aria-label^="${project.label},"]`), 20000);
  const item = row.closest("li") ?? row;
  (await waitFor("open project", () => { const b = byText<HTMLButtonElement>(item, "button", "Abrir"); return b && !b.disabled ? b : null; })).click();
  const files = await waitFor("files activity", () => root.querySelector<HTMLButtonElement>('button[aria-label="Arquivos e diff"]'), 30000);
  files.click();
  const entry = await waitFor("fixture file in explorer", () => root.querySelector<HTMLButtonElement>(`button.entry[data-name="${project.file}"]`), 30000);
  entry.click();
  await waitFor("editor mounted", () => root.querySelector(".cm-editor"), 30000);
  await sleep(300);
  const opened = loadedScripts();
  return { probes: { mounted, idle, opened } };
}

async function focusTerminal(root: HTMLElement): Promise<HTMLTextAreaElement> {
  const filesShown = root.querySelector('section.layer[aria-label="Terminal"].hidden');
  if (filesShown) root.querySelector<HTMLButtonElement>('button[aria-label="Arquivos e diff"]')?.click();
  await waitFor("live terminal", () => root.querySelector<HTMLElement>(".status-bar [data-phase=live]"), 60000);
  const target = await waitFor("terminal input target", () => root.querySelector<HTMLTextAreaElement>('textarea.ime-target'));
  target.focus();
  await waitFor("terminal focused", () => document.activeElement === target);
  return target;
}

/** Native keys/IME: the parent types on the private display and measures the PTY bytes. */
async function nativeInput(root: HTMLElement, phase: string): Promise<Record<string, unknown>> {
  const target = await focusTerminal(root);
  // The pane the App confirmed (status diagnostics), so the parent measures that PTY.
  const status = await waitFor("confirmed identity", () => {
    const title = root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "";
    return /^pane (\S+) · geração (\S+) · boot (\S+)/.exec(title);
  });
  const items = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim());
  const endpoint = items[1] ?? "";
  // Precondition, never assumed from earlier phases: native input is measured on the Local host.
  if (!endpoint.endsWith(" · Local")) throw new Error(`precondition: ${phase} needs the Local host selected in the UI; status shows ${JSON.stringify(items)}`);
  const recorder = recordTrustedEvents(target);
  const parent = await awaitParent(phase, { pane_id: status[1], generation: status[2], boot_prefix: status[3], endpoint });
  const events = recorder.take();
  return {
    parent,
    pane_id: status[1],
    generation: status[2],
    boot_prefix: status[3],
    endpoint,
    focused_at_end: document.activeElement === target,
    events: events.length,
    trusted_events: events.filter((e) => e.trusted).length,
    untrusted_events: events.filter((e) => !e.trusted).length,
    compositionend: events.filter((e) => e.type === "compositionend").map((e) => e.data ?? ""),
    commit_events: events
      .filter((e) => e.type === "compositionend")
      .map((e) => ({ data: e.data ?? "", trusted: e.trusted, t: e.t })),
    event_log: events.map((e) => [e.type, e.key ?? "", e.data ?? "", e.input ?? ""].join("|")),
  };
}

async function runPhase(phase: string, root: HTMLElement, params: Record<string, unknown>): Promise<Record<string, unknown>> {
  switch (phase) {
    case "compose-mount":
      return composeMount(root);
    case "lazy-editor":
      return lazyEditor(root, params);
    case "hosts-identity":
    case "ssh-agent-actions":
    case "ssh-files-readonly":
    case "legacy-server":
    case "host-switch-dirty":
      return runSshPhase(phase, root, params, (step, detail) => awaitParent(step, detail));
    case "native-keys":
    case "native-ime":
      await returnToLocal(root);
      return nativeInput(root, phase);
    case "paste-selection":
      await returnToLocal(root);
      return runPasteSelection(root, awaitParent);
    case "mouse-scroll-links":
      await returnToLocal(root);
      return runMouseScrollLinks(root, withClientViewport(awaitParent, window));
    case "resize-dpi":
      await returnToLocal(root);
      return runResizeDpi(domResizePage(root), terminalProbe(), windowDeps(awaitParent));
    case "a11y-navigation":
      await returnToLocal(root);
      return runA11yNavigation(domA11yPage(root), windowDeps(awaitParent));
    case "visual-frame":
      await returnToLocal(root);
      return runVisualFrame(domVisualFramePage(root), awaitParent);
    case "visual-agents":
      await returnToLocal(root);
      return runVisualAgents(domVisualAgentsPage(root), awaitParent);
    case "visual-files":
      await returnToLocal(root);
      return runVisualFiles(domVisualFilesPage(root), awaitParent, params);
    case "visual-center": {
      await returnToLocal(root);
      const probe = terminalProbe();
      return runVisualCenter(domVisualCenterPage(root, probe!), probe, awaitParent);
    }
    case "visual-projects":
      await returnToLocal(root);
      return runVisualProjects(domVisualProjectsPage(root), awaitParent);
    default:
      throw new Error(`phase ${phase} is pending (see tests/fidelity-native/plan.rs)`);
  }
}

/**
 * The single window runs the phases the parent lists, in order, reporting each one through
 * `progress` (raw observations; `error` set when the phase threw). Phases share the window.
 */
export const run: Scenario = async ({ phase, params, root, progress }) => {
  // Resource bench (not a FLOW phase): its own parent, no resource observer adding page work.
  if (phase === "resource-bench") {
    // The hidden window of the output cells is this window taken off the screen (spec 009): the
    // harness owns the hide because the product never grants `hide` to the WebView.
    const setWindowVisible = (visible: boolean) => invoke<boolean>("harness_window_visible", { visible });
    return runResourceBench({ page: domPage(root, { setWindowVisible }), probe: terminalProbe(), awaitParent: (step, detail, timeoutMs) => awaitParent(step, detail, timeoutMs), sleep: async (ms) => { await sleep(ms); } }, params);
  }
  observeResources();
  if (phase !== "flow") throw new Error(`unexpected selection ${phase}; the parent runs phase "flow"`);
  const phases = (params.phases as string[]) ?? [];
  const completed: string[] = [];
  for (const name of phases) {
    if (!(PHASES as readonly string[]).includes(name)) throw new Error(`unknown phase ${name}`);
    try {
      const observations = await runPhase(name, root, params);
      await progress({ ...observations, phase: name, error: null });
      completed.push(name);
    } catch (error) {
      const message = error instanceof Error ? error.message : JSON.stringify(error);
      await progress({ phase: name, error: message, dom: root.innerText.slice(0, 3000) });
    }
  }
  return { completed };
};
