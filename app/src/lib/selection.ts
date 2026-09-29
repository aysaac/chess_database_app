// Selected rows of the game list as sorted, non-overlapping `[start, end)`
// ranges of row indexes, so selecting a million rows stays small (DB-06,
// QOL-04). The functions return new arrays; they never change their input.

export type Ranges = [number, number][];

/** Sorts and merges touching or overlapping ranges; drops empty ones. */
export function normalize(ranges: Ranges): Ranges {
  const sorted = ranges.filter(([a, b]) => b > a).sort((x, y) => x[0] - y[0]);
  const out: Ranges = [];
  for (const [a, b] of sorted) {
    const last = out[out.length - 1];
    if (last && a <= last[1]) last[1] = Math.max(last[1], b);
    else out.push([a, b]);
  }
  return out;
}

export function contains(ranges: Ranges, i: number): boolean {
  return ranges.some(([a, b]) => i >= a && i < b);
}

export function count(ranges: Ranges): number {
  return ranges.reduce((n, [a, b]) => n + (b - a), 0);
}

/** Rows `a` to `b` inclusive, in either order. */
export function span(a: number, b: number): [number, number] {
  return [Math.min(a, b), Math.max(a, b) + 1];
}

/** Adds row `i`, or removes it if it's selected (Ctrl+click). */
export function toggle(ranges: Ranges, i: number): Ranges {
  if (!contains(ranges, i)) return normalize([...ranges, [i, i + 1]]);
  const out: Ranges = [];
  for (const [a, b] of ranges) {
    if (i < a || i >= b) out.push([a, b]);
    else {
      if (a < i) out.push([a, i]);
      if (i + 1 < b) out.push([i + 1, b]);
    }
  }
  return out;
}

/** The first `n` selected rows, in list order. */
export function firstRows(ranges: Ranges, n: number): number[] {
  const out: number[] = [];
  for (const [a, b] of ranges) {
    for (let i = a; i < b && out.length < n; i++) out.push(i);
    if (out.length >= n) break;
  }
  return out;
}

/** Only rows below `total` (after the list shrank). */
export function clamp(ranges: Ranges, total: number): Ranges {
  return normalize(ranges.map(([a, b]): [number, number] => [a, Math.min(b, total)]));
}
