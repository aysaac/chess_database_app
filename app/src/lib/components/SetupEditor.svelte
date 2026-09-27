<script lang="ts">
  import { onMount } from "svelte";
  import { Chessground } from "@lichess-org/chessground";
  import type { Api } from "@lichess-org/chessground/api";
  import { dragNewPiece } from "@lichess-org/chessground/drag";
  import type { Color, Key, Piece, Role } from "@lichess-org/chessground/types";
  import { api } from "../api";
  import { game } from "../stores/game.svelte";
  import Modal from "./Modal.svelte";

  // Position setup editor (UI-07): place pieces, choose the side to move,
  // castling rights, en passant and move number, or paste a FEN.
  let { onClose }: { onClose: () => void } = $props();

  const START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR";
  const EMPTY = "8/8/8/8/8/8/8/8";
  const ROLES: Role[] = ["king", "queen", "rook", "bishop", "knight", "pawn"];

  type Tool = "move" | "erase" | `${Color}-${Role}`;

  // Start from the position on the board.
  const parts = (game.view?.fen ?? `${START} w KQkq - 0 1`).split(" ");
  let board = $state(parts[0]);
  let turn = $state<"w" | "b">(parts[1] === "b" ? "b" : "w");
  let rights = $state({
    K: parts[2].includes("K"),
    Q: parts[2].includes("Q"),
    k: parts[2].includes("k"),
    q: parts[2].includes("q"),
  });
  let ep = $state(parts[3] && parts[3] !== "-" ? parts[3] : "");
  let fullmove = $state(Number(parts[5]) || 1);
  let tool = $state<Tool>("move");
  let orientation = $state<Color>(game.orientation);
  let error = $state<string | null>(null);
  let fenText = $state("");
  let fenFocused = $state(false);

  let el: HTMLDivElement;
  let cg: Api | undefined;

  /** Square → FEN letter (upper case for White). */
  function squares(placement: string): Map<string, string> {
    const map = new Map<string, string>();
    placement.split("/").forEach((row, r) => {
      let file = 0;
      for (const c of row) {
        if (/\d/.test(c)) file += Number(c);
        else map.set(`${"abcdefgh"[file++]}${8 - r}`, c);
      }
    });
    return map;
  }

  const map = $derived(squares(board));
  const can = $derived({
    K: map.get("e1") === "K" && map.get("h1") === "R",
    Q: map.get("e1") === "K" && map.get("a1") === "R",
    k: map.get("e8") === "k" && map.get("h8") === "r",
    q: map.get("e8") === "k" && map.get("a8") === "r",
  });
  // En passant targets: a pawn of the side that just moved, on its fourth
  // rank, with the two squares behind it empty.
  const epOptions = $derived.by(() => {
    const out: string[] = [];
    for (const f of "abcdefgh") {
      const [pawn, at, target, from] = turn === "w" ? ["p", 5, 6, 7] : ["P", 4, 3, 2];
      if (map.get(`${f}${at}`) === pawn && !map.has(`${f}${target}`) && !map.has(`${f}${from}`)) {
        out.push(`${f}${target}`);
      }
    }
    return out;
  });
  const castling = $derived(
    (["K", "Q", "k", "q"] as const).filter((c) => rights[c] && can[c]).join("") || "-",
  );
  const fen = $derived(
    `${board} ${turn} ${castling} ${epOptions.includes(ep) ? ep : "-"} 0 ${Math.max(1, Math.floor(fullmove) || 1)}`,
  );

  // Check the position as it changes; the FEN box follows unless typing.
  $effect(() => {
    const current = fen;
    if (!fenFocused) fenText = current;
    const t = setTimeout(async () => {
      try {
        const r = await api.checkFen(current);
        if (current === fen) error = r.error ? r.error.replace(/^invalid FEN: /, "") : null;
      } catch (e) {
        error = String(e);
      }
    }, 120);
    return () => clearTimeout(t);
  });

  function setBoard(placement: string) {
    board = placement;
    cg?.set({ fen: placement });
  }

  onMount(() => {
    cg = Chessground(el, {
      fen: board,
      orientation,
      coordinates: true,
      animation: { enabled: false },
      highlight: { lastMove: false, check: false },
      premovable: { enabled: false },
      drawable: { enabled: false },
      movable: { free: true, color: "both", showDests: false },
      draggable: { deleteOnDropOff: true },
      events: {
        change: () => (board = cg!.getFen()),
        select: (key) => onSquare(key),
      },
    });
    return () => cg?.destroy();
  });

  function onSquare(key: Key) {
    if (!cg || tool === "move") return;
    if (tool === "erase") {
      cg.setPieces(new Map([[key, undefined]]));
    } else {
      const [color, role] = tool.split("-") as [Color, Role];
      const here = cg.state.pieces.get(key);
      // Clicking the same piece again removes it.
      const piece = here?.color === color && here.role === role ? undefined : { color, role };
      cg.setPieces(new Map([[key, piece]]));
    }
    cg.selectSquare(null);
    board = cg.getFen();
  }

  function choose(t: Tool) {
    tool = t;
    // Pieces on the board are dragged in move mode only.
    cg?.set({ movable: { color: t === "move" ? "both" : undefined } });
  }

  function paletteDown(e: PointerEvent, piece: Piece) {
    choose(`${piece.color}-${piece.role}`);
    if (cg && e.button === 0) dragNewPiece(cg.state, piece, e, true);
  }

  function flip() {
    orientation = orientation === "white" ? "black" : "white";
    cg?.set({ orientation });
  }

  async function applyFen() {
    fenFocused = false;
    const text = fenText.trim();
    if (!text || text === fen) return;
    try {
      const r = await api.checkFen(text);
      if (r.error) {
        error = r.error.replace(/^invalid FEN: /, "");
        return;
      }
      const p = r.fen.split(" ");
      setBoard(p[0]);
      turn = p[1] === "b" ? "b" : "w";
      rights = { K: p[2].includes("K"), Q: p[2].includes("Q"), k: p[2].includes("k"), q: p[2].includes("q") };
      ep = p[3] === "-" ? "" : p[3];
      fullmove = Number(p[5]) || 1;
    } catch (e) {
      error = String(e);
    }
  }

  function start() {
    if (error) return;
    onClose();
    game.newGame(fen);
  }
