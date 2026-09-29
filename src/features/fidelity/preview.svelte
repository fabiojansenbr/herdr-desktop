<script lang="ts">
  // Spec 007 composed window under the native harness: the real App with its real IPC bridges.
  // No fake bridge exists here; outside a native harness window this is just the product App.
  import App from "../../App.svelte";
</script>

<App />
