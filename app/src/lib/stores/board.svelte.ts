import { prefs } from "./prefs.svelte";

/** Board colour themes (UI-01); the colours are in board-theme.css. */
export const BOARD_THEMES = [
  { id: "tournament", label: "Tournament green" },
  { id: "walnut", label: "Walnut" },
  { id: "slate", label: "Slate blue" },
  { id: "stone", label: "Stone grey" },
  { id: "coral", label: "Coral" },
] as const;

/** How the boards look (UI-01), saved with the preferences. */
class BoardPrefs {
  get theme(): string {
    const id = prefs.get("boardTheme", "tournament");
    return BOARD_THEMES.some((t) => t.id === id) ? id : "tournament";
  }

  set theme(id: string) {
    prefs.set("boardTheme", id);
  }

  /** File and rank labels along the edges. */
  get coordinates(): boolean {
    return prefs.get("boardCoordinates", true);
  }

  set coordinates(on: boolean) {
    prefs.set("boardCoordinates", on);
  }

  /** Dots on the squares the picked-up piece can move to. */
  get legalMoves(): boolean {
    return prefs.get("boardLegalMoves", true);
  }

  set legalMoves(on: boolean) {
    prefs.set("boardLegalMoves", on);
  }

  /** The squares of the last move tinted. */
  get lastMove(): boolean {
    return prefs.get("boardLastMove", true);
  }

  set lastMove(on: boolean) {
    prefs.set("boardLastMove", on);
  }
}

export const board = new BoardPrefs();
