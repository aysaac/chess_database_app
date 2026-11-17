// Unit tests for the Sync dialog's words (NET-13): `npm test` in app/.
import assert from "node:assert/strict";
import { test } from "node:test";
import { buttonTitle, countOf, stepLabel, summary } from "./studySync.ts";

test("the preview in one line", () => {
  assert.equal(summary([]), "Nothing to do: the database and the study are in step");
  assert.equal(summary([{ step: "unchanged", count: 5 }]), "Nothing to do: the database and the study are in step");
  assert.equal(
    summary([
      { step: "pull", count: 1 },
      { step: "unchanged", count: 4 },
      { step: "newHere", count: 3 },
      { step: "push", count: 2 },
    ]),
    "3 new games here to send, 2 changed here, 1 changed on Lichess",
  );
  assert.equal(summary([{ step: "newHere", count: 1 }]), "1 new game here to send");
  assert.equal(summary([{ step: "conflict", count: 2 }]), "2 for you to decide");
  assert.equal(countOf([{ step: "pull", count: 3 }], "pull"), 3);
  assert.equal(countOf([], "pull"), 0);
});

test("every step has words", () => {
  for (const s of [
    "unchanged",
    "pull",
    "push",
    "merge",
    "keptHere",
    "conflict",
    "pending",
    "newThere",
    "adopt",
    "newHere",
    "noRoom",
    "notSent",
    "removedThere",
    "deletedHere",
    "deleteChapter",
    "open",
  ] as const) {
    assert.ok(stepLabel(s).length > 3, s);
  }
});

test("the ribbon button says what it does", () => {
  assert.match(buttonTitle(null, 0), /^Link this database to a Lichess study/);
  const linked = { name: "Openings", study: "abcd1234", mode: "twoWay" };
  assert.equal(
    buttonTitle(linked, 0),
    "Sync this database with Openings (both ways): see what changed on each side, then confirm",
  );
  assert.match(buttonTitle({ ...linked, name: null, mode: "follow" }, 1), /abcd1234 \(from Lichess only\).*1 game waits for you$/);
});
