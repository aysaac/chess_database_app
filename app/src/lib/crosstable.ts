// How a tournament cross-table (DB-16) lays out and labels what Rust
// computed: results by opponent (the grid) or by round, their symbols and
// tooltips. Kept free of Svelte and Tauri so it runs under `node --test`
// (crosstable.test.ts).
import type { CrossResultDto } from "./bindings/CrossResultDto";
import type { CrossRowDto } from "./bindings/CrossRowDto";

/** Points as cross-tables show them: 6½, ½, 0. */
export function formatPoints(points: number): string {
  const whole = Math.floor(points);
  if (points - whole < 0.5) return String(whole);
  return whole ? `${whole}½` : "½";
}

/** A tie-break value: 23, 23.5, 23.25. */
export function formatTieBreak(value: number): string {
  return String(Math.round(value * 100) / 100);
}

/** A rank, or the places a tie shares: 3, 3–5. */
export function rankLabel(row: Pick<CrossRowDto, "rank" | "rankTo">): string {
  return row.rank === row.rankTo ? String(row.rank) : `${row.rank}–${row.rankTo}`;
}

/** A result's symbol: 1 ½ 0, + and − for forfeits, * while unfinished. */
export function resultSymbol(r: Pick<CrossResultDto, "outcome" | "forfeit">): string {
  switch (r.outcome) {
    case "win":
      return r.forfeit ? "+" : "1";
    case "loss":
      return r.forfeit ? "−" : "0";
    case "draw":
      return "½";
    default:
      return "*";
  }
}

const OUTCOME_WORDS: Record<CrossResultDto["outcome"], [string, string]> = {
  win: ["won", "won by forfeit"],
  loss: ["lost", "lost by forfeit"],
  draw: ["drew", "drew"],
  unfinished: ["unfinished", "unfinished"],
};

/** A result's tooltip: "Round 3 · White against Carlsen, M (2830) · won. Opens the game." */
export function resultTitle(r: CrossResultDto, rows: CrossRowDto[], byRound: boolean): string {
  const parts: string[] = [];
  if (byRound && r.round) parts.push(`Round ${r.round}`);
  const opponent = r.opponent === null ? undefined : rows[r.opponent];
  if (opponent) {
    const elo = opponent.elo ? ` (${opponent.elo})` : "";
    parts.push(`${r.white ? "White" : "Black"} against ${opponent.name}${elo}`);
  } else {
    parts.push("Bye");
  }
  parts.push(OUTCOME_WORDS[r.outcome][r.forfeit ? 1 : 0]);
  return `${parts.join(" · ")}. Opens the game.`;
}

/**
 * Most cells a table with results may have: `MAX_CELLS` in crates/chess-db's
 * crosstable.rs, which sends the standings only past it. The dialog checks
 * again, so a table that is too big to lay out never reaches the page.
 */
export const MAX_CELLS = 20_000;
/** Columns besides the results, as crosstable.rs counts them (`STANDINGS_COLUMNS`). */
const STANDINGS_COLUMNS = 10;

/**
 * Cells a table takes with its results, as crosstable.rs counts them: a row
 * per player times the columns of the wider layout (the rounds' `columns`,
 * or one per player in the grid) and the standings, plus the results.
 */
export function resultCells(rows: Pick<CrossRowDto, "results">[], columns: number, grid: boolean): number {
  const results = rows.reduce((sum, row) => sum + row.results.length, 0);
  return rows.length * (Math.max(columns, grid ? rows.length : 0) + STANDINGS_COLUMNS) + results;
}

/** The grid: `cells[i][j]` holds row i's games against row j, in the order played. */
export function gridCells(rows: CrossRowDto[]): CrossResultDto[][][] {
  return rows.map((row) => {
    const cells: CrossResultDto[][] = rows.map(() => []);
    for (const r of row.results) {
      if (r.opponent !== null) cells[r.opponent]?.push(r);
    }
    return cells;
  });
}

/** The rounds view's columns: the slots with games, then 0 (no round) when some games have none. */
export function slotColumns(slots: number[], unslotted: boolean): number[] {
  return unslotted ? [...slots, 0] : [...slots];
}

/** A row's games by column of `columns` (see `slotColumns`). */
export function slotCells(row: CrossRowDto, columns: number[]): CrossResultDto[][] {
  const at = new Map(columns.map((slot, i) => [slot, i]));
  const cells: CrossResultDto[][] = columns.map(() => []);
  for (const r of row.results) {
    const i = at.get(r.slot);
    if (i !== undefined) cells[i].push(r);
  }
  return cells;
}

/** A rounds view column's heading: R5 for a round, 5 for a player's 5th game, ? for no round. */
export function slotHeading(slot: number, byRound: boolean): string {
  if (slot === 0) return "?";
  return byRound ? `R${slot}` : String(slot);
}
