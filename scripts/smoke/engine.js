// Smoke-test driver for engines (ENG-01/02/03/05/09) and the blunder check
// (ANL-01/04). Needs Stockfish: run scripts/fetch-stockfish.sh first.
// See scripts/smoke/run.sh for the placeholders.
(async () => {
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, analysis, api, invoke } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden monitor: never ask about unsaved changes.
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
  const until = async (cond, what, ms = 4000) => {
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const find = (lines, id) => {
    for (const m of lines) {
      if (m.id === id) return m;
      for (const v of m.variations) {
        const hit = find(v, id);
        if (hit) return hit;
      }
    }
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await db.createAt(OUT + "/engine.chessdb");

    // The Stockfish fetched by scripts/fetch-stockfish.sh is found on first use.
    await analysis.loadEngines();
    const names = analysis.engines.map((e) => e.name);
    expect(names.some((n) => n.startsWith("Stockfish")), "Stockfish detected, got " + JSON.stringify(names));
    expect(analysis.engines[0].declared.some((o) => o.name === "Threads"), "declared options cached");

    // Analysis follows the board.
    await game.newGame();
    await analysis.start();
    expect(analysis.status.running, "analysis running");
    await until(() => analysis.current?.lines.length === 3 && analysis.current.depth >= 12, "3 lines at depth 12", 15000);
    const first = analysis.current.lines[0];
    expect(first.moves.length > 3 && first.moves[0].white && first.moves[0].moveNumber === 1, "a PV from move 1");
    expect(first.wdl?.length === 3, "WDL shown");
    expect(document.querySelector(".analysis li") && document.querySelector(".eval-bar [role='meter'] .white"),
      "pane lines and eval bar");
    await shot("e01-analysis");

    await game.play("e2", "e4");
    await until(() => analysis.current?.fen.includes(" b KQkq"), "analysis follows the move");
    await analysis.setLines(1);
    // After a restart the first lines are shallow; wait for a real PV.
    await until(() => analysis.current?.lines.length === 1 && analysis.current.lines[0].moves.length >= 3, "one line");
    expect(analysis.status.multipv === 1, "multipv 1");

    // Clicking the second move of the PV plays two moves.
    const pv = analysis.current.lines[0];
    await analysis.playLine(pv, 1);
    expect(game.view.cursorPath.length === 3, "two PV moves played, at ply " + game.view.cursorPath.length);

    // "Add line" pastes the whole line with its evaluation, cursor stays.
    await game.step("start");
    await game.forward();
    await until(() => analysis.current?.lines[0]?.moves.length > 4 && analysis.current.fen === game.view.fen,
      "analysis after 1.e4");
    const cursor = game.view.cursor;
    const added = analysis.current.lines[0];
    await analysis.addLine(added);
    expect(game.view.cursor === cursor, "cursor stays after adding a line");
    const pgn = await api.gamePgn(game.view.session);
    expect(/\{[+−]\d+\.\d\d\/\d+\}/.test(pgn), "evaluation comment in the PGN:\n" + pgn);
    await shot("e02-line-added");

    // A new game: the analysis moves with the board. The old game has
    // unsaved changes, so the new one opens in a second tab.
    const firstTab = game.session;
    await game.newGame();
    expect(game.tabs.length === 2, "second tab");
    await until(() => analysis.current?.session === game.view.session, "analysis follows the new game");
    await game.activate(firstTab);
    await until(() => analysis.current?.session === firstTab, "analysis follows the tab switch");
    await game.closeTab(firstTab);
    await until(() => analysis.current?.session === game.view.session, "analysis follows after closing a tab");

    await analysis.stop();
    expect(!analysis.status.running && !document.querySelector("section.analysis"), "stopped, pane hidden");

    // Blunder check: 2.Qg4?? hangs the queen.
    for (const [a, b] of [["e2", "e4"], ["d7", "d5"], ["d1", "g4"], ["c8", "g4"]]) await game.play(a, b);
    await analysis.blunderCheck(150);
    const qg4 = game.notation.moves[2];
    expect(qg4.san === "Qg4" && qg4.nags.includes(4), "Qg4 marked ??, got " + JSON.stringify(qg4.nags));
    expect(game.notation.moves[2].variations.length === 1, "better line added as an alternative to Qg4");
    expect(game.notation.moves.every((m) => m.eval), "every move has an evaluation");
    expect(game.view.headers.annotator.startsWith("Stockfish"), "annotator set");
    await until(() => document.querySelector("figure.graph .mark.blunder"), "eval graph with the blunder marked");
    await game.goto(qg4.id);
    await shot("e03-blunder-check");
    await game.undo();
    expect(!game.notation.moves[2].nags.length && !game.notation.moves[2].variations.length,
      "one undo removes the whole review");
    expect(!find(game.notation.moves, qg4.id)?.eval, "evals undone too");

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
