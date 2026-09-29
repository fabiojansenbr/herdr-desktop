// Spec 005 (AC-005-03) — the editor and its language modules must not be part of the GUI
// bootstrap: nothing in src statically imports CodeMirror, and the feature only reaches the
// editor entry point through a dynamic import that happens when a text tab becomes ready.
// Static source check only: which modules a mounted window actually loads (zero before opening
// a file, detected after) is measured by e2e_files_flow in src-tauri/tests/files_local.rs.
import { describe, expect, it } from "vitest";

// Raw sources through Vite itself, so the check needs no Node typings.
const sources = import.meta.glob("../**/*.{ts,svelte}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

function staticImporters(pattern: RegExp): string[] {
  return Object.entries(sources)
    .filter(([, source]) => pattern.test(source))
    .map(([file]) => file);
}

describe("editor lazy loading", () => {
  // Would catch: importing the editor (or a language package) at module scope, which would load
  // it with the GUI even with no file open.
  it("never statically imports CodeMirror anywhere in src", () => {
    const staticCodeMirror = /^\s*import\s+(?!type\s)[^;]*?from\s+["']@codemirror\//m;
    expect(staticImporters(staticCodeMirror)).toEqual([]);
  });

  // Would catch: the files feature reaching the editor through a static import (which would
  // eagerly load it when the preview mounts) or the entry point missing.
  it("reaches the editor entry point only through a dynamic import", () => {
    const key = Object.keys(sources).find((file) => file.endsWith("components/FilesWorkspace.svelte"));
    expect(key).toBeTruthy();
    expect(sources[key!]).toMatch(/import\(\s*["'][^"']*editor\/editor["']\s*\)/);
    const staticEditor = /^\s*import\s+(?!type\s)[^;]*?from\s+["'][^"']*\/editor\/editor["']/m;
    expect(staticImporters(staticEditor)).toEqual([]);
  });
});
