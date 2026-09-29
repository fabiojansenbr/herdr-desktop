<script lang="ts">
  // Status bar of the frame (spec 010, AC-010-02): server version and state, host, session, branch,
  // panes/tabs and channel, all as text (color only complements). The first three items keep the
  // order and attributes spec 007's native flow reads (phase + identity title, host, session).
  import type { SurfacePhase } from "../../shell/controller";
  import { t } from "../../i18n/index.svelte";
  import type { StatusModel } from "./status";

  let {
    phase,
    diagnostics,
    model,
    host,
    session,
    blocked,
    readOnly,
    notice = null,
  }: {
    phase: SurfacePhase;
    diagnostics: string;
    model: StatusModel;
    host: string | null;
    session: string | null;
    blocked: string | null;
    readOnly: boolean;
    /** Temporary warning (spec 027), e.g. a tab closed in another client. */
    notice?: string | null;
  } = $props();
</script>

<footer class="status-bar" data-region="status" aria-label={t("frame.status.label")}>
  <span class="status-item" data-item="server" data-phase={phase} title={diagnostics}>
    <span class="dot" aria-hidden="true"></span>
    {model.server}
  </span>
  {#if host}
    <span class="status-item" data-item="host">{host}</span>
    {#if session}
      <span class="status-item" data-item="session">{t("frame.status.session", { name: session })}</span>
    {/if}
    <span class="status-item" data-item="branch" title={t("frame.status.branchTitle")}>
      <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <line x1="6" y1="3" x2="6" y2="15" /><circle cx="18" cy="6" r="3" /><circle cx="6" cy="18" r="3" /><path d="M18 9a9 9 0 0 1-9 9" />
      </svg>
      {model.branch}
    </span>
    {#if model.counts}
      <span class="status-item" data-item="counts" title={t("frame.status.countsTitle")}>{model.counts}</span>
    {/if}
    {#if model.cwd}
      <span class="status-item" data-item="cwd" data-cwd title={model.cwd}>{model.cwd}</span>
    {/if}
  {/if}
  {#if blocked}
    <span class="status-item" data-item="blocked">{blocked}</span>
  {/if}
  {#if notice}
    <span class="status-item notice" data-item="notice" role="status">{notice}</span>
  {/if}
  <span class="spacer"></span>
  {#if readOnly}
    <span class="status-item" data-item="readonly">{t("frame.status.readOnly")}</span>
  {/if}
  {#if model.agents}
    <span class="status-item" data-item="agents">{model.agents}</span>
  {/if}
  {#if model.channel}
    <span class="status-item" data-item="channel">{model.channel}</span>
  {/if}
</footer>

<style>
  .status-bar {
    flex: none;
    height: 28px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 12px;
    border-top: none;
    border-top-width: 0px;
    background: var(--bg);
    color: var(--text-dim);
    font-family: var(--font-ui);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    user-select: none;
  }
  .status-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .notice {
    color: var(--attention);
  }
  .spacer {
    flex: 1;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--idle);
  }
  [data-phase="live"] .dot {
    background: var(--working);
  }
  [data-phase="stale"] .dot,
  [data-phase="connecting"] .dot,
  [data-phase="switching"] .dot {
    background: var(--attention);
  }
  [data-phase="disconnected"] .dot {
    background: var(--error);
  }
</style>
