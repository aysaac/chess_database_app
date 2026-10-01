<script lang="ts">
  // The Reference tab's main variations (OPEN-14 3b): the main line, then
  // up to 12 lines grouped by first move. Clicking one shows it on the
  // mini board. Counting them reads every game reaching the position, so a
  // common position shows its progress (a job) and fills in when it's done.
  import type { ReferenceLineDto } from "../bindings/ReferenceLineDto";
  import { figurine, formatCount } from "../format";
  import { jobs } from "../stores/jobs.svelte";
  import { lineText, reference } from "../stores/reference.svelte";

  // The lines of the position on the board or, while they're counted
  // directly (not by a job), the last ones, dimmed and inert.
  const lines = $derived(reference.currentLines ?? reference.staleLines);
  const stale = $derived(!reference.currentLines && !!lines);
  const job = $derived(jobs.running.find((j) => j.id === reference.linesJob?.id) ?? null);
  const progress = $derived(job && job.total > 0 ? Math.floor((job.done / job.total) * 100) : 0);

  /** The mini board shows the line `key` (`main` or `line:<n>`). */
  const shown = (key: string) => reference.mini?.key === key;

  /** The first move of a line, to group them. */
  const first = (line: ReferenceLineDto) => line.positions[1]?.uci ?? "";
  const games = (n: number) => `${formatCount(n)} ${n === 1 ? "game" : "games"}`;
</script>

<div class="lines" class:stale aria-label="Main variations" aria-busy={stale} role="region" inert={stale}>
  <div class="main-line">
    {#if lines?.mainLine}
      {@const main = lines.mainLine}
      <button
        class="main"
        class:shown={shown("main")}
        title="Show the main line on the mini board"
        onclick={() => reference.selectLine(main, "main", "Main line")}
      >
        <span class="label">Main line:</span>
        <span class="moves">{figurine(lineText(main.positions))}</span>
        <span class="count">({games(main.games)})</span>
      </button>
    {:else if lines}
      <span class="soft"
        >No main line: no move here was played in at least 20 games and a tenth of the position's games.</span
      >
    {:else if reference.linesJob}
      <span class="soft" role="status"
        >Counting the lines of {formatCount(reference.linesJob.games)} games… {progress}%</span
      >
      <button class="link" onclick={() => jobs.cancel(reference.linesJob!.id)}>Cancel</button>
    {:else if reference.linesError}
      <span class="soft" role="status">{reference.linesError}</span>
      <button class="link" onclick={() => reference.retryLines()}>Count them again</button>
    {:else if reference.linesLoading}
      <span class="soft" role="status">Counting the lines…</span>
    {/if}
  </div>

  {#if lines && lines.lines.length}
    <div class="scroll">
      <table>
        <thead>
          <tr>
            <th scope="col" class="line-col">Line</th>
            <th scope="col" class="num" title="Games that followed the whole line">Games</th>
            <th scope="col" class="num" title="Score for the side to move now">Score</th>
            <th scope="col" class="num extra" title="Last played">Year</th>
            <th scope="col" class="num extra" title="Average Elo of both players">Elo</th>
          </tr>
        </thead>
        <tbody>
          {#each lines.lines as line, i (i)}
            <tr class:group={i > 0 && first(line) !== first(lines.lines[i - 1])} class:shown={shown(`line:${i}`)}>
              <td class="line-col">
                <button
                  class="line"
                  title="Show this line on the mini board"
                  onclick={() => reference.selectLine(line, `line:${i}`, `Line ${i + 1}`)}
                  >{figurine(lineText(line.positions))}</button
                >
              </td>
              <td class="num">{formatCount(line.games)}</td>
              <td class="num">{Math.round(line.score * 100)}%</td>
              <td class="num extra">{line.lastYear ?? ""}</td>
              <td class="num extra">{line.avgElo ?? ""}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .lines {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .lines.stale {
    opacity: 0.55;
  }

  .main-line {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 28px;
    padding: 3px 10px 3px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
    white-space: nowrap;
    overflow: hidden;
  }

  .main {
    display: flex;
    gap: 6px;
    min-width: 0;
    padding: 1px 6px;
    margin-left: -6px;
  }

  .main .label {
    color: var(--ink-soft);
  }

  .main .moves {
    overflow: hidden;
    text-overflow: ellipsis;
    font-family: var(--font-book), var(--font-figurine), serif;
    font-weight: 600;
  }

  .main .count {
    color: var(--ink-soft);
  }

  .main.shown,
  tr.shown .line {
    background: var(--felt-soft);
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 10px 0 14px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th,
  td {
    width: 1%;
    padding: 1px 10px 1px 0;
    text-align: left;
    white-space: nowrap;
  }

  thead th {
    position: sticky;
    top: 0;
    background: var(--paper);
    color: var(--ink-soft);
    font-weight: 500;
  }

  /* Lines starting with another first move begin a new group. */
  tr.group td {
    border-top: 1px solid var(--rule);
  }

  .line-col {
    width: auto;
    max-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .line {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    padding: 0 4px;
    margin-left: -4px;
    font-family: var(--font-book), var(--font-figurine), serif;
    font-size: 14px;
  }

  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  /* A narrow tab keeps the moves readable: the year and Elo go. */
  @container reference (width < 480px) {
    .extra {
      display: none;
    }
  }

  .soft {
    color: var(--ink-soft);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .link {
    padding: 0;
    color: var(--felt);
    text-decoration: underline;
  }
</style>
