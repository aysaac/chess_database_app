<script lang="ts">
  import { formatEval, parseEval } from "../format";
  import { analysis, whiteChance } from "../stores/analysis.svelte";
  import { game } from "../stores/game.svelte";

  // The engine's current evaluation, else the one stored with the move
  // (`[%eval]`, e.g. from Lichess or a blunder check).
  const shown = $derived.by(() => {
    const line = analysis.current?.lines[0];
    if (line) return line.eval;
    const view = game.view;
    return view ? parseEval(game.evals.get(view.cursor)) : null;
  });
  const white = $derived(shown ? whiteChance(shown) : 0.5);
</script>

<div
  class="bar"
  class:flipped={game.orientation === "black"}
  class:empty={!shown}
  role="meter"
  aria-label="Evaluation"
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={Math.round(white * 100)}
  aria-valuetext={shown ? formatEval(shown) : "No evaluation"}
  title={shown ? `Evaluation ${formatEval(shown)}` : "No evaluation for this position"}
>
  {#if shown}
    <div class="white" style:height="{white * 100}%"></div>
    <span class="label" class:on-white={white >= 0.5}>{formatEval(shown).replace(/\.\d+$/, (d) => d.slice(0, 2))}</span>
  {/if}
</div>

<style>
  .bar {
    position: relative;
    width: 100%;
    height: 100%;
    overflow: hidden;
    border-radius: 2px;
    background: var(--eval-black);
  }

  .bar.empty {
    background: var(--panel);
    border: 1px solid var(--rule);
  }

  .white {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    background: var(--eval-white);
    transition: height 0.25s ease-out;
  }

  .flipped .white {
    top: 0;
    bottom: auto;
  }

  .label {
    position: absolute;
    left: 0;
    right: 0;
    top: 3px;
    font-size: 9px;
    font-weight: 600;
    text-align: center;
    color: var(--eval-black-ink);
    writing-mode: vertical-rl;
    transform: rotate(180deg);
    line-height: 1;
  }

  /* The label sits at the leader's end of the bar. */
  .label.on-white {
    top: auto;
    bottom: 3px;
    color: var(--eval-white-ink);
  }

  .flipped .label {
    top: auto;
    bottom: 3px;
  }

  .flipped .label.on-white {
    top: 3px;
    bottom: auto;
  }
</style>
