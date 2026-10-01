<script lang="ts">
  import { db } from "../stores/db.svelte";
  import RibbonDropdown from "./RibbonDropdown.svelte";

  // Recently opened databases (DB-07), most recent first. In the ribbon's
  // Database group.
  let { compact = false }: { compact?: boolean } = $props();
</script>

<RibbonDropdown
  class="recent-menu"
  icon="history"
  label="Recent"
  title="Recently opened databases"
  menuLabel="Recent databases"
  {compact}
  onopen={() => db.loadRecent()}
>
  {#each db.recent as r (r.path)}
    <div class="item" class:missing={!r.exists}>
      <button
        role="menuitem"
        disabled={!r.exists}
        title={r.exists ? r.path : `${r.path} (not found)`}
        onclick={() => db.openAt(r.path)}
      >
        <strong>{r.name}</strong>
        <span class="path">{r.exists ? r.path : "Not found; it was moved or deleted"}</span>
      </button>
      <button
        class="forget"
        role="menuitem"
        data-keep-open
        aria-label="Remove {r.name} from this list"
        title="Remove from this list"
        onclick={() => db.forgetRecent(r.path)}>×</button
      >
    </div>
  {:else}
    <p class="empty">Databases you open appear here.</p>
  {/each}
</RibbonDropdown>

<style>
  .item {
    display: flex;
    align-items: stretch;
    width: 380px;
    max-width: calc(100vw - 40px);
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
