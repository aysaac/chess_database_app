<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import AnalysisPane from "./lib/components/AnalysisPane.svelte";
  import Board from "./lib/components/Board.svelte";
  import BoardNav from "./lib/components/BoardNav.svelte";
  import EvalBar from "./lib/components/EvalBar.svelte";
  import EvalGraph from "./lib/components/EvalGraph.svelte";
  import GameHeader from "./lib/components/GameHeader.svelte";
  import GameTabs from "./lib/components/GameTabs.svelte";
  import MoveEntry from "./lib/components/MoveEntry.svelte";
  import ListPane from "./lib/components/ListPane.svelte";
  import OpeningTree from "./lib/components/OpeningTree.svelte";
  import Notation from "./lib/components/Notation.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import Toolbar from "./lib/components/Toolbar.svelte";
  import { analysis } from "./lib/stores/analysis.svelte";
  import { board as boardPrefs } from "./lib/stores/board.svelte";
  import { db } from "./lib/stores/db.svelte";
  import { game } from "./lib/stores/game.svelte";
  import { prefs } from "./lib/stores/prefs.svelte";
  import { tree } from "./lib/stores/tree.svelte";

  // The game list takes a share of the window (28% by default), so both panes
  // scale when the window is resized or tiled. Drag the divider to change it.
  let listShare = $state(0.28);
  let games: HTMLDivElement;

  // The board is the largest square that fits above the nav bar while leaving
  // the notation its minimum width. Keep these in step with the CSS below.
  const NOTATION_MIN = 320;
  const EVAL_BAR = 12 + 6; // bar + gap
  const BOARD_CHROME_X = 14 + 14 + 1 + EVAL_BAR; // column padding + divider + eval bar
  const BOARD_CHROME_Y = 14 + 4 + 40; // column padding + BoardNav
  const BOARD_MIN = 160;
  let workspace: HTMLElement;
  let boardSize = $state(BOARD_MIN);

  function fitBoard() {
    const fit = Math.min(
      workspace.clientHeight - BOARD_CHROME_Y,
      workspace.clientWidth - NOTATION_MIN - BOARD_CHROME_X,
    );
    // chessground snaps its board to a multiple of 8 device pixels (whole-pixel
    // squares), so snap the same way or it leaves a strip at fractional scales
    // (Windows at 120%). Half a device pixel over the target keeps its floor()
    // on the target despite layout rounding.
    const dpr = window.devicePixelRatio || 1;
    boardSize = Math.max(BOARD_MIN, (Math.floor((fit * dpr) / 8) * 8 + 0.5) / dpr);
  }

  // The board colours (UI-01) apply to every board through <html>.
  $effect(() => {
    document.documentElement.dataset.boardTheme = boardPrefs.theme;
  });

  onMount(() => {
    game.newGame();
    fitBoard();
    const resize = new ResizeObserver(fitBoard);
    resize.observe(workspace);
    const unlisten = getCurrentWindow().onCloseRequested(async (event) => {
      if (!(await game.confirmDiscard(game.tabs))) event.preventDefault();
    });
    // Restore the last session's preferences and database (QOL-01).
    void (async () => {
      await prefs.load();
      listShare = prefs.get("listShare", listShare);
      game.orientation = prefs.get<string>("orientation", "white") === "black" ? "black" : "white";
      await db.reopenLast();
      void db.loadReference();
      await analysis.loadEngines();
    })();
    return () => {
      resize.disconnect();
      unlisten.then((f) => f());
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
    // unless the list has focus or a picker on the board takes keys.
    const target = e.target as HTMLElement | null;
    const pickerOpen = game.pendingPromotion || game.pendingChoice || game.branch;
    if (
      !mod &&
      !e.altKey &&
      /^[a-hknqroKQRBNO0-9]$/.test(e.key) &&
      game.view &&
      !pickerOpen &&
      !target?.closest?.(".games")
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

  function startResize(e: PointerEvent) {
    const startY = e.clientY;
    const startH = games.offsetHeight;
    const target = e.currentTarget as HTMLElement;
    const appH = (target.parentElement as HTMLElement).clientHeight;
    target.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      listShare = Math.max(0.1, Math.min(0.7, (startH - (ev.clientY - startY)) / appH));
    };
    const up = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
      prefs.set("listShare", Math.round(listShare * 1000) / 1000);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="app" style:--list-h="{listShare * 100}%" style:--board="{boardSize}px">
  <Toolbar />
  <GameTabs />

  <main class="workspace" bind:this={workspace}>
    <section class="board-column" aria-label="Board">
      <div class="board-row">
        <div class="eval-bar"><EvalBar /></div>
        <div class="board-square">
          <Board />
          {#if game.typing !== null}<MoveEntry />{/if}
        </div>
      </div>
      <BoardNav />
    </section>
    <section class="game-column" aria-label="Game">
      <GameHeader />
      <div class="notation"><Notation /></div>
      {#if tree.visible}<OpeningTree />{/if}
      <EvalGraph />
      {#if analysis.status.running || analysis.starting}
        <AnalysisPane />
      {/if}
    </section>
  </main>

  <div
    class="resizer"
    role="separator"
    aria-orientation="horizontal"
    aria-label="Resize game list"
    onpointerdown={startResize}
  ></div>

  <div class="games" bind:this={games}><ListPane /></div>

  <StatusBar />
</div>

<style>
  .app {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) 5px minmax(120px, var(--list-h)) auto;
    height: 100%;
  }

  /* Paddings and the notation's minimum width are mirrored in fitBoard(). */
  .workspace {
    display: grid;
    grid-template-columns: auto minmax(320px, 1fr);
    min-width: 0;
    min-height: 0;
  }

  .board-column {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 14px 14px 4px;
    min-height: 0;
    border-right: 1px solid var(--rule);
  }

  .board-row {
    display: flex;
    gap: 6px;
  }

  .eval-bar {
    width: 12px;
    height: var(--board);
  }

  /* Sized in pixels (see fitBoard) so the auto column can measure it. */
  .board-square {
    position: relative;
    width: var(--board);
    height: var(--board);
    flex: none;
  }

  .game-column {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .notation {
    flex: 1;
    min-height: 0;
  }

  .resizer {
    cursor: row-resize;
    border-top: 1px solid var(--rule);
    background: var(--panel);
  }

  .resizer:hover {
    background: var(--felt-soft);
  }

  .games {
    min-height: 0;
  }
</style>
