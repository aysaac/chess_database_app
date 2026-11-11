<script lang="ts">
  import { onMount } from "svelte";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import Modal from "./Modal.svelte";

  // Saves the game on the board as a new game in a database of the user's
  // choice (DB-01): the way to keep a game played on a bare board, with no
  // database open, and to put a game into another database afterwards.
  let { onClose }: { onClose: () => void } = $props();

  let list: HTMLDivElement;

  onMount(() => {
    void db.loadRecent();
    list.querySelector<HTMLElement>("button")?.focus();
  });

  function saveIn(path: string) {
    onClose();
    void game.saveInto(path);
  }

  function inNew() {
    onClose();
    void game.saveIntoNew();
  }
</script>

<Modal title="Save as new in a database" {onClose} width="520px">
  <div class="targets" bind:this={list}>
    {#each db.saveTargets as t (t.path)}
      <button class="target" title={t.path} onclick={() => saveIn(t.path)}>
        <span class="name">
          <strong>{t.name}</strong>
          {#if t.reference}<span class="tag">reference</span>{:else if t.path === db.info?.path}<span class="tag"
              >open</span
            >{/if}
        </span>
        <span class="path">{t.path}</span>
      </button>
    {:else}
      <p class="soft">No databases were opened recently. Create one for this game.</p>
    {/each}
  </div>
  <p class="soft">
    The game is added as a new game. A database that isn't open yet is opened, so you can see the game in it.
  </p>
  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
    <button onclick={inNew}>New database…</button>
  {/snippet}
</Modal>

<style>
  .targets {
    display: grid;
    gap: 4px;
  }

  .target {
    display: grid;
    gap: 1px;
    padding: 7px 10px;
    text-align: left;
  }

  .name {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .tag {
    padding: 0 6px;
    border-radius: 8px;
    background: var(--brass-soft);
    color: var(--brass);
    font-size: 11px;
    font-weight: 600;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
    font-size: 12px;
  }

  .soft {
    color: var(--ink-soft);
    font-size: 13px;
  }
</style>
