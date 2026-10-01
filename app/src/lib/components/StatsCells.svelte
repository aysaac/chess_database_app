<script lang="ts">
  // The statistics cells of a moves table row (see StatsHead.svelte): games,
  // score for the side to move, the White/draw/Black bar, average Elo,
  // performance (with `perf`) and the last year played.
  import type { TreeRowDto } from "../bindings/TreeRowDto";
  import { formatCount } from "../format";

  let { row, perf = false }: { row: TreeRowDto; perf?: boolean } = $props();

  const pct = (n: number, of: number) => (of ? Math.round((n / of) * 100) : 0);
  const title = $derived(
    `White won ${formatCount(row.whiteWins)}, drew ${formatCount(row.draws)}, Black won ${formatCount(row.blackWins)}`,
  );
</script>

<td class="num">{formatCount(row.games)}</td>
<td class="num">{Math.round(row.score * 100)}%</td>
<td class="bar-col">
  <span class="bar" {title}>
    <span class="w" style:width="{pct(row.whiteWins, row.games)}%"></span><span
      class="d"
      style:width="{pct(row.draws, row.games)}%"
    ></span><span class="b" style:width="{pct(row.blackWins, row.games)}%"></span>
  </span>
</td>
<td class="num">{row.avgElo ?? ""}</td>
{#if perf}<td class="num">{row.performance ?? ""}</td>{/if}
<td class="num year">{row.lastYear ?? ""}</td>
