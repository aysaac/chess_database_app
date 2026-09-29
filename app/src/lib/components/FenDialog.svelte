<script lang="ts">
  // "Go to position" (QOL-10): the mouse path to what Ctrl+V does with a
  // copied FEN (FenPaste.svelte).
  import { goToPosition } from "../gotoPosition";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let text = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function go() {
    if (!text.trim() || busy) return;
    busy = true;
    const problem = await goToPosition(text);
    busy = false;
    if (!problem) onClose();
    else if (problem.kind === "noFen")
      error = "That isn't a FEN. Paste one such as the example, or a Lichess analysis link.";
    else error = `Can't go to that position: ${problem.error}.`;
  }
</script>

<Modal title="Go to position" {onClose} width="560px">
  <label>
    FEN or Lichess analysis link
    <input
      bind:value={text}
      placeholder="rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1"
      spellcheck="false"
      aria-invalid={error !== null}
      aria-describedby="fen-dialog-error"
      oninput={() => (error = null)}
      onkeydown={(e) => e.key === "Enter" && go()}
    />
  </label>
  <p id="fen-dialog-error" class="error" role="alert">{error ?? ""}</p>
  <p class="hint">
    Goes to the move where the game on the board reaches the position, or opens it as a new game, and
    shows its opening tree. Ctrl+V outside a text box does the same with a copied FEN.
  </p>
  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
    <button class="primary" disabled={!text.trim() || busy} onclick={go}>Go to position</button>
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

  .error {
    min-height: 1.4em;
    margin: 6px 0 0;
    color: var(--danger);
    font-size: 13px;
  }

  .hint {
    margin: 4px 0 0;
    color: var(--ink-soft);
    font-size: 13px;
  }
</style>
