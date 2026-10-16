// Unit tests for the preference ledger and sync (DB-07): `npm test` in app/
// runs them with Node's own test runner, which strips the types without a
// flag from Node 22.18 on (package.json's "engines").
import assert from "node:assert/strict";
import { test } from "node:test";
import { PrefLedger, PrefSync, type PrefBackend, type PrefValue } from "./prefsLedger.ts";

test("takes newer changes and drops older ones, whichever arrives first", () => {
  const l = new PrefLedger();
  assert.deepEqual(l.loaded(["appTheme"], 4), ["appTheme"]);
  assert.equal(l.saved("appTheme", 4), false, "already in the read");
  assert.equal(l.saved("appTheme", 6), true);
  assert.equal(l.saved("appTheme", 5), false, "arrived after a newer one");
  assert.equal(l.saved("boardTheme", 5), true, "revisions are per key");
});

test("a change of its own still to send wins over others' changes", () => {
  const l = new PrefLedger();
  l.queue("libraryFavorites");
  // Another window saved the key: this one's change goes later and replaces it.
  assert.equal(l.saved("libraryFavorites", 3), false);
  l.sending("libraryFavorites");
  // The reply to its own save, at a newer revision: the copy holds that value.
  assert.equal(l.saved("libraryFavorites", 4), true);
  // The echo of that save comes after the reply.
  assert.equal(l.saved("libraryFavorites", 4), false);
});

test("a change saved after its own, while its own was on the way, stands", () => {
  const l = new PrefLedger();
  l.queue("layout");
  l.sending("layout");
  // Another window's save landed after this one's, and its event came first.
  assert.equal(l.saved("layout", 8), true);
  // The reply to this window's own (older) save mustn't bring its value back.
  assert.equal(l.saved("layout", 7), false);
});

test("its own reply brings its value back over an older change taken meanwhile", () => {
  const l = new PrefLedger();
  l.queue("appTheme");
  l.sending("appTheme");
  // Another window's save came first on the server; its event arrives first too.
  assert.equal(l.saved("appTheme", 2), true);
  // This window's save is the newer one, so the copy goes back to its value.
  assert.equal(l.saved("appTheme", 3), true);
});

test("a change typed again while the first is on the way waits for the second", () => {
  const l = new PrefLedger();
  l.queue("analysisLines");
  l.sending("analysisLines");
  l.queue("analysisLines");
  assert.equal(l.saved("analysisLines", 1), false, "the first save's echo");
  assert.equal(l.saved("analysisLines", 1), false, "and its reply");
  l.sending("analysisLines");
  assert.equal(l.saved("analysisLines", 2), true);
});

test("a read keeps what changed after it and what's still to send", () => {
  const l = new PrefLedger();
  // A change saved after the read was made, whose event beat the read here.
  assert.equal(l.saved("appTheme", 10), true);
  l.queue("orientation");
  const take = l.loaded(["appTheme", "orientation", "boardTheme", "gone"], 9);
  assert.deepEqual(take, ["boardTheme", "gone"]);
  // Keys the read covered are at its revision now.
  assert.equal(l.saved("boardTheme", 9), false);
  assert.equal(l.saved("boardTheme", 11), true);
  assert.equal(l.saved("appTheme", 10), false, "still at the newer change");
  // A later read covers the newer change too.
  assert.deepEqual(l.loaded(["appTheme"], 12), ["appTheme"]);
});

// ---- PrefSync, against a fake backend --------------------------------------

/** A promise and the functions that settle it. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

/**
 * The Rust side in miniature: saves are held until the test answers them,
 * reads answer at once unless `loads` says otherwise (one entry per read:
 * "fail", "hang" or "ok").
 */
function fakeBackend(saved: Record<string, unknown> = {}, loads: ("ok" | "fail" | "hang")[] = []) {
  let revision = 0;
  const saves: { key: string; value: PrefValue; answer: () => void }[] = [];
  let take: (key: string, value: unknown, revision: number) => void = () => {};
  const backend: PrefBackend = {
    load: () => {
      const how = loads.shift() ?? "ok";
      if (how === "fail") return Promise.reject(new Error("no settings"));
      if (how === "hang") return new Promise(() => {});
      return Promise.resolve({ values: { ...saved }, revision });
    },
    save: (key, value) => {
      const reply = deferred<number>();
      saves.push({
        key,
        value,
        // Saved now; the event reaches the window first, then the reply.
        answer: () => {
          revision += 1;
          if (value === null) delete saved[key];
          else saved[key] = value;
          take(key, value, revision);
          reply.resolve(revision);
        },
      });
      return reply.promise;
    },
    listen: (t) => {
      take = t;
      return Promise.resolve();
    },
  };
  return { backend, saved, saves };
}

