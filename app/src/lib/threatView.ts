// What a window shows of the threat search (ENG-07), kept free of Svelte
// and Tauri so `npm test` can run it (threatView.test.ts).
import type { ThreatDto } from "./bindings/ThreatDto";

/**
 * Whether a `threat` event is current: its `seq` isn't below the newest one
 * seen. Events are numbered in the order the backend decided them, across
 * threat searches and windows, so a lower number is an update that was on
 * its way when the board moved: it mustn't replace the note that followed.
 */
export function isCurrentThreat(t: ThreatDto, newestSeq: number): boolean {
  return t.seq >= newestSeq;
}

/**
 * The threat to show on a board showing `view` while `on` ("Show threat" is
 * on and this window's engine runs or starts): the line or note for that
 * position, or the threat engine's failure in that game whatever position
 * the board shows (nothing more comes until "Show threat" is turned off and
 * on). Null for another game, such as another window's, for another
 * position, or before anything came.
 */
export function visibleThreat(
  t: ThreatDto | null,
  view: { session: number; fen: string } | null | undefined,
  on: boolean,
): ThreatDto | null {
  if (!t || !view || !on || t.session !== view.session) return null;
  if (t.error !== null) return t;
  return t.fen === view.fen ? t : null;
}

/**
 * Whether a `threat` event brings news of a failure. The backend tells a
 * failure again to each game the analysis follows next (a new game, another
 * tab), so the page shows it there too; it's only announced once.
 */
export function isNewFailure(kept: ThreatDto | null, next: ThreatDto): boolean {
  if (next.error === null) return false;
  return !(kept?.error === next.error && kept.startFailed === next.startFailed);
}

/** The notice for a threat engine that failed or couldn't start. */
export function threatErrorNotice(t: ThreatDto): string {
  const what = t.startFailed ? "Couldn't start the threat engine" : "The threat search stopped";
  return `${what}: ${t.error}. Turn "Show threat" off and on to retry.`;
}

/**
 * A queue that runs its tasks one at a time, in order: a task waits for the
 * ones before it, and one that fails doesn't stop the ones after it. Tasks
 * read what to send when their turn comes, so however fast "Show threat" is
 * clicked the last call sends the latest setting (a double click: on, then
 * off), and replies can't cross: a status read queued after a call sees
 * what that call did.
 */
export function taskQueue(): (task: () => Promise<void>) => Promise<void> {
  let tail: Promise<void> = Promise.resolve();
  return (task) => {
    const next = tail.then(task);
    tail = next.catch(() => {});
    return next;
  };
}
