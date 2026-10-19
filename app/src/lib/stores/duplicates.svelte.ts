import { api } from "../api";
import type { DuplicatesDto } from "../bindings/DuplicatesDto";
import type { JobDto } from "../bindings/JobDto";
import { db } from "./db.svelte";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";

/** What a job of the dialog does. */
export type DuplicatesWork = "find" | "delete" | "restore";

/**
 * Duplicate games (DB-08) in the open database: the search, deleting and
 * restoring run as jobs in Rust, which keeps the reports per database; this
 * holds the report for the dialog and runs the actions on it. Each action
 * names the report it was shown (`scan`): Rust refuses it once another
 * search (maybe in another window) replaced that report, or the database
 * changed under it, and the dialog then loads the report as it is now.
 */
class DuplicatesStore {
  /** What the last search in the open database found, if one ran. */
  report = $state<DuplicatesDto | null>(null);
  /** The database `report` is for. */
  handle = $state<number | null>(null);
  /** The job this window runs (null `id` while it starts), and its database. */
  job = $state<{ id: number | null; handle: number; work: DuplicatesWork } | null>(null);

  /** The running job's progress, once it reported any. */
  get progress(): JobDto | undefined {
    const id = this.job?.id;
    return id == null ? undefined : jobs.running.find((j) => j.id === id);
  }

  /** What this window is doing to the open database, if anything. */
  get working(): DuplicatesWork | null {
    return this.job && this.job.handle === db.info?.handle ? this.job.work : null;
  }

  /** Whether a search of the open database is running. */
  get searching(): boolean {
    return this.working === "find";
  }

  /** Loads the report of the open database (null if it has none yet). */
  async load() {
    const handle = db.info?.handle;
    if (handle === undefined) {
      this.report = null;
      this.handle = null;
      return;
    }
    try {
      const report = await api.duplicatesReport(handle);
      if (db.info?.handle !== handle) return;
      this.report = report;
      this.handle = handle;
    } catch (e) {
      notices.error(e);
    }
  }

  /** Looks for duplicates in the open database; resolves when done. */
  async find(fuzzy: boolean) {
    await this.#run("find", (handle) => api.findDuplicates(handle, fuzzy));
  }

  /** Marks the copies of the report shown as deleted (restorable until compacting). */
  async remove() {
    const scan = this.#scan();
    if (scan !== null) await this.#run("delete", (handle) => api.deleteDuplicates(handle, scan));
  }

  /** Restores the copies deleted from the database's reports. */
  async restore() {
    const scan = this.#scan();
    if (scan !== null) await this.#run("restore", (handle) => api.restoreDuplicates(handle, scan));
  }

  cancel() {
    if (this.job?.id != null) void jobs.cancel(this.job.id);
  }

  /**
   * Limits the game list to the groups of the report shown: in the list's
   * own order, each kept game comes before its copies.
   */
  async show() {
    const handle = db.info?.handle;
    const scan = this.#scan();
    if (handle === undefined || scan === null) return;
    try {
      const info = await api.showDuplicates(handle, scan);
      if (db.info?.handle !== handle) return;
      db.info = info;
      db.quickText = "";
      db.filterVersion++;
      db.listVersion++;
    } catch (e) {
      notices.error(e);
      await this.load();
    }
  }

  /** The number of the report shown, if it's the open database's. */
  #scan(): number | null {
    return this.report && this.handle === db.info?.handle ? this.report.scan : null;
  }

  async #run(work: DuplicatesWork, start: (handle: number) => Promise<number>) {
    const handle = db.info?.handle;
    if (handle === undefined || this.job) return;
    this.job = { id: null, handle, work };
    try {
      const id = await start(handle);
      this.job = { id, handle, work };
      // Its outcome is a notice (jobs.svelte.ts).
      await jobs.waitFor(id);
    } catch (e) {
      // Refused: the report shown was replaced, or the database changed.
      notices.error(e);
    } finally {
      this.job = null;
    }
    if (db.info?.handle !== handle) return;
    if (work !== "find") await db.refresh();
    await this.load();
  }
}

export const duplicates = new DuplicatesStore();
