// Matching typed moves against the legal moves (UI-02).
import type { LegalMoveDto } from "./bindings/LegalMoveDto";

/**
 * A move as typed, reduced to what identifies it: no capture, check,
 * promotion or annotation marks, no hyphens, castling with letters O.
 */
function normalize(s: string): string {
  const t = s.trim().replace(/[x+#=!?:\s-]/g, "");
  return /^[oO0]{2,3}$/.test(t) ? t.replace(/[o0]/g, "O") : t;
}

/** SAN without the disambiguation of piece moves: `Nbd2` → `Nd2`. */
function bare(san: string): string {
  return san.replace(/^([KQRBN])[a-h]?[1-8]?(?=[a-h][1-8])/, "$1");
}

export type MoveMatch = {
  /** The move meant, when the text names exactly one. */
  move: LegalMoveDto | null;
  /** Moves the text could still become (or is ambiguous between). */
  candidates: LegalMoveDto[];
};

/**
 * Finds the legal move(s) meant by `text`: SAN (`Nf3`, `exd5`, `e8=Q`,
 * `O-O`, `0-0`) with the marks optional, or UCI (`g1f3`, `e7e8q`). Piece
 * letters may be typed in lower case unless that's ambiguous (`bc4` is the
 * pawn capture if there is one).
 */
export function matchMove(text: string, moves: LegalMoveDto[]): MoveMatch {
  const q = normalize(text);
  if (!q) return { move: null, candidates: [] };
  const lower = q.toLowerCase();
  const keyed = moves.map((m) => ({ m, san: normalize(m.san), uci: m.uci }));

  // As typed first, then in any case. `Nd2` when both knights can go there
  // names no move; both are offered as candidates.
  const one = (hits: typeof keyed) => (hits.length === 1 ? hits[0].m : null);
  const move =
    one(keyed.filter((k) => k.san === q || k.uci === lower)) ??
    one(keyed.filter((k) => k.san.toLowerCase() === lower));

  let candidates = keyed.filter(
    (k) => k.san.startsWith(q) || bare(k.san).startsWith(q) || k.uci.startsWith(lower),
  );
  if (!candidates.length) {
    candidates = keyed.filter(
      (k) => k.san.toLowerCase().startsWith(lower) || bare(k.san).toLowerCase().startsWith(lower),
    );
  }
  return { move, candidates: candidates.map((k) => k.m).sort((a, b) => a.san.localeCompare(b.san)) };
}
