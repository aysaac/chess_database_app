<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api } from "./lib/api";
  import Board from "./lib/components/Board.svelte";
  import BoardNav from "./lib/components/BoardNav.svelte";
  import Dock from "./lib/components/Dock.svelte";
  import DropShelf from "./lib/components/DropShelf.svelte";
  import EvalBar from "./lib/components/EvalBar.svelte";
  import FenPaste from "./lib/components/FenPaste.svelte";
  import GameTabs from "./lib/components/GameTabs.svelte";
  import MoveEntry from "./lib/components/MoveEntry.svelte";
  import MovePopovers from "./lib/components/MovePopovers.svelte";
  import Ribbon from "./lib/components/Ribbon.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import WelcomeGuide from "./lib/components/WelcomeGuide.svelte";
  import { analysis } from "./lib/stores/analysis.svelte";
  import { db } from "./lib/stores/db.svelte";
  import { drag } from "./lib/stores/drag.svelte";
  import { game } from "./lib/stores/game.svelte";
  import { onboarding } from "./lib/stores/onboarding.svelte";
  import { panels } from "./lib/stores/panels.svelte";
  import { prefs } from "./lib/stores/prefs.svelte";
  import { beforeClose } from "./lib/windowClose";

  // The board is the largest square that fits the middle of the dock
  // (UI-04) above the nav bar, whatever the sides around it take. Keep these
  // in step with the CSS below and BoardNav's.
  const EVAL_BAR = 12 + 6; // bar + gap
  const BOARD_PAD_X = 14 + 14; // the column's padding
  const BOARD_PAD_Y = 14 + 4;
  const BOARD_MIN = 160;
  /** The nav bar's height on one row, and on two. */
  const NAV_ONE_ROW = 40;
  const NAV_TWO_ROWS = 74;
  /** Middle widths below which the nav bar is compact, and then takes two rows. */
  const NAV_COMPACT = 480;
  const NAV_WRAPS = 330;
  let column: HTMLElement;
  let navBox: HTMLElement;
  let boardSize = $state(BOARD_MIN);
  let navCompact = $state(false);
  let appHeight = $state(0);

  /**
   * The room the middle keeps (UI-04): a board of BOARD_MIN with its eval
   * bar, padding and nav bar. Dividers stop there, and sides shrink to keep
   * it when the window is small.
   */
  function middleMin(width: number) {
    const nav = width < NAV_WRAPS ? NAV_TWO_ROWS : NAV_ONE_ROW;
    return { w: BOARD_MIN + BOARD_PAD_X + EVAL_BAR, h: BOARD_MIN + BOARD_PAD_Y + nav };
  }

  function fitBoard() {
    const w = column.clientWidth;
    navCompact = w < NAV_COMPACT;
    const fit = Math.min(column.clientHeight - BOARD_PAD_Y - navBox.offsetHeight, w - BOARD_PAD_X - EVAL_BAR);
    // chessground snaps its board to a multiple of 8 device pixels (whole-pixel
    // squares), so snap the same way or it leaves a strip at fractional scales
    // (Windows at 120%). Half a device pixel over the target keeps its floor()
    // on the target despite layout rounding.
    const dpr = window.devicePixelRatio || 1;
    boardSize = Math.max(BOARD_MIN, (Math.floor((fit * dpr) / 8) * 8 + 0.5) / dpr);
  }

  onMount(() => {
    game.newGame();
    fitBoard();
    // The middle changes with the window and the sides; the nav bar's height
    // when it wraps.
    const resize = new ResizeObserver(fitBoard);
    resize.observe(column);
    resize.observe(navBox);
    // The snap depends on the device pixel ratio, which changes without a
    // resize when the window moves to a monitor with another scale or the
    // Windows scale changes. A media query matches only the current ratio,
    // so listen again after each change.
    let dprQuery: MediaQueryList | undefined;
    const watchDpr = () => {
      dprQuery?.removeEventListener("change", onDprChange);
      dprQuery = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      dprQuery.addEventListener("change", onDprChange);
    };
    const onDprChange = () => {
      fitBoard();
      watchDpr();
    };
    watchDpr();
    // Closing the window asks about unsaved games first; it then saves the
    // preferences (windowClose.ts, set up for every window in main.ts).
    const stopCheck = beforeClose(() => game.confirmDiscard(game.tabs));
    // The preferences are loaded before any window mounts (main.ts), and
    // the panel layout (UI-04) follows from them (panels.svelte.ts). Open
    // whatever database the library window spawned this editor window for
    // (DB-07); it's the library, not this window, that remembers "last
    // used" now.
    void (async () => {
      game.orientation = prefs.get<string>("orientation", "white") === "black" ? "black" : "white";
      const target = await api.takeEditorTarget(getCurrentWindow().label).catch(() => null);
      if (target) await db.openAt(target);
      else await db.loadRecent();
      // The welcome guide on a first run (QOL-08).
      void onboarding.check();
      void db.loadReference();
      await analysis.loadEngines();
    })();
    return () => {
      resize.disconnect();
      dprQuery?.removeEventListener("change", onDprChange);
      stopCheck();
    };
  });

  function isTyping(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    return !!t && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
  }

  function onKey(e: KeyboardEvent) {
    if (isTyping(e) || document.querySelector("[aria-modal='true']")) return;
    const mod = e.ctrlKey || e.metaKey;
    const key = e.key.toLowerCase();
    if (mod && e.shiftKey && key === "a") {
      e.preventDefault();
      analysis.toggle();
      return;
    }
    if (e.ctrlKey && key === "tab") {
      e.preventDefault();
      game.cycleTab(e.shiftKey ? -1 : 1);
      return;
    }
    // Typing a move (UI-02): a file or piece letter, O or a digit starts it,
    // unless the list or an open ribbon menu has focus or a picker on the
    // board takes keys.
    const target = e.target as HTMLElement | null;
    const pickerOpen = game.pendingPromotion || game.pendingChoice || game.branch;
    if (
      !mod &&
      !e.altKey &&
      /^[a-hknqroKQRBNO0-9]$/.test(e.key) &&
      game.view &&
      !pickerOpen &&
      !target?.closest?.("#panel-games, [data-popup]")
    ) {
      e.preventDefault();
      game.typing = e.key;
      return;
    }
    const actions: Record<string, () => unknown> = mod
      ? {
          z: () => (e.shiftKey ? game.redo() : game.undo()),
          y: () => game.redo(),
          s: () => (e.shiftKey ? game.saveAsNew() : game.save()),
          f: () => game.flip(),
          // New game (UI-18), in this tab unless it has unsaved changes.
          n: () => !e.shiftKey && game.newGame(),
          t: () => game.newGame(null, "tab"),
          w: () => game.activeSession !== null && game.closeTab(game.activeSession),
          arrowup: () => game.openAdjacent(-1),
          arrowdown: () => game.openAdjacent(1),
        }
      : {
          arrowleft: () => game.step("back"),
          arrowright: () => game.forward(),
          home: () => game.step("start"),
          end: () => game.step("end"),
        };
    const action = actions[key];
    if (action) {
      e.preventDefault();
      action();
    }
  }
