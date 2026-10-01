<script lang="ts">
  import EvalGraph from "./EvalGraph.svelte";
  import GameHeader from "./GameHeader.svelte";
  import Notation from "./Notation.svelte";

  // The Notation panel tab (UI-19): the game's header, its moves and, at the
  // bottom, the evaluation graph (ANL-04), which shows once at least two
  // moves have an evaluation.
</script>

<div class="notation-panel">
  <GameHeader />
  <div class="moves"><Notation /></div>
  <EvalGraph />
</div>

<style>
  .notation-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .moves {
    flex: 1;
    min-height: 0;
  }
</style>
