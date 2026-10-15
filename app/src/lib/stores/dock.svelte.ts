// Docking (UI-04): dragging a panel tab to a side of the window or into
// another group, and the tab's menu (Move to ▸ a side, Move into ▸ a
// group). Built on pointer events on our own elements, like dragging games
// (QOL-04, `drag.svelte.ts`), so smoke drivers can send synthetic ones.
import {
  DEFAULT_SIZES,
  PANEL_TITLES,
  SIDE_MIN,
  findGroup,
  groupName,
  isStacked,
  panels,
  visibleGroups,
  type DockDest,
  type PanelId,
  type Side,
} from "./panels.svelte";

/** Pixels the pointer moves with the button down before a tab drag starts. */
const THRESHOLD = 4;

/** What dropping the tab here does. */
export type DockTarget = {
  dest: DockDest;
  /** For the label under the pointer: "Dock on the left", "Move into Notation". */
  label: string;
  /** Where the drop lands, drawn over the window (viewport pixels). */
  mark: { x: number; y: number; w: number; h: number };
  /** The edge band, or the group whose tab strip, under the pointer. */
  edge?: Side;
  strip?: string;
};

const EDGE_LABELS: Record<Side, string> = {
  left: "Dock on the left",
  right: "Dock on the right",
  top: "Dock at the top",
  bottom: "Dock at the bottom",
};

const rectOf = (selector: string) => document.querySelector(selector)?.getBoundingClientRect() ?? null;

/** Dropping `panel` on the tab strip of group `id`, at `x`: before the tab under the pointer's left half. */
function stripTarget(panel: PanelId, strip: HTMLElement, x: number): DockTarget | null {
  const id = strip.dataset.dockStrip ?? "";
  const found = findGroup(panels.layout, id);
  if (!found) return null;
  const own = found.group.panels.includes(panel);
  // A tab alone in its group has nothing to reorder.
  if (own && found.group.panels.length === 1) return null;
  const tabs = [...strip.querySelectorAll<HTMLElement>("[role='tab']")].map((t) => t.getBoundingClientRect());
  let index = tabs.findIndex((r) => x < r.left + r.width / 2);
  if (index < 0) index = tabs.length;
  const box = strip.getBoundingClientRect();
  const at = index < tabs.length ? tabs[index].left - 1 : (tabs.at(-1)?.right ?? box.left) + 1;
  return {
    dest: { kind: "group", group: id, index },
    label: own ? `Move ${PANEL_TITLES[panel]} here` : `Move into ${groupName(found.group)}`,
    mark: { x: at - 1.5, y: box.top + 3, w: 3, h: box.height - 5 },
    strip: id,
  };
}

/**
 * Docking on `side` with the pointer at (x, y): a new group next to the
 * side's group nearest the pointer (before it if the pointer is in its
 * first half), or the side's first group.
 */
function edgeTarget(side: Side, x: number, y: number): DockTarget | null {
  const dock = rectOf(".dock");
  const middle = rectOf(".dock-middle");
  if (!dock || !middle) return null;
  const groups = visibleGroups(panels.layout, side);
  const stacked = isStacked(side);
  if (!groups.length) {
    // The side as it will open: its saved size (or the default), between
    // the top and bottom sides when it's left or right.
    const zone = panels.layout.zones[side];
    const appH = (document.querySelector(".app") as HTMLElement | null)?.clientHeight ?? window.innerHeight;
    const whole = stacked ? dock.width : appH;
    const span = stacked ? dock.width : dock.height;
    const px = Math.min(span * 0.6, Math.max(SIDE_MIN[side], (zone.size || DEFAULT_SIZES[side]) * whole));
    const mark = {
      left: { x: dock.left, y: middle.top, w: px, h: middle.height },
      right: { x: dock.right - px, y: middle.top, w: px, h: middle.height },
      top: { x: dock.left, y: dock.top, w: dock.width, h: px },
      bottom: { x: dock.left, y: dock.bottom - px, w: dock.width, h: px },
    }[side];
    return { dest: { kind: "side", side, slot: 0 }, label: EDGE_LABELS[side], mark, edge: side };
  }
  const pos = stacked ? y : x;
  let best = -1;
  let bestDist = Infinity;
  let r: DOMRect | null = null;
  for (const [i, g] of groups.entries()) {
    const box = rectOf(`[data-group-id="${g.id}"]`);
    if (!box) continue;
    const lo = stacked ? box.top : box.left;
    const hi = stacked ? box.bottom : box.right;
    const d = pos < lo ? lo - pos : pos > hi ? pos - hi : 0;
    if (d < bestDist) {
      bestDist = d;
      best = i;
      r = box;
    }
  }
  if (best < 0 || !r) return null;
  const after = stacked ? pos >= r.top + r.height / 2 : pos >= r.left + r.width / 2;
  const name = groupName(groups[best]);
  const where = stacked ? (after ? "below" : "above") : after ? "right of" : "left of";
  const mark = stacked
    ? { x: r.left, y: (after ? r.bottom : r.top) - 2, w: r.width, h: 4 }
    : { x: (after ? r.right : r.left) - 2, y: r.top, w: 4, h: r.height };
  return {
    dest: { kind: "side", side, slot: best + (after ? 1 : 0) },
    label: `${EDGE_LABELS[side]}, ${where} ${name}`,
    mark,
    edge: side,
  };
}

