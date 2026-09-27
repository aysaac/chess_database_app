// Smoke-test driver for the position index (SRCH-04), opening tree
// (OPEN-01), position search (SRCH-03) and opening names (OPEN-11).
// With TWIC (testdata/large/twic1600.pgn) present it also checks a real
// tournament database; with the 1M-game file (__BIG__) it times the index.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, tree, api, invoke } = window.__chessdb;
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
  const until = async (cond, what, ms = 5000) => {
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const indexed = () => db.info?.index.built && db.info.index.upToDate;
  const moves = () => tree.data.moves.map((m) => `${m.san}:${m.games}`).join(" ");
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await db.createAt(OUT + "/tree.chessdb");
    for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    await until(indexed, "the index built after the imports", 20000);

    // The tree of the starting position: 4 games start from it.
    tree.visible = true;
    await game.newGame();
    await until(() => tree.data?.total.games === 4, "4 games from the start, got " + tree.data?.total.games);
    expect(moves() === "e4:3 d4:1", "e4 in 3 games, d4 in 1, got " + moves());
    expect(document.querySelectorAll(".tree tbody tr").length === 2, "two rows in the tree");
    await shot("t01-tree-start");

    // Playing a tree move moves the board; the tree follows.
    await tree.play(tree.data.moves[0].uci);
    expect(game.view.cursorPath.length === 1, "1.e4 played");
    await until(() => tree.data?.fen === game.view.fen && tree.data.total.games === 3, "tree after 1.e4");
    expect(moves() === "e5:2 d6:1", "replies e5 (2) and d6 (1), got " + moves());
    expect(tree.data.total.score > 0 && tree.data.total.score < 1, "a score");

    // Position search: the list shows the games reaching 1.e4.
    await db.searchPosition(game.view.session);
    expect(db.info.filter?.count === 3, "3 games found, got " + db.info.filter?.count);
    await until(() => document.querySelector(".bar .status")?.textContent.includes("3 of 6"), "filter bar");
    await shot("t02-position-search");
    await db.clearFilter();

    // Opening names: in the game header and the list.
    await game.open(db.info.handle, 0, 0);
    expect(game.view.opening?.name.startsWith("Pirc"), "Kasparov–Topalov is a Pirc, got " + JSON.stringify(game.view.opening));
    const page = await api.listGames(db.info.handle, db.sort, 0, 6);
    expect(page.rows[0].opening.startsWith("Pirc") && page.rows[5].opening, "opening names in the list");
    expect(document.querySelector(".game-column .opening")?.textContent.includes("Pirc"), "opening shown in the header");

    // An edited game is still counted (replayed until the next index update).
    await game.open(db.info.handle, 5, 5);
    await game.setHeaders({ ...game.view.headers, event: "Edited" });
    await game.save();
    await game.newGame();
    await until(() => tree.data?.fen === game.view.fen && tree.data.total.games === 4, "still 4 games after the edit");
    await shot("t03-after-edit");

    // Deep positions: beyond the indexed plies a full search is offered.
    await game.newGame("8/8/4k3/8/8/4K3/8/8 w - - 0 60");
    await until(() => tree.data?.fen === game.view.fen, "tree for an endgame");
    expect(tree.data.total.games === 0 && tree.data.beyondIndex, "no games, beyond the index");
    await db.searchPosition(game.view.session, true);
    expect(db.info.filter?.count === 0, "full search finds nothing");
    await db.clearFilter();

    if (BIG) {
      await db.createAt(OUT + "/big.chessdb");
      await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
      let t = performance.now();
      await until(indexed, "1M games indexed", 300000);
      await log(`index of 1M games ready ${Math.round((performance.now() - t) / 1000)}s after the import`);
      await game.newGame();
      t = performance.now();
      await until(() => tree.data?.fen === game.view.fen && tree.data.total.games > 900000, "tree of 1M games", 20000);
      await log(`start position tree: ${tree.data.total.games} games, ${Math.round(performance.now() - t)} ms`);
      await tree.play("e2e4");
      t = performance.now();
      await until(() => tree.data?.fen === game.view.fen, "tree after 1.e4", 20000);
      await log(`tree after 1.e4: ${tree.data.total.games} games, ${Math.round(performance.now() - t)} ms`);
      t = performance.now();
      await db.searchPosition(game.view.session);
      await log(`position search: ${db.info.filter.count} games, ${Math.round(performance.now() - t)} ms`);
      await shot("t04-big-tree");
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
