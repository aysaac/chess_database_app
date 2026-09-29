// Dragging games out of the game list (QOL-04): onto the board or the tab
// strip to open them, onto a database in the drop shelf to copy them there
// (Shift moves them). Built on pointer events rather than HTML5 drag and
// drop: that behaves the same in every webview and leaves the window's
// OS-level file drop (IO-09) to Tauri.
import { formatCount } from "../format";
import { firstRows, type Ranges } from "../selection";
import { db } from "./db.svelte";
import { game } from "./game.svelte";
import { notices } from "./notify.svelte";

/** What a drop can land on; elements say so with `data-drop`. */
export type DropKind = "board" | "tabs" | "db" | "new-db";
export type DropTarget = { kind: DropKind; path?: string; name?: string };

/** Games being dragged: rows of the list as shown. */
export type GamesDrag = {
  handle: number;
  ranges: Ranges;
  count: number;
  /** "Carlsen – Nakamura" for one game, "12 games" for more. */
  label: string;
};

/** Pixels the pointer moves with the button down before a drag starts. */
const THRESHOLD = 6;
/** Most games a drop on the board opens (each gets a tab). */
export const MAX_OPEN = 10;

function targetAt(x: number, y: number): DropTarget | null {
  const el = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-drop]");
  if (!el) return null;
  return { kind: el.dataset.drop as DropKind, path: el.dataset.path, name: el.dataset.name };
}

class DragStore {
  /** The games being dragged, once the pointer has moved far enough. */
  games = $state<GamesDrag | null>(null);
  x = $state(0);
  y = $state(0);
  target = $state<DropTarget | null>(null);
  /** Shift is held: the games are moved instead of copied. */
  move = $state(false);
  #pending: { x: number; y: number; make: () => GamesDrag | null; onClick: () => void } | null = null;

  /**
   * Called on pointerdown on a list row. A drag of what `make` returns
   * starts once the pointer moves a few pixels with the button down; if it
   * doesn't, `onClick` runs on pointerup.
   */
  arm(e: PointerEvent, make: () => GamesDrag | null, onClick: () => void = () => {}) {
    this.#end();
    this.#pending = { x: e.clientX, y: e.clientY, make, onClick };
    window.addEventListener("pointermove", this.#onMove);
    window.addEventListener("pointerup", this.#onUp);
    window.addEventListener("pointercancel", this.cancel);
    window.addEventListener("blur", this.cancel);
    window.addEventListener("keydown", this.#onKey, true);
    window.addEventListener("keyup", this.#onKey, true);
  }

  /** Drops nothing. */
  cancel = () => this.#end();

  #end() {
    this.#pending = null;
    this.games = null;
    this.target = null;
    this.move = false;
    document.body.classList.remove("dragging-games");
    window.removeEventListener("pointermove", this.#onMove);
    window.removeEventListener("pointerup", this.#onUp);
    window.removeEventListener("pointercancel", this.cancel);
    window.removeEventListener("blur", this.cancel);
    window.removeEventListener("keydown", this.#onKey, true);
    window.removeEventListener("keyup", this.#onKey, true);
  }

  #onMove = (e: PointerEvent) => {
    const pending = this.#pending;
    if (!pending) return;
    if (!this.games) {
      if (Math.hypot(e.clientX - pending.x, e.clientY - pending.y) < THRESHOLD) return;
      const games = pending.make();
      if (!games) return this.#end();
      this.games = games;
      document.body.classList.add("dragging-games");
      // Fresh drop targets: databases opened since start-up.
      void db.loadRecent();
    }
    // Released outside the window: no pointerup came.
    if ((e.buttons & 1) === 0) return this.#end();
    this.x = e.clientX;
    this.y = e.clientY;
    this.move = e.shiftKey;
    this.target = targetAt(e.clientX, e.clientY);
  };

  #onUp = (e: PointerEvent) => {
    const pending = this.#pending;
    const games = this.games;
    const move = e.shiftKey;
    const target = games ? targetAt(e.clientX, e.clientY) : null;
    this.#end();
    if (!games) pending?.onClick();
    else if (target) void this.#drop(games, target, move);
  };

  #onKey = (e: KeyboardEvent) => {
    if (!this.games) return;
    if (e.key === "Escape" && e.type === "keydown") {
      e.preventDefault();
      e.stopPropagation();
      this.#end();
    } else if (e.key === "Shift") {
      this.move = e.type === "keydown";
    }
  };

  async #drop(g: GamesDrag, t: DropTarget, move: boolean) {
    const mode = move ? "move" : "copy";
    switch (t.kind) {
      case "board":
      case "tabs": {
        const indexes = firstRows(g.ranges, MAX_OPEN);
        const rows = await db.rowsAt(indexes);
        const found = indexes.flatMap((index, i) => (rows[i] ? [{ id: rows[i].id, index }] : []));
        await game.openRows(g.handle, found, t.kind === "tabs");
        if (g.count > MAX_OPEN) notices.show(`Opened the first ${MAX_OPEN} of ${formatCount(g.count)} games`);
        break;
      }
      case "db":
        if (t.path) await db.copyRows(g.ranges, t.path, mode);
        break;
      case "new-db":
        await db.copyRowsToNewDialog(g.ranges, mode);
        break;
    }
  }

  /** What dropping here would do, for the label under the pointer. */
  get hint(): string {
    const g = this.games;
    if (!g) return "";
    const verb = this.move ? "Move" : "Copy";
    switch (this.target?.kind) {
      case "board":
        return g.count === 1 ? "Open on the board" : `Open ${Math.min(g.count, MAX_OPEN)} games in tabs`;
      case "tabs":
        return g.count === 1 ? "Open in a new tab" : `Open ${Math.min(g.count, MAX_OPEN)} games in new tabs`;
      case "db":
        return `${verb} to ${this.target.name}`;
      case "new-db":
        return `${verb} to a new database`;
      default:
        return "Drop on the board, or on a database to copy";
    }
  }
}

export const drag = new DragStore();
