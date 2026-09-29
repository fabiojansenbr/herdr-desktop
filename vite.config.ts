import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import type { Plugin } from "vite";

// Build metadata only (spec 005, AC-005-03): records which source modules each emitted chunk
// contains, in dist/.vite/module-chunks.json, so native E2E probes can map the scripts a
// window actually fetched to editor/language modules. Chunking and output are unchanged.
function moduleChunks(): Plugin {
  let root = "";
  return {
    name: "herdr-module-chunks",
    apply: "build",
    configResolved(config) {
      root = `${config.root.split("\\").join("/").replace(/\/$/, "")}/`;
    },
    generateBundle(_options, bundle) {
      const chunks: Record<string, string[]> = {};
      for (const [file, output] of Object.entries(bundle)) {
        if (output.type !== "chunk") continue;
        chunks[file] = output.moduleIds
          .map((id) => {
            const path = id.split("\\").join("/");
            return path.startsWith(root) ? path.slice(root.length) : path;
          })
          .sort();
      }
      this.emitFile({
        type: "asset",
        fileName: ".vite/module-chunks.json",
        source: JSON.stringify({ version: 1, chunks }, null, 1),
      });
    },
  };
}

// Tauri expects a fixed port in dev and a static build in ../dist for production.
export default defineConfig({
  plugins: [svelte(), moduleChunks()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    target: ["es2022", "safari16"],
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
  },
  test: {
    // Spec 067 (AC-067-04): one setup file for both projects fixes the locale on `pt`, so the
    // assertions written in Portuguese never depend on the machine's language.
    setupFiles: ["./src/test-setup.ts"],
    projects: [
      {
        extends: true,
        test: {
          name: "node",
          include: ["src/**/*.test.ts"],
          exclude: [
            "src/components/frame/i18n.test.ts",
            "src/components/frame/titlebar.test.ts",
            "src/components/frame/topbar-narrow.test.ts",
            "src/components/projects/region.test.ts",
            "src/components/sidebar/**/*.test.ts",
            "src/components/**/*-visual.test.ts",
            "src/components/projects/ConnectionsFooter.test.ts",
            "src/components/center/new-agent.test.ts",
            "src/components/center/context-menu.test.ts",
            "src/components/center/orca.test.ts",
            "src/components/center/tabs-narrow.test.ts",
            "src/components/center/scroll-indicator.test.ts",
            "src/shell/casca.test.ts",
            "src/theme/apply.test.ts",
            "src/terminal/repaint.test.ts",
            "src/terminal/wheel.test.ts",
            "src/i18n/i18n.test.ts",
            "src/**/i18n.test.ts",
          ],
          environment: "node",
        },
      },
      {
        extends: true,
        resolve: { conditions: ["browser"] },
        test: {
          name: "dom",
          include: [
            "src/components/frame/i18n.test.ts",
            "src/components/frame/titlebar.test.ts",
            "src/components/frame/topbar-narrow.test.ts",
            "src/components/projects/region.test.ts",
            "src/components/sidebar/**/*.test.ts",
            "src/components/**/*-visual.test.ts",
            "src/components/projects/ConnectionsFooter.test.ts",
            "src/components/center/new-agent.test.ts",
            "src/components/center/context-menu.test.ts",
            "src/components/center/orca.test.ts",
            "src/components/center/tabs-narrow.test.ts",
            "src/components/center/scroll-indicator.test.ts",
            "src/shell/casca.test.ts",
            "src/theme/apply.test.ts",
            "src/terminal/repaint.test.ts",
            "src/terminal/wheel.test.ts",
            "src/i18n/i18n.test.ts",
            "src/**/i18n.test.ts",
          ],
          environment: "happy-dom",
        },
      },
    ],
  },
});
