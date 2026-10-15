// The database manager (DB-07): favourites over the recent-databases list
// (`db.recent`, db.svelte.ts) and the icon shown for a database, guessed
// from its name so a "Blitz Arena" gets a lightning bolt without the user
// having to say so (a manual override can follow later).
import type { IconName } from "../icons/lucide";
import { prefs } from "./prefs.svelte";

type Kind =
  "blitz" | "rapid" | "classical" | "correspondence" | "repertoire" | "training" | "default";

const KIND_ICON: Record<Kind, IconName> = {
  blitz: "zap",
  rapid: "clock",
  classical: "hourglass",
  correspondence: "mail",
  repertoire: "bookmark",
  training: "target",
  default: "database",
};

/** Keyword → kind, checked in order (the first match wins). */
const KEYWORDS: [RegExp, Kind][] = [
  [/\bblitz\b/i, "blitz"],
  [/\b(rapid|bullet)\b/i, "rapid"],
  [/\b(classical|standard|slow)\b/i, "classical"],
  [/\b(corr(espondence)?|iccf)\b/i, "correspondence"],
  [/\b(rep(ertoire)?|prep(aration)?)\b/i, "repertoire"],
  [/\b(train(ing)?|tactics?|puzzles?|exercises?)\b/i, "training"],
];

/** The icon for a database, guessed from its name. */
export function inferDbIcon(name: string): IconName {
  const kind = KEYWORDS.find(([re]) => re.test(name))?.[1] ?? "default";
  return KIND_ICON[kind];
}

/** Where the preview sits relative to the grid (a Settings choice). */
export const PREVIEW_LAYOUTS = [
  { id: "side", label: "Side" },
  { id: "bottom", label: "Bottom" },
] as const;
export type PreviewLayout = (typeof PREVIEW_LAYOUTS)[number]["id"];

class Library {
  /** Favourited database paths, most recently starred last. */
  get favorites(): string[] {
    const raw = prefs.raw("libraryFavorites");
    return Array.isArray(raw) ? raw.filter((p): p is string => typeof p === "string") : [];
  }

  isFavorite(path: string): boolean {
    return this.favorites.includes(path);
  }

  toggleFavorite(path: string) {
    const favs = this.favorites;
    prefs.set(
      "libraryFavorites",
      favs.includes(path) ? favs.filter((p) => p !== path) : [...favs, path],
    );
  }

  /** Side (a right-hand panel) or bottom (a ChessBase-style horizontal one). */
  get previewLayout(): PreviewLayout {
    const id = prefs.get("libraryPreviewLayout", "side");
    return PREVIEW_LAYOUTS.some((l) => l.id === id) ? (id as PreviewLayout) : "side";
  }

  set previewLayout(id: PreviewLayout) {
    prefs.set("libraryPreviewLayout", id);
  }
}

export const library = new Library();
