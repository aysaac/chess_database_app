import { api, errorMessage } from "../api";
import type { LinePositionDto } from "../bindings/LinePositionDto";
import type { LinesDto } from "../bindings/LinesDto";
import type { ReferenceDto } from "../bindings/ReferenceDto";
import type { ReferenceLineDto } from "../bindings/ReferenceLineDto";
import type { TopGameDto } from "../bindings/TopGameDto";
import { formatCount } from "../format";
import { db } from "./db.svelte";
import { game } from "./game.svelte";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";
import { panels } from "./panels.svelte";
import { prefs } from "./prefs.svelte";
import { tree, type TreeTarget } from "./tree.svelte";

/**
 * What the mini board (OPEN-14 3d) steps through: a game from the top games
 * or a line from the main variations. It never changes the main board.
 */
export type MiniLine = {
  kind: "game" | "line";
  /** Which game or line: `game:<id>`, `main` or `line:<n>`. */
  key: string;
  positions: LinePositionDto[];
  /** The board's position in it (where it was loaded). */
  start: number;
  /** The position shown. */
  index: number;
  /** "Kramnik – Mamedyarov, 2018", or "Main line" / "Line 3". */
  title: string;
  /** The game, for a game. */
  game: TopGameDto | null;
  /** The main board's position it was loaded at; it goes back when that changes. */
  fen: string;
};

/** Shares of the tab's height for the moves and the lines; the top games take the rest. */
export type ReferenceSizes = { moves: number; lines: number };
export const DEFAULT_SIZES: ReferenceSizes = { moves: 0.3, lines: 0.25 };

/** "6.d4", "6…dxc4": a move of a line with its number. */
export function moveLabel(p: LinePositionDto): string {
  return p.san ? `${p.moveNumber}${p.white ? "." : "…"}${p.san}` : "";
}

/** A line's moves as text, "6.b3 c5 7.Bb2", numbering Black's first move "6…c5". */
export function lineText(positions: LinePositionDto[]): string {
  return positions
    .filter((p) => p.san)
    .map((p, i) => (p.white || i === 0 ? moveLabel(p) : p.san))
    .join(" ");
}

/** "Kramnik – Mamedyarov, 2018". */
export function gameTitle(g: { white: string; black: string; year: number | null }): string {
  const name = (n: string) => n.split(",")[0].trim() || "Unknown player";
  return `${name(g.white)} – ${name(g.black)}${g.year ? `, ${g.year}` : ""}`;
}

/**
 * The Reference tab (OPEN-14) for the position on the board: moves with Hot
 * and Best players, top games, the main line and lines, and the mini board.
 * It reads the reference database (DB-13), or the open one when none is set.
 */
class ReferenceStore {
  /** The last view loaded, which may be for another position or database (see `current`). */
  data = $state.raw<ReferenceDto | null>(null);
  #dataHandle = $state<number | null>(null);
  loading = $state(false);
  /** The last lines loaded (see `currentLines`). */
  lines = $state.raw<LinesDto | null>(null);
  #linesHandle = $state<number | null>(null);
  linesLoading = $state(false);
  /** The job counting the lines of a common position, while it runs. */
  linesJob = $state<{ id: number; games: number } | null>(null);
  /** Why there are no lines for the position shown (cancelled, failed). */
  linesError = $state<string | null>(null);
  /** The top game selected (its id). */
  selected = $state<number | null>(null);
  /** What the mini board steps through; null: the main board's position. */
  mini = $state.raw<MiniLine | null>(null);
  #seq = 0;
  #miniSeq = 0;
  /** Jobs this store cancelled because the board moved on (others were cancelled by the user). */
  #cancelled = new Set<number>();

  /** Whether the Reference tab is showing: it queries only then. */
  get showing(): boolean {
    return panels.isShowing("reference");
  }

  /** The reference database when one is open, else the open database. */
  get target(): TreeTarget | null {
    return tree.referenceTarget;
  }

