// Native window scenario of spec 005 (driven by src-tauri/tests/files_local.rs through
// scripts/feature-harness/window.rs). Acts on the real files workspace — paging the explorer,
// opening tabs, editing through the mounted CodeMirror view and choosing in the conflict —
// over the real IPC bridge and provider. Phases: create (list/edit/save/limits/close) and
// conflict (dirty buffer + external change → conflict → compare → save copy → reload).

import { waitFor } from "../../harness/dom";
import type { Scenario } from "../../harness/native";

type View = { state: { doc: { toString(): string; length: number } }; dispatch: (spec: unknown) => void };

async function editorView(root: ParentNode): Promise<View | null> {
  const element = root.querySelector<HTMLElement>(".cm-editor");
  if (!element) return null;
  const { EditorView } = await import("@codemirror/view");
  return (EditorView.findFromDOM(element) as unknown as View | null) ?? null;
}

async function editorText(root: ParentNode): Promise<string> {
  const view = await editorView(root);
  return view ? view.state.doc.toString() : "";
}

async function waitForAsync<T>(
  what: string,
  probe: () => Promise<T | null | undefined | false>,
  timeoutMs = 20000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await probe();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(
        `timed out waiting for ${what}; page: ${document.body.innerText.slice(0, 1500)}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

function entry(root: ParentNode, name: string): HTMLButtonElement | null {
  return (
    Array.from(root.querySelectorAll<HTMLButtonElement>("button.entry")).find(
      (button) => button.dataset.name === name,
    ) ?? null
  );
}

function tab(root: ParentNode, name: string): HTMLElement | null {
  return (
    Array.from(root.querySelectorAll<HTMLElement>(".tab")).find(
      (element) => element.querySelector(".tab-name")?.textContent?.trim().replace(/•/g, "").trim() === name,
    ) ?? null
  );
}

function tabNames(root: ParentNode): string[] {
  return Array.from(root.querySelectorAll<HTMLElement>(".tab .tab-name")).map(
    (element) => element.textContent?.trim().replace(/•/g, "").trim() ?? "",
  );
}

function buttonByText(scope: ParentNode, text: string): HTMLButtonElement | null {
  return (
    Array.from(scope.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent?.trim() === text,
    ) ?? null
  );
}

function press(button: HTMLButtonElement | null): void {
  if (!button) throw new Error("button not found");
  if (button.disabled) throw new Error(`button "${button.textContent?.trim()}" is disabled`);
  button.click();
}

async function appendText(root: ParentNode, text: string): Promise<void> {
  const view = await waitForAsync("editor view", () => editorView(root));
  view.dispatch({ changes: { from: view.state.doc.length, insert: text } });
}

async function replaceText(root: ParentNode, text: string): Promise<void> {
  const view = await waitForAsync("editor view", () => editorView(root));
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
}

/**
 * Scripts the page has actually fetched: every resource timing entry (entry script, dynamic
 * `import()` chunks, module preloads) plus the script/modulepreload elements in the document.
 * Only pathnames are reported; the parent maps them to source modules through the build's
 * module metadata (dist/.vite/module-chunks.json) and counts editor/language modules. This
 * reads no editor code, so taking a probe never loads the editor by itself.
 */
function loadedScripts(): { at_ms: number; scripts: string[]; tabs: number; editor_dom: boolean } {
  const urls = new Set<string>();
  for (const entry of performance.getEntriesByType("resource")) urls.add(entry.name);
  for (const script of Array.from(document.querySelectorAll<HTMLScriptElement>("script[src]"))) {
    urls.add(script.src);
  }
  for (const link of Array.from(document.querySelectorAll<HTMLLinkElement>('link[rel="modulepreload"]'))) {
    urls.add(link.href);
  }
  const scripts = Array.from(urls)
    .map((url) => new URL(url, location.href).pathname)
    .filter((path) => path.endsWith(".js"))
    .sort();
  return {
    at_ms: Math.round(performance.now()),
    scripts,
    tabs: document.querySelectorAll(".tab").length,
    editor_dom: document.querySelector(".cm-editor") !== null,
  };
}

function editorDomText(root: ParentNode): string {
  return root.querySelector<HTMLElement>(".cm-editor .cm-content")?.innerText ?? "";
}

async function waitDirty(root: ParentNode, name: string, dirty: boolean): Promise<void> {
  await waitFor(`${name} dirty=${dirty}`, () => {
    const element = tab(root, name);
    return element && element.dataset.dirty === String(dirty) ? element : null;
  });
}

export const run: Scenario = async ({ phase, root, progress }) => {
  await waitFor("files workspace", () => root.querySelector("[data-files-workspace]"));

  if (phase === "create") {
    performance.setResourceTimingBufferSize(4096);
    // Mount stable (first page listed, nothing open) → at least 2000 ms idle → open a file.
    // The same probe runs at the three moments; the parent asserts zero editor/language
    // modules before opening and a positive detection after.
    await waitFor("root listing", () => root.querySelector("button.entry"));
    const probe_mounted = loadedScripts();
    await new Promise((resolve) => setTimeout(resolve, 2100));
    const probe_idle = loadedScripts();
    const editor_absent_before_open = root.querySelector(".cm-editor") === null;
    const empty_state_text = root.querySelector("[data-editor-empty]")?.textContent?.trim() ?? "";
    const initial_rows = root.querySelectorAll("button.entry").length;
    const load_more_visible = root.querySelector("[data-load-more]") !== null;

    press(root.querySelector<HTMLButtonElement>("[data-load-more]"));
    await waitFor("second page", () => {
      if (root.querySelector("[data-load-more]")) return null;
      const rows = root.querySelectorAll("button.entry").length;
      return rows > initial_rows ? rows : null;
    });
    const rows_after_more = root.querySelectorAll("button.entry").length;
    const load_more_gone = root.querySelector("[data-load-more]") === null;

    press(entry(root, "sub"));
    await waitFor("subdirectory page", () => entry(root, "um.txt"));
    const subdir_expanded = entry(root, "sub")?.getAttribute("aria-expanded") === "true";

    press(entry(root, "notas.txt"));
    await waitFor("notas tab", () => tab(root, "notas.txt"));
    // Positive probe before this scenario imports anything from CodeMirror itself: whatever
    // editor module is loaded at this point was loaded by the files feature opening the file.
    await waitFor("notas content in the editor DOM", () => editorDomText(root).includes("linha um"));
    const probe_opened = loadedScripts();
    await waitForAsync("notas content", async () =>
      (await editorText(root)).includes("linha um") ? true : null,
    );
    const tab_names = tabNames(root);
    const editor_loaded_after_open = root.querySelector(".cm-editor") !== null;

    await appendText(root, "\nOBS usuario\n");
    await waitDirty(root, "notas.txt", true);
    const typed = { dirty: true, text: await editorText(root) };

    press(buttonByText(root, "Salvar"));
    await waitDirty(root, "notas.txt", false);
    await waitFor("saved notice", () => root.querySelector("[data-notice]"));
    const saved = { dirty: false, editor_text: await editorText(root) };

    press(entry(root, "grande.txt"));
    await waitFor("large error", () => root.querySelector('[data-file-error="file_too_large"]'));
    await waitFor("editor gone for large", () => root.querySelector(".cm-editor") === null);
    const large_error = {
      code: "file_too_large",
      message: root.querySelector('[data-file-error="file_too_large"]')?.textContent?.trim() ?? "",
      editor_absent: root.querySelector(".cm-editor") === null,
    };

    press(entry(root, "bin.dat"));
    await waitFor("binary error", () => root.querySelector('[data-file-error="binary_unsupported"]'));
    await waitFor("editor gone for binary", () => root.querySelector(".cm-editor") === null);
    const binary_error = {
      code: "binary_unsupported",
      message:
        root.querySelector('[data-file-error="binary_unsupported"]')?.textContent?.trim() ?? "",
      editor_absent: root.querySelector(".cm-editor") === null,
    };

    press(entry(root, "outro.txt"));
    await waitForAsync("outro content", async () =>
      (await editorText(root)).includes("outro arquivo") ? true : null,
    );
    await appendText(root, "\nrascunho\n");
    await waitDirty(root, "outro.txt", true);
    press(tab(root, "outro.txt")?.querySelector<HTMLButtonElement>(".tab-close") ?? null);
    await waitFor("close prompt", () => root.querySelector("[data-close-prompt]"));
    const close_prompt_shown = true;
    press(buttonByText(root, "Cancelar"));
    await waitFor("close prompt gone", () => root.querySelector("[data-close-prompt]") === null);
    const close_cancelled = {
      text_kept: (await editorText(root)).includes("rascunho"),
      dirty: tab(root, "outro.txt")?.dataset.dirty === "true",
    };

    return {
      module_probe: { mounted: probe_mounted, idle: probe_idle, opened: probe_opened },
      editor_absent_before_open,
      empty_state_text,
      initial_rows,
      load_more_visible,
      rows_after_more,
      load_more_gone,
      subdir_expanded,
      tab_names,
      editor_loaded_after_open,
      typed,
      saved,
      large_error,
      binary_error,
      close_prompt_shown,
      close_cancelled,
    };
  }

  if (phase === "conflict") {
    await waitFor("root listing", () => root.querySelector("button.entry"));
    press(entry(root, "notas.txt"));
    const opened = {
      text: await waitForAsync("saved content", async () => {
        const text = await editorText(root);
        return text.includes("OBS usuario") ? text : null;
      }),
    };
    // The buffer drops one original line and adds another, so original, disk and buffer are
    // three distinct texts and each diff base yields its own added/removed lines.
    await replaceText(root, `${opened.text.replace("linha dois\n", "")}\nlinha do conflito\n`);
    await waitDirty(root, "notas.txt", true);
    await progress({ step: "dirty" });

    await waitFor("external notice", () => root.querySelector('[data-external="true"]'), 30000);
    const external_notice = true;

    press(buttonByText(root, "Salvar"));
    await waitFor("conflict panel", () => root.querySelector("[data-conflict]"), 30000);
    const buffer = await editorText(root);
    // Measured from the tab after the conflict, never assumed.
    const dirty = tab(root, "notas.txt")?.dataset.dirty ?? null;
    const conflict = {
      message: root.querySelector("[data-conflict]")?.textContent?.trim() ?? "",
      buffer_kept: buffer.includes("linha do conflito"),
      options: ["Recarregar", "Comparar", "Salvar cópia"].filter(
        (text) => buttonByText(root, text) !== null,
      ),
    };

    const rows = (op: string) =>
      Array.from(root.querySelectorAll(`[data-diff] [data-op="${op}"]`)).map((element) =>
        (element.textContent ?? "").replace(/^\s*[+−]\s*/, "").trim(),
      );
    const readDiff = () => ({
      base: root.querySelector<HTMLElement>("[data-diff]")?.dataset.diffBase ?? null,
      origins: (root.querySelector("[data-diff-sources]")?.textContent ?? "")
        .split("→")
        .map((part) => part.trim())
        .filter(Boolean),
      added: rows("added"),
      removed: rows("removed"),
      summary: root.querySelector("[data-diff-summary]")?.textContent?.trim() ?? "",
    });

    press(buttonByText(root, "Comparar"));
    await waitFor("disk diff panel", () => root.querySelector('[data-diff][data-diff-base="disk"]'));
    const diff = readDiff();

    press(root.querySelector<HTMLButtonElement>("[data-diff-original]"));
    await waitFor("original diff panel", () => root.querySelector('[data-diff][data-diff-base="original"]'));
    const diff_original = readDiff();

    press(buttonByText(root, "Salvar cópia"));
    await waitFor("recovery notice", () => root.querySelector("[data-recovery]"));
    const recovery = root.querySelector<HTMLElement>("[data-recovery]")?.dataset.recovery ?? "";

    press(buttonByText(root, "Recarregar"));
    const reloaded_text = await waitForAsync("reloaded content", async () => {
      const text = await editorText(root);
      return text === "versao externa\nagente\n" ? text : null;
    });
    await waitFor("conflict cleared", () => root.querySelector("[data-conflict]") === null);
    const reloaded = {
      text: reloaded_text,
      dirty: tab(root, "notas.txt")?.dataset.dirty === "true",
      conflict: root.querySelector("[data-conflict]") !== null,
    };

    return {
      opened,
      dirty: dirty === "true",
      dirty_attr: dirty,
      external_notice,
      conflict,
      diff,
      diff_original,
      recovery,
      reloaded,
    };
  }

  throw new Error(`unknown phase ${phase}`);
};
