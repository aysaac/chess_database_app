<script lang="ts">
  import { BOARD_THEMES, board } from "../stores/board.svelte";
  import Modal from "./Modal.svelte";

  /** Board colours and display options (UI-01); changes apply at once. */
  let { onClose }: { onClose: () => void } = $props();
</script>

<Modal title="Board appearance" {onClose} width="460px">
  <div class="themes" role="radiogroup" aria-label="Board colours">
    {#each BOARD_THEMES as t (t.id)}
      <label class="theme" class:on={board.theme === t.id}>
        <input
          type="radio"
          name="board-theme"
          value={t.id}
          checked={board.theme === t.id}
          onchange={() => (board.theme = t.id)}
        />
        <span class="swatch" data-board-theme={t.id} aria-hidden="true"></span>
        <span class="label">{t.label}</span>
      </label>
    {/each}
  </div>
  <label class="check"><input type="checkbox" bind:checked={board.coordinates} /> Show coordinates</label>
  <label class="check"
    ><input type="checkbox" bind:checked={board.legalMoves} /> Show where the picked-up piece can move</label
  >
  <label class="check"><input type="checkbox" bind:checked={board.lastMove} /> Highlight the last move</label>
  {#snippet actions()}
    <button class="primary" onclick={onClose}>Done</button>
  {/snippet}
</Modal>

<style>
  .themes {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 8px;
    margin-bottom: 14px;
  }

  .theme {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 5px;
    padding: 6px 4px;
    border: 1px solid transparent;
    border-radius: var(--radius);
    text-align: center;
    font-size: 12px;
    cursor: pointer;
  }

  .theme.on {
    border-color: var(--felt);
    background: var(--felt-soft);
  }

  .theme:has(input:focus-visible) {
    outline: 2px solid var(--felt);
    outline-offset: 1px;
  }

  /* The radio itself is the whole label; keep it for keyboard and screen readers. */
  .theme input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }

  .swatch {
    width: 56px;
    height: 56px;
    border-radius: 3px;
    background-color: var(--board-light);
    background-image: conic-gradient(
      var(--board-dark) 0 90deg,
      var(--board-light) 0 180deg,
      var(--board-dark) 0 270deg,
      var(--board-light) 0
    );
    background-size: 50% 50%;
    box-shadow: 0 0 0 1px var(--rule);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 0;
  }
</style>
