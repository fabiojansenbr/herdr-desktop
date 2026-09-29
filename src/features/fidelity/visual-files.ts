// Spec 015 — `visual-files` phase of the native flow (AC-015-01/02/03). The page observes the real
// composed window: it opens the Local project and its fixture file, edits the buffer through the
// mounted editor, opens the side-by-side diff, focuses the review's terminal dock (native keys and
// PTY measurement are the parent's, as in `native-keys`), then switches to the SSH host and reads
// two remote snapshots to compare them. The parent dictates the local buffer, writes both remote
// contents, measures the PTY and the engine; every check is computed in
// tests/fidelity-native/visual_files.rs from the parent's ledger, never from the page's report.
import { byText, waitFor } from "../../harness/dom";
import { terminalProbe } from "../../terminal/probe";
import { parseIdentity, returnToLocal, type AwaitParent } from "./ssh-flow";
import { TARGET_VIEWPORT } from "./visual-frame";

export const VISUAL_FILES_STEPS = [
  "visual-files-viewport",
  "visual-files-local",
  "visual-files-engine",
  "visual-files-keys",
  "visual-files-frames",
  "visual-files-remote-before",
  "visual-files-remote-change",
  "visual-files-evidence",
  "visual-files-restore",
] as const;

export const RESTORE_MODE = { width: 1280, height: 720 } as const;
/** Local fixture file the phase opens in the review (created by the parent before the flow). */
export const FIXTURE_FILE = "fidelity_fixture.rs";
export const REMOTE_FILE = "notas.txt";

export type Identity = { pane_id: string; generation: string; boot_prefix: string; endpoint: string };
export type Viewport = { inner_width: number; inner_height: number; dpr: number };
export type SmallText = { scope: string; text: string; color: string; background: string; opacity: number; font_size: number; font_weight: number; visibility: string };
export type DiffCell = {
  row: number;
  side: "base" | "current";
  op: string;
  line: number;
  text: string;
  mark: string;
  top: number;
  height: number;
  background: string;
  color: string;
};
export type DiffSeen = {
  base: string | null;
  sources: string;
  summary: string;
  headings: { base: string; current: string };
  cells: DiffCell[];
};
export type Probe = { at_ms: number; scripts: string[]; editor_dom: boolean };

export interface VisualFilesPage {
  identity(): Identity | null;
  viewport(): Viewport;
  waitViewport(width: number, height: number, timeoutMs: number): Promise<boolean>;
  /** Local project opened, files layer shown, review mounted with no open tab (empty state). */
  openLocalReview(project: { label: string; root: string }): Promise<Record<string, unknown>>;
  /** `+ Abrir arquivo` pressed; the focused element observed. */
  requestOpenFile(): Promise<string>;
  probe(): Probe;
  openFixtureFile(name: string): Promise<void>;
  editorText(): Promise<string>;
  applyBuffer(text: string): Promise<string>;
  toggleDiff(): Promise<void>;
  review(): Promise<Record<string, unknown>>;
  /** Focuses the dock's terminal; true when it took the focus. */
  focusDock(): boolean;
  probeCounters(): unknown;
  closeDock(): Promise<Record<string, unknown>>;
  /**
   * SSH host selected through HostsPanel, its project opened in the review and the remote tabs
   * closed, so the phase's own reads (before/after the parent's writes) are the two snapshots.
   */
  openRemoteReview(ssh: { label: string; boot_prefix: string }): Promise<Record<string, unknown>>;
  openRemoteFile(name: string): Promise<void>;
  typeRemote(text: string): Promise<Record<string, unknown>>;
  reloadRemote(): Promise<string>;
  compareRemote(): Promise<void>;
  remoteReview(): Promise<Record<string, unknown>>;
}

