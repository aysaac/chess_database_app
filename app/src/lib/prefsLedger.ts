// Which saved preference changes a window takes into its own copy (DB-07).
// Every window keeps a copy of the preferences (stores/prefs.svelte.ts). The
// Rust side numbers each change it saves (its revision) and sends it to
// every window, the one that made it included. A change can reach a window
// by two roads, the event and the reply to its own save, and in any order,
// so the window keeps, per key, the newest revision it has taken.
// Kept free of Svelte and Tauri so it runs under `node --test`
// (prefsLedger.test.ts).

export class PrefLedger {
  /** The newest revision taken, per key; a key not here is at `#base`. */
  #revisions = new Map<string, number>();
  /** The revision of the last full read of the preferences. */
  #base = 0;
  /** Keys this window changed and hasn't sent yet (debounced). */
  #unsent = new Set<string>();

  /** This window changed `key`; the change is sent later. */
  queue(key: string) {
    this.#unsent.add(key);
  }

  /** This window's change to `key` is being sent. */
  sending(key: string) {
    this.#unsent.delete(key);
  }

  /**
   * `key` was saved at `revision`, by this window or another: whether this
   * window's copy takes the saved value. Not when it has a newer revision
   * already, nor while it has a change of its own still to send, which will
   * be saved after this one and replace it everywhere.
   */
  saved(key: string, revision: number): boolean {
    if (revision <= this.#revision(key)) return false;
    this.#revisions.set(key, revision);
    return !this.#unsent.has(key);
  }

  /**
   * Every preference was read at `revision`: which of `keys` (the ones read
   * and the ones in the copy) take their value from that read, or lose it if
   * the read hasn't one. Not those with an unsent change, nor those that
   * took a newer change while the read was on its way.
   */
  loaded(keys: Iterable<string>, revision: number): string[] {
    const take = [...new Set(keys)].filter((k) => !this.#unsent.has(k) && this.#revision(k) <= revision);
    if (revision > this.#base) {
      this.#base = revision;
      for (const [key, r] of this.#revisions) if (r <= revision) this.#revisions.delete(key);
    }
    return take;
  }

  #revision(key: string): number {
    return this.#revisions.get(key) ?? this.#base;
  }
}
