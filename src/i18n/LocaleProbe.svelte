<script lang="ts">
  // Spec 067 (AC-067-02) — smallest possible reader of `t()`: it exists so a test can prove that a
  // Svelte component re-renders when the locale changes, without mounting a product region. Not
  // part of the product bundle (no product module imports it).
  import { t } from "./index.svelte";
</script>

<p data-probe="status">{t("agent.status.working")}</p>
<p data-probe="items">{t("demo.items", { count: 2 })}</p>
