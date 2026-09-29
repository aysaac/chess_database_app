<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, isAppError } from "../api";
  import type { GameRowDto } from "../bindings/GameRowDto";
  import type { SortKeyDto } from "../bindings/SortKeyDto";
  import { formatCount, resultLabel } from "../format";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { notices } from "../stores/notify.svelte";
  import FilterBar from "./FilterBar.svelte";

  const ROW = 26;
  const PAGE = 200;
  const MAX_CACHED_PAGES = 40;
  // Browsers cap element heights (~17–33M px); beyond this the scrollbar is
  // scaled and wheel scrolling is handled here.
  const MAX_SPACER = 8_000_000;

  const columns: { key: SortKeyDto | null; label: string; title?: string; num?: boolean }[] = [
    { key: "id", label: "#", num: true },
    { key: "white", label: "White" },
    { key: "whiteElo", label: "Elo", title: "White Elo", num: true },
    { key: "black", label: "Black" },
    { key: "blackElo", label: "Elo", title: "Black Elo", num: true },
    { key: "result", label: "Result" },
    { key: "date", label: "Date", num: true },
    { key: "event", label: "Event" },
    { key: "site", label: "Site" },
    { key: "round", label: "Round", num: true },
    { key: "eco", label: "ECO" },
    { key: null, label: "Opening", title: "Opening, from the positions in the game" },
    { key: "plies", label: "Moves", num: true },
    { key: null, label: "", title: "V: variations, C: comments, A: symbols or arrows, F: starts from a position" },
  ];

  let viewport: HTMLDivElement;
  let viewH = $state(0);
  let virtualTop = $state(0);
  let selected = $state<number | null>(null);
  let menu = $state<{ x: number; y: number; row: GameRowDto } | null>(null);

  let cache = new Map<number, GameRowDto[]>();
  let stale = new Map<number, GameRowDto[]>();
  let loading = new Set<number>();
  let cacheKey = "";
  let cacheVersion = $state(0);
  let expectedScrollTop = -1;

  const total = $derived(db.listTotal);
  const fullH = $derived(total * ROW);
  const spacerH = $derived(Math.min(fullH, MAX_SPACER));
  const ratio = $derived(spacerH > viewH ? (fullH - viewH) / (spacerH - viewH) : 1);
  const first = $derived(Math.floor(virtualTop / ROW));
  const offset = $derived(virtualTop - first * ROW);
  const visibleCount = $derived(Math.ceil(viewH / ROW) + 1);

  const visible = $derived.by(() => {
    void cacheVersion;
    const out: { index: number; row: GameRowDto | undefined }[] = [];
    for (let i = first; i < Math.min(total, first + visibleCount); i++) {
      const p = Math.floor(i / PAGE);
      const page = cache.get(p) ?? stale.get(p);
      out.push({ index: i, row: page?.[i - p * PAGE] });
    }
    return out;
  });

  function clampTop(v: number) {
    return Math.max(0, Math.min(v, Math.max(0, fullH - viewH)));
  }

  function setVirtualTop(v: number) {
    virtualTop = clampTop(v);
    expectedScrollTop = virtualTop / ratio;
    viewport.scrollTop = expectedScrollTop;
  }

  function onScroll() {
    const st = viewport.scrollTop;
    if (Math.abs(st - expectedScrollTop) < 2) return;
    expectedScrollTop = -1;
    virtualTop = clampTop(st * ratio);
  }

  function onWheel(e: WheelEvent) {
    if (ratio <= 1.001) return;
    e.preventDefault();
    const delta = e.deltaMode === 1 ? e.deltaY * ROW : e.deltaMode === 2 ? e.deltaY * viewH : e.deltaY;
    setVirtualTop(virtualTop + delta);
  }

  onMount(() => {
    const resize = new ResizeObserver(() => (viewH = viewport.clientHeight));
    resize.observe(viewport);
    viewport.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      resize.disconnect();
      viewport.removeEventListener("wheel", onWheel);
    };
  });

  // Reset the cache when the database, sort or content changes, keeping the
  // old rows on screen until fresh ones arrive.
  $effect(() => {
    const info = db.info;
    const key = `${info?.handle}:${db.sort.key}:${db.sort.descending}:${db.filterVersion}:${db.listVersion}`;
    if (key === cacheKey) return;
    const sameList = cacheKey.split(":").slice(0, 4).join(":") === key.split(":").slice(0, 4).join(":");
    cacheKey = key;
    stale = sameList ? cache : new Map();
    cache = new Map();
    loading = new Set();
    if (!sameList) {
      selected = null;
      virtualTop = 0;
      if (viewport) viewport.scrollTop = 0;
      // Row numbers now mean other games.
      game.listIndex = null;
    }
    cacheVersion++;
  });

  // Fetch pages for the visible window.
  $effect(() => {
    void cacheVersion;
    const info = db.info;
    if (!info || total === 0) return;
    const lastPage = Math.floor((Math.min(total, first + visibleCount) - 1) / PAGE);
    for (let p = Math.floor(first / PAGE); p <= lastPage; p++) {
      if (!cache.has(p) && !loading.has(p)) load(info.handle, p);
    }
  });

  async function load(handle: number, p: number) {
    const key = cacheKey;
    const sort = { ...db.sort };
    loading.add(p);
    try {
      const page = await api.listGames(handle, sort, p * PAGE, PAGE);
      if (key !== cacheKey) return;
      cache.set(p, page.rows);
      if (cache.size > MAX_CACHED_PAGES) cache.delete(cache.keys().next().value!);
      cacheVersion++;
    } catch (e) {
      if (isAppError(e) && e.code === "notSorted") {
        // A game edit invalidated the sort order; rebuild it.
        db.setSort(sort, true);
      } else {
        notices.error(e);
      }
    } finally {
      loading.delete(p);
    }
  }

  function rowAt(index: number): GameRowDto | undefined {
    const p = Math.floor(index / PAGE);
    return (cache.get(p) ?? stale.get(p))?.[index - p * PAGE];
  }

  function select(index: number) {
    selected = Math.max(0, Math.min(total - 1, index));
    const top = selected * ROW;
    if (top < virtualTop) setVirtualTop(top);
    else if (top + ROW > virtualTop + viewH) setVirtualTop(top + ROW - viewH);
  }

  /** Opens a row's game; `newTab` (Ctrl) opens it next to the current one. */
  async function openRow(row: GameRowDto | undefined, index: number, newTab = false) {
    if (row && db.info) await game.open(db.info.handle, row.id, index, newTab ? "tab" : "auto");
  }

  // Follow next/previous game (opened from the board) in the list.
  $effect(() => {
    const index = game.listIndex;
    if (index === null) return;
    untrack(() => {
      if (index !== selected && viewport) select(index);
    });
  });

  function onKey(e: KeyboardEvent) {
    // Ctrl+Up/Down open the previous/next game (QOL-03), from the list too:
    // focus stays here after opening a game from it.
    if ((e.ctrlKey || e.metaKey) && (e.key === "ArrowUp" || e.key === "ArrowDown")) return;
    const page = Math.max(1, Math.floor(viewH / ROW) - 1);
    const cur = selected ?? first - 1;
    const moves: Record<string, number> = {
      ArrowDown: cur + 1,
      ArrowUp: cur - 1,
      PageDown: cur + page,
      PageUp: cur - page,
      Home: 0,
      End: total - 1,
    };
    if (e.key in moves) {
      e.preventDefault();
      e.stopPropagation();
      select(moves[e.key]);
    } else if (e.key === "Enter" && selected !== null) {
      e.preventDefault();
      openRow(rowAt(selected), selected, e.ctrlKey || e.metaKey);
    } else if (e.key === "Delete" && selected !== null) {
      const row = rowAt(selected);
      if (row) db.setDeleted([row.id], !row.deleted);
    }
  }

  function flags(r: GameRowDto): string {
    return (r.hasVariations ? "V" : "") + (r.hasComments ? "C" : "") + (r.hasAnnotations ? "A" : "") + (r.hasSetup ? "F" : "");
  }

  const isOpen = (r: GameRowDto) =>
    game.view?.source?.db === db.info?.handle && game.view?.source?.game === r.id;
