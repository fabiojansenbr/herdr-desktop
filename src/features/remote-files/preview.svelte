<script lang="ts">
  // Isolated preview of the remote files feature (spec 006). In a browser
  // (`?harness=remote-files`) it runs over the in-memory fake bridge with buttons that drop and
  // renew the connection; inside the native harness window (scripts/feature-harness/window.rs) it
  // uses the real IPC bridges and backends: the local files workspace of 005 on the same root (to
  // compare Local and SSH at the same path) next to the remote workspace. Mounting in the final
  // window belongs to spec 007.
  import FilesWorkspace from "../../components/FilesWorkspace.svelte";
  import RemoteFilesWorkspace from "../../components/RemoteFilesWorkspace.svelte";
  import { tauriFilesBridge } from "../../files/bridge";
  import { createFakeRemoteFilesBridge, tauriRemoteFilesBridge, type RemoteFilesBridge } from "../../files/remote";
  import type { FileTarget } from "../../files/types";
  import { nativeHarness } from "../../harness/native";

  const native = nativeHarness();
  const fake = native ? null : createFakeRemoteFilesBridge();
  const remote: RemoteFilesBridge = fake ?? tauriRemoteFilesBridge();
  const localTarget: FileTarget | null = native
    ? { provider: "local", host: null, root: String(native.params.root ?? "") }
    : null;
</script>

<div class="preview">
  {#if localTarget}
    <section aria-label="Este computador" data-preview-local>
      <h3>Este computador (Local)</h3>
      <FilesWorkspace bridge={tauriFilesBridge()} target={localTarget} />
    </section>
  {/if}
  <section aria-label="Host SSH" data-preview-remote>
    <h3>Host SSH (SFTP, somente leitura)</h3>
    <RemoteFilesWorkspace bridge={remote} />
  </section>
  {#if fake}
    <section aria-label="Simulação">
      <h3>Simular conexão (ponte fake)</h3>
      <button onclick={() => fake.dropConnection()}>Perder conexão</button>
      <button onclick={() => fake.reconnect()}>Reconectar (nova geração)</button>
    </section>
  {/if}
</div>

<style>
  .preview {
    display: grid;
    gap: 8px;
    padding: 4px;
    background: #0b0c10;
    min-height: 100vh;
    box-sizing: border-box;
  }
  h3 {
    color: #8c93a3;
    font-size: 12px;
    margin: 4px 12px 0;
  }
</style>
