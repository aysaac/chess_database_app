<script lang="ts">
  import { analysis } from "../stores/analysis.svelte";
  import { game } from "../stores/game.svelte";
  import type { PanelId } from "../stores/panels.svelte";
  import AnalysisPane from "./AnalysisPane.svelte";
  import ListPane from "./ListPane.svelte";
  import NotationPanel from "./NotationPanel.svelte";
  import OpeningTree from "./OpeningTree.svelte";
  import ReferencePanel from "./ReferencePanel.svelte";

  // What a panel (UI-19) shows.
  let { id }: { id: PanelId } = $props();
</script>

{#if id === "notation"}
  <NotationPanel />
{:else if id === "tree"}
  <OpeningTree />
{:else if id === "reference"}
  <!-- The Reference tab (OPEN-14). -->
  <ReferencePanel />
{:else if id === "analysis"}
  <!-- The engine's lines (ENG-02), while it runs. Its tab stays (UI-04: it
       can be dragged), with a line saying the engine is off. -->
  {#if analysis.status.running || analysis.starting}
    <AnalysisPane />
  {:else}
    <p class="engine-off">
      <span>The engine is off.</span>
      <button title="Start the engine (Ctrl+Shift+A)" disabled={!game.view} onclick={() => analysis.toggle()}
        >Analyze</button
      >
    </p>
  {/if}
{:else if id === "games"}
  <ListPane />
{/if}

<style>
  .engine-off {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 10px;
    margin: 0;
    padding: 4px 10px 4px 14px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  .engine-off button {
    padding: 1px 9px;
    border-color: var(--rule);
    color: var(--ink);
  }
</style>
