<script lang="ts">
  import { onMount } from "svelte";
  import { Chessground } from "@lichess-org/chessground";
  import type { Api } from "@lichess-org/chessground/api";
  import type { DrawShape } from "@lichess-org/chessground/draw";
  import type { Key } from "@lichess-org/chessground/types";
  import type { ShapeDto } from "../bindings/ShapeDto";
  import { game } from "../stores/game.svelte";
  import PromotionPicker from "./PromotionPicker.svelte";

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
</style>
