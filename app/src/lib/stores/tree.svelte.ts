import { api } from "../api";
import type { TreeDto } from "../bindings/TreeDto";
import { db } from "./db.svelte";
import { game } from "./game.svelte";
import { notices } from "./notify.svelte";
import { panels } from "./panels.svelte";
import { prefs } from "./prefs.svelte";

/** Which database the tree reads: the open one, or the reference database (DB-13). */
export type TreeSource = "open" | "reference";

/** The database a tree is read from. */
export type TreeTarget = { handle: number; name: string; reference: boolean };

/**
 * The opening tree (OPEN-01) of the position on the board, in the Opening
 * tree panel tab (UI-19).
 */
class TreeStore {
  #source = $state<TreeSource | null>(null);
  /** The last tree loaded, which may be for another position or database (see `current`). */
  data = $state<TreeDto | null>(null);
  /** The database `data` was read from. */
  #dataHandle = $state<number | null>(null);
  loading = $state(false);
  #seq = 0;

  /** Whether the Opening tree tab is showing: the tree queries only then. */
  get showing(): boolean {
    return panels.isShowing("tree");
  }

  /**
   * The tree of the position on the board in the database the tree reads,
   * once it's loaded. The last tree stays while the tab is hidden, so going
   * back to it at the same position shows it at once.
   */
  get current(): TreeDto | null {
    const data = this.data;
    return data && data.fen === game.view?.fen && this.#dataHandle === this.target?.handle ? data : null;
  }

  get source(): TreeSource {
    return this.#source ?? (prefs.get<string>("treeSource", "open") === "reference" ? "reference" : "open");
  }

  set source(s: TreeSource) {
    this.#source = s;
    prefs.set("treeSource", s);
  }

  /** The reference database when chosen and open, else the open database. */
  get target(): TreeTarget | null {
    const ref = db.reference?.info;
    if (this.source === "reference" && ref) return { handle: ref.handle, name: ref.name, reference: true };
    return db.info ? { handle: db.info.handle, name: db.info.name, reference: false } : null;
  }

  /** Loads the tree of the position on the board, while the tab is showing. */
  async refresh() {
    if (!this.showing) return;
    const view = game.view;
    const target = this.target;
    if (!view || !target) {
      this.data = null;
      return;
    }
    const seq = ++this.#seq;
    this.loading = true;
    try {
      const tree = await api.openingTree(target.handle, view.session);
      if (seq === this.#seq) {
        this.data = tree;
        this.#dataHandle = target.handle;
      }
    } catch (e) {
      if (seq === this.#seq) notices.error(e);
    } finally {
      if (seq === this.#seq) this.loading = false;
    }
  }

  /**
   * Lists the games reaching the position (`full`: searching every move).
   * For the reference database, the list switches to it first.
   */
  async findGames(full = false) {
    const target = this.target;
    const session = game.view?.session;
    if (!target || session === undefined) return;
    if (target.reference && db.info?.handle !== target.handle && db.reference) {
      await db.openAt(db.reference.path);
      if (db.info?.handle !== target.handle) return;
    }
    await db.searchPosition(session, full);
  }

  /** Plays a tree move on the board (the usual variation choice applies). */
  play(uci: string) {
    return game.playUci(uci);
  }
}

export const tree = new TreeStore();
