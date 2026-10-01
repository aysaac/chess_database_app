<script lang="ts">
  // The header cells of a moves table's statistics, shared by the opening
  // tree (OPEN-01) and the Reference tab (OPEN-14). Styled by `.stats-table`
  // in app.css. `perf` adds the performance column.
  let { perf = false }: { perf?: boolean } = $props();
</script>

<th scope="col" class="num">Games</th>
<th scope="col" class="num" title="Score for the side to move">Score</th>
<th scope="col" class="bar-col">White · draw · Black</th>
<th scope="col" class="num" title="Average Elo of the players who chose the move">Elo</th>
{#if perf}<th scope="col" class="num" title="Performance rating with the move">Perf</th>{/if}
<th scope="col" class="num year" title="Last played">Year</th>
