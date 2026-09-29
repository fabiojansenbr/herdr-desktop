<script lang="ts">
  // Isolated preview of the connections feature. In a browser (`?harness=connections`) it runs
  // over the in-memory fake bridge with buttons that simulate host states; inside the native
  // harness window (scripts/feature-harness/window.rs) it uses the real IPC bridge and backend.
  // Mounting in the final window belongs to spec 007.
  import ConnectionsPanel from "../../connections/ConnectionsPanel.svelte";
  import { tauriConnectionsBridge, type ConnectionsBridge } from "../../connections/bridge";
  import { createFakeConnectionsBridge, SSH_ENDPOINT, type FakeConnectionsBridge } from "../../connections/fake-bridge";
  import { nativeHarness } from "../../harness/native";

  const fake: FakeConnectionsBridge | null = nativeHarness() ? null : createFakeConnectionsBridge();
  const bridge: ConnectionsBridge = fake ?? tauriConnectionsBridge();
</script>

<div class="preview">
  <ConnectionsPanel {bridge} />
  {#if fake}
    <section aria-label="Simulação">
      <h3>Simular estados (ponte fake)</h3>
      <button onclick={() => fake.setOnline("local", "boot-local")}>Local online</button>
      <button onclick={() => fake.setOnline(SSH_ENDPOINT, `boot-${Date.now()}`)}>SSH online</button>
      <button onclick={() => fake.setReconnecting(SSH_ENDPOINT)}>SSH perdido</button>
      <button onclick={() => fake.setAttention(SSH_ENDPOINT, "host_key_unknown")}>SSH chave desconhecida</button>
    </section>
  {:else}
    <h3>Harness nativo: ponte IPC real</h3>
  {/if}
</div>

<style>
  .preview {
    display: grid;
    gap: 16px;
    padding: 12px;
    background: #0b0c10;
    min-height: 100vh;
    box-sizing: border-box;
  }
  h3 {
    color: #8c93a3;
    font-size: 12px;
  }
</style>
