import { api, onAnalysis, onThreat } from "../api";
import type { AnalysisDto } from "../bindings/AnalysisDto";
import type { AnalysisStatusDto } from "../bindings/AnalysisStatusDto";
import type { EngineDto } from "../bindings/EngineDto";
import type { EvalDto } from "../bindings/EvalDto";
import type { PvLineDto } from "../bindings/PvLineDto";
import type { ThreatDto } from "../bindings/ThreatDto";
import { formatEval } from "../format";
import {
  isCurrentThreat,
  isNewFailure,
  taskQueue,
  threatErrorNotice,
  visibleThreat,
} from "../threatView";
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
  threat: false,
};

/** Engines (ENG-01), the running analysis (ENG-02), threats (ENG-07) and the blunder check (ANL-01). */
class AnalysisStore {
  engines = $state<EngineDto[]>([]);
  status = $state<AnalysisStatusDto>(STOPPED);
  /** True while the engine starts. */
  starting = $state(false);
  #latest = $state<AnalysisDto | null>(null);
  #threat = $state<ThreatDto | null>(null);
  /** The newest `threat` event's number: older ones are stale (ENG-07). */
  #threatSeq = 0;

  constructor() {
    onAnalysis((update) => {
      if (update.error) {
        this.status = STOPPED;
        this.#latest = null;
        this.#threat = null;
        notices.show(`The engine stopped: ${update.error}. Start the analysis again to retry.`, "error");
        return;
      }
      this.#latest = update;
    });
    onThreat((threat) => {
      if (!isCurrentThreat(threat, this.#threatSeq)) return;
      const news = isNewFailure(this.#threat, threat);
      this.#threatSeq = threat.seq;
      this.#threat = threat;
      // Every window gets the event: only the one whose game it is says so.
      if (news && this.threat?.error) notices.show(threatErrorNotice(threat), "error");
      if (this.threat && !this.threat.error) this.#noteThreatUp();
    });
  }

  /** Analysis of the position on the board, if the engine has sent any yet. */
  get current(): AnalysisDto | null {
    const u = this.#latest;
    const view = game.view;
    return u && view && u.session === view.session && u.fen === view.fen ? u : null;
  }

  /**
   * Whether the engine also shows what the opponent threatens (ENG-07): the
   * position on the board searched as if the side to move passed. An app
   * preference, so it's remembered and every window follows it.
   */
  get showThreat(): boolean {
    return prefs.get("showThreat", false);
  }

  /**
   * The threat in the position on the board while the engine runs with
   * "Show threat" on: the opponent's line, why there's none, or the threat
   * engine's failure in this game; null until its search has said anything.
   */
  get threat(): ThreatDto | null {
    return visibleThreat(
      this.#threat,
      game.view,
      this.showThreat && (this.status.running || this.starting),
    );
  }

  /** Turns "Show threat" on or off, for the running analysis too. */
  async setShowThreat(on: boolean) {
    prefs.set("showThreat", on);
    this.#threat = null;
    await this.#sendShowThreat();
  }

  /** Calls about the threat search, one at a time (`taskQueue`). */
  #threatCalls = taskQueue();

  /**
   * Sends "Show threat" to the running analysis, after the calls before it
   * and with the setting as it is when its turn comes, so the engine ends
   * up as the button says however fast it's clicked.
   */
  #sendShowThreat = () =>
    this.#threatCalls(async () => {
      if (!this.status.running) return;
      try {
        const status = await api.setAnalysisThreat(this.showThreat);
        // Unless the engine was stopped meanwhile.
        if (this.status.running) this.status = status;
      } catch (e) {
        notices.error(e);
      }
    });

  #statusQueued = false;

  /**
   * Reads the status again once the threat engine is up. It starts in the
   * background, after the reply to the start or to "Show threat", so the
   * first word of its search for this game is what says it's up.
   */
  #noteThreatUp() {
    if (this.status.threat || !this.status.running || this.#statusQueued) return;
    this.#statusQueued = true;
    void this.#threatCalls(async () => {
      this.#statusQueued = false;
      if (this.status.threat || !this.status.running) return;
      try {
        const status = await api.analysisStatus();
        if (this.status.running) this.status = status;
      } catch {
        // The next word of the threat search asks again.
      }
    });
  }

  get lines(): number {
    return this.status.running ? this.status.multipv : prefs.get("analysisLines", 3);
  }

  get defaultEngine(): EngineDto | undefined {
    return this.engines.find((e) => e.isDefault) ?? this.engines[0];
  }

  /** True while the engine list loads (the first time, engines are looked for and started once). */
  loadingEngines = $state(false);
  #loading: Promise<void> | null = null;

  /** Loads the engines; calls while it runs share the same load. */
  loadEngines(): Promise<void> {
    this.#loading ??= (async () => {
      this.loadingEngines = true;
      try {
        this.engines = await api.listEngines();
        this.status = await api.analysisStatus();
      } catch (e) {
        notices.error(e);
      } finally {
        this.loadingEngines = false;
        this.#loading = null;
      }
    })();
    return this.#loading;
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

  async detect() {
    this.loadingEngines = true;
    try {
      return await this.#engines(() => api.detectEngines());
    } finally {
      this.loadingEngines = false;
    }
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
    this.#threat = null;
    // The start sees to the threat engine (it starts in the background and
    // reports a failure itself), so "Show threat" is only sent again if it
    // changed while the main engine started.
    const showThreat = this.showThreat;
    try {
      this.status = await api.startAnalysis(session, engine, this.lines, showThreat);
      if (this.threat && !this.threat.error) this.#noteThreatUp();
      if (this.status.running && this.showThreat !== showThreat) await this.#sendShowThreat();
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
      this.#threat = null;
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
