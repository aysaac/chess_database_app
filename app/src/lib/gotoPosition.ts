// QOL-10 "go to position": a pasted FEN puts its position on the board and
// in the opening tree. Used by Ctrl+V anywhere (FenPaste.svelte) and the
// "Go to position" dialog (FenDialog.svelte).
import type { NotationMoveDto } from "./bindings/NotationMoveDto";
import { game } from "./stores/game.svelte";
import { notices } from "./stores/notify.svelte";
import { tree } from "./stores/tree.svelte";

/** Why a text couldn't be gone to: it has no FEN, or its position isn't legal (`error` says why). */
export type GotoProblem = { kind: "noFen" } | { kind: "invalid"; error: string };

function moveName(line: NotationMoveDto[], id: number): string | null {
  for (const m of line) {
    if (m.id === id) return `${m.moveNumber}${m.white ? "." : "…"}${m.san}`;
    for (const variation of m.variations) {
      const hit = moveName(variation, id);
      if (hit) return hit;
    }
  }
  return null;
}

/**
 * Goes to the position of the FEN in `text`: the move where the game on
 * screen reaches it, or a new game from it. The opening tree opens on it.
 * Resolves to null once the board shows it (or an error was reported), else
 * to the problem, for the caller to word.
 */
export async function goToPosition(text: string): Promise<GotoProblem | null> {
  const result = await game.gotoPosition(text);
  if (!result) return null;
  switch (result.kind) {
    case "noFen":
      return { kind: "noFen" };
    case "invalid":
      return { kind: "invalid", error: result.error };
    case "inGame": {
      const name = moveName(game.notation?.moves ?? [], result.view.cursor);
      notices.show(
        name
          ? `This game reaches that position at ${name}; went there`
          : "This game starts from that position; went to its start",
      );
      break;
    }
    case "newGame":
      notices.show("Opened the position as a new game");
      break;
  }
  tree.visible = true;
  return null;
}