/** What a drop of `panel` at (x, y) does: a tab strip first, then an edge band. */
function targetAt(panel: PanelId, x: number, y: number): DockTarget | null {
  const el = document.elementFromPoint(x, y);
  const strip = el?.closest<HTMLElement>("[data-dock-strip]");
  if (strip) return stripTarget(panel, strip, x);
  const band = el?.closest<HTMLElement>("[data-dock-edge]");
  if (band) return edgeTarget(band.dataset.dockEdge as Side, x, y);
  return null;
}

class DockStore {
  /** The panel whose tab is being dragged, once the pointer has moved far enough. */
  panel = $state<PanelId | null>(null);
  x = $state(0);
  y = $state(0);
  target = $state.raw<DockTarget | null>(null);
  /** The dock area while dragging: the edge bands run along it. */
  area = $state.raw<DOMRect | null>(null);
  /** The window (`.app`) while dragging. */
  frame = $state.raw<DOMRect | null>(null);
  /** The tab menu: Move to ▸ a side, Move into ▸ a group. */
  menu = $state.raw<{ panel: PanelId; x: number; y: number } | null>(null);
  #pending: { x: number; y: number; panel: PanelId } | null = null;

  /**
   * Called on pointerdown on a panel tab. Dragging starts once the pointer
   * moves a few pixels with the button down; a plain click stays a click.
   */
  arm(e: PointerEvent, panel: PanelId) {
    if (e.button !== 0) return;
    this.#end();
    this.#pending = { x: e.clientX, y: e.clientY, panel };
    window.addEventListener("pointermove", this.#onMove);
    window.addEventListener("pointerup", this.#onUp);
    window.addEventListener("pointercancel", this.cancel);
    window.addEventListener("blur", this.cancel);
    window.addEventListener("keydown", this.#onKey, true);
  }

  /** Drops nothing. */
  cancel = () => this.#end();

  #end() {
    this.#pending = null;
    this.panel = null;
    this.target = null;
    this.area = null;
    this.frame = null;
    document.body.classList.remove("dragging-tab");
    window.removeEventListener("pointermove", this.#onMove);
    window.removeEventListener("pointerup", this.#onUp);
    window.removeEventListener("pointercancel", this.cancel);
    window.removeEventListener("blur", this.cancel);
    window.removeEventListener("keydown", this.#onKey, true);
  }

  #onMove = (e: PointerEvent) => {
    const pending = this.#pending;
    if (!pending) return;
    // Released outside the window: no pointerup came.
    if ((e.buttons & 1) === 0) return this.#end();
    if (!this.panel) {
      if (Math.hypot(e.clientX - pending.x, e.clientY - pending.y) < THRESHOLD) return;
      this.area = rectOf(".dock");
      this.frame = rectOf(".app");
      if (!this.area) return this.#end();
      this.panel = pending.panel;
      this.menu = null;
      document.body.classList.add("dragging-tab");
    }
    this.x = e.clientX;
    this.y = e.clientY;
    this.target = targetAt(this.panel, e.clientX, e.clientY);
  };

  #onUp = (e: PointerEvent) => {
    const panel = this.panel;
    const target = panel ? targetAt(panel, e.clientX, e.clientY) : null;
    this.#end();
    if (panel && target) panels.move(panel, target.dest);
  };

  #onKey = (e: KeyboardEvent) => {
    if (!this.panel || e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    this.#end();
  };

  /** Opens the tab menu of `panel` at (x, y). */
  openMenu(panel: PanelId, x: number, y: number) {
    this.menu = { panel, x, y };
  }

  closeMenu() {
    this.menu = null;
  }
}

export const dock = new DockStore();
