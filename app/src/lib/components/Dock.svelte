<script lang="ts">
  // Docking (UI-04): the panel groups on the four sides of the window, with
  // the board in the middle. Top and bottom run the full width; left and
  // right fit between them. Each side's groups are stacked (left, right) or
  // side by side (top, bottom). The sides' sizes come from the layout's
  // shares of the window, kept to their minimums and to the room the middle
  // needs (`sidePixels`); dividers resize them.
  import type { Snippet } from "svelte";
  import { analysis } from "../stores/analysis.svelte";
  import {
    DEFAULT_SIZES,
    DIVIDER,
    GROUP_MIN,
    OPPOSITE,
    SIDE_MIN,
    SIDE_NAMES,
    fitsContent,
    groupName,
    isStacked,
    panels,
    sidePixels,
    type Group,
    type Side,
  } from "../stores/panels.svelte";
  import Divider from "./Divider.svelte";
  import DockOverlay from "./DockOverlay.svelte";
  import PanelGroup from "./PanelGroup.svelte";
  import TabMenu from "./TabMenu.svelte";

  let {
    appHeight,
    middleMin,
    children,
  }: {
    /** The window's height: top and bottom sizes are shares of it. */
    appHeight: number;
    /** The room the middle keeps for the board at a given width. */
    middleMin: (width: number) => { w: number; h: number };
    /** The middle: the board. */
    children: Snippet;
  } = $props();

  let el: HTMLElement;
  let dockW = $state(0);
  let dockH = $state(0);

  const layout = $derived(panels.layout);
  const has = (side: Side) => layout.zones[side].groups.length > 0;
  const gap = (side: Side) => (has(side) ? DIVIDER : 0);
  const px = $derived(sidePixels(layout, { appW: dockW, appH: appHeight, dockW, dockH }, middleMin));
  const columns = $derived(`${px.left}px ${gap("left")}px minmax(0, 1fr) ${gap("right")}px ${px.right}px`);
  const rows = $derived(`${px.top}px ${gap("top")}px minmax(0, 1fr) ${gap("bottom")}px ${px.bottom}px`);

  // ---- the dividers between the sides and the board ----------------------

  /** What a side's size is a share of: the window's width or height. */
  const whole = (side: Side) => Math.max(1, isStacked(side) ? dockW : appHeight);
  /** The most a side can have: the middle keeps its room. */
  function maxPx(side: Side): number {
    if (isStacked(side)) return dockW - px[OPPOSITE[side]] - gap("left") - gap("right") - middleMin(0).w;
    const midW = dockW - px.left - px.right - gap("left") - gap("right");
    return dockH - px[OPPOSITE[side]] - gap("top") - gap("bottom") - middleMin(midW).h;
  }
  /** Left and top grow when their divider moves right or down; right and bottom when it moves left or up. */
  const sign = (side: Side) => (side === "left" || side === "top" ? 1 : -1);

  function setSide(side: Side, value: number) {
    const max = maxPx(side);
    const v = Math.max(Math.min(value, max), Math.min(SIDE_MIN[side], max));
    panels.resizeSide(side, v / whole(side));
  }

  let startPx = 0;
  const percent = (v: number, side: Side) => Math.round((v / whole(side)) * 100);

  // ---- the dividers between groups -----------------------------------------

  const engineOn = $derived(analysis.status.running || analysis.starting);

  /**
   * Whether a group takes only its content's height: the engine analysis
   * alone in a group, stacked with others, while the engine is off. It shows
   * its tab and a line saying so, and the notation keeps the room, as before
   * docking.
   */
  function fits(side: Side, g: Group): boolean {
    return (
      isStacked(side) && !engineOn && fitsContent(g) && layout.zones[side].groups.some((o) => !fitsContent(o))
    );
  }

  /**
   * How much of the side's free room a group takes: its share among the
   * groups that take one. (Grow factors adding up to less than 1 would leave
   * the rest empty.)
   */
  function grow(side: Side, g: Group): number {
    const total = layout.zones[side].groups.reduce((sum, o) => sum + (fits(side, o) ? 0 : o.share), 0);
    return total > 0 ? g.share / total : 1;
  }

  /**
   * The two groups the divider before group `i` resizes: the nearest groups
   * on each side of it that take a share (not their content's size).
   */
  function pairAt(side: Side, i: number): [Group, Group] | null {
    const groups = layout.zones[side].groups;
    const a = groups
      .slice(0, i)
      .reverse()
      .find((g) => !fits(side, g));
    const b = groups.slice(i).find((g) => !fits(side, g));
    return a && b ? [a, b] : null;
  }

  /** A group's size along its side, on screen. */
  function sizeOf(side: Side, g: Group): number {
    const r = el?.querySelector(`[data-group-id="${g.id}"]`)?.getBoundingClientRect();
    return r ? (isStacked(side) ? r.height : r.width) : 0;
  }

  let pairStart = { a: 0, b: 0 };

  /** Moves the boundary between `a` and `b` so `a` is `delta` pixels bigger than at `start`. */
  function shiftPair(side: Side, a: Group, b: Group, start: { a: number; b: number }, delta: number) {
    const both = start.a + start.b;
    if (both <= 0) return;
    const min = Math.min(isStacked(side) ? GROUP_MIN.stacked : GROUP_MIN.row, both / 2);
    const na = Math.max(min, Math.min(both - min, start.a + delta));
    const shares = a.share + b.share;
    panels.resizeGroups(side, { [a.id]: (shares * na) / both, [b.id]: (shares * (both - na)) / both });
  }

  /**
   * A key moved the divider between groups `aId` and `bId`. The split comes
   * from their shares, which are current even when keys come faster than
   * the screen is redrawn; their total size doesn't change.
   */
  function stepPair(side: Side, aId: string, bId: string, delta: number) {
    const groups = layout.zones[side].groups;
    const a = groups.find((g) => g.id === aId);
    const b = groups.find((g) => g.id === bId);
    if (!a || !b) return;
    const both = sizeOf(side, a) + sizeOf(side, b);
    const now = (both * a.share) / (a.share + b.share);
    shiftPair(side, a, b, { a: now, b: both - now }, delta);
  }

  /** The length of a side along which its groups lie, less its dividers between groups. */
  function sideLength(side: Side): number {
    const n = layout.zones[side].groups.length;
    const length = isStacked(side) ? dockH - px.top - px.bottom - gap("top") - gap("bottom") : dockW;
    return Math.max(1, length - (n - 1) * DIVIDER);
  }
