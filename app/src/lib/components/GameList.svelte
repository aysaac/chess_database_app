<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, isAppError } from "../api";
  import type { GameRowDto } from "../bindings/GameRowDto";
  import type { SortKeyDto } from "../bindings/SortKeyDto";
  import type { CopyModeDto } from "../bindings/CopyModeDto";
  import { formatCount, resultLabel } from "../format";
  import { contains, count, normalize, span, toggle, type Ranges } from "../selection";
  import { db } from "../stores/db.svelte";
  import { drag, type GamesDrag } from "../stores/drag.svelte";
  import { game } from "../stores/game.svelte";
  import { notices } from "../stores/notify.svelte";
  import { onboarding } from "../stores/onboarding.svelte";
  import CopyGamesDialog from "./CopyGamesDialog.svelte";
  import FilterBar from "./FilterBar.svelte";

  const ROW = 26;
  const PAGE = 200;
  const MAX_CACHED_PAGES = 40;
  // Browsers cap element heights (~17–33M px); beyond this the scrollbar is
  // scaled and wheel scrolling is handled here.
  const MAX_SPACER = 8_000_000;

  type Column = {
    /** Saved with the column widths (QOL-07); don't rename. */
    id: string;
    key: SortKeyDto | null;
    label: string;
    /** For "Resize the … column" and the header menu. */
    name: string;
    title?: string;
    num?: boolean;
    /** Default grid track: fixed, or a share of the free space. */
    width: string;
  };

  const columns: Column[] = [
    { id: "id", key: "id", label: "#", name: "number", num: true, width: "64px" },
    { id: "white", key: "white", label: "White", name: "White", width: "minmax(110px, 1.4fr)" },
    { id: "whiteElo", key: "whiteElo", label: "Elo", name: "White Elo", title: "White Elo", num: true, width: "46px" },
    { id: "black", key: "black", label: "Black", name: "Black", width: "minmax(110px, 1.4fr)" },
    { id: "blackElo", key: "blackElo", label: "Elo", name: "Black Elo", title: "Black Elo", num: true, width: "46px" },
    { id: "result", key: "result", label: "Result", name: "Result", width: "52px" },
    { id: "date", key: "date", label: "Date", name: "Date", num: true, width: "88px" },
    { id: "event", key: "event", label: "Event", name: "Event", width: "minmax(90px, 1.2fr)" },
    { id: "site", key: "site", label: "Site", name: "Site", width: "minmax(80px, 1fr)" },
    { id: "round", key: "round", label: "Round", name: "Round", num: true, width: "52px" },
    { id: "eco", key: "eco", label: "ECO", name: "ECO", width: "42px" },
    {
      id: "opening",
      key: null,
      label: "Opening",
      name: "Opening",
      title: "Opening, from the positions in the game",
      width: "minmax(110px, 1.5fr)",
    },
    { id: "plies", key: "plies", label: "Moves", name: "Moves", num: true, width: "50px" },
    {
      id: "flags",
      key: null,
      label: "",
      name: "flags",
      title: "V: variations, C: comments, A: symbols or arrows, F: starts from a position",
      width: "44px",
    },
  ];

  // Resized columns get their saved width; the others keep their default.
  const template = $derived(
    columns.map((c) => (db.columns[c.id] ? `${db.columns[c.id]}px` : c.width)).join(" "),
  );

  let viewport: HTMLDivElement;
  let viewH = $state(0);
  let virtualTop = $state(0);
  /** The cursor row (keyboard, Enter, next/previous game). */
  let selected = $state<number | null>(null);
  /** Selected rows, for dragging (QOL-04), copying and deleting (DB-06). */
  let picked = $state<Ranges>([]);
  /** Where Shift+click and Shift+arrows extend the selection from. */
  let anchor: number | null = null;
  let menu = $state<{ x: number; y: number; row: GameRowDto } | null>(null);
  let headMenu = $state<{ x: number; y: number; column: Column } | null>(null);
  /** Id of the column whose edge is being dragged. */
  let resizing = $state<string | null>(null);

  /** Drags the right edge of a column (QOL-07); saved when it's let go. */
  function startResize(e: PointerEvent, c: Column) {
    if (e.button !== 0) return;
    e.preventDefault();
    const cell = (e.currentTarget as HTMLElement).parentElement!;
    // Where on the edge it was grabbed, so the edge doesn't jump.
    const grab = e.clientX - cell.getBoundingClientRect().right;
    let moved = false;
    // The left edge is measured each time: columns sharing the free space
    // to the left may shrink as this one grows.
    const move = (ev: PointerEvent) => {
      moved = true;
      db.setColumnWidth(c.id, ev.clientX - grab - cell.getBoundingClientRect().left, false);
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      resizing = null;
      if (moved) db.saveLayout();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    resizing = c.id;
  }

  /** Shift+Left/Right on a column header narrows or widens the column. */
  function resizeKey(e: KeyboardEvent, c: Column) {
    if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    e.stopPropagation();
    const cell = (e.currentTarget as HTMLElement).parentElement!;
    const width = db.columns[c.id] ?? cell.getBoundingClientRect().width;
    db.setColumnWidth(c.id, width + (e.key === "ArrowRight" ? 10 : -10));
  }

  function openHeadMenu(e: MouseEvent) {
    const id = (e.target as HTMLElement).closest<HTMLElement>("[data-column]")?.dataset.column;
    const column = columns.find((c) => c.id === id);
    if (!column) return;
    e.preventDefault();
    menu = null;
    headMenu = { x: e.clientX, y: e.clientY, column };
  }
  let copyDialog = $state<{ ranges: Ranges; mode: CopyModeDto } | null>(null);
  const pickedCount = $derived(count(picked));

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
      picked = [];
      anchor = null;
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

  /** Moves the cursor to `index` and selects that row alone, or with `extend` the rows from the anchor. */
  function select(index: number, extend = false) {
    selected = Math.max(0, Math.min(total - 1, index));
    if (extend && anchor !== null) {
      picked = [span(anchor, selected)];
    } else {
      picked = [[selected, selected + 1]];
      anchor = selected;
    }
    reveal(selected);
  }

  /** Scrolls row `index` into view. */
  function reveal(index: number) {
    const top = index * ROW;
    if (top < virtualTop) setVirtualTop(top);
    else if (top + ROW > virtualTop + viewH) setVirtualTop(top + ROW - viewH);
  }

  /** `Carlsen – Nakamura` for a row. */
  function players(r: GameRowDto): string {
    const last = (n: string) => (n ? n.split(",")[0] : "?");
    return `${last(r.white)} – ${last(r.black)}`;
  }

  function dragged(): GamesDrag | null {
    if (!db.info || !picked.length) return null;
    const n = count(picked);
    const one = n === 1 ? rowAt(picked[0][0]) : undefined;
    return {
      handle: db.info.handle,
      ranges: $state.snapshot(picked),
      count: n,
      label: one ? players(one) : n === 1 ? "1 game" : `${formatCount(n)} games`,
    };
  }

  /**
   * Pressing a row selects it (Ctrl adds or removes it, Shift selects the
   * rows from the anchor) and can start dragging the selection (QOL-04).
   * Pressing one of several selected rows keeps them all for the drag; a
   * plain click then selects that row alone.
   */
  function onRowDown(e: PointerEvent, index: number) {
    if (e.button !== 0) return;
    const mod = e.ctrlKey || e.metaKey;
    let collapse = false;
    if (e.shiftKey && anchor !== null) {
      picked = mod ? normalize([...picked, span(anchor, index)]) : [span(anchor, index)];
    } else if (mod) {
      picked = toggle(picked, index);
      anchor = index;
    } else if (contains(picked, index) && count(picked) > 1) {
      anchor = index;
      collapse = true;
    } else {
      picked = [[index, index + 1]];
      anchor = index;
    }
    selected = index;
    if (!contains(picked, index)) return;
    drag.arm(e, dragged, () => {
      if (collapse) picked = [[index, index + 1]];
    });
  }

  /** The rows the menu and Delete act on: the selection if it has `index`, else that row alone. */
  function rowsFor(index: number): Ranges {
    if (!contains(picked, index)) {
      picked = [[index, index + 1]];
      anchor = index;
    }
    selected = index;
    return $state.snapshot(picked);
  }

  function openMenuAt(index: number, x: number, y: number) {
    const row = rowAt(index);
    rowsFor(index);
    if (row) menu = { x, y, row };
  }

  /** Opens a row's game; `newTab` (Ctrl) opens it next to the current one. */
  async function openRow(row: GameRowDto | undefined, index: number, newTab = false) {
    if (row && db.info) await game.open(db.info.handle, row.id, index, newTab ? "tab" : "auto");
  }

  // Follow next/previous game (opened from the board) in the list. A game
  // among the selected rows (several dropped on the tab strip) keeps them.
  $effect(() => {
    const index = game.listIndex;
    if (index === null) return;
    untrack(() => {
      if (index === selected || !viewport) return;
      if (contains(picked, index)) {
        selected = index;
        reveal(index);
      } else {
        select(index);
      }
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
      select(moves[e.key], e.shiftKey);
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a" && total > 0) {
      e.preventDefault();
      picked = [[0, total]];
      anchor = selected ?? 0;
    } else if (e.key === "Enter" && selected !== null) {
      e.preventDefault();
      openRow(rowAt(selected), selected, e.ctrlKey || e.metaKey);
    } else if (e.key === "Delete" && selected !== null) {
      // The cursor row decides: a deleted one restores the selection.
      const row = rowAt(selected);
      if (row) db.setDeletedRows(rowsFor(selected), !row.deleted);
    } else if ((e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) && selected !== null) {
      e.preventDefault();
      const r = viewport.getBoundingClientRect();
      openMenuAt(selected, r.left + 80, r.top + (selected * ROW - virtualTop) + ROW);
    }
  }

  function flags(r: GameRowDto): string {
    return (r.hasVariations ? "V" : "") + (r.hasComments ? "C" : "") + (r.hasAnnotations ? "A" : "") + (r.hasSetup ? "F" : "");
  }

  const isOpen = (r: GameRowDto) =>
    game.view?.source?.db === db.info?.handle && game.view?.source?.game === r.id;
</script>

<svelte:window
  onmousedown={() => {
    menu = null;
    headMenu = null;
  }}
  onkeydown={(e) => {
    if ((menu || headMenu) && e.key === "Escape") {
      e.preventDefault();
      menu = null;
      headMenu = null;
      viewport?.focus();
    }
  }}
/>

<section class="list" class:resizing={resizing !== null} aria-label="Games" style:--columns={template}>
  {#if db.info}<FilterBar />{/if}
  <div class="row head" role="row" tabindex="-1" oncontextmenu={openHeadMenu}>
    {#each columns as c (c.id)}
      <div class="head-cell" data-column={c.id}>
        {#if c.key}
          <button
            class="cell"
            class:num={c.num}
            title={c.title ?? `Sort by ${c.label.toLowerCase()}`}
            aria-keyshortcuts="Shift+ArrowLeft Shift+ArrowRight"
            disabled={!db.info || total === 0}
            onclick={() => db.toggleSort(c.key!)}
            onkeydown={(e) => resizeKey(e, c)}
          >
            {c.label}{#if db.sortingKey === c.key}<span class="sorting"> sorting…</span>{:else if db.sort.key === c.key && c.key !== "id"}<span
                class="arrow">{db.sort.descending ? " ▼" : " ▲"}</span
              >{:else if c.key === "id" && db.sort.key === "id" && db.sort.descending}<span class="arrow"> ▼</span>{/if}
          </button>
        {:else}
          <!-- Focusable so it can be resized from the keyboard too. -->
          <span
            class="cell"
            class:idle={!db.info || total === 0}
            role="columnheader"
            tabindex="0"
            title={c.title}
            aria-label={c.title}
            aria-keyshortcuts="Shift+ArrowLeft Shift+ArrowRight"
            onkeydown={(e) => resizeKey(e, c)}>{c.label}</span
          >
        {/if}
        <span
          class="grip"
          class:active={resizing === c.id}
          role="presentation"
          title="Drag to resize the {c.name} column; double-click for its default width"
          onpointerdown={(e) => startResize(e, c)}
          ondblclick={() => db.setColumnWidth(c.id, null)}
        ></span>
      </div>
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
            class:selected={contains(picked, index)}
            class:cursor={selected === index && pickedCount > 1}
            class:deleted={row?.deleted}
            class:open={row && isOpen(row)}
            role="row"
            aria-selected={contains(picked, index)}
            tabindex="-1"
            onpointerdown={(e) => onRowDown(e, index)}
            ondblclick={(e) => openRow(row, index, e.ctrlKey || e.metaKey)}
            oncontextmenu={(e) => {
              e.preventDefault();
              headMenu = null;
              openMenuAt(index, e.clientX, e.clientY);
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
        <div>
          <button onclick={() => onboarding.show("import")}>Import PGN files into a new database…</button>
          <button title={onboarding.samplePath || undefined} onclick={() => db.openSample()}>Open the sample games</button>
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
  {@const what = pickedCount > 1 ? `${formatCount(pickedCount)} games` : "game"}
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    style:left="{menu.x}px"
    style:top="{Math.min(menu.y, window.innerHeight - 180)}px"
    onmousedown={(e) => e.stopPropagation()}
    onkeydown={(e) => {
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      e.preventDefault();
      const items = [...e.currentTarget.querySelectorAll<HTMLElement>("[role='menuitem']")];
      const i = items.indexOf(document.activeElement as HTMLElement);
      items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    }}
    {@attach (el) => el.querySelector<HTMLElement>("[role='menuitem']")?.focus()}
  >
    <button role="menuitem" onclick={() => { const i = selected; menu = null; openRow(row, i ?? 0); }}>Open game</button>
    <button role="menuitem" onclick={() => { const i = selected; menu = null; openRow(row, i ?? 0, true); }}>Open in a new tab</button>
    <hr />
    <button role="menuitem" onclick={() => { menu = null; copyDialog = { ranges: $state.snapshot(picked), mode: "copy" }; }}>
      Copy {pickedCount > 1 ? what : "game"} to database…
    </button>
    <button role="menuitem" onclick={() => { menu = null; copyDialog = { ranges: $state.snapshot(picked), mode: "move" }; }}>
      Move {pickedCount > 1 ? what : "game"} to database…
    </button>
    <hr />
    <button role="menuitem" onclick={() => { menu = null; db.setDeletedRows($state.snapshot(picked), !row.deleted); }}>
      {row.deleted ? `Restore ${what}` : `Mark ${what} as deleted`}
    </button>
  </div>
{/if}

{#if headMenu}
  {@const column = headMenu.column}
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    style:left="{Math.min(headMenu.x, window.innerWidth - 240)}px"
    style:top="{headMenu.y}px"
    onmousedown={(e) => e.stopPropagation()}
  >
    <button
      role="menuitem"
      disabled={!db.columns[column.id]}
      onclick={() => { headMenu = null; db.setColumnWidth(column.id, null); }}
    >
      Default width for the {column.name} column
    </button>
    <button
      role="menuitem"
      disabled={Object.keys(db.columns).length === 0}
      onclick={() => { headMenu = null; db.resetColumns(); }}
    >
      Default widths for all columns
    </button>
  </div>
{/if}

{#if copyDialog}
  <CopyGamesDialog
    ranges={copyDialog.ranges}
    mode={copyDialog.mode}
    onClose={() => {
      copyDialog = null;
      viewport?.focus();
    }}
  />
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
    /* From the column list in the script: defaults plus resized widths. */
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

  .head-cell {
    position: relative;
    display: flex;
    min-width: 0;
    height: 100%;
  }

  .head .cell {
    flex: 1;
    min-width: 0;
    padding: 0 6px;
    height: 100%;
    border: 0;
    border-radius: 0;
    text-align: left;
    font-weight: 600;
    line-height: 26px;
  }

  /* Like the disabled sort buttons next to it. */
  .head span.cell.idle {
    opacity: 0.4;
  }

  .head span.cell:focus-visible {
    outline: 2px solid var(--felt);
    outline-offset: -2px;
  }

  /* The column's right edge, dragged to resize it (QOL-07). */
  .grip {
    position: absolute;
    z-index: 1;
    top: 0;
    right: -4px;
    width: 8px;
    height: 100%;
    cursor: col-resize;
    touch-action: none;
  }

  .grip::after {
    content: "";
    position: absolute;
    top: 5px;
    bottom: 5px;
    left: 3px;
    width: 2px;
    border-radius: 1px;
  }

  .grip:hover::after,
  .grip.active::after {
    background: var(--felt);
  }

  .resizing,
  .resizing * {
    cursor: col-resize;
    user-select: none;
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

  /* The cursor row among several selected ones. */
  .row.cursor {
    outline: 1px dotted var(--felt);
    outline-offset: -2px;
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

  .menu hr {
    width: 100%;
    margin: 4px 0;
    border: 0;
    border-top: 1px solid var(--rule);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
</style>
