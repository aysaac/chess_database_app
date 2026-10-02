// Panel tabs (UI-19) and docking (UI-04): the panels around the board,
// grouped as tabs. These are *panel* tabs (Notation, Opening tree…), not the
// game tabs above the board (GameTabs.svelte, UI-05).
//
// Each side of the window holds groups of panels; a group shows one of its
// panels at a time. Sides have a size (a share of the window, so they scale
// when it's resized) and groups a share of their side. Any tab can be
// dragged to another side or group (`stores/dock.svelte.ts`); the whole
// layout is saved as the `layout` pref.
import { prefs } from "./prefs.svelte";

/** A panel: what a panel tab shows. */
export type PanelId = "notation" | "tree" | "reference" | "analysis" | "games";
/** A side of the window that panels dock to. */
export type Side = "left" | "right" | "top" | "bottom";
/**
 * Panels shown as tabs; `active` is the one showing, `share` the group's
 * part of its side. `id` names the group while the app runs, so moving a
 * tab doesn't rebuild the groups it leaves alone; it isn't saved.
 */
export type Group = { id: string; panels: PanelId[]; active: PanelId; share: number };
/**
 * A side's groups, stacked top to bottom on the left and right and side by
 * side at the top and bottom. `size` is the side's share of the window's
 * width (left, right) or height (top, bottom); an empty side takes no space
 * but keeps its size for when a panel docks there again.
 */
export type Zone = { size: number; groups: Group[] };
/**
 * The whole layout, saved as the `layout` pref. Version 1 (UI-19) only
 * meant which tab each group showed; version 2 (UI-04) is the real layout,
 * sizes included.
 */
export type Layout = { version: 2; zones: Record<Side, Zone> };

export const SIDES: readonly Side[] = ["left", "right", "top", "bottom"];

/** What each panel's tab says. */
export const PANEL_TITLES: Record<PanelId, string> = {
  notation: "Notation",
  tree: "Opening tree",
  reference: "Reference",
  analysis: "Engine analysis",
  games: "Game list",
};

const PANEL_IDS = Object.keys(PANEL_TITLES) as PanelId[];

/** The sides as the tab menu names them (Move to ▸ Left…). */
export const SIDE_NAMES: Record<Side, string> = { left: "Left", right: "Right", top: "Top", bottom: "Bottom" };

/** The side across the board. */
export const OPPOSITE: Record<Side, Side> = { left: "right", right: "left", top: "bottom", bottom: "top" };

/** Whether a side's groups are stacked top to bottom (left, right) rather than side by side. */
export const isStacked = (side: Side) => side === "left" || side === "right";

/** A side's smallest size in pixels while it has panels; the middle keeps room for the board. */
export const SIDE_MIN: Record<Side, number> = { left: 220, right: 220, top: 120, bottom: 120 };
/** A group's smallest size along its side when a divider between groups moves. */
export const GROUP_MIN = { stacked: 80, row: 160 };
/** The dividers' thickness (between a side and the board, and between groups). */
export const DIVIDER = 5;

/**
 * A side's size when it gets its first panel and nothing set it before, and
 * after a double-click on its divider. The right side's is the default
 * layout's (the board fills the rest at common window shapes); the bottom's
 * is the game list's share from before docking (`listShare`).
 */
export const DEFAULT_SIZES: Record<Side, number> = { left: 0.3, right: 0.6, top: 0.25, bottom: 0.28 };

let lastId = 0;
const newId = () => `g${++lastId}`;

/** Weight of a group's default share: the engine lines need less room. */
function weight(panels: PanelId[]): number {
  return panels.every((p) => p === "analysis") ? 1 / 3 : 1;
}

/**
 * The default layout: Notation, Opening tree and Reference (OPEN-14) as
 * tabs on the right, above the engine analysis, and the game list at the
 * bottom.
 */
