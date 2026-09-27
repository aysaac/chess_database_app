<script lang="ts">
  import { onMount, type Snippet } from "svelte";

  let {
    title,
    onClose,
    width = "440px",
    children,
    actions,
  }: {
    title: string;
    onClose: () => void;
    width?: string;
    children: Snippet;
    actions: Snippet;
  } = $props();

  let dialog: HTMLDivElement;

  onMount(() => {
    const first = dialog.querySelector<HTMLElement>("input, textarea, select, button.primary");
    first?.focus();
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onkeydown={onKey} onmousedown={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-label={title} style:width bind:this={dialog}>
    <h2>{title}</h2>
    <div class="body">{@render children()}</div>
    <div class="actions">{@render actions()}</div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--ink) 28%, transparent);
    z-index: 100;
  }

  .dialog {
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    padding: 18px 20px 14px;
  }

  h2 {
    margin: 0 0 12px;
    font-family: var(--font-book);
    font-size: 19px;
    font-weight: 600;
  }

  .body {
    overflow-y: auto;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
