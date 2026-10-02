<script lang="ts">
  import { dock } from "../stores/dock.svelte";
  import { PANEL_TITLES, SIDES, SIDE_NAMES, type Side } from "../stores/panels.svelte";

  // While a panel tab is dragged (UI-04): a band along each edge of the
  // window to dock it there, what the drop will do (a line where the new
  // group goes, the side as it will open, or where in a tab strip it lands),
  // and a label under the pointer. Tab strips are drop targets of their own
  // (PanelGroup.svelte), drawn above the bands.

  /** How far a band reaches in from its edge, and out over the ribbon or status bar. */
  const IN = 26;
  const OUT = 8;

  function band(side: Side, a: DOMRect, app: DOMRect | null) {
    const bottom = app ? Math.max(app.bottom, a.bottom) : a.bottom + IN;
    const boxes = {
      // Over the ribbon's lower edge and the top of the dock.
      top: { left: a.left, top: a.top - IN, width: a.width, height: IN + OUT },
      // Over the bottom of the dock and the status bar.
      bottom: { left: a.left, top: a.bottom - OUT, width: a.width, height: Math.max(IN, bottom - a.bottom) + OUT },
      left: { left: a.left, top: a.top + OUT, width: IN, height: a.height - 2 * OUT },
      right: { left: a.right - IN, top: a.top + OUT, width: IN, height: a.height - 2 * OUT },
    };
    return boxes[side];
  }

  const label = $derived(
    dock.target?.label ?? "Drop on an edge of the window to dock it there, or on a tab strip to join that group",
  );
</script>

{#if dock.panel && dock.area}
  {#each SIDES as side (side)}
    {@const b = band(side, dock.area, dock.frame)}
    <div
      class="band {side}"
      class:over={dock.target?.edge === side}
      data-dock-edge={side}
      aria-hidden="true"
      style:left="{b.left}px"
      style:top="{b.top}px"
      style:width="{b.width}px"
      style:height="{b.height}px"
    >
      <span>{SIDE_NAMES[side]}</span>
    </div>
  {/each}

  {#if dock.target}
    {@const m = dock.target.mark}
    <div
      class="mark"
      class:area={m.w > 6 && m.h > 6}
      style:left="{m.x}px"
      style:top="{m.y}px"
      style:width="{m.w}px"
      style:height="{m.h}px"
    ></div>
  {/if}

  <div class="dock-ghost" role="status" style:left="{dock.x + 14}px" style:top="{dock.y + 16}px">
    <strong>{PANEL_TITLES[dock.panel]}</strong>
    <span class:ready={dock.target}>{label}</span>
  </div>
{/if}

<style>
  .band {
    position: fixed;
    z-index: 300;
    display: grid;
    place-items: center;
    overflow: hidden;
    /* Opaque: the ribbon's captions and the panels' edges stay out of the way. */
    background: color-mix(in srgb, var(--felt) 14%, var(--paper));
    border: 1px dashed color-mix(in srgb, var(--felt) 70%, var(--paper));
    border-radius: var(--radius);
    color: var(--felt);
    font-size: 11px;
    font-weight: 600;
  }

  .band.left span,
  .band.right span {
    writing-mode: vertical-rl;
  }

  .band.over {
    background: var(--felt-soft);
    border: 2px solid var(--felt);
  }

  /* Where the drop lands: a line between groups or tabs, or the new side. */
  .mark {
    position: fixed;
    z-index: 320;
    pointer-events: none;
    background: var(--felt);
    border-radius: 2px;
  }

  .mark.area {
    background: color-mix(in srgb, var(--felt) 22%, transparent);
    border: 2px solid var(--felt);
  }

  .dock-ghost {
    position: fixed;
    z-index: 330;
    display: grid;
    max-width: 300px;
    padding: 4px 9px;
    pointer-events: none;
    background: var(--paper);
    border: 1px solid var(--felt);
    border-radius: var(--radius);
    box-shadow: 0 4px 14px color-mix(in srgb, var(--ink) 22%, transparent);
    font-size: 12px;
  }

  .dock-ghost strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dock-ghost span {
    color: var(--ink-soft);
  }

  .dock-ghost span.ready {
    color: var(--felt);
    font-weight: 600;
  }

  :global(body.dragging-tab),
  :global(body.dragging-tab *) {
    cursor: grabbing !important;
  }
</style>