export function defaultLayout(): Layout {
  const empty = (side: Side): Zone => ({ size: DEFAULT_SIZES[side], groups: [] });
  return {
    version: 2,
    zones: {
      left: empty("left"),
      right: {
        size: DEFAULT_SIZES.right,
        groups: [
          { id: newId(), panels: ["notation", "tree", "reference"], active: "notation", share: 0.75 },
          { id: newId(), panels: ["analysis"], active: "analysis", share: 0.25 },
        ],
      },
      top: empty("top"),
      bottom: {
        size: DEFAULT_SIZES.bottom,
        groups: [{ id: newId(), panels: ["games"], active: "games", share: 1 }],
      },
    },
  };
}

/** Every group of a layout, side by side in `SIDES` order. */
export function allGroups(layout: Layout): Group[] {
  return SIDES.flatMap((side) => layout.zones[side].groups);
}

/** A group by its id, with its side. */
export function findGroup(layout: Layout, id: string): { side: Side; group: Group; index: number } | null {
  for (const side of SIDES) {
    const index = layout.zones[side].groups.findIndex((g) => g.id === id);
    if (index >= 0) return { side, group: layout.zones[side].groups[index], index };
  }
  return null;
}

/** A group as a drop label names it: by the tab it shows ("Move into Notation"). */
export const groupName = (group: Group) => PANEL_TITLES[group.active];
/** A group as the tab menu lists it: all its tabs. */
export const groupLabel = (group: Group) => group.panels.map((p) => PANEL_TITLES[p]).join(" · ");

/**
 * Whether a group takes only the height of its content: the engine
 * analysis alone, while the engine is off (the caller knows that), so the
 * notation keeps the room as it did before docking.
 */
