<script lang="ts">
  import Modal from "./Modal.svelte";

  let {
    slot,
    initial,
    onSave,
    onCancel,
  }: {
    slot: "before" | "after";
    initial: string;
    onSave: (text: string) => void;
    onCancel: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let text = $state(initial);
</script>

<Modal title={slot === "after" ? "Comment after move" : "Comment before move"} onClose={onCancel}>
  <textarea
    rows="5"
    style="width: 100%"
    bind:value={text}
    placeholder="Plans, ideas, what went wrong…"
    onkeydown={(e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) onSave(text);
    }}
  ></textarea>
  <p class="hint">Leave it empty to remove the comment. Ctrl+Enter saves.</p>
  {#snippet actions()}
    <button onclick={onCancel}>Cancel</button>
    <button class="primary" onclick={() => onSave(text)}>Save comment</button>
  {/snippet}
</Modal>

<style>
  .hint {
    margin: 6px 0 0;
    color: var(--ink-soft);
    font-size: 13px;
  }
</style>
