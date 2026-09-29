<script lang="ts">
  // Command palette (spec 010, AC-010-03): opened by the search trigger or the explicit shortcut
  // (outside the terminal only); lists Projetos, Panes, Agentes and Comandos (palette.ts). Escape
  // closes it and the owner returns the focus to the element focused before opening.
  //
  // Spec 069 (AC-069-03): the Idioma/Language group comes from the palette's own model, not from
  // the owner's sources — it picks a language instead of running one of the window's actions.
  import { languageSection, type PaletteEntry, type PaletteSection } from "./palette";
  import { t } from "../../i18n/index.svelte";

  let {
    sections,
    onRun,
    onClose,
  }: {
    sections: (query: string) => PaletteSection[];
    onRun: (entry: PaletteEntry) => void;
    onClose: () => void;
  } = $props();

  const label = $derived(t("frame.palette.field"));
  let query = $state("");
  const shown = $derived([...sections(query), languageSection(query)]);

  function focusOnMount(node: HTMLInputElement) {
    node.focus();
  }

  function paletteKeys(node: HTMLElement) {
    const handler = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    node.addEventListener("keydown", handler);
    return { destroy: () => node.removeEventListener("keydown", handler) };
  }
</script>

<div class="scrim" aria-hidden="true"></div>
<div class="palette" data-palette role="dialog" aria-modal="true" aria-label={t("frame.palette.dialog")} use:paletteKeys>
  <div class="field">
    <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
    </svg>
    <input type="text" bind:value={query} aria-label={label} placeholder={label} autocomplete="off" spellcheck="false" use:focusOnMount />
    <kbd>Esc</kbd>
  </div>
  <div class="results">
    {#each shown as section (section.id)}
      <section data-palette-section={section.id} aria-labelledby={`palette-${section.id}`}>
        <h2 id={`palette-${section.id}`} data-section-title>{section.title}</h2>
        {#if section.entries.length === 0}
          <p class="empty">{t("frame.palette.empty")}</p>
        {:else}
          <ul>
            {#each section.entries as entry (entry.id)}
              <li>
                <button
                  type="button"
                  class="entry"
                  data-entry-id={entry.id}
                  role={entry.checked === undefined ? undefined : "menuitemradio"}
                  aria-checked={entry.checked === undefined ? undefined : entry.checked}
                  onclick={() => onRun(entry)}
                >
                  <span class="label" data-label>{entry.label}</span>
                  {#if entry.detail}<span class="detail">{entry.detail}</span>{/if}
                  {#if entry.checked}<span class="check" data-entry-check aria-hidden="true">✓</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/each}
  </div>
</div>

<style>
  .scrim {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 50;
  }
  .palette {
    position: absolute;
    top: 64px;
    left: 50%;
    transform: translateX(-50%);
    width: min(620px, calc(100% - 32px));
    max-height: min(520px, calc(100% - 96px));
    display: flex;
    flex-direction: column;
    background: var(--surface-2);
    border: 1px solid var(--surface-3);
    border-radius: 12px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.6);
    color: var(--text);
    font-family: var(--font-ui);
    z-index: 51;
    overflow: hidden;
  }
  .field {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--surface-3);
    color: var(--text-muted);
  }
  .palette .field input[type="text"] {
    flex: 1;
    min-width: 0;
    font-size: 14px;
    background: var(--surface);
    border: 1px solid var(--surface-3);
    border-radius: 8px;
    color: var(--text);
    padding: 6px 8px;
  }
  .palette .field input[type="text"]:hover,
  .palette .field input[type="text"]:focus-visible {
    border-color: var(--surface-3);
  }
  kbd {
    font-family: var(--font-ui);
    font-size: 11px;
    color: var(--text-muted);
    background: var(--surface-3);
    border: 1px solid var(--surface-3);
    border-radius: 4px;
    padding: 0 5px;
  }
  .results {
    overflow-y: auto;
    padding: 6px;
  }
  h2 {
    margin: 8px 8px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.6px;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .entry {
    width: 100%;
    display: flex;
    align-items: baseline;
    gap: 10px;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 6px 8px;
  }
  .entry:hover,
  .entry:focus-visible {
    background: var(--surface-3);
  }
  .label {
    color: var(--text);
    font-size: 13px;
  }
  .check {
    margin-left: auto;
    color: var(--text-muted);
    font-size: 12px;
  }
  .detail {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-size: 12px;
  }
  .empty {
    margin: 0 8px 6px;
    font-size: 12px;
    color: var(--text-muted);
  }
</style>
