// Smoke-test driver for the game-list filter: quick filter (SRCH-02),
// header search and search within results (SRCH-01/14), sorting and
// exporting a filtered list (IO-11), next/previous game (QOL-03).
// With the 1M-game file present it also times a quick filter over it.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, jobs, api, invoke } = window.__chessdb;
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
  const ids = async () => (await api.listGames(db.info.handle, db.sort, 0, 100)).rows.map((r) => r.id);
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await db.createAt(OUT + "/search.chessdb");
    for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    expect(db.info.games === 6, "6 games");

    // Quick filter: any name field, accents and case ignored.
    await db.quickFilter("CASTLING");
    expect(db.info.filter?.count === 2, "2 games with castling in the event, got " + db.info.filter?.count);
    expect(same(await ids(), [2, 4]), "ids 2 and 4");
    await until(() => document.querySelector(".bar .status")?.textContent.includes("2 of 6"), "filter bar shows 2 of 6");
    await until(() => document.querySelectorAll(".viewport .row").length === 2, "two rows listed");
    await shot("s01-quick-filter");
    await db.clearFilter();
    expect(!db.info.filter && db.listTotal === 6, "cleared");

    // Header search, then search within the results.
    await db.search({ result: "decisive" });
    expect(db.info.filter?.count === 3 && same(await ids(), [0, 2, 5]), "3 decisive games");
    await db.search({ white: "player" }, true);
    expect(same(await ids(), [5]), "within: player1's game");
    expect(db.info.filter.description.includes("then"), "description of a refined search: " + db.info.filter.description);
    await db.search({ white: "nobody" });
    await until(() => document.querySelector(".list .empty")?.textContent.includes("No games match"), "no-match message");
    await shot("s02-no-match");

    // Sorting a filtered list.
    await db.search({ result: "decisive" });
    await db.toggleSort("white");
    expect(same(await ids(), [0, 5, 2]), "filtered list sorted by White, unknown last");

    // Next/previous game follows the filtered, sorted list.
    await game.open(db.info.handle, 0, 0);
    await game.openAdjacent(1);
    expect(game.view.source.game === 5 && game.listIndex === 1, "next game is player1's");
    await game.openAdjacent(1);
    expect(game.view.source.game === 2, "then the setup game");
    await game.openAdjacent(1);
    expect(game.view.source.game === 2 && game.listIndex === 2, "stays at the end");
    await game.openAdjacent(-1);
    expect(game.view.source.game === 5, "previous game");
    await until(() => document.querySelector(".row.selected .cell")?.textContent === "6", "list selection follows");
    await shot("s03-next-game");

    // Tabs (UI-05): a game with unsaved changes stays open in its tab.
    await game.newGame();
    const tabs0 = game.tabs.length;
    await game.play("e2", "e4");
    expect(game.view.dirty, "the new game has unsaved changes");
    await game.open(db.info.handle, 0, 0);
    expect(game.tabs.length === tabs0 + 1 && game.view.source.game === 0, "opened in a new tab");
    await until(() => document.querySelectorAll("[aria-label='Open games'] [role='tab']").length === game.tabs.length, "tab strip");
    await shot("s03a-tabs");
    await game.open(db.info.handle, 5, 1);
    expect(game.tabs.length === tabs0 + 1 && game.view.source.game === 5, "a clean tab is reused");
    const dirtyTab = game.tabs.find((t) => t.view.dirty);
    await game.activate(dirtyTab.session);
    expect(game.view.dirty && game.notation.moves.length === 1, "back to the unsaved game");
    await game.closeTab(dirtyTab.session);
    expect(game.tabs.length === tabs0 && !game.tabs.some((t) => t.view.dirty), "closed");
    await game.open(db.info.handle, 5, 1);
    expect(game.tabs.length === tabs0, "an open game isn't opened twice");

    // Exporting while filtered writes only the filtered games.
    const job = await api.exportPgn(db.info.handle, OUT + "/filtered.pgn");
    const done = await jobs.waitFor(job);
    expect(done.message.startsWith("Exported 3 games"), "3 games exported, got " + done.message);

    // A new sort order or filter resets next/previous.
    await db.clearFilter();
    expect(game.listIndex === null, "list changed: no next game");

    // Player and event lists (DB-04): counts, games of a name, rename, merge.
    const h = db.info.handle;
    let players = await api.listNames(h, "player", "", true, 100);
    expect(players.total === 6, "6 players, got " + players.total);
    const kasparov = players.rows.find((r) => r.name === "Kasparov, Garry");
    await db.showNameGames("player", kasparov.id);
    expect(db.info.filter?.count === 1 && db.info.filter.description === "Player: Kasparov, Garry",
      "Kasparov's games: " + JSON.stringify(db.info.filter));
    await db.clearFilter();
    const a = players.rows.find((r) => r.name === "A");
    expect(await db.renameName("player", a.id, "Anderssen, Adolf"), "renamed");
    const natural = { key: "id", descending: false };
    expect((await api.listGames(h, natural, 0, 6)).rows.find((r) => r.id === 1).white === "Anderssen, Adolf",
      "the game shows the new name");
    const b = players.rows.find((r) => r.name === "B");
    expect(await db.renameName("player", b.id, "Anderssen, Adolf"), "merged");
    players = await api.listNames(h, "player", "anderssen", true, 100);
    expect(players.total === 1 && players.rows[0].games === 1, "one merged player with one game");
    const events = await api.listNames(h, "event", "castling", false, 100);
    expect(events.total === 2, "2 events mention castling");
    document.querySelector("[role='tab'][data-tab='player']").click();
    await until(() => document.querySelectorAll(".names .rows .row").length === 5, "players tab lists 5 players");
    await shot("s04-players");
    document.querySelector("[role='tab'][data-tab='games']").click();

    // Compacting (DB-06): deleted games go for good, open games follow
    // their new ids, the position index is rebuilt.
    await game.open(h, 5, null);
    await db.setDeleted([1, 3], true);
    await db.refresh();
    expect(db.info.deleted === 2, "2 deleted games, got " + db.info.deleted);
    await db.compact();
    expect(db.info.games === 4 && db.info.deleted === 0, "4 games left, got " + db.info.games);
    expect(game.view.source?.game === 3, "the open game (was #6) is now #4, got " + JSON.stringify(game.view.source));
    const whites = (await api.listGames(h, natural, 0, 10)).rows.map((r) => r.white);
    expect(whites.length === 4 && whites[3] === "player1", "renumbered list: " + whites.join(" | "));
    await until(() => db.info.index.built && db.info.index.upToDate, "position index rebuilt", 10000);

    // Recent databases (DB-07) and reopening the last one (QOL-01).
    await db.loadRecent();
    expect(db.recent[0]?.path.endsWith("search.chessdb") && db.recent[0].exists, "search.chessdb is the most recent");
    await db.close();
    expect(!db.info && document.querySelector(".list .recent button")?.textContent === "search",
      "closed; the empty list offers the recent database");
    await db.reopenLast();
    expect(db.info?.name === "search" && db.info.games === 4, "reopened the last database");

    if (BIG) {
      await db.createAt(OUT + "/big.chessdb");
      await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
      let t = performance.now();
      await db.quickFilter("magnus");
      await log(`quick filter over 1M games: ${db.info.filter.count} games in ${Math.round(performance.now() - t)} ms`);
      t = performance.now();
      await db.search({ eloMin: 2500, result: "draw" });
      await log(`header search over 1M games: ${db.info.filter.count} games in ${Math.round(performance.now() - t)} ms`);
      t = performance.now();
      await db.toggleSort("whiteElo");
      await log(`sorting the filtered list: ${Math.round(performance.now() - t)} ms`);
      await shot("s05-big-filter");
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
