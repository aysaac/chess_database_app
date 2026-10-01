<script lang="ts">
  import { game } from "../stores/game.svelte";
  import NextMovePicker from "./NextMovePicker.svelte";
  import VariationDialog from "./VariationDialog.svelte";

  // The variation popover and the next-move picker. They're drawn here, not
  // in the Notation tab, because a hidden tab hides everything in it, and a
  // move played from the Opening tree tab (UI-19) can ask for a variation
  // choice too.

  // Under the current move in the notation, or under the first move when
  // the cursor is at the start. With another tab showing in the Notation
  // tab's place, under that group's tab strip.
  function anchor(): HTMLElement {
    const pane = document.querySelector<HTMLElement>("[data-notation]");
    if (pane?.getClientRects().length) {
      return (
        pane.querySelector<HTMLElement>(`[data-node="${game.view?.cursor}"]`) ??
        pane.querySelector<HTMLElement>("[data-node]") ??
        pane
      );
    }
    return pane?.closest("[data-panel-group]")?.querySelector<HTMLElement>("[role='tablist']") ?? document.body;
  }
</script>

{#if game.pendingChoice}
  <VariationDialog anchor={anchor()} />
{/if}

{#if game.branch}
  <NextMovePicker moves={game.branch} anchor={anchor()} />
{/if}
