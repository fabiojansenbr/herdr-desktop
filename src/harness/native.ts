// Native harness bridge (see scripts/feature-harness/window.rs). When a feature E2E phase
// opens a real Tauri window it injects `window.__HERDR_HARNESS__`; main.ts then mounts the
// feature preview with its real IPC bridge and runs `src/features/<feature>/e2e.ts`, which
// drives the component through the DOM and reports what the user would see.

import { invoke } from "@tauri-apps/api/core";

export interface NativeHarnessConfig {
  feature: string;
  phase: string;
  params: Record<string, unknown>;
}

export interface ScenarioContext {
  phase: string;
  params: Record<string, unknown>;
  root: HTMLElement;
  /** Intermediate report (kept if the phase later fails or times out). */
  progress(report: Record<string, unknown>): Promise<void>;
}

export type Scenario = (ctx: ScenarioContext) => Promise<Record<string, unknown>>;
export type ScenarioModule = { run: Scenario };

const NAME = /^[a-z][a-z0-9-]*$/;

declare global {
  interface Window {
    __HERDR_HARNESS__?: unknown;
  }
}

/** Validated injected selection, or null outside a native harness window. */
export function parseNativeHarness(value: unknown): NativeHarnessConfig | null {
  if (!value || typeof value !== "object") return null;
  const { feature, phase, params } = value as Record<string, unknown>;
  if (typeof feature !== "string" || !NAME.test(feature)) return null;
  if (typeof phase !== "string" || !NAME.test(phase)) return null;
  if (!params || typeof params !== "object" || Array.isArray(params)) return null;
  return { feature, phase, params: params as Record<string, unknown> };
}

export function nativeHarness(): NativeHarnessConfig | null {
  return typeof window === "undefined" ? null : parseNativeHarness(window.__HERDR_HARNESS__);
}

export function scenarioLoaders(): Record<string, () => Promise<ScenarioModule>> {
  const modules = import.meta.glob<ScenarioModule>("../features/*/e2e.ts");
  const loaders: Record<string, () => Promise<ScenarioModule>> = {};
  for (const [path, loader] of Object.entries(modules)) {
    const name = /\.\.\/features\/([^/]+)\/e2e\.ts$/.exec(path)?.[1];
    if (name && NAME.test(name)) loaders[name] = loader;
  }
  return loaders;
}

function report(report: Record<string, unknown>, done: boolean): Promise<void> {
  return invoke<void>("harness_report", { report, done });
}

export async function runNativeScenario(config: NativeHarnessConfig, root: HTMLElement): Promise<void> {
  try {
    const loader = scenarioLoaders()[config.feature];
    if (!loader) throw new Error(`feature ${config.feature} has no e2e.ts scenario`);
    const { run } = await loader();
    const result = await run({
      phase: config.phase,
      params: config.params,
      root,
      progress: (partial) => report({ ...partial, done: false }, false),
    });
    await report({ ...result, phase: config.phase, error: null }, true);
  } catch (error) {
    const message = error instanceof Error ? error.message : JSON.stringify(error);
    await report({ phase: config.phase, error: message, dom: root.innerText.slice(0, 4000) }, true);
  }
}
