<script lang="ts">
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import BoardSettings from "./BoardSettings.svelte";

  let settings = $state(false);

  const path = $derived(game.view?.cursorPath ?? []);
  const atStart = $derived(path.length === 0);
</script>

<div class="nav" role="toolbar" aria-label="Move navigation">
  <button aria-label="First move (Home)" title="Home" disabled={atStart} onclick={() => game.step("start")}>
    <svg viewBox="0 0 16 16"><path d="M3 3h2v10H3zM13 3v10L6 8z" /></svg>
  </button>
  <button aria-label="Previous move (Left arrow)" title="Left arrow" disabled={atStart} onclick={() => game.step("back")}>
    <svg viewBox="0 0 16 16"><path d="M11 3v10L4 8z" /></svg>
  </button>
  <button aria-label="Next move (Right arrow)" title="Right arrow" onclick={() => game.forward()}>
    <svg viewBox="0 0 16 16"><path d="M5 3v10l7-5z" /></svg>
  </button>
  <button aria-label="Last move (End)" title="End" onclick={() => game.step("end")}>
    <svg viewBox="0 0 16 16"><path d="M3 3v10l7-5zM11 3h2v10h-2z" /></svg>
  </button>
  <span class="sep"></span>
  <button
    aria-label="Previous game in the list (Ctrl+Up)"
    title="Previous game in the list (Ctrl+Up)"
    disabled={game.listIndex === null || game.listIndex === 0}
    onclick={() => game.openAdjacent(-1)}
  >
    <svg viewBox="0 0 16 16"><path d="M8 3l5 6H3zM3 11h10v2H3z" /></svg>
  </button>
  <button
    aria-label="Next game in the list (Ctrl+Down)"
    title="Next game in the list (Ctrl+Down)"
    disabled={game.listIndex === null || game.listIndex >= db.listTotal - 1}
    onclick={() => game.openAdjacent(1)}
  >
    <svg viewBox="0 0 16 16"><path d="M3 3h10v2H3zM8 13l-5-6h10z" /></svg>
  </button>
  <span class="sep"></span>
  <button title="Flip board (Ctrl+F)" onclick={() => game.flip()}>Flip board</button>
  <button aria-label="Board appearance" title="Board appearance" onclick={() => (settings = true)}>
    <svg viewBox="0 0 16 16"
      ><path d="M2 2h6v6H2zM8 8h6v6H8z" /><path d="M8 2h6v6H8zM2 8h6v6H2z" opacity="0.35" /></svg
    >
  </button>
</div>

{#if settings}
  <BoardSettings onClose={() => (settings = false)} />
{/if}

<style>
  .nav {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    height: 40px;
  }

  .nav button {
    display: grid;
    place-items: center;
    min-width: 40px;
    height: 30px;
    border-color: var(--rule);
  }

  svg {
    width: 14px;
    height: 14px;
    fill: currentColor;
  }

  .sep {
    width: 12px;
  }
</style>
