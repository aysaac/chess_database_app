import { api } from "../api";

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

  /** Stores a preference; writes are debounced per key. */
  set(key: string, value: string | number | boolean | null) {
    this.#values[key] = value;
    clearTimeout(this.#timers.get(key));
    this.#timers.set(
      key,
      setTimeout(() => api.setUiPref(key, value).catch(console.error), 400),
    );
  }
}

export const prefs = new Prefs();
