// Unit tests for the preference ledger (DB-07): `npm test` in app/ runs them
// with Node's own test runner, which strips the types (Node 22.6+).
import assert from "node:assert/strict";
import { test } from "node:test";
import { PrefLedger } from "./prefsLedger.ts";

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