  /**
   * The view of the position on the board in the database the tab reads,
   * once loaded. The last one stays while the tab is hidden, so going back
   * to it at the same position shows it at once.
   */
  get current(): ReferenceDto | null {
    const data = this.data;
    return data && data.fen === game.view?.fen && this.#dataHandle === this.target?.handle ? data : null;
  }

  get currentLines(): LinesDto | null {
    const lines = this.lines;
    return lines && lines.fen === game.view?.fen && this.#linesHandle === this.target?.handle ? lines : null;
  }

  /**
   * Until the view of the position on the board arrives, the last one of
   * the same database, which the tab shows dimmed and inert so it doesn't
   * flash on every move.
   */
  get stale(): ReferenceDto | null {
    return !this.current && this.data && this.#dataHandle === this.target?.handle ? this.data : null;
  }

  /** The same for the lines, while new ones are being counted directly (not by a job). */
  get staleLines(): LinesDto | null {
    const quick = !this.linesJob && !this.linesError;
    return quick && !this.currentLines && this.lines && this.#linesHandle === this.target?.handle ? this.lines : null;
  }

  /** Shares of the tab's height for its sections, as last set. */
  get sizes(): ReferenceSizes {
    const raw = prefs.raw("referenceSizes") as Partial<ReferenceSizes> | undefined;
    const ok = (v: unknown) => typeof v === "number" && v >= 0.1 && v <= 0.8;
    return raw && ok(raw.moves) && ok(raw.lines) && raw.moves! + raw.lines! <= 0.9
      ? { moves: raw.moves!, lines: raw.lines! }
      : DEFAULT_SIZES;
  }

  set sizes(s: ReferenceSizes) {
    prefs.set("referenceSizes", { moves: Math.round(s.moves * 1000) / 1000, lines: Math.round(s.lines * 1000) / 1000 });
  }

  /**
   * Loads the view of the position on the board while the tab is showing,
   * then its lines. Replies for positions the board has left are dropped,
   * and a job still counting lines for one is cancelled.
   */
  async refresh() {
    if (!this.showing) return;
    const view = game.view;
    const target = this.target;
    this.#stopLines();
    if (!view || !target) {
      this.data = null;
      this.lines = null;
      return;
    }
    const seq = ++this.#seq;
    this.loading = true;
    let data: ReferenceDto;
    try {
      data = await api.referenceView(target.handle, view.session);
    } catch (e) {
      if (seq === this.#seq) {
        this.loading = false;
        notices.error(e);
      }
      return;
    }
    if (seq !== this.#seq) return;
    this.data = data;
    this.#dataHandle = target.handle;
    this.loading = false;
    if (data.total.games > 0) await this.#loadLines(seq, target.handle, view.session);
  }

  /** The tab was hidden: nothing it asked for is needed any more. */
  hide() {
    this.#seq++;
    this.#stopLines();
    this.loading = false;
  }

  /** Cancels the job counting lines for a position that's no longer wanted. */
  #stopLines() {
    const job = this.linesJob;
    this.linesJob = null;
    this.linesLoading = false;
    this.linesError = null;
    if (job) {
      this.#cancelled.add(job.id);
      void jobs.cancel(job.id);
    }
  }

