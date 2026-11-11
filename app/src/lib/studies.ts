// Helpers for the Lichess study dialogs (NET-12), kept out of the
// components so `npm test` checks them.

/**
 * The longest study name Lichess takes. It counts a name as Java does, in
 * UTF-16 code units (JavaScript's `length`, and what an input's `maxlength`
 * counts), so most emoji count twice. Rust's `check_study_name` counts the
 * same way.
 */
export const MAX_STUDY_NAME = 100;

/** What's wrong with a new study's name (trimmed, as it's sent), or null. */
export function studyNameProblem(name: string): string | null {
  const n = name.trim().length;
  return n < 2 || n > MAX_STUDY_NAME
    ? "A study's name has 2 to 100 characters (most emoji count as 2)."
    : null;
}

/**
 * When the "One of your studies" list asks Lichess on its own: once per
 * login while the list shows. A listing that failed (offline, Lichess down
 * or limiting the app) waits for "Try again", so the effect that asks,
 * which runs again whenever what it read changes, never sends a stream of
 * requests.
 */
export class AutoListing {
  #asked: string | null = null;

  /** Whether to list `user`'s studies now (`shown`: the list shows), noting that it did. */
  take(user: string | null, shown: boolean): boolean {
    if (!user) {
      this.#asked = null;
      return false;
    }
    if (!shown || this.#asked === user) return false;
    this.#asked = user;
    return true;
  }
}
