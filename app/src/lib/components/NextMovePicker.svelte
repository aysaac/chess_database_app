<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { NotationMoveDto } from "../bindings/NotationMoveDto";
  import { figurine } from "../format";
  import { placeUnder } from "../popover";
  import { game } from "../stores/game.svelte";

  /** The next moves (main line first) and the move the list opens under. */
  let { moves, anchor }: { moves: NotationMoveDto[]; anchor: HTMLElement } = $props();

  let box: HTMLDivElement;
  let pos = $state<{ left: number; top: number } | null>(null);

  onMount(() => {
    pos = placeUnder(anchor, box);
    // After the position is applied: a hidden element can't take focus.
    tick().then(() => box?.focus());
  });

  // Keys stay here (not the window), so the board's arrow keys pause while
  // it's open: Up/Down choose, Right/Enter follow, Left/Escape close.
  function onKey(e: KeyboardEvent) {
    const n = moves.length;
    const selected = game.branchSelected;
    const actions: Record<string, () => unknown> = {
      ArrowDown: () => (game.branchSelected = (selected + 1) % n),
      ArrowUp: () => (game.branchSelected = (selected - 1 + n) % n),
      ArrowRight: () => game.chooseBranch(moves[selected].id),
      Enter: () => game.chooseBranch(moves[selected].id),
      ArrowLeft: () => game.chooseBranch(null),
      Escape: () => game.chooseBranch(null),
    };
    const action = actions[e.key];
    if (!action) return;
    e.preventDefault();
    e.stopPropagation();
    action();
  }
</script>

<div
  class="picker"
  role="listbox"
  tabindex="-1"
  aria-label="Next move"
  aria-activedescendant="next-move-{moves[game.branchSelected].id}"
  style:left="{pos?.left ?? 0}px"
  style:top="{pos?.top ?? 0}px"
  style:visibility={pos ? null : "hidden"}
  bind:this={box}
  onkeydown={onKey}
  onfocusout={() => game.chooseBranch(null)}
>
  {#each moves as m, i (m.id)}
    <!-- Keys are handled by the listbox (aria-activedescendant); options never take focus. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      id="next-move-{m.id}"
      class="option"
      class:main={i === 0}
      class:selected={i === game.branchSelected}
      role="option"
      aria-selected={i === game.branchSelected}
      tabindex="-1"
      onmouseenter={() => (game.branchSelected = i)}
      onmousedown={(e) => e.preventDefault()}
      onclick={() => game.chooseBranch(m.id)}
    >
      <span class="mark" aria-hidden="true">{i === game.branchSelected ? "→" : ""}</span><span class="number"
        >{m.moveNumber}{m.white ? "." : "…"}</span
      >{figurine(m.san)}
    </div>
  {/each}
</div>

<style>
  .picker {
    position: fixed;
    z-index: 100;
    min-width: 130px;
    padding: 4px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-top: 2px solid var(--felt);
    border-radius: 0 0 6px 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
    font-family: "Source Serif 4 Variable", var(--font-figurine), serif;
    font-size: 16px;
  }

  .picker:focus {
    outline: none;
  }

  .option {
    display: flex;
    align-items: baseline;
    padding: 3px 10px 3px 4px;
    border-radius: 3px;
    cursor: pointer;
  }

  .option.main {
    font-weight: 600;
  }

  .option.selected {
    background: var(--felt-soft);
  }

  .mark {
    width: 1.3em;
    color: var(--felt);
    font-family: var(--font-ui);
    font-weight: 600;
  }

  .number {
    margin-right: 3px;
    color: var(--ink-soft);
    font-variant-numeric: tabular-nums;
  }
</style>
