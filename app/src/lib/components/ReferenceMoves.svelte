<script lang="ts">
  // The Reference tab's moves table (OPEN-14 3a): the opening tree's
  // columns (shared cells) plus Hot and Best players. A move plays on the
  // main board, with the usual variation choice.
  import type { ReferenceDto } from "../bindings/ReferenceDto";
  import type { ReferenceMoveDto } from "../bindings/ReferenceMoveDto";
  import { figurine } from "../format";
  import { reference } from "../stores/reference.svelte";
  import StatsCells from "./StatsCells.svelte";
  import StatsHead from "./StatsHead.svelte";

  let { data }: { data: ReferenceDto } = $props();

  const pct = (share: number) => `${Math.round(share * 100)}%`;
  const years = $derived(
    data.recentFrom && data.recentTo ? `${data.recentFrom}–${data.recentTo}` : null,
  );

  function hotTitle(m: ReferenceMoveDto): string {
    const hot = m.hot;
    if (!hot || !years) return "";
    const trend = hot.trend === "up" ? "; played more lately" : hot.trend === "down" ? "; played less lately" : "";
    return `${pct(hot.share)} of games from ${years} (${pct(hot.allShare)} over all years)${trend}`;
  }
</script>

<table class="stats-table ref-moves">
  <thead>
    <tr>
      <th scope="col">Move</th>
      <StatsHead />
      <th
        scope="col"
        title={years
          ? `How often the move was played in ${years}, the last three years of the database`
          : "How often the move was played lately (no game here has a year)"}>Hot</th
      >
      <th scope="col" class="best-col" title="The strongest players who chose the move, at their Elo then"
        >Best players</th
      >
    </tr>
  </thead>
  <tbody>
    {#each data.moves as m (m.row.uci ?? "end")}
      <tr>
        <td>
          {#if m.row.uci && m.row.san}
            <button class="move" title="Play {m.row.san}" onclick={() => reference.play(m.row.uci!)}
              >{figurine(m.row.san)}</button
            >
          {:else}
            <span class="soft">Game ends</span>
          {/if}
        </td>
        <StatsCells row={m.row} />
        <td class="hot-col">
          {#if m.hot}
            <!-- Read out as its tooltip: "12% of games from 2016–2018 (7% over all years)…". -->
            <span class="hot" role="img" title={hotTitle(m)} aria-label={hotTitle(m)}>
              <span class="hot-bar"><span style:width={pct(m.hot.share)}></span></span>
              {#if m.hot.trend === "up"}
                <span class="trend up" aria-hidden="true">↑</span>
              {:else if m.hot.trend === "down"}
                <span class="trend down" aria-hidden="true">↓</span>
              {/if}
            </span>
          {/if}
        </td>
        <td class="best-col">
          {#each m.best as p, i (p.name)}
            <span class="player" title="{p.name} ({p.elo})">{p.surname}</span>{i < m.best.length - 1 ? ", " : ""}
          {/each}
        </td>
      </tr>
    {/each}
  </tbody>
  <tfoot>
    <tr>
      <th scope="row">Total</th>
      <StatsCells row={data.total} />
      <td></td>
      <td></td>
    </tr>
  </tfoot>
</table>

<style>
  .soft {
    color: var(--ink-soft);
  }

  .hot {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .hot-bar {
    display: inline-flex;
    width: 44px;
    height: 8px;
    border-radius: 2px;
    background: var(--panel);
    border: 1px solid var(--rule);
    overflow: hidden;
  }

  .hot-bar > span {
    background: var(--brass);
  }

  .trend {
    width: 1ch;
    font-weight: 700;
  }

  .trend.up {
    color: var(--felt);
  }

  .trend.down {
    color: var(--danger);
  }

  /* The result bar has a set width here (the tree lets it take the rest)… */
  .ref-moves :global(.bar-col) {
    width: 120px;
    min-width: 120px;
  }

  /* …and the names take what's left, cut off at the panel's edge (like
     the event in the top games). */
  .ref-moves .best-col {
    width: 100%;
    max-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--ink-soft);
  }

  /* A narrow tab (docked on a thin side) drops the bar and the year; the
     score stays. */
  @container reference (width < 480px) {
    .ref-moves :global(:is(.bar-col, .year)) {
      display: none;
    }
  }

  .player {
    color: var(--ink);
  }
</style>