export function fitsContent(group: Group): boolean {
  return group.panels.every((p) => p === "analysis");
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isShare = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0 && v <= 1;
const isPanel = (v: unknown): v is PanelId => PANEL_IDS.includes(v as PanelId);

/**
 * The sides of a saved layout, if they're readable: all four, sizes and
 * shares from 0 to 1, groups that aren't empty and show one of their own
 * panels, and each panel at most once. Anything else gives null.
 */
function parseZones(raw: unknown): Record<Side, Zone> | null {
  if (!isRecord(raw)) return null;
  const seen = new Set<PanelId>();
  const zones = {} as Record<Side, Zone>;
  for (const side of SIDES) {
    const zone = raw[side];
    if (!isRecord(zone) || !isShare(zone.size) || !Array.isArray(zone.groups)) return null;
    const groups: Group[] = [];
    for (const g of zone.groups) {
      if (!isRecord(g) || !isShare(g.share) || !Array.isArray(g.panels) || !g.panels.length) return null;
      const panels: PanelId[] = [];
      for (const p of g.panels) {
        if (!isPanel(p) || seen.has(p)) return null;
        seen.add(p);
        panels.push(p);
      }
      if (!isPanel(g.active) || !panels.includes(g.active)) return null;
      groups.push({ id: newId(), panels, active: g.active, share: g.share });
    }
    zones[side] = { size: zone.size, groups };
  }
  return zones;
}

/**
 * Makes a layout whole: shares on each side add up to 1, a side with
 * panels has a size, and a panel the layout doesn't have (one added in a
 * later version of the app) goes where the default layout has it: into the
 * group holding its default neighbours, or a new group on its default side.
 */
function complete(layout: Layout): Layout {
  const present = new Set(allGroups(layout).flatMap((g) => g.panels));
  const defaults = defaultLayout();
  for (const side of SIDES) {
    for (const dg of defaults.zones[side].groups) {
      for (const p of dg.panels) {
        if (present.has(p)) continue;
        present.add(p);
        const home = allGroups(layout).find((g) => g.panels.some((q) => dg.panels.includes(q)));
        if (home) home.panels.push(p);
        else layout.zones[side].groups.push({ id: newId(), panels: [p], active: p, share: 1 });
      }
    }
  }
  for (const side of SIDES) {
    const zone = layout.zones[side];
    if (zone.groups.length && zone.size <= 0) zone.size = DEFAULT_SIZES[side];
    normalize(zone);
  }
  return layout;
}

/** Shares of a side's groups scaled to add up to 1 (equal when they're all 0). */
function normalize(zone: Zone) {
  const total = zone.groups.reduce((sum, g) => sum + g.share, 0);
  for (const g of zone.groups) g.share = total > 0 ? g.share / total : 1 / zone.groups.length;
}

/** `raw` as a version 2 layout (UI-04), if it is one; see `parseZones`. */
export function parseLayout(raw: unknown): Layout | null {
  if (!isRecord(raw) || raw.version !== 2) return null;
  const zones = parseZones(raw.zones);
  return zones ? complete({ version: 2, zones }) : null;
}

/**
 * The layout a session starts with. A saved version 2 layout is used as it
 * is. Older settings get the default layout, with:
 * - the tabs a version 1 layout (UI-19) showed, or without one, the Opening
 *   tree tab if the older `treeVisible` pref (QOL-01) says so;
 * - the game list's old divider (`listShare`, a share of the window's
 *   height) as the bottom side's size.
 * Anything unreadable gives the default.
 */
export function restoreLayout(saved: unknown, treeVisible: unknown, listShare: unknown): Layout {
  const parsed = parseLayout(saved);
  if (parsed) return parsed;
  const layout = defaultLayout();
  if (typeof listShare === "number" && Number.isFinite(listShare)) {
    // The range the old divider allowed.
    layout.zones.bottom.size = Math.max(0.1, Math.min(0.7, listShare));
  }
  const v1 = isRecord(saved) && saved.version === 1 ? parseZones(saved.zones) : null;
  const shown: PanelId[] = v1
    ? SIDES.flatMap((s) => v1[s].groups.map((g) => g.active))
    : treeVisible === true
      ? ["tree"]
      : [];
  const set = new Set<Group>();
  for (const id of shown) {
    const group = allGroups(layout).find((g) => g.panels.includes(id));
    if (group && !set.has(group)) {
      group.active = id;
      set.add(group);
    }
  }
  return layout;
}

/** The layout as the `layout` pref saves it: no group ids, sizes to 3 decimals. */
export function toPref(layout: Layout) {
  const round = (n: number) => Math.round(n * 1000) / 1000;
  const zones = {} as Record<Side, { size: number; groups: { panels: PanelId[]; active: PanelId; share: number }[] }>;
  for (const side of SIDES) {
    const zone = layout.zones[side];
    zones[side] = {
      size: round(zone.size),
      groups: zone.groups.map((g) => ({ panels: [...g.panels], active: g.active, share: round(g.share) })),
    };
  }
  return { version: 2 as const, zones };
}

/** A deep copy that keeps the group ids. */
function clone(layout: Layout): Layout {
  const zones = {} as Record<Side, Zone>;
  for (const side of SIDES) {
    const zone = layout.zones[side];
    zones[side] = { size: zone.size, groups: zone.groups.map((g) => ({ ...g, panels: [...g.panels] })) };
  }
  return { version: 2, zones };
}

/**
 * Takes `panel` out of its group. The group shows the tab next to it
 * instead, and goes when it was its last tab; the shares of that side's
 * other groups grow to fill its place. Returns where the group was and
 * whether it went.
 */
function detach(layout: Layout, panel: PanelId): { side: Side; index: number; removed: boolean } | null {
  for (const side of SIDES) {
    const zone = layout.zones[side];
    const index = zone.groups.findIndex((g) => g.panels.includes(panel));
    if (index < 0) continue;
    const group = zone.groups[index];
    const at = group.panels.indexOf(panel);
    group.panels.splice(at, 1);
    if (!group.panels.length) {
      zone.groups.splice(index, 1);
      normalize(zone);
      return { side, index, removed: true };
    }
    if (group.active === panel) group.active = group.panels[Math.min(at, group.panels.length - 1)];
    return { side, index, removed: false };
  }
  return null;
}

/**
 * The sides' sizes in pixels for a dock area of `dockW` × `dockH` in a
 * window of `appW` × `appH` (0 for an empty side). Each side gets its share
 * of the window, at least its minimum, and the dividers and the middle's
 * minimum (`middle(width)`, the board's room) come first: when they don't
 * fit, sides above their minimum shrink first, in proportion to what they
 * have above it, and then all of them in proportion.
 */
export function sidePixels(
  layout: Layout,
  frame: { appW: number; appH: number; dockW: number; dockH: number },
  middle: (width: number) => { w: number; h: number },
): Record<Side, number> {
  const has = (side: Side) => layout.zones[side].groups.length > 0;
  const want = (side: Side, whole: number) =>
    has(side) ? Math.max(SIDE_MIN[side], layout.zones[side].size * whole) : 0;
  const fit = (a: Side, b: Side, whole: number, room: number): [number, number] => {
    let x = want(a, whole);
    let y = want(b, whole);
    const excess = x + y - Math.max(0, room);
    if (excess <= 0) return [x, y];
    const minX = has(a) ? SIDE_MIN[a] : 0;
    const minY = has(b) ? SIDE_MIN[b] : 0;
    const spare = x - minX + (y - minY);
    if (spare >= excess) {
      x -= (excess * (x - minX)) / spare;
      y -= (excess * (y - minY)) / spare;
      return [x, y];
    }
    const scale = Math.max(0, room) / Math.max(1, minX + minY);
    return [minX * scale, minY * scale];
  };
  const dividers = (a: Side, b: Side) => (has(a) ? DIVIDER : 0) + (has(b) ? DIVIDER : 0);
  const [left, right] = fit("left", "right", frame.appW, frame.dockW - dividers("left", "right") - middle(0).w);
  const midW = frame.dockW - left - right - dividers("left", "right");
  const [top, bottom] = fit("top", "bottom", frame.appH, frame.dockH - dividers("top", "bottom") - middle(midW).h);
  return { left, right, top, bottom };
}

/** Where a dragged tab can go. */
export type DockDest =
  /** A new group on `side`, before the group at `slot` (counted before the move; the end if past it). */
  | { kind: "side"; side: Side; slot: number }
  /** Into the group `group`, before its tab at `index` (counted before the move). */
  | { kind: "group"; group: string; index: number };

/** The panel layout: which group holds each panel, which tab it shows, and the sizes (UI-19, UI-04). */
class PanelStore {
  /** Set once the layout changes in this session; until then the saved layout applies. */
  #changed = $state.raw<Layout | null>(null);
  #restored = $derived(restoreLayout(prefs.raw("layout"), prefs.raw("treeVisible"), prefs.raw("listShare")));

  get layout(): Layout {
    return this.#changed ?? this.#restored;
  }

  /** The group holding `panel`, or null when the layout doesn't have it. */
  groupOf(panel: PanelId): Group | null {
    return allGroups(this.layout).find((g) => g.panels.includes(panel)) ?? null;
  }

  /** The side holding `panel`. */
  sideOf(panel: PanelId): Side | null {
    return SIDES.find((s) => this.layout.zones[s].groups.some((g) => g.panels.includes(panel))) ?? null;
  }

  /**
   * Whether `panel` is the tab its group shows. A panel does its work (the
   * opening tree's queries, for one) only while it's showing.
   */
  isShowing(panel: PanelId): boolean {
    return this.groupOf(panel)?.active === panel;
  }

  /**
   * The group the drop shelf for dragged games (QOL-04) covers: the one
   * holding the notation, or another when that one shows the game list
   * the games come from. Null when every group shows the list.
   */
  get shelfHost(): string | null {
    const groups = allGroups(this.layout);
    const notation = this.groupOf("notation");
    if (notation && notation.active !== "games") return notation.id;
    return groups.find((g) => g.active !== "games")?.id ?? null;
  }

  /** Saves `next` as the layout. */
  #commit(next: Layout) {
    this.#changed = next;
    prefs.set("layout", toPref(next));
    // The layout replaces the older prefs: the tree's visibility (QOL-01)
    // and the game list's divider, both carried over when it was restored.
    if (prefs.raw("treeVisible") != null) prefs.set("treeVisible", null);
    if (prefs.raw("listShare") != null) prefs.set("listShare", null);
  }

  /** Shows `panel`'s tab in its group, and remembers it. */
  show(panel: PanelId) {
    if (!this.groupOf(panel) || this.isShowing(panel)) return;
    const next = clone(this.layout);
    const group = allGroups(next).find((g) => g.panels.includes(panel));
    if (group) group.active = panel;
    this.#commit(next);
  }

  /**
   * Moves `panel` to `dest` and shows it there. A group left without tabs
   * goes, and a side without groups takes no space; a side that gets its
   * first group again takes the size it had.
   */
  move(panel: PanelId, dest: DockDest) {
    const next = clone(this.layout);
    if (dest.kind === "group") {
      const target = findGroup(next, dest.group);
      if (!target) return;
      const from = target.group.panels.indexOf(panel);
      let index = dest.index;
      if (from >= 0) {
        // Reordering its own group.
        if (index > from) index--;
        target.group.panels.splice(from, 1);
      } else if (!detach(next, panel)) {
        return;
      }
      index = Math.max(0, Math.min(index, target.group.panels.length));
      target.group.panels.splice(index, 0, panel);
      target.group.active = panel;
    } else {
      const zone = next.zones[dest.side];
      // Not when its only tab is dropped back on it.
      const opens = !zone.groups.length;
      let slot = Math.max(0, Math.min(dest.slot, zone.groups.length));
      const from = detach(next, panel);
      if (!from) return;
      if (from.removed && from.side === dest.side && from.index < slot) slot--;
      if (opens) {
        // The side opens with the size it had, and takes that room from the
        // side across the board first, as long as that one keeps as much,
        // so the board keeps its size where it can.
        if (zone.size <= 0) zone.size = DEFAULT_SIZES[dest.side];
        const across = next.zones[OPPOSITE[dest.side]];
        if (across.groups.length) across.size -= Math.min(zone.size, Math.max(0, across.size - zone.size));
      }
      // A share as if every group of the side weighed the same (the engine's
      // alone a third); the others give way in proportion.
      const w = weight([panel]);
      const share = zone.groups.length ? w / (zone.groups.reduce((sum, g) => sum + weight(g.panels), 0) + w) : 1;
      for (const g of zone.groups) g.share *= 1 - share;
      zone.groups.splice(slot, 0, { id: newId(), panels: [panel], active: panel, share });
      normalize(zone);
    }
    this.#commit(next);
  }

  /** Sets a side's size (its share of the window's width or height). */
  resizeSide(side: Side, size: number) {
    const next = clone(this.layout);
    next.zones[side].size = Math.max(0.01, Math.min(1, size));
    this.#commit(next);
  }

  /**
   * Sets the shares of two groups of a side, `shares` keyed by group id
   * (the others keep theirs).
   */
  resizeGroups(side: Side, shares: Record<string, number>) {
    const next = clone(this.layout);
    for (const g of next.zones[side].groups) if (g.id in shares) g.share = Math.max(0.01, shares[g.id]);
    normalize(next.zones[side]);
    this.#commit(next);
  }

  /** How two neighbouring groups split their room by default: by weight. */
  defaultSplit(a: Group, b: Group): [number, number] {
    const both = a.share + b.share;
    const wa = weight(a.panels);
    const wb = weight(b.panels);
    return [(both * wa) / (wa + wb), (both * wb) / (wa + wb)];
  }

  /** Puts every panel back where it starts (View ▸ Reset layout). */
  reset() {
    this.#commit(defaultLayout());
  }
}

export const panels = new PanelStore();
