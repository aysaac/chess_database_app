<script lang="ts">
  import type { NameKindDto } from "../bindings/NameKindDto";
  import type { NameListDto } from "../bindings/NameListDto";
  import type { NameRowDto } from "../bindings/NameRowDto";
  import { api } from "../api";
  import { formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { notices } from "../stores/notify.svelte";
  import CrossTableDialog from "./CrossTableDialog.svelte";
  import Modal from "./Modal.svelte";

  let { kind, onShowGames }: { kind: NameKindDto; onShowGames: () => void } = $props();

  const LIMIT = 500;
  const nouns: Record<NameKindDto, [string, string]> = {
    player: ["player", "players"],
    event: ["event", "events"],
    site: ["site", "sites"],
    annotator: ["annotator", "annotators"],
  };

  let text = $state("");
  let byGames = $state(true);
  let data = $state<NameListDto | null>(null);
  let selected = $state(0);
  let renaming = $state<NameRowDto | null>(null);
  let newName = $state("");
  /** The event whose cross-table is open (DB-16). */
  let crossTable = $state<number | null>(null);
  let seq = 0;

  // Reload when the list, the filter text or the database changes.
  $effect(() => {
    const info = db.info;
    const args = [kind, text, byGames] as const;
    void db.listVersion;
    void info?.games;
    if (!info) return;
    const t = setTimeout(async () => {
      const mine = ++seq;
      try {
        const list = await api.listNames(info.handle, args[0], args[1], args[2], LIMIT);
        if (mine === seq) {
          data = list;
          selected = Math.min(selected, Math.max(0, list.rows.length - 1));
        }
      } catch (e) {
        notices.error(e);
      }
    }, 150);
    return () => clearTimeout(t);
  });

  async function show(row: NameRowDto | undefined) {
    if (!row) return;
    await db.showNameGames(kind, row.id);
    onShowGames();
  }

  function startRename(row: NameRowDto | undefined) {
    if (!row) return;
    renaming = row;
    newName = row.name;
  }

  async function rename() {
    const row = renaming;
    if (!row || !newName.trim()) return;
    renaming = null;
    await db.renameName(kind, row.id, newName);
  }

  function onKey(e: KeyboardEvent) {
    const rows = data?.rows ?? [];
    const moves: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, PageDown: 10, PageUp: -10 };
    if (e.key in moves) {
      e.preventDefault();
      selected = Math.max(0, Math.min(rows.length - 1, selected + moves[e.key]));
      document.getElementById(`name-${kind}-${selected}`)?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      show(rows[selected]);
    } else if (e.key === "F2") {
      e.preventDefault();
      startRename(rows[selected]);
    }
  }
</script>

<section class="names" aria-label="{nouns[kind][1]} list">
  <div class="bar">
    <input
      type="search"
      placeholder="Find a {nouns[kind][0]}"
      aria-label="Find a {nouns[kind][0]}"
      spellcheck="false"
      bind:value={text}
    />
    <span class="soft">
      {#if data}
        {formatCount(data.total)}
        {data.total === 1 ? nouns[kind][0] : nouns[kind][1]}{data.total > data.rows.length
          ? `, showing the first ${formatCount(data.rows.length)}`
          : ""}
      {/if}
    </span>
    <span class="spacer"></span>
    <button disabled={!data?.rows.length} onclick={() => show(data?.rows[selected])}>Show games</button>
    <button disabled={!data?.rows.length} title="F2" onclick={() => startRename(data?.rows[selected])}>Rename…</button>
    {#if kind === "event"}
      <button
        disabled={!data?.rows.length}
        title="Standings and results of the selected event"
        onclick={() => (crossTable = data?.rows[selected]?.id ?? null)}>Cross-table</button
      >
    {/if}
  </div>
  <div class="head row">
    <button class="cell" onclick={() => (byGames = false)}
      >Name{#if !byGames}<span class="arrow"> ▲</span>{/if}</button
    >
    <button class="cell num" onclick={() => (byGames = true)}
      >Games{#if byGames}<span class="arrow"> ▼</span>{/if}</button
    >
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="rows" role="grid" tabindex="0" aria-label={nouns[kind][1]} onkeydown={onKey}>
    {#each data?.rows ?? [] as row, i (row.id)}
      <div
        id="name-{kind}-{i}"
        class="row"
        class:selected={i === selected}
        role="row"
        tabindex="-1"
        onmousedown={() => (selected = i)}
        ondblclick={() => show(row)}
      >
        <span class="cell">{row.name}</span>
        <span class="cell num">{formatCount(row.games)}</span>
      </div>
    {:else}
      {#if data}<p class="empty">No {nouns[kind][1]} match “{text}”.</p>{/if}
    {/each}
  </div>
</section>

{#if crossTable !== null && db.info}
  <CrossTableDialog handle={db.info.handle} event={crossTable} onClose={() => (crossTable = null)} />
{/if}

{#if renaming}
  <Modal title="Rename {nouns[kind][0]}" onClose={() => (renaming = null)} width="480px">
    <form
      onsubmit={(e) => {
        e.preventDefault();
        rename();
      }}
    >
      <label>
        New name for “{renaming.name}” in all {formatCount(renaming.games)}
        {renaming.games === 1 ? "game" : "games"}
        <input bind:value={newName} spellcheck="false" />
      </label>
      <p class="soft">If another {nouns[kind][0]} already has this name, the two are merged.</p>
    </form>
    {#snippet actions()}
      <button onclick={() => (renaming = null)}>Cancel</button>
      <button class="primary" disabled={!newName.trim() || newName.trim() === renaming?.name} onclick={rename}
        >Rename</button
      >
    {/snippet}
  </Modal>
{/if}

<style>
  .names {
    display: flex;
    flex-direction: column;
    height: 100%;
    font-size: 13px;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    padding: 4px 8px;
    overflow: hidden;
    border-bottom: 1px solid var(--rule);
  }

  .bar input {
    flex: 0 1 260px;
    width: 260px;
    min-width: 90px;
    padding: 3px 8px;
  }

  .bar button {
    flex: none;
    padding: 2px 9px;
    border-color: var(--rule);
  }

  .spacer {
    flex: 1;
  }

  .soft {
    color: var(--ink-soft);
  }

  .row {
    display: grid;
    grid-template-columns: minmax(100px, 1fr) 90px;
    align-items: center;
    height: 26px;
    padding: 0 8px;
  }

  .head {
    border-bottom: 1px solid var(--rule);
    background: var(--panel);
  }

  .head .cell {
    height: 100%;
    border: 0;
    border-radius: 0;
    text-align: left;
    font-weight: 600;
  }

  .head .num {
    text-align: right;
  }

  .arrow {
    font-size: 10px;
    color: var(--felt);
  }

  .rows {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    outline-offset: -2px;
  }

  .cell {
    padding: 0 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .row:not(.head):hover {
    background: color-mix(in srgb, var(--felt-soft) 50%, transparent);
  }

  .row.selected {
    background: var(--felt-soft);
  }

  .empty {
    margin: 12px;
    color: var(--ink-soft);
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  label input {
    color: var(--ink);
    font-size: 14px;
  }
</style>
