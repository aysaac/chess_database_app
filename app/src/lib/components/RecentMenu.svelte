<script lang="ts">
  import { db } from "../stores/db.svelte";

  // Recently opened databases (DB-07), most recent first.
  let open = $state(false);
  let root: HTMLDivElement;

  function toggle() {
    open = !open;
    if (open) db.loadRecent();
  }

  function onWindowDown(e: MouseEvent) {
    if (open && !root.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onmousedown={onWindowDown} onkeydown={(e) => open && e.key === "Escape" && (open = false)} />

<div class="recent" bind:this={root}>
  <button aria-haspopup="menu" aria-expanded={open} title="Recently opened databases" onclick={toggle}>
    Recent ▾
  </button>
  {#if open}
    <div class="menu" role="menu">
      {#each db.recent as r (r.path)}
        <div class="item" class:missing={!r.exists}>
          <button
            role="menuitem"
            disabled={!r.exists}
            title={r.exists ? r.path : `${r.path} (not found)`}
            onclick={() => {
              open = false;
              db.openAt(r.path);
            }}
          >
            <strong>{r.name}</strong>
            <span class="path">{r.exists ? r.path : "Not found; it was moved or deleted"}</span>
          </button>
          <button class="forget" aria-label="Remove {r.name} from this list" title="Remove from this list" onclick={() => db.forgetRecent(r.path)}>×</button>
        </div>
      {:else}
        <p class="empty">Databases you open appear here.</p>
      {/each}
    </div>
  {/if}
</div>

<style>
  .recent {
    position: relative;
  }

  .menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 60;
    width: 380px;
    max-height: 60vh;
    overflow-y: auto;
    padding: 5px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .item {
    display: flex;
    align-items: stretch;
  }

  .item > button:first-child {
    flex: 1;
    min-width: 0;
    display: grid;
    text-align: left;
    gap: 1px;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
    font-size: 12px;
  }

  .missing strong {
    text-decoration: line-through;
  }

  .forget {
    padding: 0 9px;
    color: var(--ink-soft);
  }

  .empty {
    margin: 6px 8px;
    color: var(--ink-soft);
  }
</style>
