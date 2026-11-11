<script lang="ts">
  import { onMount } from "svelte";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import type { HeadersDto } from "../bindings/HeadersDto";
  import Modal from "./Modal.svelte";
  import SaveDetailsDialog from "./SaveDetailsDialog.svelte";

  // Saves the game on the board as a new game in a database of the user's
  // choice (DB-01): the way to keep a game played on a bare board, with no
  // database open, and to put a game into another database afterwards. Two
  // steps: the database, then the game's details (players, result…), which
  // can be left as they are.
  let { onClose }: { onClose: () => void } = $props();

  let list = $state<HTMLDivElement>();

  onMount(() => void db.loadRecent());

  // The first database takes the focus, also when coming back from the details.
  $effect(() => list?.querySelector<HTMLElement>("button")?.focus());

  /** A database's name from its folder, "My games" for ".../My games.chessdb". */
  function name(path: string): string {
    return (
      path
        .split(/[\\/]/)
        .filter(Boolean)
        .at(-1)
        ?.replace(/\.chessdb$/i, "") ?? path
    );
  }

  /** The database chosen, while the details are asked for. */
  let chosen = $state<{ path: string; create: boolean } | null>(null);

  function saveIn(path: string) {
    chosen = { path, create: false };
  }

  async function inNew() {
    const path = await game.newDatabasePath();
    if (path) chosen = { path, create: true };
  }

  // Props and state are read before `onClose`: closing clears what they come from.
  function save(headers: HeadersDto) {
    const target = chosen;
    onClose();
    if (target) void game.saveInto(target.path, target.create, headers);
  }
</script>

{#if chosen && game.view}
  <SaveDetailsDialog
    headers={game.view.headers}
    database={db.saveTargets.find((t) => t.path === chosen?.path)?.name ?? name(chosen.path)}
    onSave={save}
    onCancel={() => (chosen = null)}
  />
{:else}
  <Modal title="Save as new in a database" {onClose} width="520px">
    <div class="targets" bind:this={list}>
      {#each db.saveTargets as t (t.path)}
        <button class="target" title={t.path} onclick={() => saveIn(t.path)}>
          <span class="name">
            <strong>{t.name}</strong>
            {#if t.reference}<span class="tag">reference</span
              >{:else if t.path === db.info?.path}<span class="tag">open</span>{/if}
          </span>
          <span class="path">{t.path}</span>
        </button>
      {:else}
        <p class="soft">No databases were opened recently. Create one for this game.</p>
      {/each}
    </div>
    <p class="soft">
      The game is added as a new game. A database that isn't open yet is opened, so you can see the
      game in it.
    </p>
    {#snippet actions()}
      <button onclick={onClose}>Cancel</button>
      <button onclick={inNew}>New database…</button>
    {/snippet}
  </Modal>
{/if}

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
