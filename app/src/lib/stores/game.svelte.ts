import { ask, save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { CommentSlotDto } from "../bindings/CommentSlotDto";
import type { GameViewDto } from "../bindings/GameViewDto";
import type { GotoPositionDto } from "../bindings/GotoPositionDto";
import type { HeadersDto } from "../bindings/HeadersDto";
import type { ImageFormatDto } from "../bindings/ImageFormatDto";
import type { NotationDto } from "../bindings/NotationDto";
import type { NotationMoveDto } from "../bindings/NotationMoveDto";
import type { ShapeDto } from "../bindings/ShapeDto";
import type { StepDto } from "../bindings/StepDto";
import type { VariationPolicyDto } from "../bindings/VariationPolicyDto";
import { board } from "./board.svelte";
import { db, inDocuments } from "./db.svelte";
import { notices } from "./notify.svelte";
import { prefs } from "./prefs.svelte";

export type PendingMove = { orig: string; dest: string; promotion: string | null };

/** The move after `id` in the line that contains it: null at a line's end, undefined if not found. */
function moveAfter(line: NotationMoveDto[], id: number): NotationMoveDto | null | undefined {
  for (let i = 0; i < line.length; i++) {
    if (line[i].id === id) return line[i + 1] ?? null;
    for (const variation of line[i].variations) {
      const hit = moveAfter(variation, id);
      if (hit !== undefined) return hit;
    }
  }
  return undefined;
}

/** The move with node id `id`, anywhere in `line` or its variations. */
function findMove(line: NotationMoveDto[], id: number): NotationMoveDto | undefined {
  for (const m of line) {
    if (m.id === id) return m;
    for (const variation of m.variations) {
      const hit = findMove(variation, id);
      if (hit) return hit;
    }
  }
  return undefined;
}

/** A file name for a board image (QOL-06), e.g. "Carlsen - Caruana after 23...Qe7.png". */
function boardImageName(view: GameViewDto, notation: NotationDto | null): string {
  const { white, black } = view.headers;
  const players = white && black ? `${white} - ${black}` : "Board";
  const move = view.cursorPath.length ? findMove(notation?.moves ?? [], view.cursor) : undefined;
  const after = move ? ` after ${move.moveNumber}${move.white ? "." : "..."}${move.san}` : "";
  // Characters Windows doesn't allow in file names.
  return `${players}${after}.png`.replace(/[\\/:*?"<>|]/g, "_");
}

/** Where a comment is stored: a node and a slot, with its current text. */
export type CommentRef = { node: number; slot: CommentSlotDto; text: string };

/** The root node: its comment after is the game's intro comment. */
export const ROOT_NODE = 0;

/**
 * Where the comment shown before move `id` lives. As in PGN, only the first
 * move of a variation has its own; before any other move it's the previous
 * move's comment after, or the game's intro comment before the first move.
 */
export function commentBefore(notation: NotationDto, id: number): CommentRef | undefined {
  const walk = (line: NotationMoveDto[], intro: CommentRef | null): CommentRef | undefined => {
    for (let i = 0; i < line.length; i++) {
      const m = line[i];
      if (m.id === id) {
        if (i > 0) return { node: line[i - 1].id, slot: "after", text: line[i - 1].commentAfter };
        return intro ?? { node: m.id, slot: "before", text: m.commentBefore };
      }
      for (const variation of m.variations) {
        const hit = walk(variation, null);
        if (hit) return hit;
      }
    }
    return undefined;
  };
  return walk(notation.moves, { node: ROOT_NODE, slot: "after", text: notation.rootComment });
}

/**
 * The variation of `notation` that starts with move `start`, and the node it
 * branches off from (the move before it, or the root).
 */
export function branchPoint(notation: NotationDto, start: number): number | undefined {
  const walk = (line: NotationMoveDto[], before: number): number | undefined => {
    for (let i = 0; i < line.length; i++) {
      const prev = i > 0 ? line[i - 1].id : before;
      for (const variation of line[i].variations) {
        if (variation[0].id === start) return prev;
        const hit = walk(variation, prev);
        if (hit !== undefined) return hit;
      }
    }
    return undefined;
  };
  return walk(notation.moves, ROOT_NODE);
}

/** The first move of the innermost variation containing move `id`; undefined on the main line. */
export function variationOf(notation: NotationDto, id: number): number | undefined {
  const walk = (line: NotationMoveDto[], start: number | undefined): number | null | undefined => {
    for (const m of line) {
      if (m.id === id) return start ?? null;
      for (const variation of m.variations) {
        const hit = walk(variation, variation[0].id);
        if (hit !== undefined) return hit;
      }
    }
    return undefined;
  };
  return walk(notation.moves, undefined) ?? undefined;
}

/** One open game (UI-05): its editing session and what the frontend knows of it. */
export type GameTab = {
  session: number;
  view: GameViewDto;
  notation: NotationDto | null;
  /** Row in the game list it was opened from (next/previous game, QOL-03). */
  listIndex: number | null;
  /** First moves of the variations folded in the notation (UI-03). */
  collapsed: number[];
};

/** How to show a newly loaded game. */
type OpenMode = "auto" | "tab" | "replace";

class GameStore {
  /** Open games, in tab order. The store's `view`, `notation` and `listIndex` are the active tab's. */
  tabs = $state<GameTab[]>([]);
  activeSession = $state<number | null>(null);
  orientation = $state<"white" | "black">("white");
  /** A move waiting for the variation dialog. */
  pendingChoice = $state<PendingMove | null>(null);
  /** A pawn move waiting for the promotion picker. */
  pendingPromotion = $state<{ orig: string; dest: string; color: "white" | "black" } | null>(
    null,
  );
  /** Bumped to force the board to resync with the view (after a cancelled move). */
  boardVersion = $state(0);
  /** The comment being typed inline in the notation. */
  editingComment = $state<{ node: number; slot: CommentSlotDto } | null>(null);
  /** Next moves offered after stepping forward into a split (main line first). */
  branch = $state<NotationMoveDto[] | null>(null);
  /** The highlighted entry of `branch`, previewed as an arrow on the board. */
  branchSelected = $state(0);
  /** Text of a move being typed on the board (UI-02); null when closed. */
  typing = $state<string | null>(null);
  /** `[%eval]` values by node id, from the notation. */
  evals = $derived.by(() => {
    const map = new Map<number, string>();
    const walk = (line: NotationMoveDto[]) => {
      for (const m of line) {
        if (m.eval) map.set(m.id, m.eval);
        m.variations.forEach(walk);
      }
    };
    walk(this.notation?.moves ?? []);
    return map;
  });

  get active(): GameTab | undefined {
    return this.tabs.find((t) => t.session === this.activeSession);
  }

  get view(): GameViewDto | null {
    return this.active?.view ?? null;
  }

  get notation(): NotationDto | null {
    return this.active?.notation ?? null;
  }

  get listIndex(): number | null {
    return this.active?.listIndex ?? null;
  }

  set listIndex(index: number | null) {
    const tab = this.active;
    if (tab) tab.listIndex = index;
  }

  get session(): number | null {
    return this.activeSession;
  }

  /** Stores a view in the tab it belongs to (the user may have switched tabs meanwhile). */
  #apply(view: GameViewDto) {
    const tab = this.tabs.find((t) => t.session === view.session);
    if (!tab) return;
    tab.view = view;
    if (view.notation) tab.notation = view.notation;
    // Moving into a folded variation unfolds it.
    if (tab.collapsed.some((id) => view.cursorPath.includes(id))) {
      tab.collapsed = tab.collapsed.filter((id) => !view.cursorPath.includes(id));
    }
  }

  async #run(f: (session: number) => Promise<GameViewDto>) {
    const session = this.session;
    if (session === null) return;
    try {
      this.#apply(await f(session));
    } catch (e) {
      notices.error(e);
      this.boardVersion++;
    }
  }

  #resetTransient() {
    this.editingComment = null;
    this.branch = null;
    this.pendingChoice = null;
    this.pendingPromotion = null;
    this.typing = null;
  }

  /** Asks before discarding unsaved changes in `tabs` (default: the active one). True to go ahead. */
  async confirmDiscard(tabs: GameTab[] = this.active ? [this.active] : []): Promise<boolean> {
    const dirty = tabs.filter((t) => t.view.dirty);
    if (!dirty.length) return true;
    const what = dirty.length === 1 ? "This game has" : `${dirty.length} open games have`;
    return ask(`${what} unsaved changes. Discard them?`, {
      title: "Unsaved changes",
      kind: "warning",
      okLabel: "Discard changes",
      cancelLabel: "Keep editing",
    });
  }

  /**
   * Shows a newly loaded game. `auto` reuses the active tab unless it has
   * unsaved changes (then it opens a new tab), `tab` always opens a new one,
   * `replace` reuses the active tab after asking to discard changes.
   * Resolves false if the user kept editing or loading failed.
   */
  async #show(load: () => Promise<GameViewDto>, mode: OpenMode, listIndex: number | null): Promise<boolean> {
    const current = this.active;
    const reuse = current && (mode === "replace" || (mode === "auto" && !current.view.dirty));
    if (reuse && mode === "replace" && !(await this.confirmDiscard([current]))) return false;
    try {
      const view = await load();
      const tab: GameTab = { session: view.session, view, notation: view.notation, listIndex, collapsed: [] };
      if (reuse && current) {
        const old = current.session;
        this.tabs[this.tabs.indexOf(current)] = tab;
        api.closeSession(old).catch(() => {});
      } else {
        this.tabs.push(tab);
      }
      this.activeSession = view.session;
      this.#resetTransient();
      return true;
    } catch (e) {
      notices.error(e);
      return false;
    }
  }

  /** Opens a game; `listIndex` is its row in the game list, if opened from there. */
  async open(handle: number, game: number, listIndex: number | null = null, mode: OpenMode = "auto") {
    const already = this.tabs.find((t) => t.view.source?.db === handle && t.view.source.game === game);
    if (already && mode !== "tab") {
      already.listIndex = listIndex ?? already.listIndex;
      return this.activate(already.session);
    }
    await this.#show(() => api.openGame(handle, game), mode, listIndex);
  }

  newGame(fen: string | null = null, mode: OpenMode = "auto") {
    return this.#show(() => api.newGame(fen), mode, null);
  }

  /**
   * Goes to the position of a FEN in `text` (QOL-10): to the move where the
   * game on screen reaches it, else to a new game from it (in this tab
   * unless it has unsaved changes, as opening a game does). Null if that
   * failed; the error is shown.
   */
  async gotoPosition(text: string): Promise<GotoPositionDto | null> {
    try {
      const result = await api.gotoPosition(this.session, text);
      if (result.kind === "inGame") {
        this.#resetTransient();
        this.#apply(result.view);
      } else if (result.kind === "newGame" && !(await this.newGame(result.fen))) {
        return null;
      }
      return result;
    } catch (e) {
      notices.error(e);
      return null;
    }
  }

  /** Switches to another open game; a running analysis follows it. */
  async activate(session: number) {
    if (session === this.activeSession || !this.tabs.some((t) => t.session === session)) return;
    this.activeSession = session;
    this.#resetTransient();
    try {
      this.#apply(await api.activateSession(session));
    } catch (e) {
      notices.error(e);
    }
  }

  /** Closes a tab (asking about unsaved changes); the last tab is replaced by a new game. */
  async closeTab(session: number) {
    const tab = this.tabs.find((t) => t.session === session);
    if (!tab || !(await this.confirmDiscard([tab]))) return;
    const i = this.tabs.indexOf(tab);
    if (this.tabs.length === 1) {
      await this.#show(() => api.newGame(null), "tab", null);
    }
    this.tabs = this.tabs.filter((t) => t.session !== session);
    api.closeSession(session).catch(() => {});
    if (this.activeSession === session) {
      const next = this.tabs[Math.min(i, this.tabs.length - 1)];
      this.activeSession = null;
      if (next) await this.activate(next.session);
    }
  }

  /** Reloads every tab's view (after a compaction renumbered their games). */
  async refreshAll() {
    for (const t of this.tabs) {
      try {
        this.#apply(await api.gameView(t.session));
      } catch (e) {
        notices.error(e);
      }
    }
  }

  /** Cycles through the tabs (Ctrl+Tab / Ctrl+Shift+Tab). */
  cycleTab(step: number) {
    const i = this.tabs.findIndex((t) => t.session === this.activeSession);
    const next = this.tabs[(i + step + this.tabs.length) % this.tabs.length];
    if (next) return this.activate(next.session);
  }

  /** Opens the next (`+1`) or previous (`-1`) game of the list in this tab (QOL-03). */
  async openAdjacent(delta: number) {
    const index = this.listIndex;
    if (index === null || !db.info) return;
    const row = await db.rowAt(index + delta);
    if (row) await this.open(db.info.handle, row.id, index + delta, "replace");
  }

  step(dir: StepDto) {
    return this.#run((s) => api.step(s, dir));
  }

  goto(node: number) {
    return this.#run((s) => api.gotoNode(s, node));
  }

  /** Steps forward, or offers the next moves when the game splits here. */
  forward() {
    const view = this.view;
    const moves = this.notation?.moves ?? [];
    const next = view?.cursorPath.length ? moveAfter(moves, view.cursor) : moves[0];
    if (next && next.variations.length) {
      this.branch = [next, ...next.variations.map((line) => line[0])];
      this.branchSelected = 0;
    } else {
      return this.step("forward");
    }
  }

  /** Closes the next-move picker, going to `node` unless it's null. */
  chooseBranch(node: number | null) {
    if (!this.branch) return;
    this.branch = null;
    if (node !== null) return this.goto(node);
  }

  async play(
    orig: string,
    dest: string,
    promotion: string | null = null,
    policy: VariationPolicyDto | null = null,
  ) {
    const session = this.session;
    if (session === null) return;
    try {
      const result = await api.playMove(session, orig, dest, promotion, policy);
      switch (result.kind) {
        case "moved":
          this.#apply(result.view);
          if (result.dropped > 0) {
            const moves = result.dropped === 1 ? "1 later move was" : `${result.dropped} later moves were`;
            notices.show(`Inserted the move; ${moves} no longer legal and removed. Undo brings them back.`);
          }
          break;
        case "needsChoice":
          this.pendingChoice = { orig, dest, promotion };
          break;
        case "needsPromotion":
          this.pendingPromotion = { orig, dest, color: this.view?.turn === "black" ? "black" : "white" };
          break;
        case "illegal":
          this.boardVersion++;
          break;
      }
    } catch (e) {
      notices.error(e);
      this.boardVersion++;
    }
  }

  isCollapsed(start: number): boolean {
    return this.active?.collapsed.includes(start) ?? false;
  }

  /**
   * Folds the variations starting at `starts` (UI-03). If the cursor is in
   * one, it moves to where the outermost of them branches off.
   */
  collapse(starts: number[]) {
    const tab = this.active;
    const notation = this.notation;
    if (!tab || !notation) return;
    tab.collapsed = [...new Set([...tab.collapsed, ...starts])];
    const hidden = tab.view.cursorPath.find((id) => tab.collapsed.includes(id));
    const to = hidden === undefined ? undefined : branchPoint(notation, hidden);
    if (to !== undefined) return this.goto(to);
  }

  expand(start: number) {
    const tab = this.active;
    if (tab) tab.collapsed = tab.collapsed.filter((id) => id !== start);
  }

  /** Folds every variation of the main line (their sub-variations go with them). */
  collapseAll() {
    return this.collapse((this.notation?.moves ?? []).flatMap((m) => m.variations.map((line) => line[0].id)));
  }

  expandAll() {
    const tab = this.active;
    if (tab) tab.collapsed = [];
  }

  /** Plays a move given in UCI (typed, or from the tree); castling as `e1g1`. */
  playUci(uci: string) {
    return this.play(uci.slice(0, 2), uci.slice(2, 4), uci.length > 4 ? uci[4] : null);
  }

  async choosePromotion(role: string | null) {
    const p = this.pendingPromotion;
    this.pendingPromotion = null;
    if (!p || !role) {
      this.boardVersion++;
      return;
    }
    await this.play(p.orig, p.dest, role);
  }

  async chooseVariation(policy: VariationPolicyDto | null) {
    const p = this.pendingChoice;
    this.pendingChoice = null;
    if (!p || !policy) {
      this.boardVersion++;
      return;
    }
    await this.play(p.orig, p.dest, p.promotion, policy);
  }

  /** Adds engine moves (UCI) from the cursor; see `add_engine_line`. */
  addEngineLine(moves: string[], comment: string | null, goto: boolean) {
    return this.#run((s) => api.addEngineLine(s, moves, comment, goto));
  }

  /** Reloads the view, notation included (after a background job changed the game). */
  refresh() {
    return this.#run((s) => api.gameView(s));
  }

  promoteVariation(node: number) {
    return this.#run((s) => api.promoteVariation(s, node));
  }

  deleteVariation(node: number) {
    return this.#run((s) => api.deleteVariation(s, node));
  }

  deleteRemaining(node: number) {
    return this.#run((s) => api.deleteRemaining(s, node));
  }

  setComment(node: number, slot: CommentSlotDto, text: string) {
    return this.#run((s) => api.setComment(s, node, slot, text));
  }

  /** Opens an inline editor for a move's comment in the notation. */
  editComment(node: number, slot: CommentSlotDto) {
    this.editingComment = { node, slot };
  }

  /** Closes the inline editor, saving `text` unless it's null (cancelled). */
  finishComment(text: string | null) {
    const editing = this.editingComment;
    this.editingComment = null;
    if (editing && text !== null) return this.setComment(editing.node, editing.slot, text);
  }

  toggleNag(node: number, nag: number) {
    return this.#run((s) => api.toggleNag(s, node, nag));
  }

  setShapes(shapes: ShapeDto[]) {
    return this.#run((s) => api.setShapes(s, shapes));
  }

  setHeaders(headers: HeadersDto) {
    return this.#run((s) => api.setHeaders(s, headers));
  }

  undo() {
    if (this.view?.canUndo) return this.#run((s) => api.undo(s));
  }

  redo() {
    if (this.view?.canRedo) return this.#run((s) => api.redo(s));
  }

  flip() {
    this.orientation = this.orientation === "white" ? "black" : "white";
    prefs.set("orientation", this.orientation);
  }

  /** Saves over the game it came from, or into the open database if new. */
  async save() {
    const view = this.view;
    if (!view) return;
    if (view.source && view.source.db === db.info?.handle) {
      await this.#saveAs("replace");
    } else {
      await this.saveAsNew();
    }
  }

  async saveAsNew() {
    if (!db.info) {
      notices.show("Open or create a database first, then save the game into it.", "error");
      return;
    }
    await this.#saveAs("new");
  }

  async #saveAs(mode: "replace" | "new") {
    const session = this.session;
    if (session === null) return;
    try {
      this.#apply(await api.saveGame(session, mode, db.info?.handle ?? null));
      await db.refresh();
      notices.show(mode === "replace" ? "Saved changes" : "Saved as a new game");
    } catch (e) {
      notices.error(e);
    }
  }

  async copyPgn() {
    const session = this.session;
    if (session === null) return;
    try {
      await navigator.clipboard.writeText(await api.gamePgn(session));
      notices.show("Copied the game as PGN");
    } catch (e) {
      notices.error(e);
    }
  }

  async copyFen() {
    if (!this.view) return;
    try {
      await navigator.clipboard.writeText(this.view.fen);
      notices.show("Copied the position as FEN");
    } catch (e) {
      notices.error(e);
    }
  }

  /** Copies the board as a picture (QOL-06): a PNG image, or SVG markup as text. */
  async copyBoardImage(format: ImageFormatDto) {
    const session = this.session;
    if (session === null) return;
    const look = board.imageLook(this.orientation);
    try {
      await api.copyBoardImage(session, look, format);
      notices.show(
        format === "png"
          ? `Copied the board as an image (${look.size} × ${look.size} pixels)`
          : "Copied the board as SVG",
      );
    } catch (e) {
      notices.error(e);
    }
  }

  /** The file name suggested for the board image: players and last move. */
  get boardImageName(): string {
    return this.view ? boardImageName(this.view, this.notation) : "Board.png";
  }

  /** Asks where to save the board image; the file name's extension picks PNG or SVG. */
  async saveBoardImage() {
    if (!this.view) return;
    const path = await save({
      title: "Save board image",
      filters: [
        { name: "PNG image", extensions: ["png"] },
        { name: "SVG image", extensions: ["svg"] },
      ],
      defaultPath: await inDocuments(this.boardImageName),
    });
    if (path) await this.saveBoardImageTo(path);
  }

  /** Saves the board image to `path` (a PNG unless it ends in .svg; .png is added if it has neither). */
  async saveBoardImageTo(path: string) {
    const session = this.session;
    if (session === null) return;
    const file = /\.(png|svg)$/i.test(path) ? path : `${path}.png`;
    try {
      await api.saveBoardImage(session, board.imageLook(this.orientation), file);
      notices.show(`Saved the board image as ${file.split(/[\\/]/).pop()}`);
    } catch (e) {
      notices.error(e);
    }
  }
}

export const game = new GameStore();
