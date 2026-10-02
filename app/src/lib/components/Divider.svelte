<script lang="ts">
  // A divider of the dock (UI-04): between a side and the board, or between
  // two groups of a side. Drag it, or focus it and use the arrow keys (16 px
  // a press); a double-click resets it. A focusable separator is the
  // WAI-ARIA window splitter. Window listeners, like the game list's column
  // edges, so a drag past the divider keeps going.
  let {
    orientation,
    label,
    now,
    min,
    max,
    area,
    controls,
    onstart,
    onmove,
    onstep,
    onreset,
  }: {
    /** "vertical" for a divider between things side by side, moved left and right. */
    orientation: "vertical" | "horizontal";
    label: string;
    /** The position, as the percentage of its pane, and its range. */
    now: number;
    min: number;
    max: number;
    /** The grid area it sits in, for the dividers between sides and the board. */
    area?: string;
    /** The id of the pane it resizes. */
    controls?: string;
    /** A drag starts; `onmove` then gets the pointer's distance from where it started (right or down positive). */
    onstart: () => void;
    onmove: (delta: number) => void;
    /** A key moved it by `delta` pixels (right or down positive). */
    onstep: (delta: number) => void;
    onreset: () => void;
  } = $props();

  const STEP = 16;
  let dragging = $state(false);

  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const at = (ev: PointerEvent) => (orientation === "vertical" ? ev.clientX : ev.clientY);
    const start = at(e);
    onstart();
    dragging = true;
    const move = (ev: PointerEvent) => {
      // Released outside the window: no pointerup came.
      if ((ev.buttons & 1) === 0) return end();
      onmove(at(ev) - start);
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      dragging = false;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  }

  function key(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const [back, forward] = orientation === "vertical" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
    const delta = e.key === back ? -STEP : e.key === forward ? STEP : 0;
    if (!delta) return;
    // The arrows don't step through the game while a divider has the focus.
    e.preventDefault();
    e.stopPropagation();
    onstep(delta);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="divider {orientation}"
  class:active={dragging}
  role="separator"
  aria-orientation={orientation}
  aria-label={label}
  aria-valuenow={now}
  aria-valuemin={min}
  aria-valuemax={max}
  aria-controls={controls}
  tabindex="0"
  title="Drag to resize; double-click for the default size"
  style:grid-area={area}
  onpointerdown={down}
  onkeydown={key}
  ondblclick={onreset}
></div>

<style>
  /* A thin rule in a gutter, like the Reference tab's dividers. */
  .divider {
    flex: none;
    background: var(--rule);
    touch-action: none;
  }

  .divider.vertical {
    width: 5px;
    cursor: col-resize;
    border-inline: 2px solid var(--panel);
  }

  .divider.horizontal {
    height: 5px;
    cursor: row-resize;
    border-block: 2px solid var(--panel);
  }

  .divider:hover,
  .divider.active,
  .divider:focus-visible {
    background: var(--felt);
  }

  .divider:focus-visible {
    outline-offset: -1px;
  }
</style>
