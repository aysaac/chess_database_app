<script lang="ts">
  // The Reference tab (OPEN-14): what is played in the position on the
  // board, how often lately, by whom and in which strong games. Three
  // sections, with dividers that can be dragged (or moved with ↑/↓ when
  // focused; double-click resets) and whose sizes are remembered: the moves
  // table, the main variations, and the top games with the mini board, side
  // by side when the tab is wide and stacked when it's narrow (a container
  // query). It reads the reference database, or the open one when none is
  // set, and says so.
  import { untrack } from "svelte";
  import { formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { DEFAULT_SIZES, reference, type ReferenceSizes } from "../stores/reference.svelte";
  import MiniBoard from "./MiniBoard.svelte";
  import ReferenceLines from "./ReferenceLines.svelte";
  import ReferenceMoves from "./ReferenceMoves.svelte";
  import TopGames from "./TopGames.svelte";

  // Only while the tab is showing (like the opening tree, UI-19): query when
  // it's shown, then again when the position, the database or its index
  // changes. Hidden, it drops what it asked for.
  $effect(() => {
    if (!reference.showing) {
      untrack(() => reference.hide());
      return;
    }
    void game.view?.fen;
    void game.view?.session;
    void db.info?.handle;
    void db.info?.index.covered;
    void db.info?.games;
    void db.reference?.info?.handle;
    void db.reference?.info?.index.covered;
    void db.reference?.info?.games;
    void reference.target?.handle;
    const t = setTimeout(() => reference.refresh(), 60);
    return () => clearTimeout(t);
  });

  // A move on the main board takes the mini board back to its position.
  const boardFen = $derived(game.view?.fen);
  $effect(() => {
    void boardFen;
    untrack(() => reference.reset());
  });

  // The view of the position on the board or, until it arrives, the last
  // one (dimmed and inert), so the tab doesn't flash on every move.
  const data = $derived(reference.current ?? reference.stale);
  const stale = $derived(!reference.current && !!data);
  const target = $derived(reference.target);

  // ---- section sizes ---------------------------------------------------

  /** Each section keeps at least this share of the height. */
  const MIN_SHARE = 0.1;
  const KEY_STEP = 16;
  let sectionsEl: HTMLElement | undefined = $state();
  /** Sizes while dragging (and after), before the pref has them. */
  let local = $state<ReferenceSizes | null>(null);
  const sizes = $derived(local ?? reference.sizes);

  /** `which` divider moved by `dy` pixels: the boundary between its two sections moves. */
  function moved(start: ReferenceSizes, which: "moves" | "lines", dy: number): ReferenceSizes {
    // Percentage flex bases resolve against the whole height.
    const d = dy / Math.max(1, sectionsEl?.clientHeight ?? 1);
    if (which === "moves") {
      const both = start.moves + start.lines;
      const moves = Math.max(MIN_SHARE, Math.min(both - MIN_SHARE, start.moves + d));
      return { moves, lines: both - moves };
    }
    const lines = Math.max(MIN_SHARE, Math.min(1 - start.moves - MIN_SHARE, start.lines + d));
    return { moves: start.moves, lines };
  }

  /** Drags a divider; the sizes are saved when it's let go. Window
   * listeners, like the game list's column edges, so a drag past the
   * divider keeps going. */
  function startDrag(e: PointerEvent, which: "moves" | "lines") {
    if (e.button !== 0) return;
    e.preventDefault();
    const start = sizes;
    const y0 = e.clientY;
    const move = (ev: PointerEvent) => {
      // Released outside the window: no pointerup came.
      if ((ev.buttons & 1) === 0) return end();
      local = moved(start, which, ev.clientY - y0);
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      if (local) reference.sizes = local;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  }

  function dividerKey(e: KeyboardEvent, which: "moves" | "lines") {
    const dy = { ArrowUp: -KEY_STEP, ArrowDown: KEY_STEP }[e.key as "ArrowUp"];
    if (!dy || e.ctrlKey || e.metaKey || e.altKey) return;
    e.preventDefault();
    e.stopPropagation();
    local = moved(sizes, which, dy);
    reference.sizes = local;
  }

  function resetSizes() {
    local = { ...DEFAULT_SIZES };
    reference.sizes = local;
  }
</script>

<div class="reference">
  <header>
    <!-- "Mega 2019 · E05 Catalan, open · 14,276 games" -->
    <span class="name"
      >{#if target}<span class="db">{target.name}</span>{/if}{#if data?.opening}{" · "}{data.opening.eco}
        {data.opening.name}{/if}{#if data}{" · "}{formatCount(data.total.games)}
        {data.total.games === 1 ? "game" : "games"}{/if}</span
    >
    <button
      disabled={!target || !game.view || db.searching}
      title={target?.reference && target.handle !== db.info?.handle
        ? `Open ${target.name} in the game list with the games that reach this position`
        : "Show the games that reach this position in the game list"}
      onclick={() => reference.findGames()}>Find games</button
    >
  </header>
  {#if reference.fallbackNote}
    <p class="fallback" role="note">{reference.fallbackNote}</p>
  {/if}

  {#if !target}
    <p class="note">
      Open a database, or choose a reference database in the ribbon's Reference group, to see which moves
      were played here, by whom and in which games.
    </p>
  {:else if data && !data.indexed}
    <p class="note">The database's positions are being indexed; the tab fills in when that's done.</p>
  {:else if data && data.total.games === 0}
    <p class="note">
      No game in {target.name} reaches this position{data.beyondIndex
        ? ` within the first ${data.maxPly / 2} moves (the indexed part of each game)`
        : ""}.
      {#if data.beyondIndex && game.view}
        <button class="link" onclick={() => reference.findGames(true)}>Search every move of every game</button>
      {/if}
    </p>
  {:else if data}
    <div class="sections" class:stale aria-busy={stale} bind:this={sectionsEl}>
      <section class="moves" aria-label="Moves" style:flex-basis="{sizes.moves * 100}%">
        <div class="scroll" inert={stale}>
          <ReferenceMoves {data} />
        </div>
        {#if data.partial}
          <p class="note small">Recently added games are still being indexed and aren't counted yet.</p>
        {/if}
      </section>
      <!-- A focusable separator is the WAI-ARIA window splitter (↑/↓ move it). -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <div
        class="divider"
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize the moves and the main variations"
        aria-valuenow={Math.round(sizes.moves * 100)}
        aria-valuemin={10}
        aria-valuemax={80}
        tabindex="0"
        title="Drag to resize; double-click for the default sizes"
        onpointerdown={(e) => startDrag(e, "moves")}
        onkeydown={(e) => dividerKey(e, "moves")}
        ondblclick={resetSizes}
      ></div>
      <section class="lines" aria-label="Main variations" style:flex-basis="{sizes.lines * 100}%">
        <ReferenceLines />
      </section>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <div
        class="divider"
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize the main variations and the top games"
        aria-valuenow={Math.round((sizes.moves + sizes.lines) * 100)}
        aria-valuemin={20}
        aria-valuemax={90}
        tabindex="0"
        title="Drag to resize; double-click for the default sizes"
        onpointerdown={(e) => startDrag(e, "lines")}
        onkeydown={(e) => dividerKey(e, "lines")}
        ondblclick={resetSizes}
      ></div>
      <section class="bottom" aria-label="Top games and mini board">
        <div class="games-cell" inert={stale}><TopGames games={data.topGames} /></div>
        <div class="mini-cell"><MiniBoard /></div>
      </section>
    </div>
  {:else if reference.loading}
    <p class="note">Looking up the position…</p>
  {/if}
</div>

<style>
  .reference {
    container: reference / inline-size;
    display: flex;
    flex-direction: column;
    height: 100%;
    font-size: 13px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 10px 4px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
  }

  .name .db {
    color: var(--ink);
    font-weight: 600;
  }

  header button {
    padding: 1px 9px;
    border-color: var(--rule);
  }

  .fallback {
    margin: 0;
    padding: 4px 14px;
    background: var(--brass-soft);
    border-bottom: 1px solid var(--rule);
    color: var(--ink);
  }

  .note {
    margin: 10px 14px;
    color: var(--ink-soft);
  }

  .note.small {
    margin: 4px 14px;
  }

  .link {
    padding: 0;
    color: var(--felt);
    text-decoration: underline;
  }

  .sections {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* The last position's view, until this one's arrives. */
  .stale :is(.moves, .games-cell) {
    opacity: 0.55;
  }

  section {
    display: flex;
    flex-direction: column;
    flex-grow: 0;
    flex-shrink: 1;
    min-height: 0;
    overflow: hidden;
  }

  .moves .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 10px 0 14px;
  }

  .divider {
    flex: none;
    height: 5px;
    cursor: row-resize;
    background: var(--rule);
    border-block: 2px solid var(--paper);
  }

  .divider:hover,
  .divider:focus-visible {
    background: var(--felt);
  }

  /* Wide: the top games and the mini board side by side. */
  .bottom {
    flex: 1 1 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) clamp(200px, 38%, 312px);
  }

  .games-cell,
  .mini-cell {
    min-width: 0;
    min-height: 0;
  }

  .mini-cell {
    border-left: 1px solid var(--rule);
  }

  /* Narrow (a thin side): stacked, the mini board under the games. */
  @container reference (width < 560px) {
    .bottom {
      display: flex;
      flex-direction: column;
      overflow-y: auto;
    }

    .games-cell {
      flex: 1 0 140px;
    }

    .mini-cell {
      flex: none;
      height: calc(min(280px, 100cqw - 24px) + 78px);
      border-left: 0;
      border-top: 1px solid var(--rule);
    }
  }
</style>
