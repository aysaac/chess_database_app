// Downloading a player's games from Lichess or Chess.com (NET-01): what
// the dialog offers per site, and its checks before anything is asked of
// a server. Pure, so `npm test` covers it.
import type { OnlineSiteDto } from "./bindings/OnlineSiteDto";
import type { SpeedDto } from "./bindings/SpeedDto";

export const SITES: OnlineSiteDto[] = ["lichess", "chessCom"];

export const SITE_NAMES: Record<OnlineSiteDto, string> = {
  lichess: "Lichess",
  chessCom: "Chess.com",
};

const ALL_SPEEDS: SpeedDto[] = ["bullet", "blitz", "rapid", "classical", "correspondence"];

/** The time controls a site has: Chess.com's rapid takes in classical games. */
export function siteSpeeds(site: OnlineSiteDto): SpeedDto[] {
  return site === "chessCom" ? ALL_SPEEDS.filter((s) => s !== "classical") : ALL_SPEEDS;
}

export function speedLabel(site: OnlineSiteDto, speed: SpeedDto): string {
  if (speed === "correspondence") return site === "chessCom" ? "Daily" : "Correspondence";
  return speed[0].toUpperCase() + speed.slice(1);
}

/**
 * The time controls to ask the site for: `[]` when all of its own are
 * chosen (every game), null when none of them is.
 */
export function requestSpeeds(site: OnlineSiteDto, chosen: SpeedDto[]): SpeedDto[] | null {
  const own = siteSpeeds(site).filter((s) => chosen.includes(s));
  if (!own.length) return null;
  return own.length === siteSpeeds(site).length ? [] : own;
}

/** `bullet,blitz` (as saved in the preferences) → the known time controls in it. */
export function parseSpeeds(saved: string): SpeedDto[] {
  return ALL_SPEEDS.filter((s) => saved.split(",").includes(s));
}

/** What's wrong with a username (the same rule as Rust's `check_username`), or null. */
export function usernameProblem(name: string): string | null {
  const n = name.trim();
  if (!n) return "Enter the player's username.";
  if (n.length > 50 || !/^[A-Za-z0-9_-]+$/.test(n)) {
    return `"${n}" isn't a username: they're letters, digits, - and _.`;
  }
  return null;
}

/** What's wrong with a start date (`YYYY-MM-DD`), or null. `today` is `YYYY-MM-DD` too. */
export function sinceProblem(date: string, today: string): string | null {
  const d = date.trim();
  if (!d) return "Enter the first day to download games from.";
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(d);
  const valid =
    m &&
    (() => {
      const [y, mo, day] = [Number(m[1]), Number(m[2]), Number(m[3])];
      const t = new Date(Date.UTC(y, mo - 1, day));
      return t.getUTCFullYear() === y && t.getUTCMonth() === mo - 1 && t.getUTCDate() === day;
    })();
  if (!valid) return `"${d}" isn't a date. Write it as YYYY-MM-DD, like 2025-01-31.`;
  if (d > today) return "That day hasn't come yet: pick today or a day before.";
  return null;
}

/** The start of a day (`YYYY-MM-DD`, checked by `sinceProblem`) where the user is, in ms. */
export function localMidnight(date: string): number {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y, m - 1, d).getTime();
}

/** The new database's name: `DrNykterstein (Lichess)`. */
export function dbName(username: string, site: OnlineSiteDto): string {
  return `${username.trim() || "Online games"} (${SITE_NAMES[site]})`;
}

/** A message as a sentence: capitalized, with a full stop (Rust's errors have neither). */
export function sentence(text: string): string {
  const t = text.trim();
  if (!t) return t;
  const s = t[0].toUpperCase() + t.slice(1);
  return /[.!?…]$/.test(s) ? s : `${s}.`;
}

/** How long Lichess takes to send `games` games, at the 20 a second it sends without a login. */
export function lichessTime(games: number): string {
  const minutes = Math.round(games / 20 / 60);
  if (minutes < 1) return "under a minute";
  if (minutes < 90) return minutes === 1 ? "about a minute" : `about ${minutes} minutes`;
  const hours = Math.round(minutes / 60);
  return `about ${hours} hours`;
}
