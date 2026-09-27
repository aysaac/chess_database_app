<script lang="ts">
  import { formatEval, parseEval } from "../format";
  import { whiteChance } from "../stores/analysis.svelte";
  import { game } from "../stores/game.svelte";

  // Evaluation graph of the main line (ANL-04), from the `[%eval]` stored
  // with each move (Lichess games, the blunder check). Click to jump.
  const H = 100;

  const points = $derived(
    (game.notation?.moves ?? []).map((m, i) => {
      const e = parseEval(m.eval);
      return { id: m.id, ply: i + 1, eval: e, chance: e ? whiteChance(e) : null, nags: m.nags, san: m.san };
    }),
  );
  const known = $derived(points.filter((p) => p.chance !== null));
  const plies = $derived(points.length);

  // White's share as an area over the baseline, missing evals interpolated.
  const area = $derived.by(() => {
    if (known.length < 2) return "";
    const pts = known.map((p) => `${p.ply},${(1 - p.chance!) * H}`);
    return `M${known[0].ply},${H} L${pts.join(" L")} L${known[known.length - 1].ply},${H} Z`;
  });
  const line = $derived(
    known.length < 2 ? "" : "M" + known.map((p) => `${p.ply},${(1 - p.chance!) * H}`).join(" L"),
  );

  const cursorPly = $derived.by(() => {
    const view = game.view;
    if (!view || view.inVariation) return null;
    return view.cursorPath.length || null;
  });

  // Moves marked ?!, ? or ?? (for example by the blunder check).
  const marks = $derived(
    points
      .filter((p) => p.chance !== null && p.nags.some((n) => n === 2 || n === 4 || n === 6))
      .map((p) => ({ ...p, kind: p.nags.includes(4) ? "blunder" : p.nags.includes(2) ? "mistake" : "inaccuracy" })),
  );

  let svg = $state<SVGSVGElement>();
  let hover = $state<number | null>(null);

  function plyAt(e: MouseEvent): number {
    const r = svg!.getBoundingClientRect();
    const x = 0.5 + ((e.clientX - r.left) / r.width) * plies;
    return Math.max(1, Math.min(plies, Math.round(x)));
  }

  const hovered = $derived(hover !== null ? points[hover - 1] : null);
</script>

{#if known.length >= 2}
  <figure class="graph">
    <!-- A mouse shortcut: from the keyboard, moves are reached with the arrow keys. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <svg
      bind:this={svg}
      viewBox="0.5 0 {plies} {H}"
      preserveAspectRatio="none"
      role="img"
      aria-label="Evaluation graph; click to go to a move"
      onclick={(e) => game.goto(points[plyAt(e) - 1].id)}
      onmousemove={(e) => (hover = plyAt(e))}
      onmouseleave={() => (hover = null)}
    >
      <rect class="black" x="0.5" y="0" width={plies} height={H} />
      <path class="white" d={area} />
      <line class="mid" x1="0.5" x2={plies + 0.5} y1={H / 2} y2={H / 2} />
      <path class="curve" d={line} />
      {#if cursorPly}
        <line class="cursor" x1={cursorPly} x2={cursorPly} y1="0" y2={H} />
      {/if}
      {#if hover}
        <line class="hover" x1={hover} x2={hover} y1="0" y2={H} />
      {/if}
    </svg>
    <!-- Dots are HTML so they stay round in the stretched SVG. -->
    {#each marks as m (m.id)}
      <span
        class="mark {m.kind}"
        style:left="{((m.ply - 0.5) / plies) * 100}%"
        style:top="{(1 - m.chance!) * 100}%"
      ></span>
    {/each}
    <figcaption>
      {#if hovered}
        {Math.ceil(hovered.ply / 2)}{hovered.ply % 2 ? "." : "…"}{hovered.san}
        {hovered.eval ? formatEval(hovered.eval) : ""}
      {:else}
        Evaluation
      {/if}
    </figcaption>
  </figure>
{/if}

<style>
  .graph {
    position: relative;
    margin: 0;
    height: 58px;
    border-top: 1px solid var(--rule);
    cursor: pointer;
  }

  svg {
    display: block;
    width: 100%;
    height: 100%;
  }

  .black {
    fill: var(--eval-black);
  }

  .white {
    fill: var(--eval-white);
  }

  .mid {
    stroke: var(--brass);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
    opacity: 0.6;
  }

  .curve {
    fill: none;
    stroke: var(--felt);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
  }

  .cursor {
    stroke: var(--felt);
    stroke-width: 2;
    vector-effect: non-scaling-stroke;
  }

  .hover {
    stroke: var(--brass);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }

  .mark {
    position: absolute;
    width: 7px;
    height: 7px;
    margin: -3.5px 0 0 -3.5px;
    border-radius: 50%;
    border: 1px solid var(--paper);
    pointer-events: none;
  }

  .mark.inaccuracy {
    background: var(--brass);
  }

  .mark.mistake {
    background: color-mix(in srgb, var(--brass) 50%, var(--danger));
  }

  .mark.blunder {
    background: var(--danger);
  }

  figcaption {
    position: absolute;
    top: 3px;
    left: 8px;
    padding: 0 5px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--paper) 80%, transparent);
    color: var(--ink-soft);
    font-size: 11px;
    pointer-events: none;
  }
</style>
