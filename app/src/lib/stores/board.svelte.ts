import type { BoardImageDto } from "../bindings/BoardImageDto";
import { prefs } from "./prefs.svelte";

/** Board colour themes (UI-01); the colours are in board-theme.css. */
export const BOARD_THEMES = [
  { id: "tournament", label: "Tournament green" },
  { id: "walnut", label: "Walnut" },
  { id: "slate", label: "Slate blue" },
  { id: "stone", label: "Stone grey" },
  { id: "coral", label: "Coral" },
] as const;

/** Sizes offered for board images (QOL-06), in pixels per side. */
export const IMAGE_SIZES = [
  { size: 320, label: "Small" },
  { size: 480, label: "Medium" },
  { size: 800, label: "Large" },
] as const;

const DEFAULT_IMAGE_SIZE = 480;

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

  /** Width of board images in pixels (QOL-06). */
  get imageSize(): number {
    const size = prefs.get("boardImageSize", DEFAULT_IMAGE_SIZE);
    return IMAGE_SIZES.some((o) => o.size === size) ? size : DEFAULT_IMAGE_SIZE;
  }

  set imageSize(size: number) {
    prefs.set("boardImageSize", size);
  }

  /**
   * How a board image looks: the theme's own colours (not the dimmed
   * dark-mode ones), with coordinates and the last move as on the board.
   */
  imageLook(orientation: "white" | "black"): BoardImageDto {
    const css = getComputedStyle(document.documentElement);
    return {
      size: this.imageSize,
      orientation,
      light: css.getPropertyValue("--board-light-base").trim(),
      dark: css.getPropertyValue("--board-dark-base").trim(),
      coordinates: this.coordinates,
      lastMove: this.lastMove,
    };
  }
}

export const board = new BoardPrefs();
