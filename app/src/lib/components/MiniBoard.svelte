<script lang="ts">
  // The Reference tab's mini board (OPEN-14 3d): a small, view-only board
  // that steps through a top game or a line on its own, starting from the
  // main board's position. It never changes the main board, and goes back
  // to its position whenever the main board moves. ←/→ (and Home/End) step
  // while it has the focus.
  import { onMount } from "svelte";
  import { Chessground } from "@lichess-org/chessground";
  import type { Api } from "@lichess-org/chessground/api";
  import type { Key } from "@lichess-org/chessground/types";
  import { figurine } from "../format";
  import { game } from "../stores/game.svelte";
  import { moveLabel, reference } from "../stores/reference.svelte";

  // The board is the largest square that fits its area, between these, above
  // the caption and the buttons (keep CHROME in step with the CSS below).
  const MIN = 120;
  const MAX = 280;
  const CHROME = 8 + 6 + 20 + 6 + 30 + 8; // padding, gap, caption, gap, buttons, padding
  const START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

  let area: HTMLDivElement;
  let el: HTMLDivElement;
  let cg: Api | undefined = $state();
  let size = $state(MIN);

  // A game or line loaded at another position of the main board is stale.
  const mini = $derived(reference.mini && reference.mini.fen === game.view?.fen ? reference.mini : null);
  const shown = $derived(mini ? mini.positions[mini.index] : null);
  const fen = $derived(shown?.fen ?? game.view?.fen ?? START);
  const lastMove = $derived(
    shown ? (shown.uci ? [shown.uci.slice(0, 2), shown.uci.slice(2, 4)] : undefined) : (game.view?.lastMove ?? undefined),
  );
  const atStart = $derived(!mini || mini.index === 0);
  const atEnd = $derived(!mini || mini.index === mini.positions.length - 1);

  const caption = $derived.by(() => {
    if (!mini || !shown) return "The position on the board";
    const where = shown.san
      ? figurine(moveLabel(shown))
      : mini.kind === "game" && mini.start > 0
        ? "Start"
        : "The board's position";
    return `${where} · ${mini.title}`;
  });

  function fit() {
    const room = Math.min(MAX, area.clientWidth - 16, area.clientHeight - CHROME);
    // Whole squares in device pixels, as chessground draws them (see
    // fitBoard() in App.svelte), so it leaves no strip.
    const dpr = window.devicePixelRatio || 1;
    size = Math.max(MIN, (Math.floor((room * dpr) / 8) * 8 + 0.5) / dpr);
  }

  onMount(() => {
    cg = Chessground(el, {
      viewOnly: true,
      coordinates: false,
      animation: { enabled: true, duration: 120 },
      highlight: { lastMove: true, check: false },
      drawable: { enabled: false, visible: false },
    });
    fit();
    const resize = new ResizeObserver(() => {
      fit();
      cg?.redrawAll();
    });
    resize.observe(area);
    resize.observe(el);
    return () => {
      resize.disconnect();
      cg?.destroy();
    };
  });

  $effect(() => {
    cg?.set({ fen, orientation: game.orientation, lastMove: lastMove as Key[] | undefined });
  });

  function onKey(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
    const dir = ({ ArrowLeft: "back", ArrowRight: "forward", Home: "start", End: "end" } as const)[
      e.key as "ArrowLeft"
    ];
    if (!dir) return;
    // These keys step the mini board only, never the main one.
    e.preventDefault();
    e.stopPropagation();
    reference.step(dir);
  }
</script>

<div class="mini-area" bind:this={area}>
  <!-- Focusable so ←/→ step the mini board (OPEN-14 3d), like the game list's grid. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="mini"
    role="group"
    aria-label="Mini board: {caption}"
    aria-keyshortcuts="ArrowLeft ArrowRight Home End"
    tabindex="0"
    onkeydown={onKey}
    style:--mini="{size}px"
  >
    <div class="board chessboard" bind:this={el}></div>
    <div class="caption" title={caption}>{caption}</div>
    <div class="nav" role="toolbar" aria-label="Step through the game or line">
      <button aria-label="First move" title="First move (Home)" disabled={atStart} onclick={() => reference.step("start")}>
        <svg viewBox="0 0 16 16"><path d="M3 3h2v10H3zM13 3v10L6 8z" /></svg>
      </button>
      <button aria-label="Previous move" title="Previous move (Left arrow)" disabled={atStart} onclick={() => reference.step("back")}>
        <svg viewBox="0 0 16 16"><path d="M11 3v10L4 8z" /></svg>
      </button>
      <button aria-label="Next move" title="Next move (Right arrow)" disabled={atEnd} onclick={() => reference.step("forward")}>
        <svg viewBox="0 0 16 16"><path d="M5 3v10l7-5z" /></svg>
      </button>
      <button aria-label="Last move" title="Last move (End)" disabled={atEnd} onclick={() => reference.step("end")}>
        <svg viewBox="0 0 16 16"><path d="M3 3v10l7-5zM11 3h2v10h-2z" /></svg>
      </button>
      <button
        aria-label="Back to the board's position"
        title="Back to the main board's position"
        disabled={!mini}
        onclick={() => reference.reset()}
      >
        <svg viewBox="0 0 16 16">
          <path d="M4.3 4.1A5.6 5.6 0 1 1 2.4 8.4" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" />
          <path d="M1.4 1.6v4.9h4.9z" />
        </svg>
      </button>
    </div>
  </div>
</div>

<style>
  .mini-area {
    display: flex;
    justify-content: center;
    align-items: flex-start;
    width: 100%;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }

  /* As wide as its area, so the caption has room beside a small board. */
  .mini {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 6px;
    border-radius: var(--radius);
  }

  .mini:focus-visible {
    outline-offset: -2px;
  }

  .board {
    width: var(--mini);
    height: var(--mini);
    flex: none;
  }

  .caption {
    width: 100%;
    height: 20px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: center;
    color: var(--ink-soft);
    font-size: 13px;
  }

  .nav {
    display: flex;
    gap: 3px;
    height: 30px;
  }

  .nav button {
    display: grid;
    place-items: center;
    min-width: 34px;
    height: 28px;
    padding: 0 6px;
    border-color: var(--rule);
  }

  svg {
    width: 14px;
    height: 14px;
    fill: currentColor;
  }
</style>
