import { api } from "../api";
import type { HeaderQueryDto } from "../bindings/HeaderQueryDto";
import type { OpenDbDto } from "../bindings/OpenDbDto";
import type { SearchInfoDto } from "../bindings/SearchInfoDto";
import type { SortKeyDto } from "../bindings/SortKeyDto";
import type { SortSpecDto } from "../bindings/SortSpecDto";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";

/**
 * A search across several open databases (SRCH-13): its results in one
 * list (the list pane's Search results tab), each row from its own database.
 */
class CrossStore {
  /** The databases open in the app, as last asked (the search dialog's choice). */
  open = $state<OpenDbDto[]>([]);
  /** The results shown; null when there are none. */
  info = $state<SearchInfoDto | null>(null);
  sort = $state<SortSpecDto>({ key: "id", descending: false });
  sortingKey = $state<SortKeyDto | null>(null);
  searching = $state(false);
  /** Bumped whenever the rows shown may have changed (a new search, another sort). */
  version = $state(0);
  /** Bumped when a search finished: the list pane brings its results forward. */
  shown = $state(0);
  #sortSeq = 0;

  async loadOpen() {
    try {
      this.open = await api.openDatabases();
    } catch (e) {
      notices.error(e);
    }
  }

  /**
   * Searches the open databases `handles` for `query`; with `session`, only
   * the games reaching the position on that tab's board.
   */
  async run(handles: number[], query: Partial<HeaderQueryDto>, session: number | null = null) {
    this.searching = true;
    try {
      const start = await api.searchDatabases(handles, query, session);
      const done = await jobs.waitFor(start.job);
      if (done.state !== "done") return;
      const previous = this.info?.search;
      this.info = await api.searchInfo(start.search);
      this.sort = { key: "id", descending: false };
      this.version++;
      this.shown++;
      if (previous !== undefined && previous !== start.search) void api.closeSearch(previous).catch(() => {});
    } catch (e) {
      notices.error(e);
    } finally {
      this.searching = false;
    }
  }

  /** Puts the results in a sort order (made in a job the first time). Only the latest call wins. */
  async setSort(spec: SortSpecDto, force = false) {
    const info = this.info;
    if (!info) return;
    const same = spec.key === this.sort.key && spec.descending === this.sort.descending;
    if (same && !force) return;
    const seq = ++this.#sortSeq;
    try {
      this.sortingKey = spec.key;
      const job = await api.searchSort(info.search, spec);
      if (job !== null) {
        const done = await jobs.waitFor(job);
        if (done.state !== "done") return;
      }
      if (seq !== this.#sortSeq || this.info?.search !== info.search) return;
      this.sort = spec;
      this.version++;
    } catch (e) {
      notices.error(e);
    } finally {
      if (seq === this.#sortSeq) this.sortingKey = null;
    }
  }

  toggleSort(key: SortKeyDto) {
    const descending = this.sort.key === key ? !this.sort.descending : false;
    return this.setSort({ key, descending });
  }

  /** Forgets the results (the tab goes). */
  async close() {
    const info = this.info;
    this.info = null;
    this.version++;
    if (info) await api.closeSearch(info.search).catch(() => {});
  }
}

export const cross = new CrossStore();
