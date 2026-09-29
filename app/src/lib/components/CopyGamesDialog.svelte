<script lang="ts">
  import { onMount } from "svelte";
  import type { CopyModeDto } from "../bindings/CopyModeDto";
  import { formatCount } from "../format";
  import { count, type Ranges } from "../selection";
  import { db } from "../stores/db.svelte";
  import Modal from "./Modal.svelte";

  // Copies or moves the selected games to another database (DB-06): the
  // menu and keyboard way to do what dragging them onto a database does
  // (QOL-04).
  let { ranges, mode, onClose }: { ranges: Ranges; mode: CopyModeDto; onClose: () => void } = $props();

  let list: HTMLDivElement;
  const n = $derived(count(ranges));
  const what = $derived(n === 1 ? "1 game" : `${formatCount(n)} games`);
  const verb = $derived(mode === "move" ? "Move" : "Copy");

  onMount(() => {
    void db.loadRecent();
    list.querySelector<HTMLElement>("button")?.focus();
  });

  // Props are read before `onClose`: closing clears what they come from.
  function copyTo(path: string) {
    const [r, m] = [ranges, mode];
    onClose();
    void db.copyRows(r, path, m);
  }

  function toNew() {
    const [r, m] = [ranges, mode];
    onClose();
    void db.copyRowsToNewDialog(r, m);
  }
</script>

<Modal title="{verb} {what} to another database" {onClose} width="520px">
  <div class="targets" bind:this={list}>
    {#each db.copyTargets as t (t.path)}
      <button class="target" title={t.path} onclick={() => copyTo(t.path)}>
        <span class="name"><strong>{t.name}</strong>{#if t.reference}<span class="tag">reference</span>{/if}</span>
        <span class="path">{t.path}</span>
      </button>
    {:else}
      <p class="soft">No other databases were opened recently. Create one for these games.</p>
    {/each}
  </div>
  <p class="soft">
    {#if mode === "move"}
      The originals are marked as deleted here; you can restore them until you compact the database.
    {:else}
      Games marked as deleted aren't copied. You can also drag games onto a database.
    {/if}
  </p>
  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
    <button onclick={toNew}>New database…</button>
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
