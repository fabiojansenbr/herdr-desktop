// Spec 006 — explicit read-only option of the shared editor. The local files feature (005)
// keeps the default editable editor; remote tabs pass `readOnly: true`. The editor itself is
// still reached only through dynamic imports (the 005 lazy-loading checks stay unchanged).
import { describe, expect, it } from "vitest";

const sources = import.meta.glob("../**/*.{ts,svelte}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

function source(suffix: string): string {
  // Files of this directory come back as "./name"; normalise them to "../editor/name".
  const key = Object.keys(sources).find((file) => file.replace(/^\.\//, "../editor/").endsWith(suffix));
  expect(key, suffix).toBeTruthy();
  return sources[key!]!;
}

describe("editor read-only option", () => {
  // Would catch: the option ignored (remote buffer editable) or applied by default (local
  // editing of spec 005 silently disabled).
  it("makes the state read-only and non-editable only when asked", async () => {
    const { editorOptionExtensions } = await import("./editor");
    const { EditorState } = await import("@codemirror/state");
    const { EditorView } = await import("@codemirror/view");

    const remote = EditorState.create({ doc: "remoto", extensions: await editorOptionExtensions({ readOnly: true }) });
    expect(remote.readOnly).toBe(true);
    expect(remote.facet(EditorView.editable)).toBe(false);

    for (const options of [{}, { readOnly: false }]) {
      const local = EditorState.create({ doc: "local", extensions: await editorOptionExtensions(options) });
      expect(local.readOnly).toBe(false);
      expect(local.facet(EditorView.editable)).toBe(true);
    }
  });

  // Would catch: mountEditor not wiring the option into every tab state, the local workspace
  // starting to pass an option, or the remote workspace mounting an editable editor.
  it("is wired through mountEditor, default for local and explicit for remote tabs", () => {
    const editor = source("editor/editor.ts");
    expect(editor).toMatch(/export async function mountEditor\(\s*parent: HTMLElement,\s*options: EditorOptions = \{\}\s*,?\s*\)/);
    expect(editor).toMatch(/\.\.\.\(await editorOptionExtensions\(options\)\)/);
    expect(source("components/FilesWorkspace.svelte")).toMatch(/module\.mountEditor\(host\)\)/);
    const remote = source("components/RemoteFilesWorkspace.svelte");
    expect(remote).toMatch(/import\(\s*["']\.\.\/editor\/editor["']\s*\)/);
    expect(remote).toMatch(/mountEditor\(host, \{ readOnly: true \}\)/);
  });
});