export async function runVisualFiles(page: VisualFilesPage, awaitParent: AwaitParent, params: Record<string, unknown>): Promise<Record<string, unknown>> {
  const project = params.project as { label: string; root: string; file: string };
  const sshFlow = params.ssh_flow as { ssh: { label: string; root: string } } | undefined;
  const identity = page.identity();
  if (!identity || !identity.endpoint.endsWith(" · Local")) throw new Error(`precondition: visual-files needs the confirmed Local identity; got ${JSON.stringify(identity)}`);
  const parent: Record<string, Record<string, unknown>> = {};
  const step = async (name: (typeof VISUAL_FILES_STEPS)[number], detail: Record<string, unknown> = {}) => {
    parent[name] = await awaitParent(name, { ...identity, ...detail });
    return parent[name]!;
  };

  const before = page.viewport();
  await step("visual-files-viewport", { ...before, target_width: TARGET_VIEWPORT.width, target_height: TARGET_VIEWPORT.height });
  if (!(await page.waitViewport(TARGET_VIEWPORT.width, TARGET_VIEWPORT.height, 15000))) {
    throw new Error(`viewport never reached 1440×900 CSS px: ${JSON.stringify(page.viewport())}`);
  }

  // --- local review: empty state, file opened on demand, edited buffer, side-by-side diff -------
  const mount = await page.openLocalReview(project);
  const local = await step("visual-files-local", { root: project.root, file: project.file });
  const openFocus = await page.requestOpenFile();
  await page.openFixtureFile(project.file);
  const openedProbe = page.probe();
  const editorText = await page.editorText();
  const buffer = String(local["buffer"] ?? "");
  const applied = await page.applyBuffer(buffer);
  await page.toggleDiff();
  const review = await page.review();
  const diffProbe = page.probe();
  const engine = await step("visual-files-engine", { root: project.root, file: project.file });
  const confirmed = page.identity();

  // --- dock: label, native keys at the PTY, frames painted while the review is open -------------
  const dockFocused = page.focusDock();
  const keys = await step("visual-files-keys", { pane_id: confirmed?.pane_id ?? "", focused: dockFocused });
  const framesBefore = page.probeCounters();
  await step("visual-files-frames", { pane_id: confirmed?.pane_id ?? "" });
  const framesAfter = page.probeCounters();

  // --- remote review: banner, read-only file, two snapshots compared side by side ---------------
  if (!sshFlow?.ssh) throw new Error("precondition: visual-files needs the SSH fixture params");
  const sshLabel = String((params.ssh_flow as { ssh_profile?: { label?: string } }).ssh_profile?.label ?? "");
  if (!sshLabel) throw new Error("precondition: visual-files needs the SSH profile label");
  await page.openRemoteReview({ label: sshLabel, boot_prefix: identity.boot_prefix });
  await step("visual-files-remote-before", {});
  await page.openRemoteFile(REMOTE_FILE);
  const remoteReadOnly = await page.typeRemote("DIGITADO-REMOTO-015 ");
  const remoteChange = await step("visual-files-remote-change", {});
  const reloaded_label = await page.reloadRemote();
  await page.compareRemote();
  const remote = await page.remoteReview();

  // --- dock close: the diff gets the dock's height back ----------------------------------------
  const closed = await page.closeDock();
  await step("visual-files-evidence", {});
  await step("visual-files-restore");

  return {
    identity,
    viewport_before: before,
    viewport: page.viewport(),
    mount,
    open_focus: openFocus,
    editor_text: editorText,
    buffer_applied: applied,
    probes: { mount: mount["probe"], opened: openedProbe, diff: diffProbe },
    review,
    engine,
    dock: { focused: dockFocused, frames: { before: framesBefore, after: framesAfter } },
    keys,
    remote: { read_only: remoteReadOnly, reloaded_label, change: remoteChange, ...remote },
    closed,
    parent,
  };
}

// ------------------------------------------------------------------------------------ DOM page

type EditorViewLike = { state: { doc: { toString(): string; length: number } }; dispatch: (spec: unknown) => void };

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const press = (button: HTMLButtonElement | null) => {
  if (!button) throw new Error("button not found");
  if (button.disabled) throw new Error(`button "${button.textContent?.trim()}" is disabled`);
  button.click();
};

function rect(el: Element | null): { left: number; top: number; width: number; height: number } | null {
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { left: r.left, top: r.top, width: r.width, height: r.height };
}