</script>

<svelte:window
  onmousedown={() => (menu = null)}
  onkeydown={(e) => {
    if (menu && e.key === "Escape") {
      e.preventDefault();
      menu = null;
    }
  }}
/>

<section class="list" aria-label="Games">
  {#if db.info}<FilterBar />{/if}
  <div class="row head" role="row">
    {#each columns as c (c.label + c.key)}
      {#if c.key}
        <button
          class="cell"
          class:num={c.num}
          title={c.title ?? `Sort by ${c.label.toLowerCase()}`}
          disabled={!db.info || total === 0}
          onclick={() => db.toggleSort(c.key!)}
        >
          {c.label}{#if db.sortingKey === c.key}<span class="sorting"> sorting…</span>{:else if db.sort.key === c.key && c.key !== "id"}<span
              class="arrow">{db.sort.descending ? " ▼" : " ▲"}</span
            >{:else if c.key === "id" && db.sort.key === "id" && db.sort.descending}<span class="arrow"> ▼</span>{/if}
        </button>
      {:else}
        <span class="cell" title={c.title}></span>
      {/if}
    {/each}
  </div>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="viewport" bind:this={viewport} onscroll={onScroll} onkeydown={onKey} tabindex="0" role="grid" aria-rowcount={total}>
    <!-- The content box has the (scaled) full height; the row window sticks
         to the top of the viewport inside it. -->
    <div class="content" style:height="{spacerH}px">
    <div class="window" style:height="{viewH}px">
      <div style:transform="translateY({-offset}px)">
        {#each visible as { index, row } (index)}
          <div
            class="row"
            class:selected={selected === index}
            class:deleted={row?.deleted}
            class:open={row && isOpen(row)}
            role="row"
            tabindex="-1"
            onmousedown={() => (selected = index)}
            ondblclick={(e) => openRow(row, index, e.ctrlKey || e.metaKey)}
            oncontextmenu={(e) => {
              e.preventDefault();
              selected = index;
              if (row) menu = { x: e.clientX, y: e.clientY, row };
            }}
          >
            {#if row}
              <span class="cell num">{row.id + 1}</span>
              <span class="cell">{row.white}</span>
              <span class="cell num">{row.whiteElo ?? ""}</span>
              <span class="cell">{row.black}</span>
              <span class="cell num">{row.blackElo ?? ""}</span>
              <span class="cell">{resultLabel(row.result)}</span>
              <span class="cell num">{row.date}</span>
              <span class="cell">{row.event}</span>
              <span class="cell">{row.site}</span>
              <span class="cell num">{row.round}</span>
              <span class="cell" class:soft={!row.eco} title={row.eco ? undefined : "From the game's positions (no ECO tag)"}
                >{row.eco || row.openingEco}</span
              >
              <span class="cell" title={row.opening}>{row.opening}</span>
              <span class="cell num">{Math.ceil(row.plies / 2)}</span>
              <span class="cell flags">{flags(row)}</span>
            {:else}
              <span class="cell num">{index + 1}</span>
            {/if}
          </div>
        {/each}
      </div>
    </div>
    </div>

    {#if !db.info}
      <div class="empty">
        <p>Open or create a database to see its games.</p>
        <div>
          <button class="primary" onclick={() => db.createDialog()}>Create database</button>
          <button onclick={() => db.openDialog()}>Open database</button>
        </div>
        {#if db.recent.some((r) => r.exists)}
          <p class="recent-title">Recently opened</p>
          <div class="recent">
            {#each db.recent.filter((r) => r.exists).slice(0, 5) as r (r.path)}
              <button title={r.path} onclick={() => db.openAt(r.path)}>{r.name}</button>
            {/each}
          </div>
        {/if}
      </div>
    {:else if total === 0 && db.info.filter}
      <div class="empty">
        <p>No games match {db.info.filter.description}.</p>
        <div><button class="primary" onclick={() => db.clearFilter()}>Show all games</button></div>
      </div>
    {:else if total === 0}
      <div class="empty">
        <p>{db.info.name} has no games yet. Import a PGN file to add some.</p>
        <div><button class="primary" onclick={() => db.importDialog()}>Import PGN</button></div>
      </div>
    {/if}
  </div>
</section>

{#if menu}
  {@const row = menu.row}
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    style:left="{menu.x}px"
    style:top="{Math.min(menu.y, window.innerHeight - 120)}px"
    onmousedown={(e) => e.stopPropagation()}
  >
    <button role="menuitem" onclick={() => { const i = selected; menu = null; openRow(row, i ?? 0); }}>Open game</button>
    <button role="menuitem" onclick={() => { const i = selected; menu = null; openRow(row, i ?? 0, true); }}>Open in a new tab</button>
    <button role="menuitem" onclick={() => { menu = null; db.setDeleted([row.id], !row.deleted); }}>
      {row.deleted ? "Restore game" : "Mark as deleted"}
    </button>
  </div>
{/if}

{#if db.info && total > 0}
  <span class="sr-only" aria-live="polite">{formatCount(total)} games</span>
{/if}

<style>
  .list {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    font-size: 13px;
  }

  .row {
    display: grid;
    grid-template-columns:
      64px minmax(110px, 1.4fr) 46px minmax(110px, 1.4fr) 46px 52px 88px
      minmax(90px, 1.2fr) minmax(80px, 1fr) 52px 42px minmax(110px, 1.5fr) 50px 44px;
    align-items: center;
    height: 26px;
    padding: 0 8px;
  }

  .head {
    flex: none;
    border-bottom: 1px solid var(--rule);
    background: var(--panel);
    font-weight: 600;
  }

  .head .cell {
    padding: 0 6px;
    height: 100%;
    border: 0;
    border-radius: 0;
    text-align: left;
    font-weight: 600;
  }

  .head .cell.num {
    text-align: right;
  }

  .sorting {
    font-weight: 400;
    color: var(--brass);
  }

  .arrow {
    font-size: 10px;
    color: var(--felt);
  }

  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    outline-offset: -2px;
  }

  .content {
    position: relative;
  }

  .window {
    position: sticky;
    top: 0;
    overflow: hidden;
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

  .row.open {
    box-shadow: inset 3px 0 0 var(--felt);
  }

  .row.deleted .cell {
    text-decoration: line-through;
    color: var(--ink-soft);
  }

  .soft {
    color: var(--ink-soft);
  }

  .flags {
    color: var(--brass);
    font-weight: 600;
    letter-spacing: 1px;
  }

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    color: var(--ink-soft);
    font-size: 14px;
  }

  .empty p {
    margin: 0;
  }

  .empty div {
    display: flex;
    gap: 8px;
  }

  .empty .recent-title {
    margin-top: 8px;
    font-size: 13px;
  }

  .empty .recent button {
    border-color: var(--rule);
  }

  .menu {
    position: fixed;
    z-index: 50;
    display: flex;
    flex-direction: column;
    min-width: 170px;
    padding: 5px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .menu button {
    text-align: left;
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
</style>
