<script lang="ts">
  import { figurine } from "../format";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { tree } from "../stores/tree.svelte";
  import StatsCells from "./StatsCells.svelte";
  import StatsHead from "./StatsHead.svelte";

  // Only while the tab is showing (UI-19): query when it's shown, then again
  // when the position, the database or its index changes.
  $effect(() => {
    if (!tree.showing) return;
    void game.view?.fen;
    void game.view?.session;
    void db.info?.handle;
    void db.info?.index.covered;
    void db.info?.games;
    void db.reference?.info?.index.covered;
    void tree.target?.handle;
    const t = setTimeout(() => tree.refresh(), 60);
    return () => clearTimeout(t);
  });

  const data = $derived(tree.current);

  // The source picker: the open database or the reference database. The
  // reference itself is chosen in the ribbon (DB-13).
  const sourceValue = $derived(tree.target?.reference ? "reference" : "open");

  function onSource(e: Event) {
    const value = (e.currentTarget as HTMLSelectElement).value;
    if (value === "open" || value === "reference") tree.source = value;
  }
</script>

<!-- The Opening tree panel tab (UI-19) labels it. -->
<div class="tree">
  <header>
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
      <option value="reference" disabled={!db.reference?.info}
        >{db.reference?.info ? `${db.reference.name} (reference)` : "No reference database"}</option
      >
    </select>
    <button
      disabled={!tree.target || !game.view || db.searching}
      title={tree.target?.reference && tree.target.handle !== db.info?.handle
        ? `Open ${tree.target.name} in the game list with the games that reach this position`
        : "Show the games that reach this position in the game list"}
      onclick={() => tree.findGames()}>Find games</button
    >
  </header>

  {#if !tree.target}
    <p class="note">
      Open a database, or choose a reference database in the ribbon's Reference group, to see which moves
      were played here.
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
      <table class="stats-table">
        <thead>
          <tr>
            <th scope="col">Move</th>
            <StatsHead perf />
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
              <StatsCells row={r} perf />
            </tr>
          {/each}
        </tbody>
        <tfoot>
          <tr>
            <th scope="row">Total</th>
            <StatsCells row={data.total} perf />
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
</div>

<style>
  .tree {
    display: flex;
    flex-direction: column;
    height: 100%;
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
