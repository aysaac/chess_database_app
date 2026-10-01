<script lang="ts">
  import { board, IMAGE_SIZES } from "../stores/board.svelte";
  import { game } from "../stores/game.svelte";
  import RibbonDropdown from "./RibbonDropdown.svelte";

  // Board images (QOL-06): the position on screen, with its highlights and
  // drawings, copied or saved as PNG or SVG. In the ribbon's Clipboard group.
  let { compact = false }: { compact?: boolean } = $props();
</script>

<RibbonDropdown
  class="image-menu"
  icon="image"
  label="Board image"
  title="Copy or save the board as a picture"
  menuLabel="Board image"
  {compact}
  disabled={!game.view}
>
  <button role="menuitem" onclick={() => game.copyBoardImage("png")}>Copy as PNG</button>
  <button role="menuitem" onclick={() => game.copyBoardImage("svg")}>Copy as SVG</button>
  <button role="menuitem" onclick={() => game.saveBoardImage()}>Save as file…</button>
  <div class="sep" role="separator"></div>
  <div class="sizes" role="group" aria-label="Image size">
    {#each IMAGE_SIZES as o (o.size)}
      <!-- Picking a size keeps the menu open, to copy right after. -->
      <button
        role="menuitemradio"
        data-keep-open
        aria-checked={board.imageSize === o.size}
        onclick={() => (board.imageSize = o.size)}
      >
        <span class="mark" aria-hidden="true">{board.imageSize === o.size ? "✓" : ""}</span>
        {o.label}
        <span class="px">{o.size} px</span>
      </button>
    {/each}
  </div>
</RibbonDropdown>

<style>
  .sep {
    margin: 4px 2px;
    border-top: 1px solid var(--rule);
  }

  .sizes {
    display: grid;
  }

  .sizes button {
    display: flex;
    align-items: baseline;
    gap: 4px;
  }

  .mark {
    width: 14px;
    color: var(--felt);
  }

  .px {
    margin-left: auto;
    color: var(--ink-soft);
    font-size: 12px;
  }
</style>