</script>

<!-- A side and its divider, in the order they're seen (and tabbed through):
     the divider is between the side and the board. -->
{#snippet sideBlock(side: Side)}
  {#if has(side)}
    {@const first = side === "top" || side === "left"}
    {@const groups = layout.zones[side].groups}
    {@const stacked = isStacked(side)}
    {#if !first}{@render sideDivider(side)}{/if}
    <div
      class="side"
      class:stacked
      id="dock-{side}"
      data-side={side}
      role="region"
      aria-label="{SIDE_NAMES[side]} side"
      style:grid-area={side}
    >
      {#each groups as group, i (group.id)}
        {#if i > 0}
          {@const pair = pairAt(side, i)}
          {#if pair}
            {@const [a, b] = pair}
            {@const pairPx = (grow(side, a) + grow(side, b)) * sideLength(side)}
            {@const lo = Math.round((Math.min(stacked ? GROUP_MIN.stacked : GROUP_MIN.row, pairPx / 2) / pairPx) * 100)}
            <Divider
              orientation={stacked ? "horizontal" : "vertical"}
              label="Resize {groupName(a)} and {groupName(b)}"
              now={Math.round((a.share / (a.share + b.share)) * 100)}
              min={lo}
              max={100 - lo}
              onstart={() => (pairStart = { a: sizeOf(side, a), b: sizeOf(side, b) })}
              onmove={(d) => shiftPair(side, a, b, pairStart, d)}
              onstep={(d) => stepPair(side, a.id, b.id, d)}
              onreset={() => {
                const [sa, sb] = panels.defaultSplit(a, b);
                panels.resizeGroups(side, { [a.id]: sa, [b.id]: sb });
              }}
            />
          {:else}
            <div class="rule" class:stacked></div>
          {/if}
        {/if}
        <PanelGroup {group} fit={fits(side, group)} grow={grow(side, group)} />
      {/each}
    </div>
    {#if first}{@render sideDivider(side)}{/if}
  {/if}
{/snippet}

{#snippet sideDivider(side: Side)}
  <Divider
    orientation={isStacked(side) ? "vertical" : "horizontal"}
    area="d{side}"
    controls="dock-{side}"
    label="Resize the {side} side"
    now={percent(px[side], side)}
    min={percent(Math.min(SIDE_MIN[side], maxPx(side)), side)}
    max={percent(Math.max(SIDE_MIN[side], maxPx(side)), side)}
    onstart={() => (startPx = px[side])}
    onmove={(d) => setSide(side, startPx + sign(side) * d)}
    onstep={(d) => setSide(side, px[side] + sign(side) * d)}
    onreset={() => setSide(side, DEFAULT_SIZES[side] * whole(side))}
  />
{/snippet}

<main
  class="dock"
  bind:this={el}
  bind:clientWidth={dockW}
  bind:clientHeight={dockH}
  style:grid-template-columns={columns}
  style:grid-template-rows={rows}
>
  {@render sideBlock("top")}
  {@render sideBlock("left")}
  <div class="dock-middle">{@render children()}</div>
  {@render sideBlock("right")}
  {@render sideBlock("bottom")}
</main>

<DockOverlay />
<TabMenu />

<style>
  /* The sides' tracks are sized in pixels from the layout (see sidePixels);
     an empty side's are 0. Keep the dividers' 5 px in step with DIVIDER. */
  .dock {
    display: grid;
    grid-template-areas:
      "top top top top top"
      "dtop dtop dtop dtop dtop"
      "left dleft middle dright right"
      "dbottom dbottom dbottom dbottom dbottom"
      "bottom bottom bottom bottom bottom";
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .dock-middle {
    grid-area: middle;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  /* A side never widens the window: its panels scroll inside. */
  .side {
    display: flex;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .side.stacked {
    flex-direction: column;
  }

  /* Between a group that takes its content's size and its neighbour. */
  .rule {
    flex: none;
    width: 1px;
    background: var(--rule);
  }

  .rule.stacked {
    width: auto;
    height: 1px;
  }
</style>
