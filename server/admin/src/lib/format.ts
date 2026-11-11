// Formatting and parsing for the tables. Pure: `npm test` runs format.test.ts.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** 1536 → "1.5 KB" (powers of 1024). */
export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** "2 GB", "500 MB", "1.5gb" → bytes; "" → null (no quota); anything else → undefined. */
export function parseBytes(text: string): number | null | undefined {
  const t = text.trim().toLowerCase();
  if (t === "" || t === "none" || t === "-") return null;
  const m = /^(\d+(?:\.\d+)?)\s*(b|kb|mb|gb|tb)?$/.exec(t);
  if (!m) return undefined;
  const unit = UNITS.findIndex((u) => u.toLowerCase() === (m[2] ?? "mb"));
  return Math.round(Number(m[1]) * 1024 ** unit);
}

/** Unix seconds → a local date and time. */
export function when(seconds: number | null | undefined): string {
  if (!seconds) return "—";
  return new Date(seconds * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function count(n: number): string {
  return n.toLocaleString("en-US");
}

/** A job's progress, 0…1, or null when its total is unknown. */
export function fraction(done: number, total: number): number | null {
  return total > 0 ? Math.min(1, done / total) : null;
}

export function visibilityLabel(v: string): string {
  return v === "server" ? "Server-wide" : v === "shared" ? "Shared" : "Private";
}
