<script lang="ts">
  // Group header with collapse toggle, color indicator, name and project count (spec 011, AC-011-01).
  import { t } from "../../i18n/index.svelte";

  interface Props {
    name: string;
    color: string;
    count: number;
    collapsed: boolean;
    onToggle: () => void;
  }

  let { name, color, count, collapsed, onToggle }: Props = $props();

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onToggle();
    }
  }
</script>

<div
  class="group-header"
  role="button"
  tabindex="0"
  aria-expanded={!collapsed}
  aria-label={t("projects.group.header", { name, count })}
  onclick={onToggle}
  onkeydown={onKeydown}
>
  <span class="caret" aria-hidden="true">{collapsed ? "▸" : "▾"}</span>
  <span class="color-badge color-dot" style="background-color: {color};" aria-hidden="true"></span>
  <span class="name group-name">{name}</span>
  <span class="count project-count">{count}</span>
</div>

<style>
  .group-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    cursor: pointer;
    user-select: none;
    font-size: 13px;
    font-weight: 500;
    color: var(--text, #e7e9ee);
    border-radius: 4px;
    transition: background-color 0.15s ease;
  }
  .group-header:hover {
    background-color: var(--surface-2, #171a21);
  }
  .group-header:focus-visible {
    outline: 2px solid var(--accent, #8fa8ff);
    outline-offset: -1px;
  }
  .caret {
    font-size: 11px;
    color: var(--text-muted, #8c93a3);
    width: 12px;
    text-align: center;
    flex-shrink: 0;
  }
  .color-badge {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex-shrink: 0;
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    font-size: 12px;
    font-weight: 400;
    color: var(--text-muted, #8c93a3);
    flex-shrink: 0;
  }
</style>
