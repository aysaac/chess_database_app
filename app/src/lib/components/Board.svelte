<script lang="ts">
  import { onMount } from "svelte";
  import { Chessground } from "@lichess-org/chessground";
  import type { Api } from "@lichess-org/chessground/api";
  import type { DrawBrush, DrawBrushes, DrawShape } from "@lichess-org/chessground/draw";
  import type { Key } from "@lichess-org/chessground/types";
  import type { NotationMoveDto } from "../bindings/NotationMoveDto";
  import type { ShapeDto } from "../bindings/ShapeDto";
  import { analysis } from "../stores/analysis.svelte";
  import { board as boardPrefs } from "../stores/board.svelte";
  import { game } from "../stores/game.svelte";
  import PromotionPicker from "./PromotionPicker.svelte";

  // The colour here only fills a label; the CSS gives the arrow its own.
  const THREAT_BRUSH: DrawBrush = { key: "threat", color: "#a23a2c", opacity: 0.9, lineWidth: 13 };

  let el: HTMLDivElement;
  let cg: Api | undefined = $state();

  function toDests(dests: { [key in string]?: string[] }): Map<Key, Key[]> {
    const map = new Map<Key, Key[]>();
    for (const [from, tos] of Object.entries(dests)) map.set(from as Key, (tos ?? []) as Key[]);
    return map;
  }

  function toShapes(shapes: ShapeDto[]): DrawShape[] {
    return shapes.map((s) => ({ orig: s.orig as Key, dest: s.dest as Key | undefined, brush: s.brush }));
  }

  onMount(() => {
    cg = Chessground(el, {
      coordinates: true,
      animation: { enabled: true, duration: 160 },
      highlight: { lastMove: true, check: true },
      premovable: { enabled: false },
      movable: {
        free: false,
        showDests: true,
        events: { after: (orig, dest) => game.play(orig, dest) },
      },
      drawable: {
        enabled: true,
        // Added to chessground's own brushes (its config merges). The
        // threat arrow (ENG-07) is dashed, in --danger, by the CSS below.
        brushes: { threat: THREAT_BRUSH } as Partial<DrawBrushes> as DrawBrushes,
        onChange: (shapes) =>
          game.setShapes(
            shapes
              .filter((s) => s.brush)
              .map((s) => ({ orig: s.orig, dest: s.dest, brush: s.brush! })),
          ),
      },
    });
    const resize = new ResizeObserver(() => cg?.redrawAll());
    resize.observe(el);
    return () => {
      resize.disconnect();
      cg?.destroy();
    };
  });

  // Sync the board with the current view.
  $effect(() => {
    const view = game.view;
    void game.boardVersion;
    if (!cg || !view) return;
    const turn = view.turn === "black" ? "black" : "white";
    cg.set({
      fen: view.fen,
      orientation: game.orientation,
      turnColor: turn,
      lastMove: (view.lastMove ?? undefined) as Key[] | undefined,
      check: view.check,
      movable: { color: turn, dests: toDests(view.dests) },
    });
    cg.setShapes(toShapes(view.shapes));
  });

  // Display options (UI-01). Coordinates are drawn once, so they need a redraw.
  $effect(() => {
    const coordinates = boardPrefs.coordinates;
    const showDests = boardPrefs.legalMoves;
    const lastMove = boardPrefs.lastMove;
    if (!cg) return;
    cg.set({ coordinates, movable: { showDests }, highlight: { lastMove, check: true } });
    cg.redrawAll();
  });

  // While the next-move picker is open, its moves show as arrows (the
  // highlighted one on top), over the opponent's threat (ENG-07) while the
  // engine shows it. Auto shapes aren't part of the game's drawings.
  $effect(() => {
    const branch = game.branch;
    const selected = game.branchSelected;
    const threat = analysis.threat?.line?.moves[0];
    if (!cg) return;
    const arrow = (m: { uci: string }, brush: string): DrawShape => ({
      orig: m.uci.slice(0, 2) as Key,
      dest: m.uci.slice(2, 4) as Key,
      brush,
    });
    const others = (branch ?? []).filter((_, i) => i !== selected);
    const arrows = threat ? [arrow(threat, "threat")] : [];
    arrows.push(...others.map((m: NotationMoveDto) => arrow(m, "paleGrey")));
    if (branch?.[selected]) arrows.push(arrow(branch[selected], "blue"));
    cg.setAutoShapes(arrows);
  });
</script>

<div class="board chessboard" bind:this={el}></div>
{#if game.pendingPromotion}
  <PromotionPicker
    color={game.pendingPromotion.color}
    onPick={(role) => game.choosePromotion(role)}
  />
{/if}

<style>
  .board {
    width: 100%;
    height: 100%;
  }

  /* The threat arrow (ENG-07): dashed and in --danger, unlike the solid
     arrows the user draws and the next-move picker's. CSS overrides the
     colours chessground writes as attributes. */
  .board :global(svg.cg-shapes line[marker-end="url(#arrowhead-threat)"]) {
    stroke: var(--danger);
    stroke-linecap: butt;
    stroke-dasharray: 0.24 0.12;
  }

  .board :global(svg.cg-shapes marker#arrowhead-threat path) {
    fill: var(--danger);
  }
</style>