/** Visible elements carrying their own text in `scope`, with the colors needed for contrast. */
function smallTexts(scope: string, root: Element | null): SmallText[] {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>("*"))
    .filter((el) => Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? "").trim() !== "") && el.getClientRects().length > 0)
    .map((el) => {
      const style = getComputedStyle(el);
      return {
        scope,
        text: (el.innerText || el.textContent || "").trim().slice(0, 60),
        color: style.color,
        background: effectiveBackground(el),
        opacity: opacityOf(el),
        font_size: parseFloat(style.fontSize),
        font_weight: parseInt(style.fontWeight, 10) || 400,
        visibility: style.visibility,
      };
    });
}

function parseColor(css: string): { r: number; g: number; b: number; a: number } | null {
  const text = css.trim().toLowerCase();
  if (text.startsWith("#")) {
    const hex = text.slice(1);
    const byte = (i: number) => parseInt(hex.slice(i, i + 2), 16);
    if (hex.length !== 6 && hex.length !== 8) return null;
    return { r: byte(0), g: byte(2), b: byte(4), a: hex.length === 8 ? byte(6) / 255 : 1 };
  }
  const inner = text.startsWith("rgba(") ? text.slice(5, -1) : text.startsWith("rgb(") ? text.slice(4, -1) : null;
  if (inner === null) return null;
  const parts = inner
    .split(/[,/\s]+/)
    .filter(Boolean)
    .map((part) => (part.endsWith("%") ? parseFloat(part) / 100 : parseFloat(part)));
  if (parts.length === 3 && parts.every((v) => !Number.isNaN(v))) return { r: parts[0]!, g: parts[1]!, b: parts[2]!, a: 1 };
  if (parts.length === 4 && parts.every((v) => !Number.isNaN(v))) return { r: parts[0]!, g: parts[1]!, b: parts[2]!, a: parts[3]! };
  return null;
}

/** Background actually under `el`: own and ancestors' backgrounds composited over the window bg. */
function effectiveBackground(el: Element): string {
  const layers: { r: number; g: number; b: number; a: number }[] = [];
  for (let node: Element | null = el; node; node = node.parentElement) {
    const color = parseColor(getComputedStyle(node).backgroundColor);
    if (color && color.a > 0) layers.push(color);
    if (color && color.a >= 1) break;
  }
  const under = layers.reverse().reduce<{ r: number; g: number; b: number; a: number }>((base, layer) => {
    const a = layer.a + base.a * (1 - layer.a);
    return { r: (layer.r * layer.a + base.r * base.a * (1 - layer.a)) / a, g: (layer.g * layer.a + base.g * base.a * (1 - layer.a)) / a, b: (layer.b * layer.a + base.b * base.a * (1 - layer.a)) / a, a };
  }, { r: 11, g: 12, b: 16, a: 1 });
  return `rgba(${Math.round(under.r)}, ${Math.round(under.g)}, ${Math.round(under.b)}, 1)`;
}

function opacityOf(el: Element): number {
  let product = 1;
  for (let node: Element | null = el; node; node = node.parentElement) product *= parseFloat(getComputedStyle(node).opacity) || 0;
  return product;
}

function loadedScripts(): Probe {
  const urls = new Set<string>();
  for (const entry of performance.getEntriesByType("resource")) urls.add(entry.name);
  for (const script of Array.from(document.querySelectorAll<HTMLScriptElement>("script[src]"))) urls.add(script.src);
  for (const link of Array.from(document.querySelectorAll<HTMLLinkElement>('link[rel="modulepreload"]'))) urls.add(link.href);
  const scripts = Array.from(urls)
    .map((url) => new URL(url, location.href).pathname)
    .filter((path) => path.endsWith(".js"))
    .sort();
  return { at_ms: Math.round(performance.now()), scripts, editor_dom: document.querySelector(".cm-editor") !== null };
}

