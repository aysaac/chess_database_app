<script lang="ts">
  import { db } from "../stores/db.svelte";
  import Icon from "./Icon.svelte";
  import RibbonDropdown from "./RibbonDropdown.svelte";

  // The reference database (DB-13), chosen from the recently opened ones
  // (DB-07). In the ribbon's Reference group.
  let { compact = false }: { compact?: boolean } = $props();

  type Entry = { path: string; name: string; exists: boolean };
  /** A titled part of the menu. Favourites (T-3) will be a section above the recent ones. */
  type Section = { label: string; entries: Entry[] };

  const reference = $derived(db.reference);
  const sections = $derived.by((): Section[] => {
    const recent: Entry[] = db.recent.map((r) => ({ path: r.path, name: r.name, exists: r.exists }));
    // The reference may never have been opened in the list.
    if (reference && !recent.some((r) => r.path === reference.path)) {
      recent.unshift({ path: reference.path, name: reference.name, exists: !!reference.info });
    }
    return [{ label: "Recent databases", entries: recent }];
  });
  /** The open database is the reference already. */
  const openIsReference = $derived(
    !!db.info &&
      !!reference &&
      (reference.info ? reference.info.handle === db.info.handle : reference.path === db.info.path),
  );
  const title = $derived(
    "Reference database: what the opening tree reads" +
      (reference?.error ? `. ${reference.name} couldn't be opened: ${reference.error}` : ""),
  );
</script>

<RibbonDropdown
  class="reference-menu"
  size="wide"
  icon="library"
  label={reference?.name ?? "No reference database"}
  {title}
  menuLabel="Reference database"
  {compact}
  onopen={() => db.loadRecent()}
>
  {#each sections as section (section.label)}
    <div class="section" role="group" aria-label={section.label}>
      <div class="heading" aria-hidden="true">{section.label}</div>
      {#each section.entries as e (e.path)}
        {@const current = e.path === reference?.path}
        <button
          role="menuitemradio"
          aria-checked={current}
          disabled={!e.exists}
          title={e.exists ? e.path : `${e.path} (not found)`}
          onclick={() => db.setReference(e.path)}
        >
          <span class="tick">{#if current}<Icon name="check" size={14} />{/if}</span>
          <span class="name">{e.name}</span>
          {#if !e.exists}<span class="missing">Not found</span>{/if}
        </button>
      {:else}
        <p class="empty">Databases you open appear here.</p>
      {/each}
    </div>
  {/each}
  <div class="sep" role="separator"></div>
  <button role="menuitem" onclick={() => db.chooseReference()}>Choose a database…</button>
  {#if db.info && !openIsReference}
    <button role="menuitem" onclick={() => db.info && db.setReference(db.info.path)}
      >Use the open database ({db.info.name})</button
    >
  {/if}
  <button role="menuitem" disabled={!reference} onclick={() => db.setReference(null)}
    >Stop using a reference database</button
  >
</RibbonDropdown>

<style>
  .section {
    display: grid;
    min-width: 260px;
    max-width: min(420px, calc(100vw - 40px));
  }

  .heading {
    padding: 2px 9px 3px;
    color: var(--ink-soft);
    font-size: 12px;
  }

  .section button {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .tick {
    display: flex;
    width: 14px;
    color: var(--felt);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .missing {
    color: var(--ink-soft);
    font-size: 12px;
  }

  .empty {
    margin: 4px 9px 6px;
    color: var(--ink-soft);
  }

  .sep {
    margin: 4px 2px;
    border-top: 1px solid var(--rule);
  }
</style>
