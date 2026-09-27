import { documentDir, join } from "@tauri-apps/api/path";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { DbInfoDto } from "../bindings/DbInfoDto";
import type { GameRowDto } from "../bindings/GameRowDto";
import type { HeaderQueryDto } from "../bindings/HeaderQueryDto";
import type { NameKindDto } from "../bindings/NameKindDto";
import type { RecentDbDto } from "../bindings/RecentDbDto";
import type { SortKeyDto } from "../bindings/SortKeyDto";
import type { SortSpecDto } from "../bindings/SortSpecDto";
import { formatCount } from "../format";
import { game } from "./game.svelte";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";

const PGN_FILTER = [{ name: "PGN games", extensions: ["pgn"] }];

/**
 * Where file dialogs start: the Documents folder rather than the working
 * directory, which under `tauri dev` is src-tauri/ (writing there used to
 * restart the app).
 */
async function inDocuments(name = ""): Promise<string | undefined> {
  try {
    const dir = await documentDir();
    return name ? await join(dir, name) : dir;
  } catch {
    return name || undefined;
  }
}

class DbStore {
  info = $state<DbInfoDto | null>(null);
  sort = $state<SortSpecDto>({ key: "id", descending: false });
  /** Bumped whenever cached list rows may be stale. */
  listVersion = $state(0);
  sortingKey = $state<SortKeyDto | null>(null);
  /** Recently opened databases, most recent first (DB-07). */
  recent = $state<RecentDbDto[]>([]);
  /** Bumped when the filter changes (the list starts over at the top). */
  filterVersion = $state(0);
  /** Text of the quick filter box (SRCH-02). */
  quickText = $state("");
  /** The last header search, to start the search dialog from. */
  lastQuery = $state<Partial<HeaderQueryDto>>({});
  searching = $state(false);

  /** Rows in the list: the filtered games when a filter is active. */
  get listTotal(): number {
    return this.info?.filter?.count ?? this.info?.games ?? 0;
  }
  #importJobs = new Set<number>();
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    // Let the list grow live while an import is running.
    jobs.onProgress((job) => {
      if (this.#importJobs.has(job.id)) this.#scheduleRefresh();
    });
  }

