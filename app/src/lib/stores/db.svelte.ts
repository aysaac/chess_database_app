import { documentDir, join } from "@tauri-apps/api/path";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, isAppError } from "../api";
import type { CopyModeDto } from "../bindings/CopyModeDto";
import type { DbInfoDto } from "../bindings/DbInfoDto";
import type { DbViewDto } from "../bindings/DbViewDto";
import type { DownloadRequestDto } from "../bindings/DownloadRequestDto";
import type { GameRowDto } from "../bindings/GameRowDto";
import type { HeaderQueryDto } from "../bindings/HeaderQueryDto";
import type { ImportReportDto } from "../bindings/ImportReportDto";
import type { JobDto } from "../bindings/JobDto";
import type { ListSelectionDto } from "../bindings/ListSelectionDto";
import type { NameKindDto } from "../bindings/NameKindDto";
import type { RecentDbDto } from "../bindings/RecentDbDto";
import type { ReferenceDbDto } from "../bindings/ReferenceDbDto";
import type { SortKeyDto } from "../bindings/SortKeyDto";
import type { SortSpecDto } from "../bindings/SortSpecDto";
import { formatCount } from "../format";
import { count, type Ranges } from "../selection";
import { game } from "./game.svelte";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";

export const PGN_FILTER = [{ name: "PGN games", extensions: ["pgn"] }];
/** What Import opens: a PGN file, or a ChessBase database by its `.cbh` file (IO-03). */
const IMPORT_FILTER = [
  { name: "PGN files and ChessBase databases", extensions: ["pgn", "cbh"] },
  ...PGN_FILTER,
  { name: "ChessBase databases", extensions: ["cbh"] },
];
/** List columns can be resized within these widths (CSS pixels). */
export const MIN_COLUMN_WIDTH = 24;
export const MAX_COLUMN_WIDTH = 1200;

/** A database games can be copied or moved to (DB-06, QOL-04). */
export type CopyTarget = { path: string; name: string; reference: boolean };

/**
 * Where file dialogs start: the Documents folder rather than the working
 * directory, which under `tauri dev` is src-tauri/ (writing there used to
 * restart the app).
 */
