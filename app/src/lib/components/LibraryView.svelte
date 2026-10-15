<script lang="ts">
  // The database manager (DB-07) as a modal over an editor window: a quick
  // way to browse and open another database without leaving this one.
  // Opening one always spawns a separate editor window (LibraryBrowser's
  // `onOpen`); it never replaces this window's own database. Opened from
  // the ribbon's database group.
  import { api } from "../api";
  import { notices } from "../stores/notify.svelte";
  import Icon from "./Icon.svelte";
  import LibraryBrowser from "./LibraryBrowser.svelte";

  let { onClose }: { onClose: () => void } = $props();

  async function open(path: string) {
    try {
      await api.openEditorWindow(path);
      onClose();
    } catch (e) {
      notices.error(e);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" onkeydown={onKey} onmousedown={(e) => e.target === e.currentTarget && onClose()}>
  <div class="window" role="dialog" aria-modal="true" aria-label="Database manager">
    <header>
      <div class="row title">
        <Icon name="database" size={18} />
        <span>Database manager</span>
      </div>
      <button class="close" aria-label="Close" title="Close" onclick={onClose}>✕</button>
    </header>
    <LibraryBrowser onOpen={open} />
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

  .window {
    width: min(1200px, 96vw);
    height: min(820px, 92vh);
    display: flex;
    flex-direction: column;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    overflow: hidden;
  }

  .row {
    display: flex;
    align-items: center;
  }

  header {
    justify-content: space-between;
    padding: 10px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
    flex: none;
  }

  .title {
    gap: 8px;
    font-family: var(--font-book);
    font-weight: 700;
    font-size: 15px;
  }

  .close {
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
    border-color: var(--rule);
    color: var(--ink-soft);
    font-size: 11px;
  }
</style>
