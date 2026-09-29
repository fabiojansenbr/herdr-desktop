<script lang="ts">
  // Spec 012 — one project card of the home screen: name, branch, path (host-prefixed for SSH),
  // up to three thumbnails (agent name and state dot, amber for the agent waiting for the user)
  // and the last activity with avatars and time. The thumbnail body is the *static* `pane.read`
  // snapshot of the visit; this component renders it and never refreshes it by itself.
  import { t } from "../../i18n/index.svelte";
  import type { HomeCard } from "./home-model";

  interface Props {
    card: HomeCard;
    cache: boolean;
  }

  let { card, cache }: Props = $props();

  function snapshotLines(text: string): string[] {
    return text
      .split("\n")
      .map((line) => line.trimEnd())
      .filter((line) => line.trim() !== "")
      .slice(-3);
  }
</script>

<article class="card" data-project-card={card.id}>
  <header>
    <h3 class="name">{card.name}</h3>
    <span class="branch" aria-label={t("home.card.branch", { name: card.branch })}>⎇ {card.branch}</span>
  </header>
  <p class="path">{card.path}{#if card.isSsh && cache}<span class="cache" data-cache>{t("home.cache")}</span>{/if}</p>

  {#if card.thumbnails.length > 0}
    <div class="thumbs" data-thumbnails>
      {#each card.thumbnails as thumb (thumb.paneId)}
        <div
          class="thumb"
          class:waiting={thumb.dot.waiting}
          data-thumbnail-pane={thumb.paneId}
          data-status={thumb.dot.status}
          role="img"
          aria-label={t("home.card.thumbnail", { name: thumb.name, state: thumb.dot.label })}
        >
          <span class="thumb-head">
            <span class="dot" style={`--dot:${thumb.dot.color}`} aria-hidden="true"></span>
            <span class="thumb-name">{thumb.name}</span>
          </span>
          {#if thumb.text}
            <span class="thumb-text" data-thumbnail-text>{snapshotLines(thumb.text).join("\n")}</span>
          {/if}
        </div>
      {/each}
    </div>
    {#if card.overflow > 0}
      <p class="overflow" data-thumbnail-overflow>+{card.overflow}</p>
    {/if}
  {:else}
    <p class="none">{t("home.card.noPanes")}</p>
  {/if}

  <footer class="activity">
    <span class="avatars" aria-hidden="true">
      {#each card.activity.avatars as avatar, index (index)}
        <span class="avatar">{avatar}</span>
      {/each}
    </span>
    {#if card.activity.summary}
      <span class="summary">{card.activity.summary}</span>
    {/if}
    <span class="time" data-time>{card.activity.time}</span>
  </footer>
</article>

<style>
  .card {
    display: grid;
    gap: 8px;
    align-content: start;
    min-width: 0;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    color: var(--text);
    font-family: var(--font-ui);
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    min-width: 0;
  }
  .name {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .branch {
    flex: 0 0 auto;
    font-size: 11px;
    color: var(--text-muted);
  }
  .path {
    margin: 0;
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cache {
    margin-left: 6px;
    padding: 0 4px;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 10px;
  }
  .thumbs {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 6px;
  }
  .thumb {
    display: grid;
    gap: 4px;
    align-content: start;
    min-width: 0;
    min-height: 56px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
  }
  /* The agent waiting for the user is the amber one (AC-012-02). */
  .thumb.waiting {
    border-color: var(--attention);
  }
  .thumb-head {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }
  .dot {
    flex: 0 0 auto;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dot, var(--idle));
  }
  .thumb-name {
    font-size: 11px;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .thumb-text {
    font-family: var(--font-mono);
    font-size: 10px;
    line-height: 1.25;
    color: var(--text-muted);
    white-space: pre;
    overflow: hidden;
  }
  .overflow,
  .none {
    margin: 0;
    font-size: 11px;
    color: var(--text-muted);
  }
  .activity {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .avatars {
    display: flex;
    flex: 0 0 auto;
  }
  .avatar {
    width: 16px;
    height: 16px;
    margin-left: -4px;
    display: inline-grid;
    place-items: center;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--text);
    font-size: 10px;
    text-transform: uppercase;
  }
  .avatar:first-child {
    margin-left: 0;
  }
  .summary {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .time {
    flex: 0 0 auto;
    margin-left: auto;
    font-size: 11px;
    color: var(--text-muted);
  }
</style>