</script>

<Modal title="Set up a position" {onClose} width="860px">
  <div class="editor">
    <div class="board-side">
      <div class="palette" role="toolbar" aria-label="Black pieces">
        {#each ROLES as role (role)}
          <button
            class="cg-wrap"
            class:active={tool === `black-${role}`}
            aria-label="Place black {role}"
            title="Black {role}: click squares, or drag onto the board"
            onpointerdown={(e) => paletteDown(e, { color: "black", role })}
            ><piece class="black {role}"></piece></button
          >
        {/each}
      </div>
      <div class="board chessboard" bind:this={el}></div>
      <div class="palette" role="toolbar" aria-label="White pieces">
        {#each ROLES as role (role)}
          <button
            class="cg-wrap"
            class:active={tool === `white-${role}`}
            aria-label="Place white {role}"
            title="White {role}: click squares, or drag onto the board"
            onpointerdown={(e) => paletteDown(e, { color: "white", role })}
            ><piece class="white {role}"></piece></button
          >
        {/each}
      </div>
      <div class="tools" role="group" aria-label="Tools">
        <button class:active={tool === "move"} onclick={() => choose("move")}>Move pieces</button>
        <button class:active={tool === "erase"} onclick={() => choose("erase")}>Remove pieces</button>
        <button onclick={flip}>Flip</button>
      </div>
    </div>

    <div class="fields">
      <div class="row">
        <button onclick={() => setBoard(START)}>Start position</button>
        <button onclick={() => setBoard(EMPTY)}>Clear board</button>
      </div>

      <fieldset>
        <legend>Side to move</legend>
        <label><input type="radio" bind:group={turn} value="w" /> White</label>
        <label><input type="radio" bind:group={turn} value="b" /> Black</label>
      </fieldset>

      <fieldset>
        <legend>Castling rights</legend>
        <label class:off={!can.K}><input type="checkbox" bind:checked={rights.K} disabled={!can.K} /> White O-O</label>
        <label class:off={!can.Q}><input type="checkbox" bind:checked={rights.Q} disabled={!can.Q} /> White O-O-O</label>
        <label class:off={!can.k}><input type="checkbox" bind:checked={rights.k} disabled={!can.k} /> Black O-O</label>
        <label class:off={!can.q}><input type="checkbox" bind:checked={rights.q} disabled={!can.q} /> Black O-O-O</label>
      </fieldset>

      <label class="stack">
        En passant square
        <select bind:value={ep} disabled={!epOptions.length}>
          <option value="">None</option>
          {#each epOptions as sq (sq)}<option value={sq}>{sq}</option>{/each}
        </select>
      </label>

      <label class="stack">
        Move number
        <input type="number" min="1" bind:value={fullmove} />
      </label>

      <label class="stack">
        FEN
        <input
          class="fen"
          spellcheck="false"
          bind:value={fenText}
          onfocus={() => (fenFocused = true)}
          onblur={applyFen}
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              applyFen();
            }
          }}
        />
      </label>

      <p class="status" class:error={!!error} role="status">
        {#if error}
          This position can't be played: {error}.
        {:else}
          {map.size} pieces, {turn === "w" ? "White" : "Black"} to move.
        {/if}
      </p>
    </div>
  </div>
  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
    <button class="primary" disabled={!!error} onclick={start}>Start a game from here</button>
  {/snippet}
</Modal>

<style>
  .editor {
    display: grid;
    grid-template-columns: 400px 1fr;
    gap: 22px;
  }

  .board-side {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .board {
    width: 400px;
    height: 400px;
  }

  .palette {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 4px;
  }

  .palette button {
    position: relative;
    height: 48px;
    padding: 4px;
    border-color: var(--rule);
    touch-action: none;
  }

  .palette button.active,
  .tools button.active {
    background: var(--felt-soft);
    border-color: var(--felt);
  }

  .palette piece {
    position: static;
    display: block;
    width: 100%;
    height: 100%;
    transform: none;
  }

  .tools,
  .row {
    display: flex;
    gap: 6px;
  }

  .tools button,
  .row button {
    border-color: var(--rule);
  }

  .fields {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }

  fieldset {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 4px 12px;
    margin: 0;
    padding: 8px 10px;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
  }

  legend {
    padding: 0 4px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  label {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  label.off {
    opacity: 0.5;
  }

  label.stack {
    flex-direction: column;
    align-items: stretch;
    gap: 3px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  label.stack input,
  label.stack select {
    color: var(--ink);
    font-size: 14px;
  }

  .fen {
    font-family: var(--font-ui);
  }

  .status {
    margin: 0;
    color: var(--ink-soft);
  }

  .status.error {
    color: var(--danger);
  }
</style>
