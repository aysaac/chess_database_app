<script lang="ts">
  import { formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import HeaderSearchDialog from "./HeaderSearchDialog.svelte";

  let dialog = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  // Filter as you type, once typing pauses.
  function onInput(e: Event) {
    const text = (e.currentTarget as HTMLInputElement).value;
    db.quickText = text;
    clearTimeout(timer);
    timer = setTimeout(() => (text.trim() ? db.quickFilter(text) : db.clearFilter()), 300);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" && db.quickText) {
      e.stopPropagation();
      clearTimeout(timer);
      db.clearFilter();
    } else if (e.key === "Enter") {
      clearTimeout(timer);
      if (db.quickText.trim()) db.quickFilter(db.quickText);
    }
  }
</script>

<div class="bar">
  <input
    type="search"
    class="quick"
    placeholder="Filter by player, event or site"
    aria-label="Filter games by player, event or site"
    spellcheck="false"
    value={db.quickText}
    oninput={onInput}
    onkeydown={onKey}
  />
  <button onclick={() => (dialog = true)}>Search…</button>
  {#if db.info?.filter}
    <span class="status" title={db.info.filter.description}>
      <strong>{formatCount(db.info.filter.count)}</strong> of {formatCount(db.info.games)} games ·
      {db.info.filter.description}
    </span>
    <button onclick={() => db.clearFilter()}>Show all games</button>
  {:else if db.searching}
    <span class="status">Searching…</span>
  {/if}
</div>

{#if dialog}
  <HeaderSearchDialog onClose={() => (dialog = false)} />
{/if}

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--rule);
    min-width: 0;
  }

  .quick {
    width: 260px;
    padding: 3px 8px;
  }

  .bar button {
    padding: 2px 9px;
    border-color: var(--rule);
  }

  .status {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
  }

  .status strong {
    color: var(--felt);
  }
</style>
