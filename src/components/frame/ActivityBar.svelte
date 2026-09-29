<script lang="ts">
  // Activity bar of the frame (spec 010): 52 px, icon buttons with accessible names. Labels are the
  // ones spec 007's native flow drives; Início (spec 012) shows the home screen, while profile and
  // settings stay disabled with reason until their screens exist.
  import type { ActivityId } from "../../shell/Workbench.types";
  import { t } from "../../i18n/index.svelte";

  let {
    active,
    sidebarOpen,
    agentsOpen,
    searchHint,
    onSelect,
  }: {
    active: ActivityId;
    sidebarOpen: boolean;
    agentsOpen: boolean;
    searchHint: string;
    onSelect: (id: ActivityId) => void;
  } = $props();
</script>

<aside class="activity" data-region="activity" aria-label={t("frame.activity.label")}>
  <div class="group">
    <button type="button" class="item" class:active={active === "projects" && sidebarOpen} onclick={() => onSelect("projects")} aria-label={t("frame.activity.projects")} aria-pressed={active === "projects" && sidebarOpen} title={t("frame.activity.projects")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z" />
        <path d="M8 10v4" /><path d="M12 10v2" /><path d="M16 10v6" />
      </svg>
    </button>
    <button type="button" class="item" class:active={agentsOpen} onclick={() => onSelect("agents")} aria-label={t("frame.activity.agents")} aria-pressed={agentsOpen} title={t("frame.activity.agents")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M12 8V4H8" /><rect width="16" height="12" x="4" y="8" rx="2" /><path d="M2 14h2" /><path d="M20 14h2" /><path d="M15 13v2" /><path d="M9 13v2" />
      </svg>
    </button>
    <button type="button" class="item" class:active={active === "connections"} onclick={() => onSelect("connections")} aria-label={t("frame.activity.connections")} aria-pressed={active === "connections"} title={t("frame.activity.connections")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="10" /><line x1="2" y1="12" x2="22" y2="12" />
        <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
      </svg>
    </button>
    <button type="button" class="item" class:active={active === "files"} onclick={() => onSelect("files")} aria-label={t("frame.activity.files")} aria-pressed={active === "files"} title={t("frame.activity.files")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <line x1="6" y1="3" x2="6" y2="15" /><circle cx="18" cy="6" r="3" /><circle cx="6" cy="18" r="3" /><path d="M18 9a9 9 0 0 1-9 9" />
      </svg>
    </button>
    <button type="button" class="item" onclick={() => onSelect("search")} aria-label={t("frame.activity.search", { hint: searchHint })} title={t("frame.activity.search", { hint: searchHint })}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
      </svg>
    </button>
  </div>

  <div class="spacer"></div>

  <div class="group">
    <button type="button" class="item" class:active={active === "home"} onclick={() => onSelect("home")} aria-label={t("frame.activity.home")} aria-pressed={active === "home"} title={t("frame.activity.home")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="m3 10 9-7 9 7v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" /><path d="M9 22V12h6v10" />
      </svg>
    </button>
    <button type="button" class="item" disabled aria-label={t("frame.activity.profile")} title={t("frame.activity.profile")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="10" /><circle cx="12" cy="10" r="3" /><path d="M7 20.662V19a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v1.662" />
      </svg>
    </button>
    <button type="button" class="item" disabled aria-label={t("frame.activity.settings")} title={t("frame.activity.settings")}>
      <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="3" />
        <path d="M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1" />
      </svg>
    </button>
  </div>
</aside>

<style>
  .activity {
    flex: none;
    width: 52px;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 10px 0;
    background: var(--surface);
    border-right: 1px solid var(--border);
    user-select: none;
    z-index: 5;
  }
  .group {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }
  .spacer {
    flex: 1;
  }
  .item {
    width: 36px;
    height: 36px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
  }
  .item:hover:not(:disabled),
  .item.active {
    color: var(--text);
    background: var(--surface-3);
  }
  .item.active {
    box-shadow: inset 2px 0 0 var(--accent);
  }
</style>
