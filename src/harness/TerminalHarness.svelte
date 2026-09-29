<script lang="ts">
  // Isolated harness for the terminal component: feeds tests/fixtures/surface-cells-v1.json
  // through the same FrameEvent path the host uses. No IPC, no engine.
  import TerminalView from "../terminal/TerminalView.svelte";
  import { fixtureEvents } from "./fixture";
  import type { FrameEvent, InputDto } from "../terminal/types";

  let handler: ((event: FrameEvent) => void) | null = null;
  let log = $state<string[]>([]);
  let visible = $state(true);

  function subscribe(h: (event: FrameEvent) => void) {
    handler = h;
    queueMicrotask(() => replay("full"));
    return () => (handler = null);
  }

  function replay(step: "full" | "patches") {
    const events = fixtureEvents();
    if (!handler) return;
    if (step === "full") handler(events.full);
    else for (const patch of events.patches) handler(patch);
    log = [...log, `replayed ${step}`];
  }

  function onInput(events: InputDto[]) {
    log = [...log, `input ${JSON.stringify(events)}`];
  }
</script>

<div class="harness">
  <div class="controls">
    <button onclick={() => replay("full")}>Full frame</button>
    <button onclick={() => replay("patches")}>Patches</button>
    <label><input type="checkbox" bind:checked={visible} /> visível</label>
  </div>
  <div class="view">
    <TerminalView {subscribe} {onInput} onResize={() => {}} {visible} fontSize={20} />
  </div>
  <pre>{log.join("\n")}</pre>
</div>

<style>
  .harness {
    padding: 12px;
    display: grid;
    gap: 8px;
  }
  .view {
    width: 100%;
    height: 200px;
  }
  pre {
    font-size: 11px;
    color: #8b97a5;
  }
</style>
