// Unit tests for the online download helpers (NET-01): `npm test` in app/.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  dbName,
  lichessTime,
  localMidnight,
  parseSpeeds,
  requestSpeeds,
  sentence,
  sinceProblem,
  siteSpeeds,
  speedLabel,
  usernameProblem,
} from "./online.ts";

test("each site has its own time controls", () => {
  assert.deepEqual(siteSpeeds("lichess"), [
    "bullet",
    "blitz",
    "rapid",
    "classical",
    "correspondence",
  ]);
  assert.deepEqual(siteSpeeds("chessCom"), ["bullet", "blitz", "rapid", "correspondence"]);
  assert.equal(speedLabel("chessCom", "correspondence"), "Daily");
  assert.equal(speedLabel("lichess", "correspondence"), "Correspondence");
  assert.equal(speedLabel("lichess", "blitz"), "Blitz");
});

test("all of a site's time controls ask for every game; none asks for nothing", () => {
  const all = siteSpeeds("lichess");
  assert.deepEqual(requestSpeeds("lichess", all), []);
  assert.deepEqual(requestSpeeds("chessCom", all), [], "classical isn't Chess.com's");
  assert.deepEqual(requestSpeeds("chessCom", ["bullet", "blitz", "rapid", "correspondence"]), []);
  assert.deepEqual(requestSpeeds("lichess", ["blitz", "rapid"]), ["blitz", "rapid"]);
  assert.equal(requestSpeeds("chessCom", ["classical"]), null);
  assert.equal(requestSpeeds("lichess", []), null);
});

test("saved time controls", () => {
  assert.deepEqual(parseSpeeds("blitz,bullet,nonsense"), ["bullet", "blitz"]);
  assert.deepEqual(parseSpeeds(""), []);
});

test("usernames", () => {
  assert.equal(usernameProblem(" DrNykterstein "), null);
  assert.equal(usernameProblem("a-b_c9"), null);
  assert.match(usernameProblem("") ?? "", /Enter/);
  assert.match(usernameProblem("two words") ?? "", /isn't a username/);
  assert.match(usernameProblem("x".repeat(51)) ?? "", /isn't a username/);
});

test("start dates", () => {
  const today = "2026-10-23";
  assert.equal(sinceProblem("2026-01-31", today), null);
  assert.equal(sinceProblem("2026-10-23", today), null);
  assert.match(sinceProblem("", today) ?? "", /Enter/);
  assert.match(sinceProblem("2026-02-30", today) ?? "", /isn't a date/);
  assert.match(sinceProblem("31/01/2026", today) ?? "", /isn't a date/);
  assert.match(sinceProblem("2026-10-24", today) ?? "", /hasn't come yet/);
});

test("the new database is named after the player and the site", () => {
  assert.equal(dbName(" DrNykterstein ", "lichess"), "DrNykterstein (Lichess)");
  assert.equal(dbName("hikaru", "chessCom"), "hikaru (Chess.com)");
  assert.equal(dbName("", "lichess"), "Online games (Lichess)");
});

test("messages read as sentences", () => {
  assert.equal(
    sentence("couldn't reach Lichess (timeout). Check the internet connection"),
    "Couldn't reach Lichess (timeout). Check the internet connection.",
  );
  assert.equal(sentence("Pick at least one time control."), "Pick at least one time control.");
  assert.equal(sentence(""), "");
});

test("how long Lichess takes", () => {
  assert.equal(lichessTime(500), "under a minute");
  assert.equal(lichessTime(1200), "about a minute");
  assert.equal(lichessTime(10_455), "about 9 minutes");
  assert.equal(lichessTime(500_000), "about 7 hours");
});

test("a first day starts at midnight where the user is", () => {
  const t = localMidnight("2026-10-01");
  const d = new Date(t);
  assert.deepEqual(
    [d.getFullYear(), d.getMonth(), d.getDate(), d.getHours(), d.getMinutes()],
    [2026, 9, 1, 0, 0],
  );
});
