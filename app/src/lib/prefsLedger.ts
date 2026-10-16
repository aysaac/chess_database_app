// How a window keeps its copy of the preferences (DB-07). Every window
// keeps a copy (stores/prefs.svelte.ts). The Rust side numbers each change
// it saves (its revision) and sends it to every window, the one that made
// it included. A change can reach a window by two roads, the event and the
// reply to its own save, and in any order, so the window keeps, per key, the
// newest revision it has taken (PrefLedger). PrefSync loads the copy, saves
// this window's changes and takes the others'.
// Kept free of Svelte and Tauri so it runs under `node --test`
// (prefsLedger.test.ts).

/** A value a preference can hold: anything JSON can, as the settings file stores it. */
export type PrefValue = string | number | boolean | null | PrefValue[] | { [key: string]: PrefValue };

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

/** Where a window's preferences are saved: the Tauri commands and the `ui-pref` event, or fakes in tests. */
export interface PrefBackend {
  /** Every saved preference, as of `revision`. */
  load(): Promise<{ values: Record<string, unknown>; revision: number }>;
  /** Saves one preference (null removes it); resolves to the revision it's saved at. */
  save(key: string, value: PrefValue): Promise<number>;
  /** Calls `take` with every change a window saves that this window shares. */
  listen(take: (key: string, value: unknown, revision: number) => void): Promise<unknown>;
}

export interface PrefTimings {
  /** How long a change waits for the next one before it's saved (per key). */
  saveDelayMs: number;
  /** The most `init` waits, and the most a read may take before it's tried again. */
  waitMs: number;
  /** The first pause before reading again after a read failed or didn't answer; it doubles each time, up to `retryMaxMs`. */
  retryMs: number;
  retryMaxMs: number;
}

const TIMINGS: PrefTimings = { saveDelayMs: 400, waitMs: 2000, retryMs: 1000, retryMaxMs: 30_000 };

/** A change made from the value saved before it (see `PrefSync.update`). */
export type PrefEdit = (saved: unknown) => PrefValue;

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** `promise`'s value, or undefined if it hasn't settled within `ms`. */
async function within<T>(promise: Promise<T>, ms: number): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([promise, new Promise<undefined>((r) => (timer = setTimeout(r, ms)))]);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * One window's copy of the preferences, in `values` (a Svelte `$state`
 * object in the app, so reads are reactive), kept in step with the saved
 * ones.
 */
export class PrefSync {
  #values: Record<string, unknown>;
  #backend: PrefBackend;
  #t: PrefTimings;
  #ledger = new PrefLedger();
  /** Changes waiting for their debounce or for the key's save on its way, and the debounce timers, by key. */
  #unsent = new Map<string, PrefValue>();
  #timers = new Map<string, ReturnType<typeof setTimeout>>();
  /** The save of each key on its way, until it returns and any change made meanwhile is sent too. */
  #inFlight = new Map<string, Promise<void>>();
  /**
   * Changes made with `update` before a read succeeded, and the saved value
   * they apply to (the newest this window has seen), by key.
   */
  #held = new Map<string, { saved: unknown; edits: PrefEdit[] }>();
  #loaded = false;
  #ready: Promise<void> | null = null;

  constructor(values: Record<string, unknown>, backend: PrefBackend, timings: Partial<PrefTimings> = {}) {
    this.#values = values;
    this.#backend = backend;
    this.#t = { ...TIMINGS, ...timings };
  }

  /** Whether a read of the saved preferences has succeeded. */
  get loaded(): boolean {
    return this.#loaded;
  }

  /**
   * Loads the saved preferences, then keeps this window's copy current.
   * main.ts waits for it before mounting either shell (the library or an
   * editor), so nothing reads a default where a value is saved. It never
   * fails, and returns within `waitMs` whatever happens, so a window always
   * shows: until a read succeeds, reads are tried again in the background,
   * and the copy fills in when one does. Later calls return the same promise.
   */
  init(): Promise<void> {
    this.#ready ??= within(this.#start(), this.#t.waitMs).then(() => undefined);
    return this.#ready;
  }

