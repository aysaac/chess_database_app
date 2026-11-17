import { api } from "../api";
import type { StudySyncStatusDto } from "../bindings/StudySyncStatusDto";

/**
 * The Sync button (NET-13): the open database's links to Lichess studies,
 * whether its dialog is open, and the games waiting for the user. Rust
 * keeps the links in the database (by permanent game id); this only shows
 * them.
 */
class StudySyncStore {
  /** The open database's links, once loaded. */
  status = $state<StudySyncStatusDto | null>(null);
  /** The Sync dialog is open. */
  dialog = $state(false);
  #handle: number | null = null;
  #seq = 0;

  /** The study the database is linked to (the one with the most games), or null. */
  get linked() {
    return this.status?.studies[0] ?? null;
  }

  /** How many games wait for the user, over every linked study. */
  get pending(): number {
    return this.status?.studies.reduce((n, s) => n + s.pending.length, 0) ?? 0;
  }

  /** Loads the links of database `handle` (none: no database open). */
  async load(handle: number | null) {
    const seq = ++this.#seq;
    this.#handle = handle;
    if (handle === null) {
      this.status = null;
      return;
    }
    try {
      const status = await api.studySyncStatus(handle);
      if (seq === this.#seq) this.status = status;
    } catch (e) {
      // A database closed meanwhile, or one that can't be read: no links to show.
      console.error("couldn't read the study links", e);
      if (seq === this.#seq) this.status = null;
    }
  }

  /** Loads them again, after a sync or a change to the database. */
  async reload() {
    await this.load(this.#handle);
  }
}

export const studySync = new StudySyncStore();
