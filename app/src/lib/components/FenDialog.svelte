<script lang="ts">
  import Modal from "./Modal.svelte";

  let { onStart, onCancel }: { onStart: (fen: string) => void; onCancel: () => void } = $props();

  let fen = $state("");
</script>

<Modal title="New game from a position" onClose={onCancel} width="560px">
  <label>
    Paste a FEN
    <input
      bind:value={fen}
      placeholder="rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"
      spellcheck="false"
      onkeydown={(e) => e.key === "Enter" && fen.trim() && onStart(fen)}
    />
  </label>
  {#snippet actions()}
    <button onclick={onCancel}>Cancel</button>
    <button class="primary" disabled={!fen.trim()} onclick={() => onStart(fen)}>Start game</button>
  {/snippet}
</Modal>

<style>
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  input {
    font-family: var(--font-ui);
    font-size: 14px;
  }
</style>