  async #start() {
    // Listen before reading, so no change saved meanwhile is missed; the
    // ledger sorts out one that comes both ways. A listener that fails or
    // doesn't answer doesn't hold the read up: the window then only misses
    // other windows' changes.
    const listening = this.#backend
      .listen((key, value, revision) => this.#take(key, value, revision))
      .catch((e) => console.error("couldn't follow the preferences other windows save:", e));
    await within(listening, this.#t.waitMs);
    if (!(await within(this.#read(), this.#t.waitMs))) await this.#retry();
  }

  /** Reads again, waiting longer each time, until a read succeeds. */
  async #retry() {
    for (let pause = this.#t.retryMs; !this.#loaded; pause = Math.min(pause * 2, this.#t.retryMaxMs)) {
      await sleep(pause);
      // A read that didn't answer in time may have succeeded since.
      if (this.#loaded || (await within(this.#read(), this.#t.waitMs))) return;
    }
  }

  /** One read of every saved preference into the copy; whether it succeeded. */
  async #read(): Promise<boolean> {
    let read;
    try {
      read = await this.#backend.load();
    } catch (e) {
      console.error("couldn't read the saved preferences:", e);
      return false;
    }
    const { values, revision } = read;
    for (const key of this.#ledger.loaded([...Object.keys(values), ...Object.keys(this.#values)], revision)) {
      this.#show(key, key in values ? values[key] : null);
    }
    if (!this.#loaded) {
      this.#loaded = true;
      // The held changes now apply to the saved values, and are saved.
      for (const [key, held] of [...this.#held]) {
        this.#held.delete(key);
        this.set(key, replay(held.saved, held.edits));
      }
    }
    return true;
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
    // A whole new value: changes held for the key no longer matter.
    this.#held.delete(key);
    this.#assign(key, value);
    this.#ledger.queue(key);
    this.#unsent.set(key, value);
    clearTimeout(this.#timers.get(key));
    this.#timers.set(
      key,
      setTimeout(() => void this.#send(key), this.#t.saveDelayMs),
    );
  }

  /**
   * Stores a preference made from the saved one (`edit` gets the saved
   * value, which the caller validates), such as a list with one more
   * database starred. Until a read of the preferences has succeeded, the
   * copy may lack the saved value (it holds a default), and saving `edit`'s
   * result would replace the user's list with one made from nothing. So the
   * change shows in this window at once but is held, and only saved, made
   * from the saved value, once a read succeeds. Holding it, rather than
   * dropping it, keeps what the user did; it's lost only if no read ever
   * succeeds, and then nothing was saved over their preferences either.
   */
  update(key: string, edit: PrefEdit) {
    if (this.#loaded) {
      this.set(key, edit(this.#values[key]));
      return;
    }
    const held = this.#held.get(key) ?? { saved: this.#values[key], edits: [] };
    held.edits.push(edit);
    this.#held.set(key, held);
    this.#assign(key, replay(held.saved, held.edits));
  }

  /**
   * Sends every change still waiting for its debounce; resolves once they,
   * and the saves already on their way, have returned (saved or failed).
   * Changes held by `update` wait for a read.
   */
  async flush(): Promise<void> {
    const keys = new Set([...this.#unsent.keys(), ...this.#inFlight.keys()]);
    await Promise.all([...keys].map((key) => this.#send(key)));
  }

  /**
   * Sends the latest change to `key`. One save per key at a time: a change
   * made while one is on its way waits for it, then goes. Two saves of a key
   * on their way at once could be saved in either order (Tauri doesn't
   * order concurrent commands), and the reply to the first would show the
   * older value in this window while the second was still on its way.
   */
  #send(key: string): Promise<void> {
    clearTimeout(this.#timers.get(key));
    this.#timers.delete(key);
    const flying = this.#inFlight.get(key);
    if (flying) return flying;
    if (!this.#unsent.has(key)) return Promise.resolve();
    const value = this.#unsent.get(key) ?? null;
    this.#unsent.delete(key);
    this.#ledger.sending(key);
    const done = this.#backend
      .save(key, value)
      .then(
        (revision) => this.#take(key, value, revision),
        (e) => console.error(`couldn't save the preference ${key}:`, e),
      )
      .then(() => {
        this.#inFlight.delete(key);
        // A change made meanwhile goes now, unless it's still in its debounce.
        if (this.#unsent.has(key) && !this.#timers.has(key)) return this.#send(key);
      });
    this.#inFlight.set(key, done);
    return done;
  }

  /** `key` was saved as `value` at `revision`, by this window or another. */
  #take(key: string, value: unknown, revision: number) {
    if (this.#ledger.saved(key, revision)) this.#show(key, value);
  }

  /** Shows a saved value in the copy, with this window's held changes on top. */
  #show(key: string, value: unknown) {
    const held = this.#held.get(key);
    if (held) {
      held.saved = value;
      value = replay(value, held.edits);
    }
    // The reply to this window's own save mostly brings what it has already.
    if (JSON.stringify(this.#values[key] ?? null) !== JSON.stringify(value ?? null)) this.#assign(key, value);
  }

  #assign(key: string, value: unknown) {
    if (value === null || value === undefined) delete this.#values[key];
    else this.#values[key] = value;
  }
}

function replay(saved: unknown, edits: PrefEdit[]): PrefValue {
  return edits.reduce<unknown>((v, edit) => edit(v), saved) as PrefValue;
}
