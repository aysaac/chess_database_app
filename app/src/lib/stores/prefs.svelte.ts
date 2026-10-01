import { api } from "../api";

/** A value a preference can hold: anything JSON can, as the settings file stores it. */
export type PrefValue = string | number | boolean | null | PrefValue[] | { [key: string]: PrefValue };

/**
 * Frontend preferences (board orientation, pane sizes, analysis lines…),
 * saved in the app's settings file by the Rust side (INF-02).
 */
class Prefs {
  #values = $state<Record<string, unknown>>({});
  #timers = new Map<string, ReturnType<typeof setTimeout>>();

  async load() {
    try {
      this.#values = await api.uiPrefs();
    } catch (e) {
      console.error(e);
    }
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
    this.#values[key] = value;
    clearTimeout(this.#timers.get(key));
    this.#timers.set(
      key,
      setTimeout(() => api.setUiPref(key, value).catch(console.error), 400),
    );
  }
}

export const prefs = new Prefs();