  async #loadLines(seq: number, handle: number, session: number) {
    this.linesLoading = true;
    this.linesError = null;
    let retries = 2;
    try {
      for (;;) {
        const r = await api.referenceLines(handle, session);
        if (seq !== this.#seq) return;
        if (r.kind === "ready") {
          this.lines = r.lines;
          this.#linesHandle = handle;
          return;
        }
        // A common position: a job counts them (progress and Cancel in the
        // status bar); ask again when it's done, for the cached result.
        this.linesJob = { id: r.job, games: r.games };
        const done = await jobs.waitFor(r.job);
        if (seq !== this.#seq) return;
        this.linesJob = null;
        // The board left the position and came back while the job was
        // being cancelled: start it again.
        const ours = this.#cancelled.delete(r.job);
        if ((done.state === "done" || (done.state === "cancelled" && ours)) && retries-- > 0) continue;
        this.linesError =
          done.state === "cancelled"
            ? "Counting the lines was cancelled."
            : done.state === "done"
              ? "The database changed while the lines were counted."
              : `Counting the lines failed: ${done.error ?? "unknown error"}.`;
        return;
      }
    } catch (e) {
      if (seq === this.#seq) this.linesError = `Can't count the lines: ${errorMessage(e)}`;
    } finally {
      if (seq === this.#seq) this.linesLoading = false;
    }
  }

  /** Counts the lines again after they were cancelled. */
  retryLines() {
    const view = game.view;
    const target = this.target;
    if (!view || !target || !this.current) return;
    void this.#loadLines(this.#seq, target.handle, view.session);
  }

  /** Plays a move of the moves table on the main board (the usual variation choice applies). */
  play(uci: string) {
    return game.playUci(uci);
  }

  /** Lists the games reaching the position in the game list (`full`: searching every move). */
  findGames(full = false) {
    return tree.findGames(full, this.target);
  }

  // ---- the mini board -------------------------------------------------

  /** Shows a top game on the mini board, at the board's position. */
  async selectGame(g: TopGameDto) {
    const view = game.view;
    const target = this.target;
    if (!view || !target) return;
    this.selected = g.id;
    const seq = ++this.#miniSeq;
    try {
      const line = await api.gameLine(target.handle, g.id, view.session);
      if (seq !== this.#miniSeq || game.view?.fen !== view.fen) return;
      this.mini = {
        kind: "game",
        key: `game:${g.id}`,
        positions: line.positions,
        start: line.start,
        index: line.start,
        title: gameTitle(line),
        game: g,
        fen: view.fen,
      };
    } catch (e) {
      if (seq === this.#miniSeq) notices.error(e);
    }
  }

  /**
   * Shows a line on the mini board, at the board's position (its start).
   * `key` names it: `main`, or `line:<n>` for the n-th line.
   */
  selectLine(line: ReferenceLineDto, key: string, title: string) {
    const view = game.view;
    if (!view) return;
    this.#miniSeq++;
    this.selected = null;
    this.mini = {
      kind: "line",
      key,
      positions: line.positions,
      start: 0,
      index: 0,
      title: `${title} (${formatCount(line.games)} ${line.games === 1 ? "game" : "games"})`,
      game: null,
      fen: view.fen,
    };
  }

  /** Steps the mini board through its game or line; nothing moves on the main board. */
  step(dir: "start" | "back" | "forward" | "end") {
    const mini = this.mini;
    if (!mini) return;
    const last = mini.positions.length - 1;
    const to = { start: 0, back: mini.index - 1, forward: mini.index + 1, end: last }[dir];
    this.mini = { ...mini, index: Math.max(0, Math.min(last, to)) };
  }

  /** Back to the main board's position (⟲, and whenever the main board moves). */
  reset() {
    this.#miniSeq++;
    this.mini = null;
    this.selected = null;
  }

  /** Opens a top game on the main board where it reaches the position, as opening from the list does. */
  async openGame(g: TopGameDto, newTab = false) {
    const target = this.target;
    if (!target) return;
    await game.open(target.handle, g.id, null, newTab ? "tab" : "auto", g.ply);
  }

  /** Whether `target` is the reference database, or the open one read because none is set. */
  get readsReference(): boolean {
    return !!this.target?.reference;
  }

  /** The note when the tab reads the open database for want of a reference one. */
  get fallbackNote(): string | null {
    const target = this.target;
    if (!target || target.reference) return null;
    if (db.reference && !db.reference.info) {
      return `Reading ${target.name}: the reference database ${db.reference.name} can't be opened. Choose another in the ribbon.`;
    }
    return `Reading ${target.name}: no reference database is set. Choose one in the ribbon.`;
  }
}

export const reference = new ReferenceStore();
