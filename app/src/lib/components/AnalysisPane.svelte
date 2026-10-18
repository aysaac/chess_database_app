<script lang="ts">
  import type { PvLineDto } from "../bindings/PvLineDto";
  import { figurine, formatCount, formatEval, formatNps } from "../format";
  import { analysis } from "../stores/analysis.svelte";
  import { game } from "../stores/game.svelte";

  const current = $derived(analysis.current);
  const status = $derived(analysis.status);
  const wdl = $derived(current?.lines[0]?.wdl ?? null);
  const threat = $derived(analysis.threat);
  // Who would pass, and who gets the extra move (ENG-07).
  const mover = $derived(game.view?.turn === "black" ? "Black" : "White");
  const opponent = $derived(mover === "White" ? "Black" : "White");

  function boundMark(line: PvLineDto): string {
    return line.bound === "lower" ? "≥" : line.bound === "upper" ? "≤" : "";
  }

  const pct = (permille: number) => `${Math.round(permille / 10)}%`;
  const blackAhead = (line: PvLineDto) =>
    line.eval.mate !== null ? line.eval.mate < 0 : (line.eval.cp ?? 0) < 0;
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
    <button
      class="toggle"
      aria-pressed={analysis.showThreat}
      title="Show what the opponent threatens: the engine also looks at the position as if the side to move passed"
      onclick={() => analysis.setShowThreat(!analysis.showThreat)}>Show threat</button
    >
    <button class="stop" title="Stop the engine (Ctrl+Shift+A)" onclick={() => analysis.stop()}>Stop</button>
  </header>

  <ol>
    {#each current?.lines ?? [] as line (line.multipv)}
      <li>
        <span class="eval" class:ahead-black={blackAhead(line)}>{boundMark(line)}{formatEval(line.eval)}</span
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
  {#if analysis.showThreat}
    <!-- ENG-07: the opponent's best line had the side to move passed. -->
    <p class="threat">
      <span class="label">Threat</span>
      {#if threat?.error}
        <span class="note"
          >{threat.startFailed ? "The threat engine couldn't start." : "The threat search stopped."} Turn
          "Show threat" off and on to retry.</span
        >
      {:else if threat?.unavailable === "inCheck"}
        <span class="note">None while {mover} is in check: passing isn't legal.</span>
      {:else if threat?.unavailable === "gameOver"}
        <span class="note">None: the game is over.</span>
      {:else if threat?.unavailable === "noMoves"}
        <span class="note">None: {opponent} would have no legal move.</span>
      {:else if threat?.line}
        {@const line = threat.line}
        <span class="eval" class:ahead-black={blackAhead(line)}>{boundMark(line)}{formatEval(line.eval)}</span>
        <span class="pv"
          >{#each line.moves as m, i (i)}<span class="tmove" class:first={i === 0}
              >{#if m.white || i === 0}<span class="num">{m.moveNumber}{m.white ? "." : "…"}</span>{/if}{figurine(
                m.san,
              )}</span
            >{" "}{/each}</span
        >
        <span class="depth">depth {line.depth}</span>
      {:else}
        <span class="note">Looking…</span>
      {/if}
    </p>
  {/if}
</section>

<style>
  /* Under its own tab (UI-04), filling its group. */
  .analysis {
    min-height: 100%;
    background: var(--panel);
    font-size: 13px;
  }

  /* Wraps when docked on a narrow side (UI-04). */
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
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

  .stop,
  .toggle {
    border-color: var(--rule);
    padding: 1px 9px;
  }

  .toggle[aria-pressed="true"] {
    background: var(--felt-soft);
    border-color: var(--felt);
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

  /* ENG-07: one line, marked in the colour of the threat arrow on the board. */
  .threat {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-height: 24px;
    margin: 0 10px 6px 14px;
    padding: 0 0 0 8px;
    border-left: 3px solid var(--danger);
  }

  .threat .label {
    flex: none;
    color: var(--danger);
    font-weight: 600;
  }

  .threat .note,
  .threat .depth {
    color: var(--ink-soft);
  }

  .threat .depth {
    flex: none;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .tmove {
    padding: 0 2px;
  }

  .tmove.first {
    font-weight: 600;
  }
</style>
