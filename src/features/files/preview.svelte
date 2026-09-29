<script lang="ts">
  // Isolated preview of the files feature. In a browser (`?harness=files`) it runs over the
  // in-memory fake bridge; inside the native harness window (scripts/feature-harness/window.rs)
  // it uses the real IPC bridge and backend with the root from the injected params.
  // Mounting in the final window belongs to spec 007.
  import FilesWorkspace from "../../components/FilesWorkspace.svelte";
  import { tauriFilesBridge, type FilesBridge } from "../../files/bridge";
  import { createFakeFilesBridge } from "../../files/fake-bridge";
  import type { FileTarget } from "../../files/types";
  import { nativeHarness } from "../../harness/native";

  const native = nativeHarness();
  const fake = native ? null : createFakeFilesBridge();
  const bridge: FilesBridge = fake ?? tauriFilesBridge();
  const target: FileTarget = native
    ? { provider: "local", host: null, root: String(native.params.root ?? "") }
    : { provider: "local", host: null, root: "/projeto" };
</script>

<div class="preview">
  <FilesWorkspace {bridge} {target} />
  {#if fake}
    <p class="muted">Prévia isolada sobre a ponte fake em memória; o editor carrega só ao abrir um arquivo.</p>
  {/if}
</div>

<style>
  .preview {
    padding: 4px;
  }
  .muted {
    color: #8c93a3;
    font-size: 11px;
    padding: 0 12px 12px;
  }
</style>
