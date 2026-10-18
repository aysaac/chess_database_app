// Smoke-test driver for ENG-07, the threat display. Needs Stockfish: run
// scripts/fetch-stockfish.sh first. See scripts/smoke/run.sh for the
// placeholders.
//
// The board is an editor's (DB-07): the main window is the library, so
// there the driver only opens an editor window, which runs the same script.
// With the engine on, "Show threat" in the engine pane starts a second
// engine on the position after a pass. After 1.e4 e5 2.Bc4 Nc6 3.Qh5 the
// threat is 4.Qxf7#: the pane shows it with #1, the board a dashed arrow
// h5-f7, and the main analysis keeps its three lines and deepens. The
// threat follows the board (3...g6: Black threatens ...gxh5; back a move:
// Qxf7# again), says why there's none in check, after mate and when the
// opponent would have no move, and goes when switched off or when the
// engine stops. A double click while the threat engine starts (on, then
// off before it's up), and on/off/on/off, end off with no threat engine
// left in the backend, also after a move. The setting is remembered: after
// a reload, starting the engine brings the threat back. Screenshots in dark
// and light mode.
(async () => {
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, analysis, appearance, api, invoke } = window.__chessdb;
  // A native dialog would open on the user's screen: never ask about
  // unsaved changes.
  game.confirmDiscard = async () => true;
  const log = (m) => invoke("dev_log", { message: String(m) });
  const shot = async (name) => {
    await wait(800);
    await log("shot:" + name);
    await wait(1500);
  };
  const expect = (ok, what) => {
    if (!ok) throw new Error("expected " + what);
  };
  const until = async (cond, what, ms = 8000) => {
    const end = performance.now() + ms;
    while (!(await cond()) && performance.now() < end) await wait(50);
    expect(await cond(), what);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  if (document.querySelector(".home")) {
    await log("library window: opening an editor window for the checks");
    await api.openEditorWindow(null);
    return;
  }

  // After 1.e4 e5 2.Bc4 Nc6 3.Qh5, Black to move.
  const SCHOLAR = "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3";
  const toggle = () => document.querySelector("section.analysis button.toggle");
  const threatRow = () => document.querySelector("section.analysis p.threat");
  const threatText = () => threatRow()?.textContent.replace(/\s+/g, " ").trim() ?? "";
  // chessground names each drawn shape by its squares and brush (cgHash).
  const threatArrows = () =>
    [...document.querySelectorAll(".board svg.cg-shapes g")]
      .map((g) => g.getAttribute("cgHash") ?? "")
      .filter((h) => h.endsWith(",threat") || h.includes(",threat,"));
  const hasArrow = (from, to) => threatArrows().some((h) => h.includes(`,${from},${to},threat`));
  const first = () => analysis.threat?.line?.moves[0];
  const savedPref = async () => (await api.uiPrefs()).values.showThreat;

  const phase = sessionStorage.getItem("threat-phase") || "start";
  try {
    await log("start, phase=" + phase);

    if (phase === "start") {
      await db.createAt(OUT + "/threat.chessdb");
      await analysis.loadEngines();
      const names = analysis.engines.map((e) => e.name);
      expect(names.some((n) => n.startsWith("Stockfish")), "Stockfish detected, got " + JSON.stringify(names));

      await game.newGame();
      expect(!analysis.showThreat, "Show threat off on a fresh install");
      await analysis.start();
      expect(analysis.status.running && !analysis.status.threat, "analysis running without the threat");
      expect(toggle()?.textContent.trim() === "Show threat", "a Show threat button in the engine pane");
      expect(toggle().getAttribute("aria-pressed") === "false", "the button not pressed");
      expect(!threatRow(), "no threat row while it's off");

      for (const [a, b] of [["e2", "e4"], ["e7", "e5"], ["f1", "c4"], ["b8", "c6"], ["d1", "h5"]]) {
        await game.play(a, b);
      }
      expect(game.view.fen === SCHOLAR, "the scholar's mate threat on the board, got " + game.view.fen);
      await until(() => analysis.current?.lines.length === 3, "three main lines before the threat");

      // The real button, as a click does it.
      toggle().click();
      await until(() => analysis.status.threat, "the threat engine started");
      expect(toggle().getAttribute("aria-pressed") === "true", "the button pressed");
      await until(() => first()?.san === "Qxf7#", "the threat 4.Qxf7#, got " + JSON.stringify(first()), 15000);
      const line = analysis.threat.line;
      expect(line.eval.mate === 1 && first().moveNumber === 4 && first().white, "#1 for White at move 4");
      await until(() => threatText().includes("4.♕xf7#") && threatText().includes("#1"), "the pane's threat row");
      expect(threatText().startsWith("Threat"), "the row says Threat, got " + threatText());
      await until(() => hasArrow("h5", "f7"), "a threat arrow h5-f7, got " + JSON.stringify(threatArrows()));
      const stroke = document.querySelector('.board svg.cg-shapes line[marker-end="url(#arrowhead-threat)"]');
      const style = getComputedStyle(stroke);
      expect(style.strokeDasharray && style.strokeDasharray !== "none", "a dashed arrow, got " + style.strokeDasharray);
      await log("arrow stroke " + style.stroke + ", dashes " + style.strokeDasharray);

      // The main analysis is untouched: three lines for Black, still deepening.
      const depth = analysis.current.depth;
      await until(
        () => analysis.current?.depth >= Math.max(depth + 2, 14) && analysis.current.lines.length === 3,
        "the main analysis deepening next to the threat (from depth " + depth + ")",
        20000,
      );
      expect(analysis.current.lines.every((l) => !l.moves[0].white), "main lines are Black's moves");
      expect(analysis.current.fen === SCHOLAR, "main analysis on the board's position");
      await log(`main depth ${analysis.current.depth}, threat depth ${analysis.threat.depth}`);
      appearance.theme = "dark";
      await shot("t01-mate-threat-dark");
      appearance.theme = "light";
      await shot("t02-mate-threat-light");
      appearance.theme = "dark";

      // It follows the board: after 3...g6 Black threatens the queen.
      await game.play("g7", "g6");
      await until(() => first()?.uci === "g6h5", "the threat ...gxh5, got " + JSON.stringify(first()));
      expect(!first().white && first().moveNumber === 4 && first().san === "gxh5", "4...gxh5");
      await until(() => hasArrow("g6", "h5") && !hasArrow("h5", "f7"), "the arrow moved to g6-h5");
      await shot("t03-follows-the-board");
      await game.step("back");
      await until(() => first()?.san === "Qxf7#", "Qxf7# again a move back");
      await until(() => hasArrow("h5", "f7"), "the arrow back on h5-f7");

      // No threat in check, after mate, or with nothing for the opponent to move.
      await game.newGame("rnbqkbnr/ppppp1pp/8/5p1Q/4P3/8/PPPP1PPP/RNB1KBNR b KQkq - 1 2");
      await until(() => analysis.threat?.unavailable === "inCheck", "no threat in check");
      await until(() => threatText().includes("None while Black is in check"), "the in-check note, got " + threatText());
      // chessground redraws its arrows on the next animation frame.
      await until(() => threatArrows().length === 0, "no threat arrow in check, got " + threatArrows());
      await until(() => analysis.current?.lines.length > 0, "the main analysis still runs in check");
      await shot("t04-in-check");
      await game.newGame("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3");
      await until(() => analysis.threat?.unavailable === "gameOver", "no threat after mate");
      expect(threatText().includes("the game is over"), "the game-over note, got " + threatText());
      await game.newGame("7k/8/6Q1/8/8/8/8/K7 w - - 0 1");
      await until(() => analysis.threat?.unavailable === "noMoves", "no threat with no reply");
      expect(threatText().includes("Black would have no legal move"), "the no-moves note, got " + threatText());

      // Off: the row and arrow go, the analysis goes on.
      await game.newGame(SCHOLAR);
      await until(() => hasArrow("h5", "f7"), "the threat in a new game");
      toggle().click();
      await until(() => !analysis.status.threat, "the threat engine stopped");
      await until(() => !threatRow() && threatArrows().length === 0, "no threat row or arrow once off");
      expect(analysis.status.running, "the analysis still runs");
      await until(async () => (await savedPref()) === false, "Show threat saved as off");
      await shot("t05-off");

      // A double click while the threat engine starts: on, then off before
      // it's up. It ends off in the backend too, with no threat engine
      // searching after a move.
      const settled = async (what) => {
        await wait(3000);
        const st = await api.analysisStatus();
        await log(`${what}: showThreat=${analysis.showThreat} status.threat=${analysis.status.threat} backend=${st.threat}`);
        expect(!analysis.showThreat && toggle().getAttribute("aria-pressed") === "false", what + ": the button off");
        expect(st.running && !st.threat, what + ": no threat engine in the backend");
        expect(!analysis.status.threat && analysis.status.running, what + ": the page's status agrees");
        expect(!threatRow() && threatArrows().length === 0, what + ": no threat row or arrow");
      };
      toggle().click();
      await wait(60);
      toggle().click();
      await settled("double click");
      await game.play("g7", "g6");
      await settled("double click, after a move");
      await game.step("back");
      for (const pause of [40, 40, 40]) {
        toggle().click();
        await wait(pause);
      }
      toggle().click();
      await settled("on, off, on, off");

      // On again, then the engine stops: the threat goes with it.
      toggle().click();
      await until(() => analysis.status.threat && hasArrow("h5", "f7"), "the threat back on");
      await until(async () => (await savedPref()) === true, "Show threat saved as on");
      await analysis.stop();
      expect(!analysis.status.running && !analysis.status.threat, "both engines stopped");
      await until(() => threatArrows().length === 0, "no threat arrow once the engine stops");

      sessionStorage.setItem("threat-phase", "reloaded");
      await log("reloading");
      location.reload();
      return;
    }

    if (phase === "reloaded") {
      sessionStorage.removeItem("threat-phase");
      expect(analysis.showThreat, "Show threat remembered after a reload");
      await game.newGame(SCHOLAR);
      await analysis.loadEngines();
      await analysis.start();
      expect(analysis.status.running && analysis.status.threat, "the threat starts with the engine");
      await until(() => first()?.san === "Qxf7#", "the threat after the reload", 15000);
      await until(() => hasArrow("h5", "f7"), "its arrow after the reload");
      expect(toggle().getAttribute("aria-pressed") === "true", "the button pressed after the reload");
      await shot("t06-after-reload");
      await analysis.stop();
      await log("done");
    }
  } catch (e) {
    sessionStorage.removeItem("threat-phase");
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)) + (e?.stack ? "\n" + e.stack : ""));
  }
})();