function diffSnapshot(scope: Element | null): DiffSeen {
  const root = scope?.querySelector<HTMLElement>("[data-diff]") ?? null;
  const cells = Array.from(root?.querySelectorAll<HTMLElement>("[data-diff] [data-op][data-side]") ?? []).map((cell) => {
    const style = getComputedStyle(cell);
    return {
      row: parseInt(cell.dataset.row ?? "0", 10),
      side: (cell.dataset.side ?? "base") as "base" | "current",
      op: cell.dataset.op ?? "",
      line: parseInt(cell.dataset.line ?? "0", 10),
      text: cell.querySelector("code")?.textContent ?? "",
      mark: cell.querySelector(".mark")?.textContent ?? "",
      top: Math.round(cell.getBoundingClientRect().top),
      height: Math.round(cell.getBoundingClientRect().height),
      background: style.backgroundColor,
      color: style.color,
    };
  });
  return {
    base: root?.dataset.diffBase ?? null,
    sources: root?.querySelector("[data-diff-sources]")?.textContent?.trim() ?? "",
    summary: root?.querySelector("[data-diff-summary]")?.textContent?.trim() ?? "",
    headings: {
      base: root?.querySelector('[data-diff-heading="base"]')?.textContent?.trim() ?? "",
      current: root?.querySelector('[data-diff-heading="current"]')?.textContent?.trim() ?? "",
    },
    cells,
  };
}

const cleanLabel = (text: string | null | undefined) => (text ?? "").replace(/•/g, "").replace(/\s+/g, " ").trim();

function tabSnapshot(scope: ParentNode): { kind: string; label: string; active: boolean }[] {
  return Array.from(scope.querySelectorAll<HTMLElement>("[data-review-tab]")).map((tab) => ({
    kind: tab.dataset.reviewTab ?? "",
    label: cleanLabel(tab.querySelector(".tab-name")?.textContent),
    active: tab.classList.contains("active"),
  }));
}

