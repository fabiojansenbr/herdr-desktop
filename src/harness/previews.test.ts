// Component preview discovery (shared harness prepared by spec 002 for 003–005).
import { describe, expect, it } from "vitest";
import { previewLoaders, resolveHarness } from "./previews";

describe("preview discovery", () => {
  // Would catch: previews registered by hand (a new feature would need to edit shared files)
  // or eagerly imported (editor modules loaded at bootstrap).
  it("discovers feature previews lazily from src/features/*/preview.svelte", () => {
    const loaders = previewLoaders();
    expect(Object.keys(loaders)).toContain("projects");
    for (const loader of Object.values(loaders)) expect(typeof loader).toBe("function");
  });

  // Would catch: an unknown ?harness value falling back to a different preview, or the
  // terminal harness of 001 shadowed by discovery.
  it("resolves ?harness to terminal, a discovered feature, or an explicit unknown", () => {
    const names = ["projects", "files"];
    expect(resolveHarness("", names)).toEqual({ kind: "app" });
    expect(resolveHarness("?harness=terminal", names)).toEqual({ kind: "terminal" });
    expect(resolveHarness("?harness=projects", names)).toEqual({ kind: "feature", name: "projects" });
    expect(resolveHarness("?harness=agents", names)).toEqual({ kind: "unknown", name: "agents", available: ["terminal", "files", "projects"] });
    expect(resolveHarness("?harness=../x", names)).toEqual({ kind: "unknown", name: "../x", available: ["terminal", "files", "projects"] });
  });
});

describe("native harness selection", () => {
  // Would catch: an arbitrary injected object selecting a feature/phase that is not a plain
  // name (path-like values), or a missing params object accepted.
  it("accepts only a well-formed injected selection", async () => {
    const { parseNativeHarness, scenarioLoaders } = await import("./native");
    expect(parseNativeHarness({ feature: "projects", phase: "create", params: { a: 1 } })).toEqual({
      feature: "projects",
      phase: "create",
      params: { a: 1 },
    });
    expect(parseNativeHarness(undefined)).toBeNull();
    expect(parseNativeHarness({ feature: "../x", phase: "create", params: {} })).toBeNull();
    expect(parseNativeHarness({ feature: "projects", phase: "Create Now", params: {} })).toBeNull();
    expect(parseNativeHarness({ feature: "projects", phase: "create", params: [] })).toBeNull();
    expect(Object.keys(scenarioLoaders())).toContain("projects");
  });
});
