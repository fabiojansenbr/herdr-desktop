<script lang="ts">
  // Spec 010 name kept as a thin mount of the 017 title bar so existing imports keep working.
  import TitleBar from "./TitleBar.svelte";
  let props = $props();
</script>

<TitleBar {...props} />
