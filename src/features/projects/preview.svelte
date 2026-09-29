<script lang="ts">
  // Isolated preview of the projects feature. In a browser (`?harness=projects`) it runs over
  // the in-memory fake bridge and shows the commands it recorded; inside the native harness
  // window (scripts/feature-harness/window.rs) it uses the real IPC bridge and backend.
  // Mounting in the final window belongs to spec 007.
  import ProjectNavigator from "../../components/ProjectNavigator.svelte";
  import { tauriProjectsBridge, type ProjectsBridge } from "../../projects/bridge";
  import { createFakeProjectsBridge, type FakeProjectsBridge } from "../../projects/fake-bridge";
  import { nativeHarness } from "../../harness/native";

  const fake: FakeProjectsBridge | null = nativeHarness() ? null : createFakeProjectsBridge({ bootId: "preview-boot" });
  const bridge: ProjectsBridge = fake ?? tauriProjectsBridge();
  let log = $state<string[]>([]);
  if (fake) {
    const origPush = fake.calls.push.bind(fake.calls);
    fake.calls.push = (...items) => {
      const n = origPush(...items);
      log = fake.calls.map((c) => `${c.command} ${JSON.stringify(c.args)}`);
      return n;
    };
  }
</script>

<div class="preview">
  <ProjectNavigator {bridge} />
  <section>
    {#if fake}
      <h3>Comandos registrados pela ponte fake</h3>
      <button onclick={() => fake.reboot(`preview-boot-${Date.now()}`)}>Simular reinício do servidor</button>
      <pre>{log.join("\n")}</pre>
    {:else}
      <h3>Harness nativo: ponte IPC real</h3>
    {/if}
  </section>
</div>

<style>
  .preview {
    display: flex;
    gap: 16px;
    padding: 12px;
    align-items: flex-start;
  }
  pre {
    font-size: 11px;
    color: #8c93a3;
    white-space: pre-wrap;
  }
</style>
