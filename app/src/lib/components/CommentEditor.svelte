<script lang="ts">
  import { onMount } from "svelte";
  import { game } from "../stores/game.svelte";

  /** Typed in place in the notation: clicking away or Enter saves, Escape cancels. */
  let { initial }: { initial: string } = $props();

  let el: HTMLSpanElement;
  // svelte-ignore state_referenced_locally
  let blank = $state(initial === "");
  let done = false;

  onMount(() => {
    el.textContent = initial;
    el.focus();
    // Caret at the end, ready to type.
    const range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
  });

  const tidy = (s: string) => s.replace(/\s+/g, " ").trim();

  function finish(save: boolean) {
    if (done) return;
    done = true;
    const text = tidy(el.textContent ?? "");
    // Unchanged text isn't saved, so it doesn't mark the game as edited.
    game.finishComment(save && text !== tidy(initial) ? text : null);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      finish(e.key === "Enter");
    }
  }
</script>

<span
  class="editor"
  class:blank
  role="textbox"
  tabindex="0"
  aria-label="Comment"
  contenteditable="plaintext-only"
  data-placeholder="Type a comment"
  spellcheck="true"
  bind:this={el}
  oninput={() => (blank = !el.textContent)}
  onkeydown={onKey}
  onblur={() => finish(true)}
></span>

<style>
  .editor {
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--felt-soft);
    color: var(--ink);
    font-style: italic;
    font-weight: 400;
    white-space: pre-wrap;
    cursor: text;
    user-select: text;
    -webkit-user-select: text;
  }

  .editor:focus {
    outline: 1px solid var(--felt);
    outline-offset: 0;
  }

  .editor.blank::before {
    content: attr(data-placeholder);
    color: var(--ink-soft);
  }
</style>
