// IO-08: pasting a game or a position. The ribbon's "Paste game" and "Paste
// position" read the clipboard through Rust (a page only gets it in a paste
// event); Ctrl+V outside a text box (FenPaste.svelte) gets the text from its
// paste event.
import { api } from "./api";
import { formatCount } from "./format";
import { goToPosition } from "./gotoPosition";
import { game } from "./stores/game.svelte";
import { notices } from "./stores/notify.svelte";

export const NO_GAME = "The clipboard has no PGN game. Copy a game with its moves, or use Paste position for a FEN.";
export const NOTHING_TO_PASTE =
  "The clipboard has no game or position. Copy a PGN game, a FEN or a Lichess link and try again.";
export const NO_FEN = "There's no FEN on the clipboard. Copy a position's FEN or a Lichess link and try again.";

/** A PGN tag pair at the start of a line, such as `[Event "…"]`. */
const PGN_TAG = /^\s*\[\s*[A-Za-z0-9_]+\s+"/m;

/** The clipboard's text ("" when it holds none), or null when it can't be read (the error is shown). */
async function clipboardText(): Promise<string | null> {
  try {
    return (await api.clipboardText()) ?? "";
  } catch (e) {
    notices.error(e);
    return null;
  }
}

/** What opening a pasted game did: opened it (or showed why not), or found none in the text. */
type GameOutcome = { kind: "done" } | { kind: "none"; reason: string | null };

async function openGame(text: string): Promise<GameOutcome> {
  if (!text.trim()) return { kind: "none", reason: null };
  const result = await game.pasteGame(text);
  if (!result) return { kind: "done" };
  if (result.kind === "noGame") return { kind: "none", reason: result.reason };
  let message = result.games > 1 ? `Pasted the first of ${formatCount(result.games)} games.` : "Pasted the game.";
  const [first, ...more] = result.warnings;
  if (first) message += ` ${first.message}`;
  if (more.length) message += more.length === 1 ? " 1 more line stops early." : ` ${more.length} more lines stop early.`;
  notices.show(message);
  return { kind: "done" };
}

function cantOpen(reason: string) {
  notices.show(`Can't open the pasted game. ${reason}`, "error");
}

/** "Paste game": opens the PGN game on the clipboard as a new game. */
export async function pasteGame() {
  const text = await clipboardText();
  if (text === null) return;
  const outcome = await openGame(text);
  if (outcome.kind === "none") {
    if (outcome.reason) cantOpen(outcome.reason);
    else notices.show(NO_GAME);
  }
}

/** "Paste position": does what "Go to position…" does with the clipboard's text (QOL-10). */
export async function pastePosition() {
  const text = await clipboardText();
  if (text === null) return;
  const problem = text.trim() ? await goToPosition(text) : { kind: "noFen" as const };
  if (problem?.kind === "noFen") notices.show(NO_FEN);
  else if (problem?.kind === "invalid") notices.show(`Can't go to the pasted position: ${problem.error}.`, "error");
}

/**
 * Ctrl+V outside a text box: a position (QOL-10), else a PGN game. Text
 * with PGN tags is tried as a game first, so a game from a set-up position
 * opens with its moves rather than as its `[FEN]` tag's position.
 */
export async function pasteText(text: string) {
  let reason: string | null = null;
  const tryGame = async () => {
    const outcome = await openGame(text);
    if (outcome.kind === "none") reason = outcome.reason;
    return outcome.kind === "done";
  };
  const tagged = PGN_TAG.test(text);
  if (tagged && (await tryGame())) return;
  const problem = await goToPosition(text);
  if (!problem) return;
  if (!tagged && (await tryGame())) return;
  if (reason) cantOpen(reason);
  else if (problem.kind === "invalid") notices.show(`Can't go to the pasted position: ${problem.error}.`, "error");
  else notices.show(NOTHING_TO_PASTE);
}
