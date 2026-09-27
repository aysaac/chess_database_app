import { api } from "../api";
import type { TreeDto } from "../bindings/TreeDto";
import { db } from "./db.svelte";
import { game } from "./game.svelte";
import { notices } from "./notify.svelte";
import { prefs } from "./prefs.svelte";

/** The opening tree (OPEN-01) of the position on the board, from the open database. */
class TreeStore {
  #visible = $state<boolean | null>(null);
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

  async refresh() {
    const view = game.view;
    const info = db.info;
    if (!this.visible || !view || !info) {
      this.data = null;
      return;
    }
    const seq = ++this.#seq;
    this.loading = true;
    try {
      const tree = await api.openingTree(info.handle, view.session);
      if (seq === this.#seq) this.data = tree;
    } catch (e) {
      if (seq === this.#seq) notices.error(e);
    } finally {
      if (seq === this.#seq) this.loading = false;
    }
  }

  /** Plays a tree move on the board (the usual variation choice applies). */
  play(uci: string) {
    return game.play(uci.slice(0, 2), uci.slice(2, 4), uci.length > 4 ? uci[4] : null);
  }
}

export const tree = new TreeStore();
