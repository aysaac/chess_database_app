<script lang="ts">
  import type { TreeRowDto } from "../bindings/TreeRowDto";
  import { figurine, formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { tree } from "../stores/tree.svelte";

  // Refresh when the position, the database or its index changes.
  $effect(() => {
    void game.view?.fen;
    void game.view?.session;
    void db.info?.handle;
    void db.info?.index.covered;
    void db.info?.games;
    void db.reference?.info?.index.covered;
    void tree.target?.handle;
    void tree.visible;
    const t = setTimeout(() => tree.refresh(), 60);
    return () => clearTimeout(t);
  });

  const data = $derived(tree.data?.fen === game.view?.fen ? tree.data : null);
  const pct = (n: number, of: number) => (of ? Math.round((n / of) * 100) : 0);

  // The source picker: the open database, the reference database, and
  // actions that set the reference (they don't stay selected).
  const sourceValue = $derived(tree.target?.reference ? "reference" : "open");

  async function onSource(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = select.value;
    select.value = sourceValue;
    if (value === "open" || value === "reference") {
      tree.source = value;
    } else if (value === "choose") {
      await db.chooseReference();
      if (db.reference?.info) tree.source = "reference";
    } else if (value === "use-open" && db.info) {
      await db.setReference(db.info.path);
      if (db.reference?.info) tree.source = "reference";
    } else if (value === "clear") {
      await db.setReference(null);
      tree.source = "open";
    }
  }

  function scoreTitle(r: TreeRowDto) {
    return `White won ${formatCount(r.whiteWins)}, drew ${formatCount(r.draws)}, Black won ${formatCount(r.blackWins)}`;
  }
</script>

<section class="tree" aria-label="Opening tree">
  <header>
    <strong>Opening tree</strong>
    <span class="name">
      {#if data?.opening}{data.opening.eco} {data.opening.name}{/if}
    </span>
    <select
      aria-label="Database the tree reads"
      title="The database the tree reads: the open one, or your reference database"
      value={sourceValue}
      onchange={onSource}
    >
      <option value="open" disabled={!db.info}>{db.info?.name ?? "No database open"}</option>
      {#if db.reference?.info}
        <option value="reference">{db.reference.name} (reference)</option>
      {/if}
      <option value="choose">Choose reference database…</option>
      {#if db.info && db.reference?.info?.handle !== db.info.handle}
        <option value="use-open">Use {db.info.name} as reference</option>
      {/if}
      {#if db.reference}
        <option value="clear">Stop using a reference database</option>
      {/if}
    </select>
    <button
      disabled={!tree.target || !game.view || db.searching}
      title={tree.target?.reference && tree.target.handle !== db.info?.handle
        ? `Open ${tree.target.name} in the game list with the games that reach this position`
        : "Show the games that reach this position in the game list"}
      onclick={() => tree.findGames()}>Find games</button
    >
    <button aria-label="Close the opening tree" title="Close" onclick={() => (tree.visible = false)}
      >×</button
    >
  </header>

  {#if !tree.target}
    <p class="note">
      Open a database, or choose a reference database above, to see which moves were played here.
    </p>
  {:else if data && !data.indexed}
    <p class="note">
      The database's positions are being indexed; the tree fills in when that's done.
    </p>
  {:else if data && data.total.games === 0}
    <p class="note">
      No game in {tree.target.name} reaches this position{data.beyondIndex
        ? ` within the first ${data.maxPly / 2} moves (the indexed part of each game)`
        : ""}.
      {#if data.beyondIndex && game.view}
        <button class="link" onclick={() => tree.findGames(true)}>Search every move of every game</button>
      {/if}
    </p>
  {:else if data}
    <div class="scroll">
      <table>
        <thead>
          <tr>
            <th scope="col">Move</th>
            <th scope="col" class="num">Games</th>
            <th scope="col" class="num" title="Score for the side to move">Score</th>
            <th scope="col" class="bar-col">White · draw · Black</th>
            <th scope="col" class="num" title="Average Elo of the players who chose the move"
              >Elo</th
            >
            <th scope="col" class="num" title="Performance rating with the move">Perf</th>
            <th scope="col" class="num" title="Last played">Year</th>
          </tr>
        </thead>
        <tbody>
          {#each data.moves as r (r.uci ?? "end")}
            <tr>
              <td>
                {#if r.uci && r.san}
                  <button class="move" title="Play {r.san}" onclick={() => tree.play(r.uci!)}
                    >{figurine(r.san)}</button
                  >
                {:else}
                  <span class="soft">Game ends</span>
                {/if}
              </td>
              <td class="num">{formatCount(r.games)}</td>
              <td class="num">{Math.round(r.score * 100)}%</td>
              <td class="bar-col">
                <span class="bar" title={scoreTitle(r)}>
                  <span class="w" style:width="{pct(r.whiteWins, r.games)}%"></span><span
                    class="d"
                    style:width="{pct(r.draws, r.games)}%"
                  ></span><span class="b" style:width="{pct(r.blackWins, r.games)}%"></span>
                </span>
              </td>
              <td class="num">{r.avgElo ?? ""}</td>
              <td class="num">{r.performance ?? ""}</td>
              <td class="num">{r.lastYear ?? ""}</td>
            </tr>
          {/each}
        </tbody>
        <tfoot>
          <tr>
            <th scope="row">Total</th>
            <td class="num">{formatCount(data.total.games)}</td>
            <td class="num">{Math.round(data.total.score * 100)}%</td>
            <td class="bar-col">
              <span class="bar" title={scoreTitle(data.total)}>
                <span class="w" style:width="{pct(data.total.whiteWins, data.total.games)}%"
                ></span><span class="d" style:width="{pct(data.total.draws, data.total.games)}%"
                ></span><span class="b" style:width="{pct(data.total.blackWins, data.total.games)}%"
                ></span>
              </span>
            </td>
            <td class="num">{data.total.avgElo ?? ""}</td>
            <td class="num">{data.total.performance ?? ""}</td>
            <td class="num">{data.total.lastYear ?? ""}</td>
          </tr>
        </tfoot>
      </table>
    </div>
    {#if data.partial}
      <p class="note">Recently added games are still being indexed and aren't counted yet.</p>
    {/if}
  {:else if tree.loading}
    <p class="note">Looking up the position…</p>
  {/if}
</section>

<style>
  .tree {
    display: flex;
    flex-direction: column;
    height: 230px;
    border-top: 1px solid var(--rule);
    font-size: 13px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 10px 4px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
  }

  header button {
    padding: 1px 9px;
    border-color: var(--rule);
  }

  header select {
    max-width: 16em;
    min-width: 0;
    padding: 1px 4px;
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

  /* Columns shrink to their content; the result bar takes the rest. */
  th,
  td {
    width: 1%;
    padding: 2px 10px 2px 0;
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

  tfoot th,
  tfoot td {
    border-top: 1px solid var(--rule);
    font-weight: 600;
  }

  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .move {
    padding: 0 4px;
    font-family: "Source Serif 4 Variable", var(--font-figurine), serif;
    font-size: 15px;
    font-weight: 600;
  }

  .bar-col {
    width: auto;
    min-width: 90px;
  }

  .bar {
    display: flex;
    height: 10px;
    border-radius: 2px;
    overflow: hidden;
    border: 1px solid var(--rule);
  }

  .w {
    background: var(--eval-white);
  }

  .d {
    background: color-mix(in srgb, var(--eval-white) 45%, var(--eval-black));
  }

  .b {
    background: var(--eval-black);
  }

  .note {
    margin: 10px 14px;
    color: var(--ink-soft);
  }

  .soft {
    color: var(--ink-soft);
  }

  .link {
    padding: 0;
    color: var(--felt);
    text-decoration: underline;
  }
</style>
