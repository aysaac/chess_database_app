// Display helpers for chess notation.
import type { EvalDto } from "./bindings/EvalDto";

const FIGURINES: Record<string, string> = { K: "♔", Q: "♕", R: "♖", B: "♗", N: "♘" };

/** SAN with figurines for piece letters (files are lower-case, castling uses O). */
export function figurine(san: string): string {
  return san.replace(/[KQRBN]/g, (c) => FIGURINES[c]);
}

const NAG_SYMBOLS: Record<number, string> = {
  1: "!",
  2: "?",
  3: "!!",
  4: "??",
  5: "!?",
  6: "?!",
  7: "□",
  10: "=",
  13: "∞",
  14: "⩲",
  15: "⩱",
  16: "±",
  17: "∓",
  18: "+−",
  19: "−+",
  22: "⨀",
  23: "⨀",
  32: "⟳",
  33: "⟳",
  36: "↑",
  37: "↑",
  40: "→",
  41: "→",
  132: "⇆",
  133: "⇆",
  138: "⊕",
  139: "⊕",
  140: "∆",
  146: "N",
};

export function nagSymbol(nag: number): string {
  return NAG_SYMBOLS[nag] ?? `$${nag}`;
}

/** Move-quality NAGs are glued to the move; the rest follow with a space. */
export function isMoveNag(nag: number): boolean {
  return nag >= 1 && nag <= 7;
}

export const MOVE_NAGS = [
  { nag: 1, label: "Good move" },
  { nag: 2, label: "Mistake" },
  { nag: 3, label: "Brilliant move" },
  { nag: 4, label: "Blunder" },
  { nag: 5, label: "Interesting move" },
  { nag: 6, label: "Dubious move" },
];

export const POSITION_NAGS = [
  { nag: 10, label: "Equal" },
  { nag: 13, label: "Unclear" },
  { nag: 14, label: "White is slightly better" },
  { nag: 15, label: "Black is slightly better" },
  { nag: 16, label: "White is better" },
  { nag: 17, label: "Black is better" },
  { nag: 18, label: "White is winning" },
  { nag: 19, label: "Black is winning" },
];

export function playerLabel(name: string, elo: number | null): string {
  const who = name || "Unknown player";
  return elo ? `${who} (${elo})` : who;
}

export function resultLabel(result: string): string {
  return result === "1/2-1/2" ? "½–½" : result === "*" ? "" : result.replace("-", "–");
}

export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}

export function formatBytes(n: number): string {
  if (n < 1e6) return `${Math.max(1, Math.round(n / 1e3))} kB`;
  if (n < 1e9) return `${(n / 1e6).toFixed(1)} MB`;
  return `${(n / 1e9).toFixed(2)} GB`;
}

/** An evaluation as chess software shows it: `+0.35`, `−1.20`, `#3`, `#−2`. */
export function formatEval(e: EvalDto): string {
  if (e.mate !== null) return `#${e.mate < 0 ? "−" : ""}${Math.abs(e.mate)}`;
  const cp = e.cp ?? 0;
  return `${cp < 0 ? "−" : "+"}${(Math.abs(cp) / 100).toFixed(2)}`;
}

/** Parses a PGN `[%eval]` value (`0.36`, `-1.2`, `#-3`, `0.36,20`). */
export function parseEval(s: string | null | undefined): EvalDto | null {
  if (!s) return null;
  const v = s.split(",")[0].trim();
  if (v.startsWith("#")) {
    const n = Number.parseInt(v.slice(1), 10);
    return Number.isFinite(n) ? { cp: null, mate: n } : null;
  }
  const f = Number.parseFloat(v);
  return Number.isFinite(f) ? { cp: Math.round(f * 100), mate: null } : null;
}

/** Nodes per second, e.g. `1.4 Mn/s`. */
export function formatNps(nps: number): string {
  if (nps >= 1e6) return `${(nps / 1e6).toFixed(1)} Mn/s`;
  if (nps >= 1e3) return `${Math.round(nps / 1e3)} kn/s`;
  return `${nps} n/s`;
}
