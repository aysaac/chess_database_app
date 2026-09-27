import { api, onAnalysis } from "../api";
import type { AnalysisDto } from "../bindings/AnalysisDto";
import type { AnalysisStatusDto } from "../bindings/AnalysisStatusDto";
import type { EngineDto } from "../bindings/EngineDto";
import type { EvalDto } from "../bindings/EvalDto";
import type { PvLineDto } from "../bindings/PvLineDto";
import { formatEval } from "../format";
import { game } from "./game.svelte";
import { jobs } from "./jobs.svelte";
import { notices } from "./notify.svelte";
import { prefs } from "./prefs.svelte";

const STOPPED: AnalysisStatusDto = {
  running: false,
  engineId: null,
  engineName: null,
  session: null,
  multipv: 1,
};

/** Engines (ENG-01), the running analysis (ENG-02) and the blunder check (ANL-01). */
class AnalysisStore {
  engines = $state<EngineDto[]>([]);
  status = $state<AnalysisStatusDto>(STOPPED);
  /** True while the engine starts. */
  starting = $state(false);
  #latest = $state<AnalysisDto | null>(null);

  constructor() {
    onAnalysis((update) => {
      if (update.error) {
        this.status = STOPPED;
        this.#latest = null;
        notices.show(`The engine stopped: ${update.error}. Start the analysis again to retry.`, "error");
        return;
      }
      this.#latest = update;
    });
  }

  /** Analysis of the position on the board, if the engine has sent any yet. */
  get current(): AnalysisDto | null {
    const u = this.#latest;
    const view = game.view;
    return u && view && u.session === view.session && u.fen === view.fen ? u : null;
  }

  get lines(): number {
    return this.status.running ? this.status.multipv : prefs.get("analysisLines", 3);
  }

  get defaultEngine(): EngineDto | undefined {
    return this.engines.find((e) => e.isDefault) ?? this.engines[0];
  }

  async loadEngines() {
    try {
      this.engines = await api.listEngines();
      this.status = await api.analysisStatus();
    } catch (e) {
      notices.error(e);
    }
  }

  async #engines(f: () => Promise<EngineDto[]>) {
    try {
      this.engines = await f();
      return true;
    } catch (e) {
      notices.error(e);
      return false;
    }
  }

  detect() {
    return this.#engines(() => api.detectEngines());
  }

  add(path: string) {
    return this.#engines(() => api.addEngine(path));
  }

  async remove(id: string) {
    await this.#engines(() => api.removeEngine(id));
    this.status = await api.analysisStatus().catch(() => STOPPED);
  }

  setDefault(id: string) {
    return this.#engines(() => api.setDefaultEngine(id));
  }

  setOptions(id: string, options: [string, string][]) {
    return this.#engines(() => api.setEngineOptions(id, options));
  }

  toggle() {
    return this.status.running ? this.stop() : this.start();
  }

  async start(engine: string | null = null) {
    const session = game.session;
    if (session === null || this.starting) return;
    if (!this.engines.length) await this.loadEngines();
    if (!this.engines.length) {
      notices.show("No engine is set up. Add one under Engines, or run scripts/fetch-stockfish.sh.", "error");
      return;
    }
    this.starting = true;
    try {
      this.status = await api.startAnalysis(session, engine, this.lines);
    } catch (e) {
      notices.error(e);
    } finally {
      this.starting = false;
    }
  }

  async stop() {
    try {
      this.status = await api.stopAnalysis();
      this.#latest = null;
    } catch (e) {
      notices.error(e);
    }
  }

  async setLines(n: number) {
    const lines = Math.max(1, Math.min(5, n));
    prefs.set("analysisLines", lines);
    if (!this.status.running) return;
    try {
      this.status = await api.setAnalysisLines(lines);
    } catch (e) {
      notices.error(e);
    }
  }

  /** Plays a principal variation up to and including move `upto`. */
  playLine(line: PvLineDto, upto: number) {
    const moves = line.moves.slice(0, upto + 1).map((m) => m.uci);
    return game.addEngineLine(moves, null, true);
  }

  /** Adds a principal variation to the notation, ending with its evaluation. */
  addLine(line: PvLineDto) {
    const comment = `${formatEval(line.eval)}/${line.depth}`;
    return game.addEngineLine(
      line.moves.map((m) => m.uci),
      comment,
      false,
    );
  }

  /** Runs the blunder check on the game; the annotations appear when it's done. */
  async blunderCheck(movetimeMs: number) {
    const session = game.session;
    if (session === null) return;
    try {
      const id = await api.blunderCheck(session, null, movetimeMs);
      const done = await jobs.waitFor(id);
      if (done.state === "done" && game.session === session) await game.refresh();
    } catch (e) {
      notices.error(e);
    }
  }
}

/** Winning chances for White in 0…1 (Lichess's model). */
export function whiteChance(e: EvalDto): number {
  if (e.mate !== null) return e.mate > 0 ? 1 : e.mate < 0 ? 0 : 0.5;
  return 1 / (1 + Math.exp(-0.00368208 * (e.cp ?? 0)));
}

export const analysis = new AnalysisStore();
