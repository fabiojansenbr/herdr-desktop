// Discovery of isolated component previews. Each feature owns
// `src/features/<name>/preview.svelte`; nothing here lists features by hand, so fronts
// 003–005 add a preview without editing shared files. Loaders are lazy: a preview (and
// whatever it imports, e.g. the editor) is fetched only when `?harness=<name>` selects it.

import type { Component } from "svelte";

export type PreviewModule = { default: Component };
export type PreviewLoader = () => Promise<PreviewModule>;

const NAME = /^[a-z][a-z0-9-]*$/;

export function previewLoaders(): Record<string, PreviewLoader> {
  const modules = import.meta.glob<PreviewModule>("../features/*/preview.svelte");
  const loaders: Record<string, PreviewLoader> = {};
  for (const [path, loader] of Object.entries(modules)) {
    const name = /\.\.\/features\/([^/]+)\/preview\.svelte$/.exec(path)?.[1];
    if (name && NAME.test(name) && name !== "terminal") loaders[name] = loader;
  }
  return loaders;
}

export type HarnessSelection =
  | { kind: "app" }
  | { kind: "terminal" }
  | { kind: "feature"; name: string }
  | { kind: "unknown"; name: string; available: string[] };

/** Maps `location.search` to what `main.ts` mounts. Unknown names never fall back to another preview. */
export function resolveHarness(search: string, featureNames: string[]): HarnessSelection {
  const name = new URLSearchParams(search).get("harness");
  if (name === null) return { kind: "app" };
  if (name === "terminal") return { kind: "terminal" };
  if (NAME.test(name) && featureNames.includes(name)) return { kind: "feature", name };
  return { kind: "unknown", name, available: ["terminal", ...[...featureNames].sort()] };
}
