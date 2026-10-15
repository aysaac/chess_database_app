import { api, onUiPref } from "../api";
import { PrefLedger } from "../prefsLedger";

/** A value a preference can hold: anything JSON can, as the settings file stores it. */
export type PrefValue = string | number | boolean | null | PrefValue[] | { [key: string]: PrefValue };

/**
 * Preferences that describe one window rather than the app (DB-07: several
 * editor windows can be open at once). A window saves them as usual, so
 * one opened later starts from the last saved, but a change doesn't reach
 * the other windows already open: dragging a divider or moving a panel in
 * one editor shouldn't rearrange the others, nor flipping its board turn
 * theirs. Every other preference belongs to the app (the theme, the
 * board's look, the engine lines, the library's favourites, layout and
 * preview size…) and reaches every open window as soon as it's saved.
 */
const PER_WINDOW: ReadonlySet<string> = new Set([
  // The dock's groups, tabs and sizes (UI-04, UI-19), and the older prefs
  // it replaced, which are removed when it's saved.
  "layout",
  "treeVisible",
  "listShare",
  // The Reference tab's dividers (OPEN-14).
  "referenceSizes",
  // Which side of the board is at the bottom.
  "orientation",
  // Which database the opening tree reads.
  "treeSource",
]);

const SAVE_DELAY_MS = 400;

/**
 * Frontend preferences (board orientation, pane sizes…), saved in the app's
 * settings file by the Rust side (INF-02). Each window holds a copy, which
 * `init` loads and keeps in step with the changes other windows save.
 */
class Prefs {
  #values = $state<Record<string, unknown>>({});
  #ledger = new PrefLedger();
  /** Changes waiting for their debounce, and its timers, by key. */
  #unsent = new Map<string, PrefValue>();
  #timers = new Map<string, ReturnType<typeof setTimeout>>();
  #ready: Promise<void> | null = null;

  /**
   * Loads the saved preferences, then keeps this window's copy current.
   * main.ts waits for it before mounting either shell (the library or an
   * editor), so nothing reads a default where a value is saved, or saves a
   * change made from a default over it. Later calls return the same promise.
   */
  init(): Promise<void> {
    this.#ready ??= this.#init();
    return this.#ready;
  }

  async #init() {
    try {
      // Listen before reading, so no change saved meanwhile is missed; the
      // ledger sorts out one that comes both ways.
      await onUiPref((change) => {
        if (!PER_WINDOW.has(change.key)) this.#take(change.key, change.value, change.revision);
      });
      const { values, revision } = await api.uiPrefs();
      const next = { ...this.#values };
      for (const key of this.#ledger.loaded([...Object.keys(values), ...Object.keys(next)], revision)) {
        if (key in values) next[key] = values[key];
        else delete next[key];
      }
      this.#values = next;
    } catch (e) {
      console.error(e);
    }
    // Send what's waiting for its debounce when the window loses focus (the
    // next window may show it at once) or the page goes away.
    window.addEventListener("blur", () => this.flush());
    window.addEventListener("pagehide", () => this.flush());
  }

  /** The saved value, or `fallback` if there's none or it has another type. */
  get<T extends string | number | boolean>(key: string, fallback: T): T {
    const v = this.#values[key];
    return typeof v === typeof fallback ? (v as T) : fallback;
  }

  /**
   * The saved value as it was stored, for values `get` can't check, such as
   * the panel layout (UI-19). The caller validates it: the file may hold
   * anything.
   */
  raw(key: string): unknown {
    return this.#values[key];
  }

  /** Stores a preference (null removes it); writes are debounced per key. */
  set(key: string, value: PrefValue) {
    this.#assign(key, value);
    this.#ledger.queue(key);
    this.#unsent.set(key, value);
    clearTimeout(this.#timers.get(key));
    this.#timers.set(
      key,
      setTimeout(() => this.#send(key), SAVE_DELAY_MS),
    );
  }

  /** Sends every change still waiting for its debounce. */
  flush() {
    for (const key of [...this.#unsent.keys()]) this.#send(key);
  }

  #send(key: string) {
    clearTimeout(this.#timers.get(key));
    this.#timers.delete(key);
    if (!this.#unsent.has(key)) return;
    const value = this.#unsent.get(key) ?? null;
    this.#unsent.delete(key);
    this.#ledger.sending(key);
    api
      .setUiPref(key, value)
      .then((revision) => this.#take(key, value, revision))
      .catch(console.error);
  }

  /** `key` was saved as `value` at `revision`, by this window or another. */
  #take(key: string, value: unknown, revision: number) {
    if (!this.#ledger.saved(key, revision)) return;
    // The reply to this window's own save mostly brings what it has already.
    if (JSON.stringify(this.#values[key] ?? null) !== JSON.stringify(value ?? null)) this.#assign(key, value);
  }

  #assign(key: string, value: unknown) {
    if (value === null || value === undefined) delete this.#values[key];
    else this.#values[key] = value;
  }
}

export const prefs = new Prefs();
