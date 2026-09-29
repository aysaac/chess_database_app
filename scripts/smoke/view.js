// Smoke-test driver for QOL-07: the game list remembers its sort order,
// column widths and filter per database. Resizes columns by dragging the
// header edge (synthetic pointer events), with Shift+arrows and by
// double-click; closes and reopens the database after each kind of filter
// (quick filter, header search within results, player, position, full
// position search) and checks it all comes back; checks that another
// database keeps its own view and that a filter that can't be made again is
// dropped. With the 1M-game file present it also times a reopen that
// restores a sort and a quick filter.
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
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const sameCols = (a, b) => same(Object.entries(a).sort(), Object.entries(b).sort());
  const cell = (id) => document.querySelector(`.head-cell[data-column="${id}"]`);
  const width = (id) => cell(id).getBoundingClientRect().width;
  const near = (a, b) => Math.abs(a - b) <= 1.5;
  const ids = async () => (await api.listGames(db.info.handle, db.sort, 0, 100)).rows.map((r) => r.id);
  const status = () => document.querySelector(".bar .status")?.textContent ?? "";
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  /** Closes the database and opens it again, as after a restart. */
  const reopen = async () => {
    const path = db.info.path;
    await db.close();
    expect(!db.info && !db.restoring, "closed");
    await db.openAt(path);
    expect(db.info?.path === path && !db.restoring, "reopened with the view restored");
  };

  try {
    await log("start");
    const path = OUT + "/view.chessdb";
    await db.createAt(path);
    for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    expect(db.info.games === 6, "6 games");
    expect(same(db.columns, {}) && db.sort.key === "id", "a new database has the default view");
    await until(() => cell("opening")?.textContent.trim() === "Opening", "the Opening column has its header");

    // Sort: by White, descending.
    await db.toggleSort("white");
    await db.toggleSort("white");
    expect(db.sort.key === "white" && db.sort.descending, "sorted by White, descending");

    // Drag the right edge of the White column 80 px to the right.
    const w0 = width("white");
    const r0 = cell("white").getBoundingClientRect();
    const grip = cell("white").querySelector(".grip");
    grip.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, clientX: r0.right, clientY: r0.top + 5 }));
    await until(() => document.querySelector(".list.resizing"), "the list shows it's resizing");
    for (const dx of [20, 50, 80]) {
      window.dispatchEvent(new PointerEvent("pointermove", { clientX: r0.right + dx, clientY: r0.top + 5 }));
      await wait(30);
    }
    window.dispatchEvent(new PointerEvent("pointerup", { clientX: r0.right + 80, clientY: r0.top + 5 }));
    await until(() => near(width("white"), w0 + 80), `White 80 px wider (${w0} → ${width("white")})`);
    expect(near(db.columns.white, w0 + 80), "White's width kept: " + JSON.stringify(db.columns));
    expect(!document.querySelector(".list.resizing"), "done resizing");
    const rows = document.querySelectorAll(".viewport .row");
    expect(near(rows[0].children[1].getBoundingClientRect().width, w0 + 80), "the rows follow the header");

    // Keyboard: Shift+arrows on a header (a sort button or the Opening header).
    const event0 = width("event");
    cell("event").querySelector("button").dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowLeft", shiftKey: true, bubbles: true, cancelable: true }),
    );
    await until(() => near(width("event"), event0 - 10), "Event 10 px narrower");
    const opening = cell("opening").querySelector(".cell");
    opening.focus();
    expect(document.activeElement === opening, "the Opening header takes focus");
    for (let i = 0; i < 3; i++) {
      opening.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, bubbles: true, cancelable: true }));
    }
    await until(() => db.columns.opening !== undefined, "Opening resized");
    await shot("v01-resized");
    // Double-click on its edge: back to the default width.
    cell("opening").querySelector(".grip").dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(db.columns.opening === undefined, "Opening back to its default");

    // The header menu offers the default widths.
    const site = cell("site").getBoundingClientRect();
    cell("site").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: site.left + 10, clientY: site.top + 10 }));
    await until(() => document.querySelectorAll(".menu [role='menuitem']").length === 2, "header menu");
    const items = [...document.querySelectorAll(".menu [role='menuitem']")];
    expect(items[0].textContent.includes("Site") && items[0].disabled, "Site isn't resized: " + items[0].textContent);
    expect(!items[1].disabled, "some columns are");
    await shot("v02-header-menu");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await until(() => !document.querySelector(".menu"), "menu closed");

    // A quick filter, then close and reopen: everything comes back.
    await db.quickFilter("castling");
    expect(db.info.filter?.count === 2, "2 castling games");
    const columns = { ...db.columns };
    const widths = { white: width("white"), event: width("event") };
    await reopen();
    expect(db.sort.key === "white" && db.sort.descending, "sort restored: " + JSON.stringify(db.sort));
    expect(sameCols(db.columns, columns), "widths restored: " + JSON.stringify(db.columns));
    await until(() => near(width("white"), widths.white) && near(width("event"), widths.event), "the header has them");
    expect(db.info.filter?.count === 2 && db.quickText === "castling", "quick filter restored");
    expect(same((await ids()).sort(), [2, 4]), "the castling games: " + (await ids()));
    await until(() => status().includes("2 of 6"), "filter bar shows 2 of 6");
    await until(() => document.querySelector(".head-cell[data-column='white'] .arrow")?.textContent.includes("▼"), "sort arrow");
    await shot("v03-restored");

    // A header search within its results.
    db.quickText = "";
    await db.search({ result: "decisive" });
    await db.search({ white: "player" }, true);
    expect(db.info.filter?.count === 1, "one decisive game of player1");
    const refined = db.info.filter.description;
    db.lastQuery = {};
    await reopen();
    expect(db.info.filter?.description === refined && db.info.filter.count === 1, "refined search restored: " + JSON.stringify(db.info.filter));
    expect(db.quickText === "" && db.lastQuery.white === "player", "search dialog starts from the last search");

    // A player's games.
    const kasparov = (await api.listNames(db.info.handle, "player", "kasparov", true, 10)).rows[0];
    await db.showNameGames("player", kasparov.id);
    await reopen();
    expect(db.info.filter?.description === "Player: Kasparov, Garry" && db.info.filter.count === 1, "player filter restored");

    // A position: 1.e4.
    await game.newGame();
    await game.play("e2", "e4");
    await db.searchPosition(game.view.session);
    expect(db.info.filter?.count === 3, "3 games reach 1.e4");
    const position = db.info.filter.description;
    await game.newGame();
    await reopen();
    expect(db.info.filter?.description === position && db.info.filter.count === 3, "position filter restored: " + JSON.stringify(db.info.filter));
    // And a full search.
    await game.newGame("8/8/4k3/8/8/4K3/8/8 w - - 0 60");
    await db.searchPosition(game.view.session, true);
    expect(db.info.filter?.description.includes("full search"), "full search");
    await reopen();
    expect(db.info.filter?.description.includes("full search") && db.info.filter.count === 0, "full search restored");

    // Showing all games is remembered too.
    await db.clearFilter();
    await reopen();
    expect(!db.info.filter && db.sort.key === "white", "all games, still sorted");

    // Another database has its own view; the first one's comes back.
    await db.createAt(OUT + "/other.chessdb");
    expect(same(db.columns, {}) && db.sort.key === "id" && !db.info.filter && db.quickText === "", "the new database has the default view");
    await until(() => !near(width("white"), widths.white), "default White width");
    await db.openAt(path);
    expect(sameCols(db.columns, columns) && db.sort.key === "white", "view.chessdb's view is back");

    // A filter that can't be made again (the player was renamed) is dropped
    // with a notice, and not tried again.
    const a = (await api.listNames(db.info.handle, "player", "", false, 100)).rows.find((r) => r.name === "A");
    await db.showNameGames("player", a.id);
    expect(db.info.filter?.count === 1, "A's game");
    expect(await db.renameName("player", a.id, "Anderssen, Adolf"), "renamed A");
    await reopen();
    expect(!db.info.filter && db.quickText === "", "the filter couldn't be restored: all games");
    await until(() => document.querySelector(".notice")?.textContent.includes("“A” isn't in the database any more"),
      "a notice says why: " + document.querySelector(".notice")?.textContent);
    expect((await api.dbView(db.info.handle)).filter === null, "and it's forgotten");
    await shot("v04-not-restored");

    if (BIG) {
      await db.createAt(OUT + "/big.chessdb");
      await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
      await db.toggleSort("whiteElo");
      await db.quickFilter("magnus");
      const count = db.info.filter.count;
      const bigPath = db.info.path;
      // Else the index build still holds the database and the reopen reuses
      // it, sort order and all: time a real open from disk.
      await until(() => db.info.index.built && db.info.index.upToDate, "the index built", 180000);
      await db.close();
      const t = performance.now();
      await db.openAt(bigPath);
      await log(`reopen of 1M games with sort + quick filter restored: ${Math.round(performance.now() - t)} ms`);
      expect(db.info.filter?.count === count && db.sort.key === "whiteElo", "big view restored");
      await shot("v05-big-restored");
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