export async function inDocuments(name = ""): Promise<string | undefined> {
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
  /** The reference database (DB-13) the opening tree can use, if one is set. */
  reference = $state<ReferenceDbDto | null>(null);
  /** Warnings of the last import into the open database, until dismissed. */
  importReport = $state<ImportReportDto | null>(null);
  /**
   * Widths in CSS pixels of the list columns the user resized, by column
   * id; saved per database with the sort order (QOL-07).
   */
  columns = $state<Record<string, number>>({});
  /** Description of the saved filter being made again after opening (QOL-07). */
  restoring = $state<string | null>(null);
  /**
   * Bumped when Show games outside the list pane (a cross-table's, DB-16)
   * has limited the list: the pane brings its Games tab forward.
   */
  gamesShown = $state(0);

  /** Rows in the list: the filtered games when a filter is active. */
  get listTotal(): number {
    return this.info?.filter?.count ?? this.info?.games ?? 0;
  }

  /**
   * Where games of the open database can be copied (QOL-04): the reference
   * database and the recently opened ones that still exist, not the open one.
   */
  get copyTargets(): CopyTarget[] {
    const here = this.info;
    const out: CopyTarget[] = [];
    const ref = this.reference;
    if (ref?.info && ref.info.handle !== here?.handle) out.push({ path: ref.path, name: ref.name, reference: true });
    for (const r of this.recent) {
      if (r.exists && r.path !== here?.path && !out.some((t) => t.path === r.path)) {
        out.push({ path: r.path, name: r.name, reference: false });
      }
    }
    return out.slice(0, 8);
  }
  #importJobs = new Set<number>();
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  /** Layout saves, one after the other so the last one wins. */
  #saving: Promise<unknown> = Promise.resolve();
  /** Bumped by each `setSort`, so only the latest one is applied. */
  #sortSeq = 0;

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

  /** Databases whose position index is being brought up to date. */
  #indexing = new Set<number>();

  /**
   * Brings the position index of `handle` up to date in the background.
   * `after` runs after each round and says whether to go on (false once the
   * database is no longer used).
   */
  async #index(handle: number, after: () => Promise<boolean>) {
    if (this.#indexing.has(handle)) return;
    this.#indexing.add(handle);
    try {
      // Games imported while it ran need another round.
      for (;;) {
        const job = await api.updateIndex(handle);
        if (job === null) break;
        const done = await jobs.waitFor(job);
        if (!(await after()) || done.state !== "done") break;
      }
    } catch (e) {
      notices.error(e);
    } finally {
      this.#indexing.delete(handle);
    }
  }

  /**
   * Indexes the open database (after opening and importing), then refreshes
   * the list for the opening column.
   */
  async ensureIndex() {
    const handle = this.info?.handle;
    if (handle === undefined) return;
    await this.#index(handle, async () => {
      if (this.info?.handle !== handle) return false;
      await this.refresh();
      return true;
    });
  }

  /** Loads the reference database from the settings (opening it), and indexes it. */
  async loadReference() {
    try {
      this.#setReference(await api.referenceDb());
    } catch (e) {
      notices.error(e);
    }
  }

  #setReference(reference: ReferenceDbDto | null) {
    this.reference = reference;
    if (reference?.error) notices.show(`Reference database: ${reference.error}`, "error");
    const handle = reference?.info?.handle;
    if (handle === undefined) return;
    void this.#index(handle, async () => {
      if (this.reference?.info?.handle !== handle) return false;
      const info = await api.dbInfo(handle);
      const current = this.reference;
      if (current?.info?.handle !== handle) return false;
      current.info = info;
      return true;
    });
  }

  /** Makes `path` the reference database (DB-13), or stops using one (null). */
  async setReference(path: string | null) {
    try {
      this.#setReference(await api.setReferenceDb(path));
      if (this.reference?.info) notices.show(`${this.reference.name} is now the reference database`);
    } catch (e) {
      notices.error(e);
    }
  }

  async chooseReference() {
    const path = await open({
      directory: true,
      title: "Choose the reference database (.chessdb folder)",
      defaultPath: await inDocuments(),
    });
    if (typeof path === "string") await this.setReference(path);
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

  /**
   * Limits the list to the games of one edition of event `event` (a
   * cross-table's Show games, DB-16), and has the list pane show them.
   */
  async showEditionGames(event: number, edition: number) {
    const info = this.info;
    if (!info) return;
    this.quickText = "";
    try {
      this.info = await api.showEditionGames(info.handle, event, edition);
      this.filterVersion++;
      this.listVersion++;
      this.gamesShown++;
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
      await this.#saving;
      await api.closeDb(this.info.handle).catch(() => {});
    }
    this.info = info;
    this.importReport = null;
    this.sort = { key: "id", descending: false };
    this.sortingKey = null;
    this.columns = {};
    this.restoring = null;
    this.quickText = "";
    this.filterVersion++;
    this.listVersion++;
    void this.loadRecent();
    void this.ensureIndex();
    await this.#restoreView(info.handle);
  }

  /**
   * Puts back the sort order, column widths and filter the list of `handle`
   * had last time (QOL-07). Resolves once they're in place.
   */
  async #restoreView(handle: number) {
    let view: DbViewDto;
    try {
      view = await api.dbView(handle);
    } catch (e) {
      notices.error(e);
      return;
    }
    if (this.info?.handle !== handle) return;
    this.columns = view.columns;
    if (view.lastQuery) this.lastQuery = view.lastQuery;
    await Promise.all([
      view.sort && this.setSort(view.sort, false, false),
      view.filter && this.#restoreFilter(handle, view.filter, view.quickText),
    ]);
  }

  async #restoreFilter(handle: number, description: string, quickText: string) {
    this.quickText = quickText;
    this.restoring = description;
    try {
      const job = await api.restoreFilter(handle);
      if (job !== null) await jobs.waitFor(job);
      if (this.info?.handle !== handle) return;
      this.info = await api.dbInfo(handle);
      // It couldn't be made again (the job said why): nothing to show in the box.
      if (!this.info.filter && this.quickText === quickText) this.quickText = "";
      this.filterVersion++;
      this.listVersion++;
    } catch (e) {
      notices.error(e);
    } finally {
      if (this.info?.handle === handle) this.restoring = null;
    }
  }

  /**
   * Saves the sort order and column widths of the open database's list
   * (QOL-07). Saves run in order, so the last one wins.
   */
  saveLayout() {
    const handle = this.info?.handle;
    if (handle === undefined) return;
    const sort = { ...this.sort };
    const columns = { ...this.columns };
    this.#saving = this.#saving
      .then(() => api.saveDbLayout(handle, sort, columns))
      .catch((e) => console.error("couldn't save the list layout", e));
  }

  /**
   * Sets the width of a list column in CSS pixels, or with `null` goes back
   * to its default width. `save` = false while dragging (save at the end).
   */
  setColumnWidth(id: string, width: number | null, save = true) {
    const next = { ...this.columns };
    if (width === null) delete next[id];
    else next[id] = Math.round(Math.min(MAX_COLUMN_WIDTH, Math.max(MIN_COLUMN_WIDTH, width)));
    this.columns = next;
    if (save) this.saveLayout();
  }

  /** Every list column back to its default width. */
  resetColumns() {
    this.columns = {};
    this.saveLayout();
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
      // The warnings' game ids were renumbered too.
      if (this.importReport) this.importReport = await api.importReport(info.handle);
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
      await this.#saving;
      await api.closeDb(info.handle);
    } catch (e) {
      notices.error(e);
    }
    this.info = null;
    this.importReport = null;
    this.restoring = null;
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

  /** Opens the sample games (QOL-08), creating the database the first time. */
  async openSample(): Promise<boolean> {
    try {
      await this.#use(await api.openSampleDb());
      return true;
    } catch (e) {
      notices.error(e);
      return false;
    }
  }

  async createDialog() {
    const path = await save({ title: "Create a database", defaultPath: await inDocuments("My games.chessdb") });
    if (path) await this.createAt(path);
  }

  /** Creates a database and shows it; throws if it can't be created. */
  async create(path: string) {
    await this.#use(await api.createDb(path));
    notices.show(`Created ${this.info?.name}`);
  }

  async createAt(path: string) {
    try {
      await this.create(path);
    } catch (e) {
      notices.error(e);
    }
  }

  async importDialog() {
    if (!this.info) return;
    const path = await open({
      multiple: false,
      filters: IMPORT_FILTER,
      title: "Import games",
      defaultPath: await inDocuments(),
    });
    if (typeof path === "string") await this.importFrom(path);
  }

  /**
   * Imports a PGN file, or a ChessBase database given its `.cbh` file, into
   * the open database; resolves when done, with whether everything was
   * imported (false if cancelled or failed).
   */
  async importFrom(path: string): Promise<boolean> {
    const done = await this.#addGames((handle) =>
      /\.cbh$/i.test(path) ? api.importCb(handle, path) : api.importPgn(handle, path),
    );
    return done?.state === "done";
  }

  /**
   * Downloads a player's games from Lichess or Chess.com (NET-01) into the
   * open database; resolves with the job's final status (null if it
   * couldn't start). `started` gets the job's id.
   */
  download(request: DownloadRequestDto, started?: (job: number) => void): Promise<JobDto | null> {
    return this.#addGames((handle) => api.downloadGames(handle, request), started);
  }

  /** Runs a job that adds games to the open database, the list growing as it goes. */
  async #addGames(
    start: (handle: number) => Promise<number>,
    started?: (job: number) => void,
  ): Promise<JobDto | null> {
    if (!this.info) return null;
    const handle = this.info.handle;
    try {
      this.importReport = null;
      const id = await start(handle);
      started?.(id);
      this.#importJobs.add(id);
      const done = await jobs.waitFor(id);
      this.#importJobs.delete(id);
      if (this.info?.handle === handle) this.importReport = await api.importReport(handle);
      await this.refresh();
      if (this.sort.key !== "id") await this.setSort(this.sort, true);
      void this.ensureIndex();
      return done;
    } catch (e) {
      notices.error(e);
      return null;
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

  /**
   * Builds the sort order if needed, then switches the list to it and
   * (with `save`) remembers it for the database. Only the latest call wins.
   */
  async setSort(spec: SortSpecDto, force = false, save = true) {
    if (!this.info) return;
    const handle = this.info.handle;
    const same = spec.key === this.sort.key && spec.descending === this.sort.descending;
    if (same && !force) return;
    const seq = ++this.#sortSeq;
    try {
      this.sortingKey = spec.key;
      const job = await api.sortGames(handle, spec);
      if (job !== null) {
        const done = await jobs.waitFor(job);
        if (done.state !== "done") return;
      }
      if (seq !== this.#sortSeq || this.info?.handle !== handle) return;
      this.sort = spec;
      this.listVersion++;
      if (save) this.saveLayout();
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
      await this.refresh();
      const what = ids.length === 1 ? "game" : `${formatCount(ids.length)} games`;
      notices.show(deleted ? `Marked ${what} as deleted` : `Restored ${what}`);
    } catch (e) {
      notices.error(e);
    }
  }

  /**
   * Runs `f` on the selected rows (`ranges` of the list as shown). If a
   * saved game invalidated the list's sort order meanwhile, it's rebuilt and
   * `f` runs again.
   */
  async #onRows<T>(ranges: Ranges, f: (handle: number, selection: ListSelectionDto) => Promise<T>): Promise<T> {
    const info = this.info;
    if (!info) throw new Error("No database is open");
    const selection: ListSelectionDto = { sort: { ...this.sort }, ranges: ranges.map(([a, b]) => [a, b]) };
    try {
      return await f(info.handle, selection);
    } catch (e) {
      if (!isAppError(e) || e.code !== "notSorted") throw e;
      await this.setSort(selection.sort, true);
      return f(info.handle, selection);
    }
  }

  /** Marks the selected rows as deleted, or restores them. */
  async setDeletedRows(ranges: Ranges, deleted: boolean) {
    if (!this.info || !ranges.length) return;
    try {
      const changed = await this.#onRows(ranges, (h, sel) => api.setDeletedRows(h, sel, deleted));
      await this.refresh();
      const n = count(ranges);
      const what = n === 1 ? "game" : `${formatCount(n)} games`;
      if (changed === 0) notices.show(deleted ? "Already marked as deleted" : "Not marked as deleted");
      else notices.show(deleted ? `Marked ${what} as deleted` : `Restored ${what}`);
    } catch (e) {
      notices.error(e);
    }
  }

  /**
   * Copies or moves the selected rows to the database at `target` (DB-06),
   * as dropping them on it does (QOL-04); `create` makes the database first
   * if it doesn't exist. Resolves with the finished job, or null on error.
   */
  async copyRows(ranges: Ranges, target: string, mode: CopyModeDto, create = false): Promise<JobDto | null> {
    const info = this.info;
    if (!info || !ranges.length) return null;
    try {
      const job = await this.#onRows(ranges, (h, sel) => api.copyGames(h, sel, target, create, mode));
      const done = await jobs.waitFor(job);
      // Moved games are marked as deleted here now.
      if (mode === "move" && this.info?.handle === info.handle) await this.refresh();
      if (create) void this.loadRecent();
      if (this.reference?.path === target) void this.loadReference();
      return done;
    } catch (e) {
      notices.error(e);
      return null;
    }
  }

  /** Asks where to create a database, then copies or moves the selected rows into it. */
  async copyRowsToNewDialog(ranges: Ranges, mode: CopyModeDto): Promise<JobDto | null> {
    const path = await save({
      title: mode === "move" ? "Move the games to a new database" : "Copy the games to a new database",
      defaultPath: await inDocuments("Clip.chessdb"),
    });
    return path ? this.copyRows(ranges, path, mode, true) : null;
  }

  /** The rows at `indexes` of the list as shown (null where there's none). */
  rowsAt(indexes: number[]): Promise<(GameRowDto | null)[]> {
    return Promise.all(indexes.map((i) => this.rowAt(i)));
  }
}

export const db = new DbStore();
