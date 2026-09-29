<script lang="ts">
  import { board, IMAGE_SIZES } from "../stores/board.svelte";
  import { game } from "../stores/game.svelte";

  // Board images (QOL-06): the position on screen, with its highlights and
  // drawings, copied or saved as PNG or SVG.
  let open = $state(false);
  let root: HTMLDivElement;
  let trigger: HTMLButtonElement;

  function close() {
    open = false;
    trigger.focus();
  }

  function run(action: () => unknown) {
    close();
    action();
  }

  function onWindowDown(e: MouseEvent) {
    if (open && !root.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onmousedown={onWindowDown} onkeydown={(e) => open && e.key === "Escape" && close()} />

<div class="image-menu" bind:this={root}>
  <button
    bind:this={trigger}
    aria-haspopup="menu"
    aria-expanded={open}
    disabled={!game.view}
    title="Copy or save the board as a picture"
    onclick={() => (open = !open)}
  >
    Board image ▾
  </button>
  {#if open}
    <div class="menu" role="menu" aria-label="Board image">
      <button role="menuitem" onclick={() => run(() => game.copyBoardImage("png"))}>Copy as PNG</button>
      <button role="menuitem" onclick={() => run(() => game.copyBoardImage("svg"))}>Copy as SVG</button>
      <button role="menuitem" onclick={() => run(() => game.saveBoardImage())}>Save as file…</button>
      <div class="sep" role="separator"></div>
      <div class="sizes" role="group" aria-label="Image size">
        {#each IMAGE_SIZES as o (o.size)}
          <button
            role="menuitemradio"
            aria-checked={board.imageSize === o.size}
            onclick={() => (board.imageSize = o.size)}
          >
            <span class="mark" aria-hidden="true">{board.imageSize === o.size ? "✓" : ""}</span>
            {o.label}
            <span class="px">{o.size} px</span>
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .image-menu {
    position: relative;
  }

  .menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 60;
    display: grid;
    width: 210px;
    padding: 5px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .menu button {
    text-align: left;
  }

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