  #scheduleRefresh() {
    if (this.#refreshTimer) return;
    this.#refreshTimer = setTimeout(async () => {
      this.#refreshTimer = undefined;
      await this.refresh();
    }, 400);
  }

  async refresh() {
    if (!this.info) return;
    try {
      this.info = await api.dbInfo(this.info.handle);
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    }
  }

  async loadRecent() {
    try {
      this.recent = await api.recentDatabases();
    } catch (e) {
      notices.error(e);
    }
  }

  /** Opens the database used last time, if it still exists (QOL-01). */
  async reopenLast() {
    await this.loadRecent();
    const last = this.recent[0];
    if (last?.exists && !this.info) await this.openAt(last.path);
  }

  async forgetRecent(path: string) {
    await api.forgetRecentDatabase(path).catch(notices.error.bind(notices));
    await this.loadRecent();
  }

  #indexing: number | null = null;

  /**
   * Brings the position index up to date in the background (after opening
   * and importing), then refreshes the list for the opening column.
   */
  async ensureIndex() {
    const info = this.info;
    if (!info || this.#indexing !== null) return;
    try {
      // Games imported while it ran need another round.
      for (;;) {
        const job = await api.updateIndex(info.handle);
        if (job === null) break;
        this.#indexing = job;
        const done = await jobs.waitFor(job);
        if (this.info?.handle === info.handle) await this.refresh();
        if (done.state !== "done" || this.info?.handle !== info.handle) break;
      }
    } catch (e) {
      notices.error(e);
    } finally {
      this.#indexing = null;
    }
  }

  /** Limits the list to the games of one player, event, site or annotator (DB-04). */
  async showNameGames(kind: NameKindDto, id: number) {
    const info = this.info;
    if (!info) return;
    this.quickText = "";
    try {
      this.info = await api.showNameGames(info.handle, kind, id);
      this.filterVersion++;
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    }
  }

  /** Renames a name in every game (merging with an existing one). */
  async renameName(kind: NameKindDto, id: number, name: string): Promise<boolean> {
    const info = this.info;
    if (!info) return false;
    try {
      notices.show(await api.renameName(info.handle, kind, id, name));
      this.listVersion++;
      return true;
    } catch (e) {
      notices.error(e);
      return false;
    }
  }

  /** Limits the list to the games reaching the position on screen (SRCH-03). */
  async searchPosition(session: number, full = false) {
    const info = this.info;
    if (!info) return;
    this.quickText = "";
    this.searching = true;
    try {
      if (full) {
        const done = await jobs.waitFor(await api.scanPositionGames(info.handle, session));
        if (done.state !== "done") return;
        this.info = await api.dbInfo(info.handle);
      } else {
        this.info = await api.searchPosition(info.handle, session);
      }
      this.filterVersion++;
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    } finally {
      this.searching = false;
    }
  }

  async #use(info: DbInfoDto) {
    if (this.info && this.info.handle !== info.handle) {
      await api.closeDb(this.info.handle).catch(() => {});
    }
    this.info = info;
    this.sort = { key: "id", descending: false };
    this.quickText = "";
    this.filterVersion++;
    this.listVersion++;
    void this.loadRecent();
    void this.ensureIndex();
  }

  /**
   * Compacts the database (DB-06): removes deleted games for good.
   * Afterwards the list, open games and the position index are refreshed.
   */
  async compact() {
    const info = this.info;
    if (!info) return;
    try {
      const done = await jobs.waitFor(await api.compactDb(info.handle));
      if (this.info?.handle !== info.handle) return;
      this.quickText = "";
      this.info = await api.dbInfo(info.handle);
      this.filterVersion++;
      this.listVersion++;
      await game.refreshAll();
      if (done.state === "done") void this.ensureIndex();
    } catch (e) {
      notices.error(e);
    }
  }

  /** Closes the open database (the game on the board stays). */
  async close() {
    const info = this.info;
    if (!info) return;
    try {
      await api.closeDb(info.handle);
    } catch (e) {
      notices.error(e);
    }
    this.info = null;
    this.quickText = "";
    this.filterVersion++;
    this.listVersion++;
  }

  async openDialog() {
    const path = await open({
      directory: true,
      title: "Open a database (.chessdb folder)",
      defaultPath: await inDocuments(),
    });
    if (typeof path === "string") await this.openAt(path);
  }

  async openAt(path: string) {
    try {
      await this.#use(await api.openDb(path));
    } catch (e) {
      notices.error(e);
    }
  }

  async createDialog() {
    const path = await save({ title: "Create a database", defaultPath: await inDocuments("My games.chessdb") });
    if (path) await this.createAt(path);
  }

  async createAt(path: string) {
    try {
      await this.#use(await api.createDb(path));
      notices.show(`Created ${this.info?.name}`);
    } catch (e) {
      notices.error(e);
    }
  }

  async importDialog() {
    if (!this.info) return;
    const path = await open({
      multiple: false,
      filters: PGN_FILTER,
      title: "Import PGN",
      defaultPath: await inDocuments(),
    });
    if (typeof path === "string") await this.importFrom(path);
  }

  /** Imports a PGN file into the open database; resolves when done. */
  async importFrom(path: string) {
    if (!this.info) return;
    try {
      const id = await api.importPgn(this.info.handle, path);
      this.#importJobs.add(id);
      await jobs.waitFor(id);
      this.#importJobs.delete(id);
      await this.refresh();
      if (this.sort.key !== "id") await this.setSort(this.sort, true);
      void this.ensureIndex();
    } catch (e) {
      notices.error(e);
    }
  }

  async exportDialog() {
    if (!this.info) return;
    const filtered = !!this.info.filter;
    const path = await save({
      title: filtered ? `Export the ${formatCount(this.info.filter!.count)} filtered games` : "Export all games",
      filters: PGN_FILTER,
      defaultPath: await inDocuments(`${this.info.name}${filtered ? " (filtered)" : ""}.pgn`),
    });
    if (!path) return;
    try {
      await api.exportPgn(this.info.handle, path);
    } catch (e) {
      notices.error(e);
    }
  }

  /** Builds the sort order if needed, then switches the list to it. */
  async setSort(spec: SortSpecDto, force = false) {
    if (!this.info) return;
    const same = spec.key === this.sort.key && spec.descending === this.sort.descending;
    if (same && !force) return;
    try {
      this.sortingKey = spec.key;
      const job = await api.sortGames(this.info.handle, spec);
      if (job !== null) {
        const done = await jobs.waitFor(job);
        if (done.state !== "done") return;
      }
      this.sort = spec;
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    } finally {
      this.sortingKey = null;
    }
  }

  toggleSort(key: SortKeyDto) {
    const descending = this.sort.key === key ? !this.sort.descending : false;
    return this.setSort({ key, descending });
  }

  /**
   * Runs a header search whose results become the list (SRCH-01/14);
   * `quick` is the filter box (SRCH-02). An empty query shows all games.
   */
  async search(query: Partial<HeaderQueryDto>, within = false, quick = false) {
    const info = this.info;
    if (!info) return;
    if (!quick) this.lastQuery = query;
    this.searching = true;
    try {
      const job = await api.searchGames(info.handle, query, within, quick);
      if (job !== null) {
        const done = await jobs.waitFor(job);
        if (done.state !== "done") return;
      }
      if (this.info?.handle !== info.handle) return;
      this.info = await api.dbInfo(info.handle);
      this.filterVersion++;
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    } finally {
      this.searching = false;
    }
  }

  quickFilter(text: string) {
    this.quickText = text;
    return this.search({ text }, false, true);
  }

  async clearFilter() {
    const info = this.info;
    if (!info) return;
    this.quickText = "";
    try {
      this.info = await api.clearFilter(info.handle);
      this.filterVersion++;
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    }
  }

  /** The row at `index` of the list as shown (sort and filter applied). */
  async rowAt(index: number): Promise<GameRowDto | null> {
    if (!this.info || index < 0 || index >= this.listTotal) return null;
    try {
      const page = await api.listGames(this.info.handle, this.sort, index, 1);
      return page.rows[0] ?? null;
    } catch (e) {
      notices.error(e);
      return null;
    }
  }

  async setDeleted(ids: number[], deleted: boolean) {
    if (!this.info) return;
    try {
      await api.setDeleted(this.info.handle, ids, deleted);
      this.listVersion++;
      const what = ids.length === 1 ? "game" : `${formatCount(ids.length)} games`;
      notices.show(deleted ? `Marked ${what} as deleted` : `Restored ${what}`);
    } catch (e) {
      notices.error(e);
    }
  }
}

export const db = new DbStore();