const fast = { saveDelayMs: 5, waitMs: 50, retryMs: 5, retryMaxMs: 20 };

test("one save per key at a time: a change made while one is on its way waits for it", async () => {
  const { backend, saved, saves } = fakeBackend();
  const values: Record<string, unknown> = {};
  const sync = new PrefSync(values, backend, fast);
  await sync.init();
  sync.set("analysisLines", 2);
  const first = sync.flush();
  sync.set("analysisLines", 4);
  const second = sync.flush();
  assert.equal(saves.length, 1, "the second change waits for the first save");
  saves[0].answer();
  await tick();
  // The first save's event and reply have come back; the copy keeps the newer value.
  assert.equal(values.analysisLines, 4);
  assert.equal(saves.length, 2, "the newer change goes once the first save returns");
  assert.equal(saves[1].value, 4);
  saves[1].answer();
  await Promise.all([first, second]);
  assert.equal(values.analysisLines, 4);
  assert.equal(saved.analysisLines, 4);
});

test("flush resolves once the saves on their way have returned", async () => {
  const { backend, saves } = fakeBackend();
  const sync = new PrefSync({}, backend, fast);
  await sync.init();
  sync.set("appTheme", "dark");
  let done = false;
  const flushed = sync.flush().then(() => (done = true));
  await tick();
  assert.equal(done, false);
  saves[0].answer();
  await flushed;
  assert.equal(done, true);
});

test("a read that fails is tried again, and a starred change is applied to the list it reads", async (t) => {
  t.mock.method(console, "error", () => {});
  const { backend, saved, saves } = fakeBackend({ libraryFavorites: ["a"], appTheme: "dark" }, ["fail"]);
  const values: Record<string, unknown> = {};
  const sync = new PrefSync(values, backend, fast);
  const star = (path: string) => (s: unknown) => [...(Array.isArray(s) ? s : []), path];
  // Starred before any read succeeded: shown, not saved over the list.
  sync.update("libraryFavorites", star("b"));
  assert.deepEqual(values.libraryFavorites, ["b"]);
  await sync.init();
  for (let i = 0; i < 50 && !sync.loaded; i++) await tick(5);
  assert.equal(sync.loaded, true, "a later read succeeded");
  assert.equal(values.appTheme, "dark");
  assert.deepEqual(values.libraryFavorites, ["a", "b"], "the star made on the saved list");
  await tick(fast.saveDelayMs * 2);
  assert.equal(saves.length, 1);
  assert.deepEqual(saves[0].value, ["a", "b"]);
  saves[0].answer();
  await sync.flush();
  assert.deepEqual(saved.libraryFavorites, ["a", "b"]);
  // Once loaded, a change goes as usual.
  sync.update("libraryFavorites", star("c"));
  assert.deepEqual(values.libraryFavorites, ["a", "b", "c"]);
});

test("a read that doesn't answer doesn't hold the window up", async () => {
  const { backend, saves } = fakeBackend({ boardTheme: "walnut" }, ["hang"]);
  const values: Record<string, unknown> = {};
  const sync = new PrefSync(values, backend, fast);
  const started = Date.now();
  await sync.init();
  assert.ok(Date.now() - started < fast.waitMs * 4, "init gave up waiting");
  // A plain change doesn't depend on the copy, so it's saved before the read.
  sync.set("appTheme", "light");
  const flushed = sync.flush();
  assert.equal(saves.length, 1, "sent before any read succeeded");
  saves[0].answer();
  await flushed;
  for (let i = 0; i < 50 && !sync.loaded; i++) await tick(5);
  assert.equal(sync.loaded, true, "the read was tried again");
  assert.equal(values.boardTheme, "walnut");
});

test("a listener that fails doesn't stop the read", async (t) => {
  t.mock.method(console, "error", () => {});
  const { backend } = fakeBackend({ appTheme: "dark" });
  backend.listen = () => Promise.reject(new Error("no events"));
  const values: Record<string, unknown> = {};
  const sync = new PrefSync(values, backend, fast);
  await sync.init();
  assert.equal(sync.loaded, true);
  assert.equal(values.appTheme, "dark");
});
