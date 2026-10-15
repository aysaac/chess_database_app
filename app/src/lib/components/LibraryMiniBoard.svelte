<script lang="ts">
  // The database manager's mini board (DB-07): steps through the selected
  // sample game's main line, view-only. A smaller, standalone cousin of the
  // Reference tab's MiniBoard (no main board to sync with here).
  import { onMount } from "svelte";
  import { Chessground } from "@lichess-org/chessground";
  import type { Api } from "@lichess-org/chessground/api";
  import type { Key } from "@lichess-org/chessground/types";
  import type { GameLineDto } from "../bindings/GameLineDto";
  import { figurine } from "../format";
  import { moveLabel } from "../stores/reference.svelte";

  let { line, loading }: { line: GameLineDto | null; loading: boolean } = $props();

  const START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

  let el: HTMLDivElement;
  let cg: Api | undefined = $state();
  let index = $state(0);

  // A new game resets to its start.
  $effect(() => {
    void line?.id;
    index = line?.start ?? 0;
  });

  const shown = $derived(line?.positions[index]);
  const fen = $derived(shown?.fen ?? START);
  const lastMove = $derived(shown?.uci ? ([shown.uci.slice(0, 2), shown.uci.slice(2, 4)] as Key[]) : undefined);
  const atStart = $derived(!line || index === 0);
  const atEnd = $derived(!line || index === line.positions.length - 1);
  const caption = $derived.by(() => {
    if (!line) return loading ? "Loading…" : "Pick a database to see a sample game";
    const who = shown?.san ? figurine(moveLabel(shown)) : "Start";
    return `${who} · ${line.white} – ${line.black}`;
  });

  onMount(() => {
    cg = Chessground(el, {
      viewOnly: true,
      coordinates: false,
      animation: { enabled: true, duration: 120 },
      highlight: { lastMove: true, check: false },
      drawable: { enabled: false, visible: false },
    });
    return () => cg?.destroy();
  });

  $effect(() => {
    cg?.set({ fen, lastMove });
  });

  function step(dir: "start" | "back" | "forward" | "end") {
    if (!line) return;
    const last = line.positions.length - 1;
    index = { start: 0, back: Math.max(0, index - 1), forward: Math.min(last, index + 1), end: last }[dir];
  }

  function onKey(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey || !line) return;
    const dir = ({ ArrowLeft: "back", ArrowRight: "forward", Home: "start", End: "end" } as const)[
      e.key as "ArrowLeft"
    ];
    if (!dir) return;
    e.preventDefault();
    e.stopPropagation();
    step(dir);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="mini"
  role="group"
  aria-label="Sample game board: {caption}"
  aria-keyshortcuts="ArrowLeft ArrowRight Home End"
  tabindex="0"
  onkeydown={onKey}
>
  <div class="board chessboard" class:dim={!line} bind:this={el}></div>
  <div class="caption" title={caption}>{caption}</div>
  <div class="nav" role="toolbar" aria-label="Step through the sample game">
    <button aria-label="First move" title="First move (Home)" disabled={atStart} onclick={() => step("start")}>
      <svg viewBox="0 0 16 16"><path d="M3 3h2v10H3zM13 3v10L6 8z" /></svg>
    </button>
    <button aria-label="Previous move" title="Previous move (Left arrow)" disabled={atStart} onclick={() => step("back")}>
      <svg viewBox="0 0 16 16"><path d="M11 3v10L4 8z" /></svg>
    </button>
    <button aria-label="Next move" title="Next move (Right arrow)" disabled={atEnd} onclick={() => step("forward")}>
      <svg viewBox="0 0 16 16"><path d="M5 3v10l7-5z" /></svg>
    </button>
    <button aria-label="Last move" title="Last move (End)" disabled={atEnd} onclick={() => step("end")}>
      <svg viewBox="0 0 16 16"><path d="M3 3v10l7-5zM11 3h2v10h-2z" /></svg>
    </button>
  </div>
</div>

<style>
  .mini {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 4px 0 2px;
    border-radius: var(--radius);
  }

  .mini:focus-visible {
    outline-offset: -2px;
  }

  .board {
    width: clamp(160px, 100%, 240px);
    aspect-ratio: 1;
  }

  .board.dim {
    opacity: 0.55;
  }

  .caption {
    width: 100%;
    max-width: 240px;
    height: 18px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: center;
    color: var(--ink-soft);
    font-size: 12.5px;
  }

  .nav {
    display: flex;
    gap: 3px;
  }

  .nav button {
    display: grid;
    place-items: center;
    min-width: 30px;
    height: 26px;
    padding: 0 5px;
    border-color: var(--rule);
  }

  svg {
    width: 13px;
    height: 13px;
    fill: currentColor;
  }
</style>