async function waitForAsync<T>(what: string, probe: () => Promise<T | null | undefined | false>, timeoutMs = 20000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await probe();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}; page: ${document.body.innerText.slice(0, 1500)}`);
    await sleep(50);
  }
}

async function editorView(root: ParentNode): Promise<EditorViewLike | null> {
  const element = root.querySelector<HTMLElement>(".cm-editor");
  if (!element) return null;
  const { EditorView } = await import("@codemirror/view");
  return (EditorView.findFromDOM(element) as unknown as EditorViewLike | null) ?? null;
}

async function openProjectRow(root: HTMLElement, label: string): Promise<void> {
  const rowOf = () => root.querySelector<HTMLElement>(`[aria-label^="${label},"]`)?.closest<HTMLElement>("li") ?? null;
  const row = await waitFor(`project ${label}`, rowOf, 20000);
  const status = () => row.querySelector<HTMLElement>(".status");
  let opening = false;
  const observer = new MutationObserver(() => {
    if (status()?.dataset.status === "opening") opening = true;
  });
  observer.observe(row, { attributes: true, subtree: true, childList: true });
  try {
    press(await waitFor("open project", () => { const b = byText<HTMLButtonElement>(row, "button", "Abrir"); return b && !b.disabled ? b : null; }));
    await waitFor(`project ${label} open finished`, () => (opening || status()?.dataset.status === "opening") && (opening = true) && status()?.dataset.status !== "opening", 90000);
  } finally {
    observer.disconnect();
  }
}

const filesLayerShown = (root: HTMLElement) => {
  const layer = root.querySelector<HTMLElement>('section.layer[aria-label="Arquivos"]');
  return layer !== null && !layer.classList.contains("hidden");
};

async function showFiles(root: HTMLElement): Promise<void> {
  if (filesLayerShown(root)) return;
  press(await waitFor("files toggle", () => root.querySelector<HTMLButtonElement>('button[aria-label="Arquivos e diff"]'), 30000));
  await waitFor("files layer shown", () => filesLayerShown(root), 30000);
}

/** Closes every open tab of `workspace`, discarding dirty buffers through the real prompt. */
async function closeAllTabs(workspace: HTMLElement): Promise<void> {
  for (let guard = 0; guard < 12; guard++) {
    const close = workspace.querySelector<HTMLButtonElement>("[data-review-tab] .tab-close");
    if (!close) return;
    close.click();
    const prompt = await waitFor("close prompt or closed tab", () => workspace.querySelector<HTMLElement>("[data-close-prompt]") ?? (workspace.querySelector("[data-review-tab] .tab-close") !== close ? "closed" : null), 10000).catch(() => null);
    if (prompt instanceof HTMLElement) {
      press(byText<HTMLButtonElement>(workspace, "button", "Descartar"));
    }
    await sleep(100);
  }
}

export function domVisualFilesPage(root: HTMLElement): VisualFilesPage {
  const local = () => root.querySelector<HTMLElement>("[data-files-workspace]");
  const remote = () => root.querySelector<HTMLElement>("[data-remote-files-workspace]");
  const dock = () => root.querySelector<HTMLElement>("[data-terminal-dock]");
  return {
    identity: () => {
      const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
      const endpoint = Array.from(root.querySelectorAll<HTMLElement>(".status-bar .status-item")).map((i) => i.innerText.trim())[1] ?? "";
      return id && endpoint ? { ...id, endpoint } : null;
    },
    viewport: () => ({ inner_width: window.innerWidth, inner_height: window.innerHeight, dpr: window.devicePixelRatio }),
    waitViewport: async (width, height, timeoutMs) => {
      const deadline = performance.now() + timeoutMs;
      while (performance.now() < deadline) {
        if (window.innerWidth === width && window.innerHeight === height) return true;
        await sleep(50);
      }
      return false;
    },
    openLocalReview: async (project) => {
      await returnToLocal(root);
      await openProjectRow(root, project.label);
      await showFiles(root);
      // The dock names the session's agent; the composed panel is the same engine list.
      const toggle = root.querySelector<HTMLButtonElement>('button[aria-label="Painel de agentes"]');
      if (toggle && toggle.getAttribute("aria-pressed") !== "true") toggle.click();
      await waitFor("an engine agent in the agents panel", () => root.querySelector('[data-slot="agents"] li[data-agent-pane]'), 60000);
      const workspace = await waitFor("local review screen", () => {
        const found = local();
        return found && found.querySelector("[data-review-header]") ? found : null;
      }, 30000);
      await closeAllTabs(workspace);
      await waitFor("review empty state", () => workspace.querySelector("[data-editor-empty]"), 10000);
      return {
        tabs: tabSnapshot(workspace),
        empty_text: workspace.querySelector<HTMLElement>("[data-editor-empty]")?.innerText.trim() ?? "",
        open_file_label: workspace.querySelector<HTMLElement>("[data-open-file]")?.innerText.trim() ?? "",
        probe: loadedScripts(),
      };
    },
    requestOpenFile: async () => {
      const workspace = await waitFor("local review screen", () => local(), 10000);
      press(await waitFor("open file button", () => workspace.querySelector<HTMLButtonElement>("[data-open-file]")));
      await waitFor("explorer entry focused", () => (document.activeElement as HTMLElement | null)?.matches("button.entry[data-name]") === true, 5000);
      return (document.activeElement as HTMLElement).dataset.name ?? "";
    },
    probe: () => loadedScripts(),
    openFixtureFile: async (name) => {
      const workspace = await waitFor("local review screen", () => local(), 10000);
      press(await waitFor("fixture entry", () => workspace.querySelector<HTMLButtonElement>(`button.entry[data-name="${name}"]`), 20000));
      await waitFor("fixture tab", () => {
        const tab = Array.from(workspace.querySelectorAll<HTMLElement>(".tab-name")).find((t) => t.textContent?.trim() === name);
        return tab && workspace.querySelector(".cm-editor") ? tab : null;
      }, 30000);
    },
    editorText: async () => (await editorView(root))?.state.doc.toString() ?? "",
    applyBuffer: async (text) => {
      const view = await waitForAsync("editor view", () => editorView(root));
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
      await waitFor("edited buffer", () => ((view.state.doc.toString() === text) ? view : null), 10000);
      return view.state.doc.toString();
    },
    toggleDiff: async () => {
      const workspace = await waitFor("local review screen", () => local(), 10000);
      press(await waitFor("diff button", () => workspace.querySelector<HTMLButtonElement>("[data-diff-original]")));
      await waitFor("side-by-side diff", () => {
        const seen = diffSnapshot(workspace);
        return seen.headings.base === "Snapshot anterior" && seen.cells.some((cell) => cell.op === "removed") && seen.cells.some((cell) => cell.op === "added") ? seen : null;
      }, 15000);
    },
    review: async () => {
      const workspace = await waitFor("local review screen", () => local(), 10000);
      const seen = workspace ? diffSnapshot(workspace) : null;
      return {
        title: workspace?.querySelector("[data-review-title]")?.textContent?.trim() ?? "",
        breadcrumb: workspace?.querySelector("[data-review-breadcrumb]")?.textContent?.replace(/\s+/g, " ").trim() ?? "",
        tabs: tabSnapshot(workspace ?? root),
        ordered: Array.from(workspace?.querySelectorAll("[data-review-tab] .tab-name, [data-open-file]") ?? []).map((el) => cleanLabel(el.textContent)),
        diff: seen,
        editor_dom: workspace?.querySelector(".cm-editor") !== null,
        dock: dock() ? { rect: rect(dock()), label: dock()!.querySelector("[data-dock-label]")?.textContent?.trim() ?? "" } : null,
        small_texts: [...smallTexts("review", workspace), ...smallTexts("dock", dock())],
      };
    },
    focusDock: () => {
      const target = root.querySelector<HTMLTextAreaElement>("[data-terminal-dock] textarea.ime-target");
      target?.focus();
      return !!target && document.activeElement === target;
    },
    probeCounters: () => terminalProbe()?.snapshot() ?? null,
    closeDock: async () => {
      await waitFor("review screen with a diff", () => root.querySelector("[data-diff]"), 10000);
      // "Fechar o dock devolve a altura ao diff": the review body is what the diff area gains.
      const diffHeight = () => Number(root.querySelector<HTMLElement>("[data-review-body]")?.clientHeight ?? 0);
      const box = dock()?.getBoundingClientRect();
      const style = dock() ? getComputedStyle(dock()!) : null;
      const band = Number(box?.height ?? 0) + Number.parseFloat(style?.marginTop ?? "0") + Number.parseFloat(style?.marginBottom ?? "0");
      const before = diffHeight();
      press(await waitFor("dock close", () => root.querySelector<HTMLButtonElement>("[data-terminal-dock] [data-dock-close]")));
      await waitFor("dock closed", () => dock() === null, 10000);
      await sleep(200);
      return {
        present: dock() !== null,
        dock_height: Math.round(Number(box?.height ?? 0)),
        dock_band: Math.round(band),
        diff_before: Math.round(before),
        diff_after: Math.round(diffHeight()),
        workspace: root.querySelector("[data-review-workspace], [data-remote-review-workspace]") !== null,
      };
    },
    openRemoteReview: async (ssh) => {
      const activity = await waitFor("connections activity", () => root.querySelector<HTMLButtonElement>('button[aria-label="Conexões remotas"]'), 30000);
      if (activity.getAttribute("aria-pressed") !== "true") activity.click();
      const find = () => Array.from(root.querySelectorAll<HTMLElement>("li.host[data-host]")).find((li) => li.querySelector("strong")?.textContent?.trim() === ssh.label) ?? null;
      const row = await waitFor(`host ${ssh.label}`, find, 30000);
      if (!row.classList.contains("selected")) {
        press(await waitFor("select ssh host", () => { const b = byText<HTMLButtonElement>(row, "button", "Usar este host"); return b && !b.disabled ? b : null; }, 30000));
      }
      await waitFor(`host ${ssh.label} selected`, () => find()?.classList.contains("selected") === true, 30000);
      const host = await waitFor("ssh host selected", () => root.querySelector<HTMLElement>("li.host.selected"), 30000);
      const endpoint = host.dataset.host ?? "";
      await waitFor("ssh live identity", () => {
        const id = parseIdentity(root.querySelector<HTMLElement>(".status-bar [data-phase=live]")?.title ?? "");
        return id && id.boot_prefix !== ssh.boot_prefix ? id : null;
      }, 60000);
      // The navigator replaces the HostsPanel in the sidebar; the project row lives there.
      const projects = await waitFor("projects activity", () => root.querySelector<HTMLButtonElement>('button[aria-label="Projetos e coleções"]'), 30000);
      if (projects.getAttribute("aria-pressed") !== "true") projects.click();
      await openProjectRow(root, "Fidelidade SSH");
      await showFiles(root);
      const workspace = await waitFor("remote review screen", () => {
        const found = remote();
        return found && found.querySelector("[data-review-header]") ? found : null;
      }, 30000);
      await closeAllTabs(workspace);
      return { endpoint, tab: tabSnapshot(workspace) };
    },
    openRemoteFile: async (name) => {
      const workspace = await waitFor("remote review screen", () => remote(), 10000);
      const entry = () => workspace.querySelector<HTMLButtonElement>(`button.remote-entry[data-name="${name}"]`);
      try {
        await waitFor("remote tree", entry, 5000);
      } catch {
        press(await waitFor("Atualizar enabled", () => { const b = workspace.querySelector<HTMLButtonElement>("[data-remote-refresh]"); return b && !b.disabled ? b : null; }, 30000));
        await waitFor("remote tree after Atualizar", entry, 30000);
      }
      press(entry());
      await waitFor("remote file read", () => {
        const document_ = workspace.querySelector<HTMLElement>("[data-remote-document]");
        return document_?.querySelector("[data-snapshot-label]") && workspace.querySelector(".cm-editor") ? document_ : null;
      }, 30000);
      await waitFor("remote content", () => (workspace.querySelector<HTMLElement>(".cm-content")?.innerText ?? "").trim() !== "", 20000);
    },
    typeRemote: async (text) => {
      const workspace = await waitFor("remote review screen", () => remote(), 10000);
      const content = workspace.querySelector<HTMLElement>("[data-remote-document] .cm-content");
      const before = workspace.querySelector<HTMLElement>(".cm-content")?.innerText ?? "";
      content?.focus();
      document.execCommand("insertText", false, text);
      await sleep(300);
      const after = workspace.querySelector<HTMLElement>(".cm-content")?.innerText ?? "";
      return {
        contenteditable: content?.getAttribute("contenteditable") ?? "",
        text_before: before,
        text_after: after,
        typed_ignored: before === after && !after.includes(text),
        save_buttons: Array.from(workspace.querySelectorAll("button")).filter((b) => b.textContent?.trim() === "Salvar").length,
      };
    },
    reloadRemote: async () => {
      const workspace = await waitFor("remote review screen", () => remote(), 10000);
      const label = () => workspace.querySelector<HTMLElement>("[data-remote-document] [data-snapshot-label]")?.textContent?.trim() ?? "";
      const previous = label();
      press(await waitFor("reload", () => { const b = workspace.querySelector<HTMLButtonElement>("[data-remote-document] [data-reload]"); return b && !b.disabled ? b : null; }, 30000));
      return waitFor("new remote snapshot", () => { const next = label(); return next && next !== previous ? next : null; }, 30000);
    },
    compareRemote: async () => {
      const workspace = await waitFor("remote review screen", () => remote(), 10000);
      press(await waitFor("compare", () => { const b = workspace.querySelector<HTMLButtonElement>("[data-remote-document] [data-compare]"); return b && !b.disabled ? b : null; }, 30000));
      await waitFor("remote side-by-side diff", () => {
        const seen = diffSnapshot(workspace);
        return seen.headings.current === "Versão atual" && seen.cells.some((cell) => cell.op === "added") ? seen : null;
      }, 15000);
    },
    remoteReview: async () => {
      const workspace = await waitFor("remote review screen", () => remote(), 10000);
      const banner = workspace?.querySelector<HTMLElement>("[data-review-banner]");
      return {
        banner: banner
          ? { text: banner.innerText.trim().replace(/\s+/g, " "), background: getComputedStyle(banner).backgroundColor, color: getComputedStyle(banner).color }
          : null,
        breadcrumb: workspace?.querySelector("[data-review-breadcrumb]")?.textContent?.replace(/\s+/g, " ").trim() ?? "",
        tabs: tabSnapshot(workspace ?? root),
        ordered: Array.from(workspace?.querySelectorAll("[data-review-tab] .tab-name, [data-open-file]") ?? []).map((el) => cleanLabel(el.textContent)),
        diff: workspace ? diffSnapshot(workspace) : null,
        editor_dom: workspace?.querySelector(".cm-editor") !== null,
        small_texts: smallTexts("remote-review", workspace),
      };
    },
  };
}
