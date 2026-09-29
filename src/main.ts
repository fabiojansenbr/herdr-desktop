import { mount } from "svelte";
import App from "./App.svelte";
import TerminalHarness from "./harness/TerminalHarness.svelte";
import { nativeHarness, runNativeScenario } from "./harness/native";
import { previewLoaders, resolveHarness } from "./harness/previews";
import { initLocale } from "./i18n/index.svelte";

// `?harness=terminal` mounts the isolated terminal harness (fixture-fed, no IPC);
// `?harness=<feature>` lazily loads src/features/<feature>/preview.svelte. A native harness
// window (scripts/feature-harness/window.rs) injects its selection instead of a query and
// runs the feature scenario against the real backend.
const target = document.getElementById("app")!;
const loaders = previewLoaders();
const native = nativeHarness();
const selection = native
  ? resolveHarness(`?harness=${encodeURIComponent(native.feature)}`, Object.keys(loaders))
  : resolveHarness(window.location.search, Object.keys(loaders));

async function start() {
  // Spec 067: the language of the environment (or the saved preference) before anything renders,
  // so no region ever paints in the wrong language. `index.html` announces `en` until this runs.
  await initLocale();
  switch (selection.kind) {
    case "app":
      return mount(App, { target });
    case "terminal":
      return mount(TerminalHarness, { target });
    case "feature": {
      const preview = await loaders[selection.name]!();
      const mounted = mount(preview.default, { target });
      if (native) void runNativeScenario(native, target);
      return mounted;
    }
    case "unknown":
      target.textContent = `Harness desconhecido: ${selection.name}. Disponíveis: ${selection.available.join(", ")}`;
      return null;
  }
}

export default start();
