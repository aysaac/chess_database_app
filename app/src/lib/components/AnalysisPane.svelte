<script lang="ts">
  import type { PvLineDto } from "../bindings/PvLineDto";
  import { figurine, formatCount, formatEval, formatNps } from "../format";
  import { analysis } from "../stores/analysis.svelte";

  const current = $derived(analysis.current);
  const status = $derived(analysis.status);
  const wdl = $derived(current?.lines[0]?.wdl ?? null);

  function boundMark(line: PvLineDto): string {
    return line.bound === "lower" ? "≥" : line.bound === "upper" ? "≤" : "";
  }

  const pct = (permille: number) => `${Math.round(permille / 10)}%`;
</script>

<section class="analysis" aria-label="Engine analysis">
  <header>
    <strong>{status.engineName ?? "Engine"}</strong>
    <span class="stats">
      {#if current}
        depth {current.depth}{current.seldepth ? `/${current.seldepth}` : ""} · {formatNps(current.nps)} ·
        {formatCount(current.nodes)} nodes
      {:else if analysis.starting}
        Starting the engine…
      {:else if status.running}
        Thinking…
      {/if}
    </span>
    <span class="lines" role="group" aria-label="Number of lines">
      <button
        aria-label="Show fewer lines"
        disabled={status.multipv <= 1}
        onclick={() => analysis.setLines(status.multipv - 1)}>−</button
      >
      <span>{status.multipv} {status.multipv === 1 ? "line" : "lines"}</span>
      <button
        aria-label="Show more lines"
        disabled={status.multipv >= 5}
        onclick={() => analysis.setLines(status.multipv + 1)}>+</button
      >
    </span>
    <button class="stop" title="Stop the engine (Ctrl+Shift+A)" onclick={() => analysis.stop()}>Stop</button>
  </header>

  <ol>
    {#each current?.lines ?? [] as line (line.multipv)}
      <li>
        <span class="eval" class:ahead-black={line.eval.mate !== null ? line.eval.mate < 0 : (line.eval.cp ?? 0) < 0}
          >{boundMark(line)}{formatEval(line.eval)}</span
        >
        <span class="pv"
          >{#each line.moves as m, i (i)}<button
              class="pvmove"
              title="Play up to this move"
              onclick={() => analysis.playLine(line, i)}
              >{#if m.white || i === 0}<span class="num">{m.moveNumber}{m.white ? "." : "…"}</span>{/if}{figurine(
                m.san,
              )}</button
            >{" "}{/each}</span
        >
        <button
          class="add"
          title="Add this line to the notation as a variation"
          onclick={() => analysis.addLine(line)}>Add line</button
        >
      </li>
    {/each}
  </ol>
  {#if wdl}
    <p class="wdl">White wins {pct(wdl[0])} · draw {pct(wdl[1])} · Black wins {pct(wdl[2])}</p>
  {/if}
</section>

<style>
  .analysis {
    border-top: 1px solid var(--rule);
    background: var(--panel);
    font-size: 13px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 10px 3px 14px;
  }

  .stats {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
    font-variant-numeric: tabular-nums;
  }

  .lines {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--ink-soft);
  }

  .lines button {
    padding: 0 7px;
    border-color: var(--rule);
  }

  .stop {
    border-color: var(--rule);
    padding: 1px 9px;
  }

  ol {
    margin: 0;
    padding: 0 10px 4px 14px;
    list-style: none;
  }

  li {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-height: 24px;
  }

  .eval {
    flex: none;
    min-width: 54px;
    padding: 0 5px;
    border-radius: 3px;
    background: var(--eval-white);
    color: var(--eval-white-ink);
    font-weight: 600;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .eval.ahead-black {
    background: var(--eval-black);
    color: var(--eval-black-ink);
  }

  .pv {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-family: "Source Serif 4 Variable", var(--font-figurine), serif;
    font-size: 15px;
  }

  .pvmove {
    display: inline;
    padding: 0 2px;
    border: 0;
    font: inherit;
  }

  .num {
    color: var(--ink-soft);
    margin-right: 2px;
  }

  .add {
    flex: none;
    padding: 0 8px;
    border-color: var(--rule);
    font-size: 12px;
  }

  .wdl {
    margin: 0;
    padding: 0 14px 6px;
    color: var(--ink-soft);
    font-size: 12px;
  }
</style>
