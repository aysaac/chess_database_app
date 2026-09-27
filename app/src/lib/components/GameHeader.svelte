<script lang="ts">
  import type { HeadersDto } from "../bindings/HeadersDto";
  import { playerLabel, resultLabel } from "../format";
  import { game } from "../stores/game.svelte";
  import HeaderEditor from "./HeaderEditor.svelte";

  let editing = $state(false);

  const view = $derived(game.view);
  const h = $derived(view?.headers);

  const details = $derived.by(() => {
    if (!h) return "";
    const round = h.round ? `round ${h.round}` : "";
    return [h.event, h.site, h.date, round, h.eco].filter(Boolean).join(", ");
  });

  const status = $derived.by(() => {
    if (!view) return null;
    const side = view.winner === "white" ? "White" : "Black";
    switch (view.status) {
      case "checkmate":
        return `${side} wins by checkmate`;
      case "stalemate":
        return "Draw by stalemate";
      case "insufficientMaterial":
        return "Draw by insufficient material";
      case "fiftyMoves":
        return "Fifty moves without a capture or pawn move: a draw can be claimed";
      case "threefold":
        return "Third repetition of this position: a draw can be claimed";
      default:
        return null;
    }
  });
</script>

{#if view && h}
  <header>
    <div class="title">
      <h1>
        {#if !h.white && !h.black}
          New game
        {:else}
          {playerLabel(h.white, h.whiteElo)}
          <span class="vs">vs</span>
          {playerLabel(h.black, h.blackElo)}
        {/if}
        {#if resultLabel(h.result)}<span class="result">{resultLabel(h.result)}</span>{/if}
      </h1>
      <button onclick={() => (editing = true)}>Edit details</button>
    </div>
    {#if view.opening}
      <p class="opening">{view.opening.eco} · {view.opening.name}</p>
    {/if}
    <p class="details">
      {details || "No event details yet."}
      {#if view.dirty}<span class="dirty">Unsaved changes</span>{/if}
    </p>
    {#if status}<p class="status">{status}</p>{/if}
  </header>
{/if}

{#if editing && h}
  <HeaderEditor
    headers={h}
    onSave={(next: HeadersDto) => {
      editing = false;
      game.setHeaders(next);
    }}
    onCancel={() => (editing = false)}
  />
{/if}

<style>
  header {
    padding: 12px 18px 10px;
    border-bottom: 1px solid var(--rule);
  }

  .title {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
  }

  h1 {
    margin: 0;
    font-family: var(--font-book);
    font-size: 21px;
    font-weight: 600;
    line-height: 1.25;
  }

  .vs {
    font-weight: 400;
    font-style: italic;
    color: var(--ink-soft);
    margin: 0 4px;
  }

  .result {
    margin-left: 10px;
    font-weight: 400;
    color: var(--felt);
  }

  .details {
    margin: 3px 0 0;
    color: var(--ink-soft);
  }

  .opening {
    margin: 3px 0 0;
    font-family: var(--font-book);
    font-style: italic;
  }

  .dirty {
    margin-left: 10px;
    padding: 1px 7px;
    border-radius: 10px;
    background: var(--brass-soft);
    color: var(--brass);
    font-weight: 600;
    font-size: 12px;
  }

  .status {
    margin: 8px 0 0;
    padding: 5px 10px;
    border-left: 3px solid var(--felt);
    background: var(--felt-soft);
    font-weight: 600;
  }
</style>
