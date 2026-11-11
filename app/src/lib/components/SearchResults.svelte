<script lang="ts">
  import { onMount } from "svelte";
  import { api, isAppError } from "../api";
  import type { SearchRowDto } from "../bindings/SearchRowDto";
  import type { SortKeyDto } from "../bindings/SortKeyDto";
  import { formatCount, resultLabel } from "../format";
  import { cross } from "../stores/cross.svelte";
  import { game } from "../stores/game.svelte";
  import { notices } from "../stores/notify.svelte";

  // The results of a search across databases (SRCH-13): one list, each row
  // from its own database (the Database column), in any sort order. Rows
  // are fetched a page at a time for the visible window, like the game list.
  const ROW = 26;
  const PAGE = 200;
  const MAX_CACHED_PAGES = 40;
  // Browsers cap element heights; past this the scrollbar is scaled.
  const MAX_SPACER = 8_000_000;

  type Column = { id: string; key: SortKeyDto | null; label: string; title?: string; num?: boolean; width: string };
  const columns: Column[] = [
    { id: "database", key: null, label: "Database", title: "The database the game is in", width: "minmax(90px, 1.1fr)" },
    { id: "id", key: "id", label: "#", title: "Sort by database, then game number", num: true, width: "64px" },
    { id: "white", key: "white", label: "White", width: "minmax(80px, 1.4fr)" },
    { id: "whiteElo", key: "whiteElo", label: "Elo", title: "White Elo", num: true, width: "46px" },
    { id: "black", key: "black", label: "Black", width: "minmax(80px, 1.4fr)" },
    { id: "blackElo", key: "blackElo", label: "Elo", title: "Black Elo", num: true, width: "46px" },
    { id: "result", key: "result", label: "Result", width: "52px" },
    { id: "date", key: "date", label: "Date", num: true, width: "88px" },
    { id: "event", key: "event", label: "Event", width: "minmax(70px, 1.2fr)" },
    { id: "site", key: "site", label: "Site", width: "minmax(60px, 1fr)" },
    { id: "eco", key: "eco", label: "ECO", width: "42px" },
    { id: "plies", key: "plies", label: "Moves", num: true, width: "50px" },
  ];
  /** The order columns give way in when the list is narrow: the last ones first. */
  const PRIORITY = ["database", "white", "black", "result", "date", "whiteElo", "blackElo", "event", "id", "eco", "plies", "site"];
  const MIN: Record<string, number> = { database: 90, id: 64, white: 80, whiteElo: 46, black: 80, blackElo: 46, result: 52, date: 88, event: 70, site: 60, eco: 42, plies: 50 };

  let listW = $state(0);
  const shown = $derived.by(() => {
    if (listW <= 0) return columns;
    const room = listW - 16 - 14;
    const keep = new Set<string>();
    let used = 0;
    for (const id of PRIORITY) {
      if (keep.size && used + MIN[id] > room) break;
      keep.add(id);
      used += MIN[id];
    }
    return columns.filter((c) => keep.has(c.id));
  });
  const template = $derived(shown.map((c) => c.width).join(" "));

  let viewport: HTMLDivElement;
  let viewH = $state(0);
  let virtualTop = $state(0);
  /** The selected row (clicked, or moved to with the keys). */
  let selected = $state<number | null>(null);
  let cache = new Map<number, SearchRowDto[]>();
  let stale = new Map<number, SearchRowDto[]>();
  let loading = new Set<number>();
  let cacheKey = "";
  let cacheVersion = $state(0);
  let expectedScrollTop = -1;

  const total = $derived(cross.info?.total ?? 0);
  const fullH = $derived(total * ROW);
  const spacerH = $derived(Math.min(fullH, MAX_SPACER));
  const ratio = $derived(spacerH > viewH ? (fullH - viewH) / (spacerH - viewH) : 1);
  const first = $derived(Math.floor(virtualTop / ROW));
  const offset = $derived(virtualTop - first * ROW);
  const visibleCount = $derived(Math.ceil(viewH / ROW) + 1);

  const visible = $derived.by(() => {
    void cacheVersion;
    const out: { index: number; row: SearchRowDto | undefined }[] = [];
    for (let i = first; i < Math.min(total, first + visibleCount); i++) {
      const p = Math.floor(i / PAGE);
      out.push({ index: i, row: (cache.get(p) ?? stale.get(p))?.[i - p * PAGE] });
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

  // A new search or sort: start over, keeping the old rows on screen until
  // fresh ones arrive when it's the same search in another order.
  $effect(() => {
    const key = `${cross.info?.search}:${cross.sort.key}:${cross.sort.descending}:${cross.version}`;
    if (key === cacheKey) return;
    const sameSearch = cacheKey.split(":")[0] === key.split(":")[0];
    cacheKey = key;
    stale = sameSearch ? cache : new Map();
    cache = new Map();
    loading = new Set();
    if (!sameSearch) {
      selected = null;
      virtualTop = 0;
      if (viewport) viewport.scrollTop = 0;
    }
    cacheVersion++;
  });

  $effect(() => {
    void cacheVersion;
    const info = cross.info;
    if (!info || total === 0) return;
    const lastPage = Math.floor((Math.min(total, first + visibleCount) - 1) / PAGE);
    for (let p = Math.floor(first / PAGE); p <= lastPage; p++) {
      if (!cache.has(p) && !loading.has(p)) load(info.search, p);
    }
  });

  async function load(search: number, p: number) {
    const key = cacheKey;
    const sort = { ...cross.sort };
    loading.add(p);
    try {
      const page = await api.searchPage(search, sort, p * PAGE, PAGE);
      if (key !== cacheKey) return;
      cache.set(p, page.rows);
      if (cache.size > MAX_CACHED_PAGES) cache.delete(cache.keys().next().value!);
      cacheVersion++;
    } catch (e) {
      if (isAppError(e) && e.code === "notSorted") cross.setSort(sort, true);
      else notices.error(e);
    } finally {
      loading.delete(p);
    }
  }

  function rowAt(index: number): SearchRowDto | undefined {
    const p = Math.floor(index / PAGE);
    return (cache.get(p) ?? stale.get(p))?.[index - p * PAGE];
  }

  function reveal(index: number) {
    const top = index * ROW;
    if (top < virtualTop) setVirtualTop(top);
    else if (top + ROW > virtualTop + viewH) setVirtualTop(top + ROW - viewH);
  }

  function select(index: number) {
    if (total === 0) return;
    selected = Math.max(0, Math.min(total - 1, index));
    reveal(selected);
  }

  /** Opens a row's game from its own database; `newTab` (Ctrl) next to the current one. */
  async function openRow(row: SearchRowDto | undefined, newTab = false) {
    if (row) await game.open(row.handle, row.row.id, null, newTab ? "tab" : "auto");
  }

  function onKey(e: KeyboardEvent) {
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
    if (e.key in moves && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      e.stopPropagation();
      select(moves[e.key]);
    } else if (e.key === "Enter" && selected !== null) {
      e.preventDefault();
      openRow(rowAt(selected), e.ctrlKey || e.metaKey);
    }
  }

  const isOpen = (r: SearchRowDto) => game.view?.source?.db === r.handle && game.view?.source?.game === r.row.id;

  function arrow(key: SortKeyDto): string {
    if (cross.sort.key !== key) return "";
    if (key === "id" && !cross.sort.descending) return "";
    return cross.sort.descending ? " ▼" : " ▲";
  }
</script>

{#snippet cell(c: Column, r: SearchRowDto)}
  {@const row = r.row}
  {#if c.id === "database"}
    <span class="cell db" title={r.database}>{r.database}</span>
  {:else if c.id === "id"}
    <span class="cell num">{row.id + 1}</span>
  {:else if c.id === "white"}
    <span class="cell">{row.white}</span>
  {:else if c.id === "whiteElo"}
    <span class="cell num">{row.whiteElo ?? ""}</span>
  {:else if c.id === "black"}
    <span class="cell">{row.black}</span>
  {:else if c.id === "blackElo"}
    <span class="cell num">{row.blackElo ?? ""}</span>
  {:else if c.id === "result"}
    <span class="cell">{resultLabel(row.result)}</span>
  {:else if c.id === "date"}
    <span class="cell num">{row.date}</span>
  {:else if c.id === "event"}
    <span class="cell">{row.event}</span>
  {:else if c.id === "site"}
    <span class="cell">{row.site}</span>
  {:else if c.id === "eco"}
    <span class="cell">{row.eco || row.openingEco}</span>
  {:else if c.id === "plies"}
    <span class="cell num">{Math.ceil(row.plies / 2)}</span>
  {/if}
{/snippet}

<section class="list" aria-label="Search results" style:--columns={template} bind:clientWidth={listW}>
  {#if cross.info}
    <div class="bar">
      <span class="what" title={cross.info.description}>
        {formatCount(total)}
        {total === 1 ? "game" : "games"} in {cross.info.databases.length} databases: {cross.info.description}
      </span>
      <span class="counts">
        {#each cross.info.databases as d (d.handle)}
          <span class="count">{d.name} {formatCount(d.count)}</span>
        {/each}
      </span>
      <button class="close" aria-label="Close the search results" title="Close the search results" onclick={() => cross.close()}
        >×</button
      >
    </div>
  {/if}
  <div class="row head" role="row">
    {#each shown as c (c.id)}
      {#if c.key}
        <button
          class="cell"
          class:num={c.num}
          title={c.title ?? `Sort by ${c.label.toLowerCase()}`}
          disabled={!cross.info || total === 0}
          onclick={() => cross.toggleSort(c.key!)}
          >{c.label}{#if cross.sortingKey === c.key}<span class="sorting"> sorting…</span>{:else}<span class="arrow"
              >{arrow(c.key)}</span
            >{/if}</button
        >
      {:else}
        <span class="cell" role="columnheader" title={c.title}>{c.label}</span>
      {/if}
    {/each}
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="viewport"
    bind:this={viewport}
    onscroll={onScroll}
    onkeydown={onKey}
    tabindex="0"
    role="grid"
    aria-label="Games found"
    aria-rowcount={total}
  >
    <div class="content" style:height="{spacerH}px">
      <div class="window" style:height="{viewH}px">
        <div style:transform="translateY({-offset}px)">
          {#each visible as { index, row } (index)}
            <div
              class="row"
              class:selected={selected === index}
              class:deleted={row?.row.deleted}
              class:open={row && isOpen(row)}
              role="row"
              aria-selected={selected === index}
              tabindex="-1"
              data-index={index}
              onpointerdown={(e) => {
                if (e.button === 0) selected = index;
              }}
              ondblclick={(e) => openRow(row, e.ctrlKey || e.metaKey)}
            >
              {#if row}
                {#each shown as c (c.id)}{@render cell(c, row)}{/each}
              {/if}
            </div>
          {/each}
        </div>
      </div>
    </div>
    {#if cross.info && total === 0}
      <div class="empty"><p>No games match {cross.info.description} in these databases.</p></div>
    {/if}
  </div>
</section>

<style>
  .list {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    font-size: 13px;
  }

  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 3px 4px 3px 12px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  .what {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .counts {
    flex: 0 1 auto;
    display: flex;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
    color: var(--ink-soft);
    white-space: nowrap;
  }

  .count {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .close {
    flex: none;
    padding: 0 8px;
    font-size: 16px;
    line-height: 20px;
  }

  .row {
    display: grid;
    grid-template-columns: var(--columns);
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
    min-width: 0;
    padding: 0 6px;
    height: 100%;
    border: 0;
    border-radius: 0;
    text-align: left;
    font-weight: 600;
    line-height: 26px;
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

  .db {
    color: var(--ink-soft);
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

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 8px 12px;
    color: var(--ink-soft);
    font-size: 14px;
    text-align: center;
  }
</style>
