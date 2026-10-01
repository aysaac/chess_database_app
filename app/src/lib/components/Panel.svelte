<script lang="ts">
  import { analysis } from "../stores/analysis.svelte";
  import type { PanelId } from "../stores/panels.svelte";
  import AnalysisPane from "./AnalysisPane.svelte";
  import ListPane from "./ListPane.svelte";
  import NotationPanel from "./NotationPanel.svelte";
  import OpeningTree from "./OpeningTree.svelte";

  // What a panel (UI-19) shows. The Reference tab (OPEN-14) isn't built yet.
  let { id }: { id: PanelId } = $props();
</script>

{#if id === "notation"}
  <NotationPanel />
{:else if id === "tree"}
  <OpeningTree />
{:else if id === "analysis"}
  <!-- The engine's lines (ENG-02), while it runs. -->
  {#if analysis.status.running || analysis.starting}<AnalysisPane />{/if}
{:else if id === "games"}
  <ListPane />
{/if}