</script>

<svelte:window onkeydown={onKey} />
<FenPaste />
<MovePopovers />

<!-- The panels dock to the four sides (UI-04, Dock.svelte), from the panel
     layout (UI-19, panels.svelte.ts); the board stays in the middle. -->
<div class="app" bind:clientHeight={appHeight} style:--board="{boardSize}px">
  <GameTabs />
  <Ribbon />

  <Dock {appHeight} {middleMin}>
    <section class="board-column" aria-label="Board" bind:this={column}>
      <div class="board-row">
        <div class="eval-bar"><EvalBar /></div>
        <!-- Games dragged from the list open here (QOL-04). -->
        <div class="board-square" data-drop="board" class:drop-over={drag.target?.kind === "board"}>
          <Board />
          {#if game.typing !== null}<MoveEntry />{/if}
        </div>
      </div>
      <div class="nav-box" bind:this={navBox}><BoardNav compact={navCompact} /></div>
      <!-- The drop shelf for dragged games covers the notation's group, or
           here when every group shows the game list. -->
      {#if panels.shelfHost === null}<DropShelf />{/if}
    </section>
  </Dock>

  <StatusBar />
</div>

{#if onboarding.open}<WelcomeGuide />{/if}

<style>
  .app {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto;
    height: 100%;
  }

  /* Fills the middle of the dock; the board and nav bar are centred in it.
     Paddings are mirrored in fitBoard(). */
  .board-column {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    padding: 14px 14px 4px;
    overflow: hidden;
  }

  .board-row {
    display: flex;
    gap: 6px;
  }

  .eval-bar {
    width: 12px;
    height: var(--board);
  }

  /* Sized in pixels (see fitBoard). */
  .board-square {
    position: relative;
    width: var(--board);
    height: var(--board);
    flex: none;
  }

  .board-square.drop-over {
    outline: 3px solid var(--felt);
    outline-offset: 3px;
  }

  /* As wide as the column, so the nav bar wraps in a narrow one. */
  .nav-box {
    align-self: stretch;
    flex: none;
  }
</style>
