// Databases kept up to date (NET-01, IO-10, NET-03): how the dialog and the
// ribbon word a source. Pure, so `npm test` covers it.
import type { DbSyncDto } from "./bindings/DbSyncDto";
import type { SyncSourceDto } from "./bindings/SyncSourceDto";

// Self-contained (like the other tested modules): the same names online.ts uses.
const SITE_NAMES = { lichess: "Lichess", chessCom: "Chess.com" } as const;

function speedLabel(site: keyof typeof SITE_NAMES, speed: string): string {
  if (speed === "correspondence") return site === "chessCom" ? "Daily" : "Correspondence";
  return speed[0].toUpperCase() + speed.slice(1);
}

/** A day (ms since the epoch) as `YYYY-MM-DD` where the user is. */
export function day(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** The end of a day (`YYYY-MM-DD`) where the user is, in ms: its last millisecond. */
export function endOfDay(date: string): number {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y, m - 1, d + 1).getTime() - 1;
}

/** A server's address without its scheme, as people say it. */
export function hostOf(url: string): string {
  return url.replace(/^[a-z]+:\/\//i, "");
}

/** What a source is, in a few words. */
export function describeSource(source: SyncSourceDto): string {
  switch (source.kind) {
    case "player": {
      const speeds = source.speeds.length
        ? ` (${source.speeds.map((s) => speedLabel(source.site, s).toLowerCase()).join(", ")}${source.ratedOnly ? ", rated" : ""})`
        : source.ratedOnly
          ? " (rated)"
          : "";
      const extra = [
        source.color ? `as ${source.color}` : "",
        source.since !== null || source.until !== null
          ? `${source.since !== null ? "from " + day(source.since) : ""}${source.since !== null && source.until !== null ? " " : ""}${source.until !== null ? "to " + day(source.until) : ""}`
          : "",
      ].filter(Boolean);
      return `${source.username}'s ${SITE_NAMES[source.site]} games${speeds}${extra.length ? ` — ${extra.join(", ")}` : ""}`;
    }
    case "study":
      return `Lichess study: ${source.name}${source.chapter ? " (one chapter)" : ""}`;
    case "server":
      return `${source.name} on ${hostOf(source.url)}`;
  }
}

/** A name for a database made from `source`. */
export function nameFor(source: SyncSourceDto): string {
  switch (source.kind) {
    case "player":
      return `${source.username} (${SITE_NAMES[source.site]})`;
    case "study":
      return source.name;
    case "server":
      return source.name;
  }
}

/** When the last update ended, for the ribbon's tooltip; "never" if it hasn't run. */
export function lastUpdate(sync: DbSyncDto | null, now: number): string {
  if (!sync || sync.lastUpdate === null) return "Not updated yet";
  const minutes = Math.round((now - sync.lastUpdate) / 60000);
  const when =
    minutes < 1
      ? "just now"
      : minutes < 60
        ? `${minutes} min ago`
        : minutes < 60 * 48
          ? `${Math.round(minutes / 60)} h ago`
          : `${Math.round(minutes / 1440)} days ago`;
  return `Updated ${when}${sync.lastResult ? `: ${sync.lastResult}` : ""}`;
}
