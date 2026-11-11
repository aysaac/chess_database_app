// Unit tests for the Lichess study dialogs' helpers (NET-12): `npm test` in app/.
import assert from "node:assert/strict";
import { test } from "node:test";
import { AutoListing, studyNameProblem } from "./studies.ts";

test("the user's studies are listed on their own once per login", () => {
  const auto = new AutoListing();
  assert.equal(auto.take(null, true), false, "nobody logged in");
  assert.equal(auto.take("Bob", false), false, "not while the list doesn't show");
  assert.equal(auto.take("Bob", true), true);
  // The listing failed and the effect ran again (the account reloaded, the
  // list's state changed): no new request until "Try again".
  for (let i = 0; i < 100; i++) assert.equal(auto.take("Bob", true), false);
  assert.equal(auto.take("Ann", true), true, "another login");
  assert.equal(auto.take(null, true), false, "logged out");
  assert.equal(auto.take("Ann", false), false);
  assert.equal(auto.take("Ann", true), true, "logged in again");
});

test("a study's name is counted as Lichess counts it", () => {
  assert.equal(studyNameProblem(" ab "), null);
  assert.match(studyNameProblem(" a ") ?? "", /2 to 100 characters/);
  assert.equal(studyNameProblem("x".repeat(100)), null);
  assert.notEqual(studyNameProblem("x".repeat(101)), null);
  assert.equal(studyNameProblem("é".repeat(100)), null, "one unit each");
  // 60 emoji are 60 characters but 120 UTF-16 code units: Lichess refuses them.
  assert.notEqual(studyNameProblem("🙂".repeat(60)), null);
  assert.equal(studyNameProblem("🙂".repeat(50)), null);
});
