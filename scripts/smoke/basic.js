// Smoke-test driver, evaluated inside the app's webview by the debug-only
// CHESSDB_SMOKE hook (see scripts/smoke/run.sh, which fills in __ROOT__,
// __OUT__ and __BIG__). It drives the real stores, so every step goes
// through the webview → IPC → Rust path, and it checks the results.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke } = window.__chessdb;
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
  const until = async (cond, what) => {
    for (let i = 0; i < 40 && !cond(); i++) await wait(50);
    expect(cond(), what);
  };
  const key = (el, k) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await shot("01-start");

    // Database + import (fixtures only, so this runs on a fresh checkout).
    await db.createAt(OUT + "/smoke.chessdb");
    for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    expect(db.info.games === 6, "6 imported games, got " + db.info.games);
    await shot("02-imported");

    // Sorting (unknown names last, case-insensitive).
    await db.toggleSort("white");
    expect(db.sort.key === "white", "sort by white");
    const page = await api.listGames(db.info.handle, db.sort, 0, 6);
    expect(page.rows[0].white === "A" && page.rows[1].white === "Kasparov, Garry",
      "A, then Kasparov first; got " + page.rows.map((r) => r.white).join(" | "));
    await shot("03-sorted-by-white");

    // Open Kasparov–Topalov (game 0) and step through it.
    await game.open(db.info.handle, 0);
    for (let i = 0; i < 16; i++) await game.step("forward");
    expect(game.view.fen.startsWith("r1bqk2r/p2npp1p/2pp1npb/1p6/3PP3/2N2P2/PPPQN1PP/R3KB1R w"),
      "position after 8...Bxh6, got " + game.view.fen);
    await shot("04-game-open");

    // Stepping forward into a split offers the next moves, main line first:
    // after 7...Nbd7 the game has 8.Bh6 and the variation 8.g4.
    const at16 = game.view.cursor;
    const picker = () => document.querySelector("[role='listbox'][aria-label='Next move']");
    await game.step("start");
    for (let i = 0; i < 14; i++) await game.forward();
    expect(game.view.cursorPath.length === 14 && !picker(), "no picker where the game doesn't split");
    game.forward();
    await until(() => picker() && document.activeElement === picker(), "next-move picker focused");
    const opts = [...picker().querySelectorAll("[role='option']")].map((o) => o.textContent);
    expect(opts.length === 2 && opts[0].endsWith("h6") && opts[1].endsWith("g4"),
      "8.Bh6 then 8.g4 offered, got " + opts.join(" | "));
    await shot("04a-next-moves");
    expect(document.querySelectorAll(".cg-shapes line").length >= 2, "the next moves drawn as arrows");
    key(picker(), "Escape");
    await until(() => !picker(), "Escape closes the picker");
    expect(game.view.cursorPath.length === 14, "Escape doesn't move");
    game.forward();
    await until(() => document.activeElement === picker(), "picker open again");
    key(picker(), "ArrowDown");
    key(picker(), "ArrowRight");
    await until(() => !picker() && game.view.inVariation, "Down then Right follows 8.g4");
    expect(game.view.cursorPath.length === 15, "one move forward into the variation");
    await game.goto(at16);

    // A different move where one exists asks for a policy.
    // It's a popover under the current move that leaves the board visible.
    await game.play("g2", "g4");
    expect(game.pendingChoice?.dest === "g4", "variation dialog");
    await wait(100);
    const pop = document.querySelector("[role='dialog'][aria-modal='true']").getBoundingClientRect();
    const cur = document.querySelector(".move.current").getBoundingClientRect();
    const board = document.querySelector(".board-square").getBoundingClientRect();
    expect(pop.top >= cur.bottom && pop.top - cur.bottom < 12, "popover just under the current move");
    expect(pop.left >= board.right, "popover clear of the board");
    expect(document.activeElement?.textContent === "New variation", "New variation focused (Enter picks it)");
    await shot("05-variation-dialog");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(!game.pendingChoice, "Escape cancels the variation popover");
    await game.play("g2", "g4");
    await wait(100);
    document.querySelector("cg-board").dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(!game.pendingChoice, "a click outside cancels the variation popover");
    await game.play("g2", "g4");
    await game.chooseVariation("newVariation");
    await game.play("e7", "e5");
    expect(game.view.inVariation && game.view.cursorPath.length === 18, "cursor in the new variation");

    // Annotate.
    await game.toggleNag(game.view.cursor, 5);

    // Comments are typed in place in the notation (no dialog); clicking away
    // saves, Escape cancels.
    const editor = () => document.querySelector(".notation [contenteditable]");
    game.editComment(game.view.cursor, "before");
    await until(() => document.activeElement === editor(), "inline comment editor focused");
    await shot("06a-empty-comment");
    document.execCommand("insertText", false, "not kept");
    editor().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await until(() => !editor(), "Escape closes the comment editor");
    expect(!document.querySelector(".notation").textContent.includes("not kept"), "Escape discards the text");

    game.editComment(game.view.cursor, "after");
    await until(() => document.activeElement === editor(), "inline comment editor focused");
    expect(!document.querySelector("[aria-modal='true']"), "no dialog for comments");
    document.execCommand("insertText", false, "Smoke test comment: sharp play.");
    await shot("06a-typing-comment");
    editor().blur();
    await until(
      () => !editor() && [...document.querySelectorAll(".notation .comment")].some((c) =>
        c.textContent === "Smoke test comment: sharp play."),
      "clicking away saves the comment into the notation",
    );
    document.querySelector(".move.current").dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, clientX: 600, clientY: 300 }),
    );
    await until(() => document.querySelector("[role='menu']"), "move menu open");
    const labels = [...document.querySelectorAll("[role='menu'] button")].map((b) => b.textContent);
    expect(labels.includes("Edit comment after move") && labels.includes("Add comment before move"),
      "menu offers to edit the existing comment, got " + labels.slice(0, 2).join(" | "));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));

    await game.setShapes([{ orig: "g4", dest: "g5", brush: "red" }, { orig: "f5", brush: "green" }]);
    expect(game.view.shapes.length === 2 && game.view.dirty, "two shapes and unsaved changes");
    await shot("06-annotated-variation");

    // Promote, undo, save, reopen.
    await game.promoteVariation(game.view.cursor);
    expect(!game.view.inVariation, "promoted to main line");
    await shot("07-promoted");
    await game.undo();
    expect(game.view.inVariation, "undo restores the variation");
    await game.save();
    expect(!game.view.dirty, "saved");
    await game.open(db.info.handle, 0);
    const pgn = await api.gamePgn(game.view.session);
    expect(pgn.includes("(9. g4 e5 $5 {[%csl Gf5][%cal Rg4g5] Smoke test comment"),
      "saved variation with annotations in:\n" + pgn.slice(0, 800));
    for (let i = 0; i < 17; i++) await game.step("forward");
    await shot("08-reopened-after-save");

    // Insert replaces a move and keeps the moves after it while legal (GAME-03).
    await game.newGame();
    for (const [o, d] of [["e2", "e4"], ["e7", "e5"], ["g1", "f3"], ["b8", "c6"]]) await game.play(o, d);
    await game.step("back");
    await game.step("back");
    await game.play("b1", "c3");
    expect(game.pendingChoice?.dest === "c3", "variation dialog for 2.Nc3");
    await wait(100);
    const choices = [...document.querySelectorAll("[role='dialog'][aria-modal='true'] button")]
      .map((b) => b.textContent);
    expect(choices.includes("Insert"), "Insert offered, got " + choices.join(" | "));
    await game.chooseVariation("insert");
    const inserted = await api.gamePgn(game.view.session);
    expect(inserted.includes("1. e4 e5 2. Nc3 Nc6 *"), "2.Nf3 replaced, 2...Nc6 kept:\n" + inserted);

    // The intro comment (before the first move) is edited in place too.
    game.editComment(0, "after");
    await until(() => document.activeElement === editor() && editor().closest(".intro"), "intro editor");
    document.execCommand("insertText", false, "Vienna-style.");
    editor().blur();
    await until(() => document.querySelector(".notation .intro")?.textContent.trim() === "Vienna-style.",
      "intro comment shown");
    expect((await api.gamePgn(game.view.session)).includes("{Vienna-style.} 1. e4"), "intro in the PGN");

    // Promotion picker and underpromotion; K+N vs K is a dead draw.
    await game.newGame("8/4P3/8/8/8/8/k7/4K3 w - - 0 1");
    await game.play("e7", "e8");
    expect(game.pendingPromotion?.dest === "e8", "promotion picker");
    await shot("09-promotion-picker");
    await game.choosePromotion("n");
    expect(game.view.fen.startsWith("4N3/"), "knight on e8");
    expect(game.view.status === "insufficientMaterial", "insufficient material");

    if (BIG) {
      await db.createAt(OUT + "/big.chessdb");
      const importing = db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
      await wait(8000);
      await log("games after 8s of import: " + db.info.games);
      await shot("10-import-in-progress");
      const t0 = performance.now();
      await importing;
      expect(db.info.games === 1000000, "1M games, got " + db.info.games);
      await log("1M import done " + Math.round((performance.now() - t0) / 1000) + "s after the screenshot");

      const vp = document.querySelector(".viewport");
      vp.focus();
      vp.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
      await wait(1200);
      const last = document.querySelector(".row.selected .cell")?.textContent;
      expect(last === "1000000", "last row selected, got " + last);
      await shot("11-scrolled-to-last-row");

      const t1 = performance.now();
      await db.setSort({ key: "blackElo", descending: true });
      expect(db.sort.key === "blackElo", "sort by black Elo");
      await log("sort by black Elo took " + Math.round(performance.now() - t1) + "ms");
      await shot("12-sorted-black-elo-desc");
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
