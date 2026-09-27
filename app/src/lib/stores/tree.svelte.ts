import { api } from "../api";
import type { TreeDto } from "../bindings/TreeDto";
import { db } from "./db.svelte";
import { game } from "./game.svelte";
import { notices } from "./notify.svelte";
import { prefs } from "./prefs.svelte";

/** Which database the tree reads: the open one, or the reference database (DB-13). */
export type TreeSource = "open" | "reference";

/** The database a tree is read from. */
export type TreeTarget = { handle: number; name: string; reference: boolean };

/** The opening tree (OPEN-01) of the position on the board. */
class TreeStore {
  #visible = $state<boolean | null>(null);
  #source = $state<TreeSource | null>(null);
  data = $state<TreeDto | null>(null);
  loading = $state(false);
  #seq = 0;

  get visible(): boolean {
    return this.#visible ?? prefs.get("treeVisible", false);
  }

  set visible(v: boolean) {
    this.#visible = v;
    prefs.set("treeVisible", v);
  }

  toggle() {
    this.visible = !this.visible;
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

  async refresh() {
    const view = game.view;
    const target = this.target;
    if (!this.visible || !view || !target) {
      this.data = null;
      return;
    }
    const seq = ++this.#seq;
    this.loading = true;
    try {
      const tree = await api.openingTree(target.handle, view.session);
      if (seq === this.#seq) this.data = tree;
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
    return game.play(uci.slice(0, 2), uci.slice(2, 4), uci.length > 4 ? uci[4] : null);
  }
}

export const tree = new TreeStore();
