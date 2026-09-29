<script lang="ts">
  // Sidebar head of the approved design (design/v2-01-workspace.png): the herdr mark and the
  // control that collapses the sidebar, which is the window's own action (spec 041, AC-041-01).
  import { t } from "../../i18n/index.svelte";
  import BrandMark from "./BrandMark.svelte";

  let { onCollapse }: { onCollapse: () => void } = $props();
</script>

<header class="head" data-sidebar-header>
  <span class="brand">
    <span class="logo" aria-hidden="true"><BrandMark size={24} /></span>
    <span class="brand-name">herdr</span>
  </span>
  <button type="button" class="collapse" data-collapse-sidebar aria-label={t("sidebar.collapse")} title={t("sidebar.collapse")} onclick={onCollapse}>
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <line x1="9" y1="4" x2="9" y2="20" />
    </svg>
  </button>
</header>

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: 48px;
    padding: 0 12px;
    flex-shrink: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .logo {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    flex-shrink: 0;
  }
  .brand-name {
    font-size: 15px;
    font-weight: 600;
    color: var(--text, #ededed);
    letter-spacing: 0.01em;
  }
  .collapse {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 6px;
    padding: 4px;
    color: var(--text-muted, #8c93a3);
    cursor: pointer;
    transition: color 0.15s ease, background-color 0.15s ease;
  }
  .collapse:hover {
    color: var(--text, #e7e9ee);
    background-color: var(--surface-2, #171a21);
  }
  .collapse:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
  }
</style>
