// Panel tabs (UI-19): the panels around the board, grouped as tabs. These
// are *panel* tabs (Notation, Opening tree…), not the game tabs above the
// board (GameTabs.svelte, UI-05).
//
// The layout is the model docking (UI-04) builds on: each side of the
// window holds groups of panels, a group shows one of its panels at a time,
// and sides and groups have sizes. Until docking, the app always uses the
// default layout; what's restored from the saved one is the tab each group
// shows.
import { prefs } from "./prefs.svelte";

/** A panel: what a panel tab shows. */
export type PanelId = "notation" | "tree" | "reference" | "analysis" | "games";
/** A side of the window that panels dock to. */
export type Side = "left" | "right" | "top" | "bottom";
/** Panels shown as tabs; `active` is the one showing, `share` the group's part of its side. */
export type Group = { panels: PanelId[]; active: PanelId; share: number };
/** A side's groups; `size` is the side's share of the window (0 when it has none). */
export type Zone = { size: number; groups: Group[] };
/** The whole layout, saved as the `layout` pref. */
export type Layout = { version: 1; zones: Record<Side, Zone> };

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

/**
 * Today's layout: Notation and Opening tree as tabs on the right, above the
 * engine analysis, and the game list at the bottom. Sizes and shares aren't
 * applied before docking (UI-04): the right side takes the width the board
 * leaves, and the list keeps its own divider (the `listShare` pref).
 */
export function defaultLayout(): Layout {
  const empty = (): Zone => ({ size: 0, groups: [] });
  return {
    version: 1,
    zones: {
      left: empty(),
      right: {
        size: 0.6,
        groups: [
          { panels: ["notation", "tree"], active: "notation", share: 0.75 },
          { panels: ["analysis"], active: "analysis", share: 0.25 },
        ],
      },
      top: empty(),
      bottom: { size: 0.28, groups: [{ panels: ["games"], active: "games", share: 1 }] },
    },
  };
}

/** Every group of a layout, side by side in `SIDES` order. */
export function allGroups(layout: Layout): Group[] {
  return SIDES.flatMap((side) => layout.zones[side].groups);
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isShare = (v: unknown): v is number => typeof v === "number" && v >= 0 && v <= 1;
const isPanel = (v: unknown): v is PanelId => PANEL_IDS.includes(v as PanelId);

/**
 * `raw` as a layout, if it is one: version 1, all four sides, sizes and
 * shares from 0 to 1, groups that aren't empty and show one of their own
 * panels, and each panel at most once. Anything else gives null.
 */
export function parseLayout(raw: unknown): Layout | null {
  if (!isRecord(raw) || raw.version !== 1 || !isRecord(raw.zones)) return null;
  const seen = new Set<PanelId>();
  const zones = {} as Record<Side, Zone>;
  for (const side of SIDES) {
    const zone = raw.zones[side];
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
      groups.push({ panels, active: g.active, share: g.share });
    }
    zones[side] = { size: zone.size, groups };
  }
  return { version: 1, zones };
}

/**
 * The layout a session starts with: the default one, showing the tabs the
 * saved layout (`saved`, the `layout` pref) showed. Without a readable saved
 * layout, the older `treeVisible` pref (QOL-01) says whether the Opening
 * tree tab shows; anything else unreadable gives the default.
 */
export function restoreLayout(saved: unknown, treeVisible: boolean): Layout {
  const layout = defaultLayout();
  const parsed = parseLayout(saved);
  const shown: PanelId[] = parsed ? allGroups(parsed).map((g) => g.active) : treeVisible ? ["tree"] : [];
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

/**
 * Whether a group takes only the height of its content. Until docking
 * (UI-04) sizes groups by their share, the engine analysis does (it shows
 * while the engine runs) and the other groups of a side share the rest.
 */
export function fitsContent(group: Group): boolean {
  return group.panels.every((p) => p === "analysis");
}

/** The panel layout and which tab each group shows (UI-19). */
class PanelStore {
  /** Set once a tab is chosen in this session; until then the saved layout applies. */
  #chosen = $state.raw<Layout | null>(null);
  #restored = $derived(restoreLayout(prefs.raw("layout"), prefs.get("treeVisible", false)));

  get layout(): Layout {
    return this.#chosen ?? this.#restored;
  }

  /** The group holding `panel`, or null when the layout doesn't have it. */
  groupOf(panel: PanelId): Group | null {
    return allGroups(this.layout).find((g) => g.panels.includes(panel)) ?? null;
  }

  /**
   * Whether `panel` is the tab its group shows. A panel does its work (the
   * opening tree's queries, for one) only while it's showing.
   */
  isShowing(panel: PanelId): boolean {
    return this.groupOf(panel)?.active === panel;
  }

  /** Shows `panel`'s tab in its group, and remembers it. */
  show(panel: PanelId) {
    if (!this.groupOf(panel) || this.isShowing(panel)) return;
    const { zones } = this.layout;
    const next = { version: 1, zones: {} } as Layout;
    for (const side of SIDES) {
      next.zones[side] = {
        ...zones[side],
        groups: zones[side].groups.map((g) => (g.panels.includes(panel) ? { ...g, active: panel } : g)),
      };
    }
    this.#chosen = next;
    prefs.set("layout", next);
    // The layout replaces the older pref (QOL-01 kept the tree's visibility).
    if (prefs.raw("treeVisible") != null) prefs.set("treeVisible", null);
  }
}

export const panels = new PanelStore();
