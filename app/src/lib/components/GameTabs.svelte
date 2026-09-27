<script lang="ts">
  import type { GameTab } from "../stores/game.svelte";
  import { game } from "../stores/game.svelte";

  // Open games as tabs (UI-05).
  function title(t: GameTab): string {
    const h = t.view.headers;
    if (h.white || h.black) {
      const last = (n: string) => (n ? n.split(",")[0] : "?");
      return `${last(h.white)} – ${last(h.black)}`;
    }
    return t.view.fen.startsWith("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR") || t.notation?.moves.length
      ? "New game"
      : "New game from a position";
  }

  function onKey(e: KeyboardEvent) {
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    game.cycleTab(step);
    requestAnimationFrame(() =>
      (e.currentTarget as HTMLElement | null)?.querySelector<HTMLElement>("[aria-selected='true']")?.focus(),
    );
  }
</script>

<div class="tabs" role="tablist" aria-label="Open games" tabindex="-1" onkeydown={onKey}>
  {#each game.tabs as t (t.session)}
    {@const active = t.session === game.activeSession}
    <div class="tab" class:active>
      <button
        role="tab"
        aria-selected={active}
        tabindex={active ? 0 : -1}
        title={title(t) + (t.view.dirty ? " (unsaved changes)" : "")}
        onclick={() => game.activate(t.session)}
        onauxclick={(e) => e.button === 1 && game.closeTab(t.session)}
      >
        {#if t.view.dirty}<span class="dirty" aria-label="unsaved changes">●</span>{/if}
        <span class="label">{title(t)}</span>
      </button>
      <button class="close" aria-label="Close {title(t)} (Ctrl+W)" title="Close (Ctrl+W)" onclick={() => game.closeTab(t.session)}
        >×</button
      >
    </div>
  {/each}
  <button class="new" aria-label="New game in a new tab (Ctrl+T)" title="New game in a new tab (Ctrl+T)" onclick={() => game.newGame(null, "tab")}
    >+</button
  >
</div>

<style>
  .tabs {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    padding: 4px 8px 0;
    border-bottom: 1px solid var(--rule);
    overflow-x: auto;
    scrollbar-width: thin;
  }

  .tab {
    display: flex;
    align-items: center;
    max-width: 220px;
    min-width: 0;
    border: 1px solid var(--rule);
    border-bottom: 0;
    border-radius: 5px 5px 0 0;
    background: var(--panel);
  }

  .tab.active {
    background: var(--paper);
    box-shadow: inset 0 2px 0 var(--felt);
  }

  .tab button[role="tab"] {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    padding: 3px 4px 3px 10px;
    border: 0;
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tab.active .label {
    font-weight: 600;
  }

  .dirty {
    color: var(--brass);
    font-size: 10px;
  }

  .close {
    padding: 0 7px;
    border: 0;
    color: var(--ink-soft);
  }

  .new {
    margin-bottom: 2px;
    padding: 0 9px;
    color: var(--ink-soft);
  }
</style>
