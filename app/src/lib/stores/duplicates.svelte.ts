import { api } from "../api";
import type { DuplicatesDto } from "../bindings/DuplicatesDto";
import type { JobDto } from "../bindings/JobDto";
import { formatCount } from "../format";
import { db } from "./db.svelte";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";

function copies(n: number): string {
  return n === 1 ? "1 copy" : `${formatCount(n)} copies`;
}

/**
 * Duplicate games (DB-08) in the open database: the search runs as a job in
 * Rust, which keeps what it found per database; this holds its report for
 * the dialog and runs the actions on it.
 */
class DuplicatesStore {
  /** What the last search in the open database found, if one ran. */
  report = $state<DuplicatesDto | null>(null);
  /** The database `report` is for. */
  handle = $state<number | null>(null);
  /** The search running in this window, and the database it searches. */
  job = $state<{ id: number; handle: number } | null>(null);
  /** A delete or restore on its way. */
  busy = $state(false);

  /** The running search's progress, once it reported any. */
  get progress(): JobDto | undefined {
    const id = this.job?.id;
    return id === undefined ? undefined : jobs.running.find((j) => j.id === id);
  }

  /** Whether a search of the open database is running. */
  get searching(): boolean {
    return !!this.job && this.job.handle === db.info?.handle;
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
    const handle = db.info?.handle;
    if (handle === undefined || this.job) return;
    try {
      const id = await api.findDuplicates(handle, fuzzy);
      this.job = { id, handle };
      await jobs.waitFor(id);
    } catch (e) {
      notices.error(e);
    } finally {
      this.job = null;
    }
    if (db.info?.handle === handle) await this.load();
  }

  cancel() {
    if (this.job) void jobs.cancel(this.job.id);
  }

  /** Marks the copies found as deleted (restorable until compacting). */
  async remove() {
    await this.#act(async (handle) => {
      const n = await api.deleteDuplicates(handle);
      notices.show(
        n ? `Marked ${copies(n)} as deleted` : "The copies were marked as deleted already",
      );
    });
  }

  /** Restores the copies `remove` marked as deleted. */
  async restore() {
    await this.#act(async (handle) => {
      const n = await api.restoreDuplicates(handle);
      notices.show(`Restored ${copies(n)}`);
    });
  }

  /** Limits the game list to the groups found, each kept game first. */
  async show() {
    const handle = db.info?.handle;
    if (handle === undefined) return;
    try {
      const info = await api.showDuplicates(handle);
      if (db.info?.handle !== handle) return;
      db.info = info;
      db.quickText = "";
      db.filterVersion++;
      db.listVersion++;
    } catch (e) {
      notices.error(e);
    }
  }

  async #act(f: (handle: number) => Promise<void>) {
    const handle = db.info?.handle;
    if (handle === undefined || this.busy) return;
    this.busy = true;
    try {
      await f(handle);
      await db.refresh();
    } catch (e) {
      notices.error(e);
    } finally {
      this.busy = false;
    }
    await this.load();
  }
}

export const duplicates = new DuplicatesStore();
