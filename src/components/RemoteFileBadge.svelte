<script lang="ts">
  // Identification of a remote file (spec 006): which SSH host it came from, that it is read-only
  // in this version and, when the connection it was read on is gone or renewed, that the content
  // is a cached read (Desatualizado). States carry text, not only color.
  import { t } from "../i18n/index.svelte";

  interface Props {
    host: string;
    stale?: boolean;
  }

  let { host, stale = false }: Props = $props();
</script>

<span class="remote-badge" data-remote-badge data-host={host} data-stale={stale}>
  <span class="host" title={t("files.badge.host")}>SSH · {host}</span>
  <span class="read-only" data-read-only>{t("files.badge.readOnly")}</span>
  {#if stale}<span class="stale" data-stale-label>{t("files.badge.stale")}</span>{/if}
</span>

<style>
  .remote-badge {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    font-size: 11px;
  }
  .remote-badge > span {
    border-radius: 4px;
    padding: 1px 5px;
    border: 1px solid #2c3242;
    white-space: nowrap;
  }
  .host {
    color: #9fb4ff;
  }
  .read-only {
    color: #b8c2cc;
  }
  .stale {
    color: #f4b454;
    border-color: #5a4a24;
  }
</style>
