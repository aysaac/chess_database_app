<script lang="ts">
  import { formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { drag } from "../stores/drag.svelte";

  // While games are dragged out of the list (QOL-04): the databases they can
  // be dropped on, over the notation, and a label under the pointer. The
  // board and the tab strip are drop targets of their own.
  const what = $derived(drag.games ? (drag.games.count === 1 ? "1 game" : `${formatCount(drag.games.count)} games`) : "");
  const over = (kind: string, path?: string) =>
    drag.target?.kind === kind && (path === undefined || drag.target.path === path);
</script>

{#if drag.games}
  <div class="shelf" aria-live="polite">
    <p class="title">{drag.move ? "Move" : "Copy"} {what} to</p>
    <div class="targets">
      {#each db.copyTargets as t (t.path)}
        <div class="target" class:over={over("db", t.path)} data-drop="db" data-path={t.path} data-name={t.name}>
          <span class="name">
            <strong>{t.name}</strong>{#if t.reference}<span class="tag">reference</span>{/if}
          </span>
          <span class="path">{t.path}</span>
        </div>
      {/each}
      <div class="target new" class:over={over("new-db")} data-drop="new-db">
        <span class="name"><strong>New database…</strong></span>
        <span class="path">Asks where to create it</span>
      </div>
    </div>
    <p class="hint">
      {#if drag.move}
        The originals are marked as deleted; restore them until you compact.
      {:else}
        Hold Shift to move instead. Drop on the board to open {drag.games.count === 1 ? "it" : "them"}.
      {/if}
    </p>
  </div>

  <div class="ghost" style:left="{drag.x + 14}px" style:top="{drag.y + 16}px">
    <strong>{drag.games.label}</strong>
    <span class:ready={drag.target}>{drag.hint}</span>
  </div>
{/if}

<style>
  .shelf {
    position: absolute;
    inset: 10px 10px auto;
    z-index: 40;
    max-height: calc(100% - 20px);
    display: flex;
    flex-direction: column;
    padding: 10px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 8px 24px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .title {
    margin: 0 0 8px;
    font-family: var(--font-book);
    font-size: 16px;
    font-weight: 600;
  }

  .targets {
    display: grid;
    gap: 4px;
    min-height: 0;
    overflow-y: auto;
  }

  .target {
    display: grid;
    gap: 1px;
    padding: 7px 10px;
    border: 1px dashed var(--rule);
    border-radius: var(--radius);
  }

  .target.over {
    border: 1px solid var(--felt);
    background: var(--felt-soft);
    box-shadow: inset 3px 0 0 var(--felt);
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

  .hint {
    margin: 8px 2px 0;
    color: var(--ink-soft);
    font-size: 12px;
  }

  .ghost {
    position: fixed;
    z-index: 200;
    display: grid;
    max-width: 280px;
    padding: 4px 9px;
    pointer-events: none;
    background: var(--paper);
    border: 1px solid var(--felt);
    border-radius: var(--radius);
    box-shadow: 0 4px 14px color-mix(in srgb, var(--ink) 22%, transparent);
    font-size: 12px;
  }

  .ghost strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ghost span {
    color: var(--ink-soft);
  }

  .ghost span.ready {
    color: var(--felt);
    font-weight: 600;
  }
</style>
