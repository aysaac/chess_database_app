// Words for the Sync dialog (NET-13), kept out of the component so `npm test`
// checks them.
import type { PendingKindDto } from "./bindings/PendingKindDto";
import type { SyncCountDto } from "./bindings/SyncCountDto";
import type { SyncStepDto } from "./bindings/SyncStepDto";

type Count = Pick<SyncCountDto, "step" | "count">;

function games(n: number, one = "game", many = "games"): string {
  return `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;
}

/** What the steps are called in the preview's summary, in the order it lists them. */
const SUMMARY: [SyncStepDto, (n: number) => string][] = [
  ["newHere", (n) => `${games(n, "new game", "new games")} here to send`],
  ["push", (n) => `${n.toLocaleString("en-US")} changed here`],
  ["pull", (n) => `${n.toLocaleString("en-US")} changed on Lichess`],
  ["merge", (n) => `${n.toLocaleString("en-US")} changed on both sides, merged`],
  ["newThere", (n) => `${games(n, "new chapter", "new chapters")} on Lichess`],
  ["adopt", (n) => `${games(n)} here linked to their chapters`],
  ["conflict", (n) => `${n.toLocaleString("en-US")} for you to decide`],
  ["removedThere", (n) => `${games(n, "chapter", "chapters")} removed on Lichess`],
  ["deleteChapter", (n) => `${games(n, "chapter", "chapters")} to remove on Lichess`],
];

/**
 * The preview in one line: "3 new games here to send, 2 changed here, 1
 * changed on Lichess"; "Nothing to do: the database and the study are in
 * step" when nothing changes.
 */
export function summary(counts: Count[]): string {
  const by = new Map(counts.map((c) => [c.step, c.count]));
  const parts = SUMMARY.filter(([s]) => (by.get(s) ?? 0) > 0).map(([s, f]) => f(by.get(s) ?? 0));
  if (parts.length === 0) return "Nothing to do: the database and the study are in step";
  return parts.join(", ");
}

/** A step, for the list of what a sync does. */
export function stepLabel(step: SyncStepDto): string {
  switch (step) {
    case "unchanged":
      return "Unchanged";
    case "pull":
      return "Comes from Lichess";
    case "push":
      return "Goes to Lichess";
    case "merge":
      return "Merged both ways";
    case "keptHere":
      return "Changed here, kept here (following only)";
    case "conflict":
      return "For you to decide";
    case "pending":
      return "Waits for you";
    case "newThere":
      return "New on Lichess: comes here";
    case "adopt":
      return "Linked to its chapter";
    case "newHere":
      return "New here: goes to the study";
    case "noRoom":
      return "Not sent: the study is full (64 chapters)";
    case "notSent":
      return "Not sent";
    case "removedThere":
      return "Removed on Lichess: marked as deleted here";
    case "deletedHere":
      return "Deleted here: its chapter stays on Lichess";
    case "deleteChapter":
      return "Deleted here: its chapter is removed on Lichess";
    case "open":
      return "Open in a tab: left until it's closed";
  }
}

/** The count of `step` among `counts`. */
export function countOf(counts: Count[], step: SyncStepDto): number {
  return counts.find((c) => c.step === step)?.count ?? 0;
}

/** Why a game waits for the user, in a sentence. */
export function pendingText(kind: PendingKindDto): string {
  switch (kind) {
    case "both":
      return "It changed here and on Lichess, in ways that don't merge on their own.";
    case "removedThere":
      return "Its chapter was removed on Lichess, and the game changed here since the last sync.";
    case "noBase":
      return "It is this chapter (its ChapterURL says so) but differs from it, and they were never synced, so nothing says which side changed.";
  }
}

/** The ribbon button's tooltip. */
export function buttonTitle(linked: { name: string | null; study: string; mode: string } | null, pending: number): string {
  if (!linked) {
    return "Link this database to a Lichess study: one of yours or a new private one, then keep them in step";
  }
  const name = linked.name ?? linked.study;
  const how = linked.mode === "twoWay" ? "both ways" : "from Lichess only";
  const waiting = pending > 0 ? `. ${games(pending)} ${pending === 1 ? "waits" : "wait"} for you` : "";
  return `Sync this database with ${name} (${how}): see what changed on each side, then confirm${waiting}`;
}
