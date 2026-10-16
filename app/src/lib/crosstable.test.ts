// Unit tests for the cross-table layout helpers (DB-16): `npm test` in app/.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { CrossResultDto } from "./bindings/CrossResultDto";
import type { CrossRowDto } from "./bindings/CrossRowDto";
import {
  formatPoints,
  formatTieBreak,
  gridCells,
  rankLabel,
  resultSymbol,
  resultTitle,
  slotCells,
  slotColumns,
  slotHeading,
} from "./crosstable.ts";

function result(over: Partial<CrossResultDto>): CrossResultDto {
  return { game: 0, slot: 1, round: "1", opponent: 1, white: true, outcome: "win", forfeit: false, ...over };
}

function row(name: string, elo: number | null, results: CrossResultDto[]): CrossRowDto {
  return {
    name,
    elo,
    rank: 1,
    rankTo: 1,
    points: 0,
    games: results.length,
    wins: 0,
    draws: 0,
    losses: 0,
    percent: null,
    avgOpponent: null,
    performance: null,
    ratedGames: 0,
    tieBreaks: [],
    results,
  };
}

test("points, tie-breaks and shared ranks", () => {
  assert.equal(formatPoints(6.5), "6½");
  assert.equal(formatPoints(0.5), "½");
  assert.equal(formatPoints(0), "0");
  assert.equal(formatPoints(7), "7");
  assert.equal(formatTieBreak(23), "23");
  assert.equal(formatTieBreak(23.25), "23.25");
  assert.equal(formatTieBreak(23.5), "23.5");
  assert.equal(rankLabel({ rank: 3, rankTo: 3 }), "3");
  assert.equal(rankLabel({ rank: 3, rankTo: 5 }), "3–5");
});

test("result symbols", () => {
  assert.equal(resultSymbol({ outcome: "win", forfeit: false }), "1");
  assert.equal(resultSymbol({ outcome: "win", forfeit: true }), "+");
  assert.equal(resultSymbol({ outcome: "loss", forfeit: true }), "−");
  assert.equal(resultSymbol({ outcome: "loss", forfeit: false }), "0");
  assert.equal(resultSymbol({ outcome: "draw", forfeit: false }), "½");
  assert.equal(resultSymbol({ outcome: "unfinished", forfeit: false }), "*");
});

test("the grid puts each game under its opponent, both games of a double round robin together", () => {
  const rows = [
    row("A", 2700, [
      result({ game: 1, opponent: 1 }),
      result({ game: 2, opponent: 2, white: false }),
      result({ game: 3, opponent: 1, white: false, outcome: "draw" }),
    ]),
    row("B", null, [result({ game: 1, opponent: 0, white: false, outcome: "loss" })]),
    row("C", null, [result({ game: 2, opponent: 0, outcome: "loss" }), result({ game: 9, opponent: null })]),
  ];
  const cells = gridCells(rows);
  assert.deepEqual(
    cells[0].map((c) => c.map((r) => r.game)),
    [[], [1, 3], [2]],
  );
  assert.deepEqual(
    cells[2].map((c) => c.map((r) => r.game)),
    [[2], [], []],
    "a bye has no column",
  );
});

test("the rounds view: a column per round, games without one last", () => {
  const columns = slotColumns([1, 2, 4], true);
  assert.deepEqual(columns, [1, 2, 4, 0]);
  assert.deepEqual(slotColumns([1, 2], false), [1, 2]);
  const r = row("A", null, [
    result({ game: 1, slot: 1 }),
    result({ game: 2, slot: 4 }),
    result({ game: 3, slot: 4 }),
    result({ game: 4, slot: 0 }),
  ]);
  assert.deepEqual(
    slotCells(r, columns).map((c) => c.map((x) => x.game)),
    [[1], [], [2, 3], [4]],
  );
  assert.equal(slotHeading(4, true), "R4");
  assert.equal(slotHeading(4, false), "4");
  assert.equal(slotHeading(0, true), "?");
});

test("tooltips name the round, the colour, the opponent and the result", () => {
  const rows = [row("A", 2700, []), row("Carlsen, M", 2830, [])];
  assert.equal(
    resultTitle(result({ round: "3.1" }), rows, true),
    "Round 3.1 · White against Carlsen, M (2830) · won. Opens the game.",
  );
  assert.equal(
    resultTitle(result({ white: false, outcome: "loss", forfeit: true, opponent: 0 }), rows, false),
    "Black against A (2700) · lost by forfeit. Opens the game.",
  );
  assert.equal(resultTitle(result({ opponent: null, outcome: "draw" }), rows, true), "Round 1 · Bye · drew. Opens the game.");
});
