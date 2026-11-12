import assert from "node:assert/strict";
import { test } from "node:test";
import type { SyncSourceDto } from "./bindings/SyncSourceDto";
import { day, describeSource, endOfDay, hostOf, lastUpdate, nameFor } from "./sync.ts";

const player: SyncSourceDto = {
  kind: "player",
  site: "chessCom",
  username: "hikaru",
  speeds: ["blitz", "correspondence"],
  ratedOnly: true,
  color: null,
  since: null,
  until: null,
  latest: null,
  reached: null,
};

test("sources are described in a few words", () => {
  assert.equal(describeSource(player), "hikaru's Chess.com games (blitz, daily, rated)");
  assert.equal(
    describeSource({ ...player, site: "lichess", speeds: [], ratedOnly: false }),
    "hikaru's Lichess games",
  );
  assert.equal(
    describeSource({ kind: "study", study: "abc", chapter: null, name: "Openings", lastModified: null }),
    "Lichess study: Openings",
  );
  const server: SyncSourceDto = {
    kind: "server",
    url: "https://chess.example.com",
    username: "me",
    database: "d1",
    name: "My games",
    next: null,
    epoch: null,
  };
  assert.equal(describeSource(server), "My games on chess.example.com");
  assert.equal(nameFor(server), "My games");
  assert.equal(nameFor(player), "hikaru (Chess.com)");
  assert.equal(hostOf("http://192.168.1.2:8080"), "192.168.1.2:8080");
});

test("filters are part of the description", () => {
  const first = new Date(2025, 0, 31).getTime();
  const last = endOfDay("2025-12-31");
  assert.equal(day(first), "2025-01-31");
  assert.equal(day(last), "2025-12-31", "the last millisecond is still that day");
  assert.equal(
    describeSource({ ...player, speeds: [], ratedOnly: false, color: "white", since: first, until: last }),
    "hikaru's Chess.com games — as white, from 2025-01-31 to 2025-12-31",
  );
  assert.equal(
    describeSource({ ...player, speeds: [], ratedOnly: false, until: last }),
    "hikaru's Chess.com games — to 2025-12-31",
  );
  assert.equal(
    describeSource({ ...player, speeds: [], ratedOnly: false, latest: 50 }),
    "hikaru's Chess.com games — latest 50",
  );
});

test("the last update is told in words", () => {
  const now = 10_000_000_000;
  const sync = {
    sources: [],
    auto: true,
    limit: null,
    limitPolicy: "stop" as const,
    lastUpdate: now - 5 * 60000,
    lastResult: "2 new games",
    lastFailed: false,
  };
  assert.equal(lastUpdate(null, now), "Not updated yet");
  assert.equal(lastUpdate({ ...sync, lastUpdate: null }, now), "Not updated yet");
  assert.equal(lastUpdate(sync, now), "Updated 5 min ago: 2 new games");
  assert.equal(lastUpdate({ ...sync, lastUpdate: now - 3 * 3600000, lastResult: null }, now), "Updated 3 h ago");
});
