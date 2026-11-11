// Smoke-test driver for saving a game from a bare board (DB-01): with no
// database open, the Reference tab reads a reference database attached
// afterwards, Save as new asks where to save, and the game goes into any
// database: the reference one as it is, another one opened for it, a new
// one; Save then replaces in place, also in the reference database.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, panels, reference, notices, api, invoke } = window.__chessdb;
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
  const untilAsync = async (cond, what, ms = 5000) => {
    const end = performance.now() + ms;
    while (performance.now() < end) {
      if (await cond()) return;
      await wait(100);
    }
    expect(await cond(), what);
  };
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const text = (el) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();
  const shown = (el) => !!el && el.getClientRects().length > 0;
  const key = (el, k, opts = {}) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...opts }));
  const pointer = (type, x, y) => {
    const el = document.elementFromPoint(x, y) ?? document.body;
    el.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        composed: true,
        clientX: x,
        clientY: y,
        pointerId: 1,
        pointerType: "mouse",
        isPrimary: true,
        button: type === "pointermove" ? -1 : 0,
        buttons: type === "pointerup" ? 0 : 1,
      }),
    );
  };
  const rb = (label) => $$(".ribbon > .groups button").find((b) => b.getAttribute("aria-label") === label);
  const dialog = () => $("[role='dialog']");
  const targets = () => $$("[role='dialog'] button.target").map((b) => text($(".name strong", b)));
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  // The board is an editor window's (DB-07): the library window opens one,
  // with no database, and the checks run there.
  if ($(".home")) {
    await log("library window: opening a board from its button for the checks");
    const button = $$(".home header button").find((b) => text(b) === "Open board");
    expect(button, "an Open board button in the library");
    button.click();
    return;
  }

  try {
    await log("start");
    // A reference database with games, made then closed: only the board is left.
    await db.createAt(OUT + "/ref.chessdb");
    await db.importFrom(ROOT + "/testdata/pgn/reference.pgn");
    await until(() => db.info?.index.built && db.info.index.upToDate, "the reference indexed", 20000);
    await db.close();
    if (!game.view) await game.newGame();
    expect(!db.info, "no database open");

    // ---- A bare board: save asks where, with nowhere yet to list.
    await game.play("e2", "e4");
    await game.play("e7", "e5");
    await game.save();
    await until(() => shown(dialog()), "Save asking where to save with no database open");
    expect(game.choosingSave, "the dialog flag set");
    expect(targets().length >= 1, "the recent databases listed, got " + targets().join());
    await shot("save-dialog");
    $("[role='dialog'] button:not(.target)").click(); // Cancel
    await until(() => !dialog(), "Cancel closing the dialog");
    expect(game.view.dirty, "the game still unsaved");
    expect(!game.view.source, "no source after cancelling");

    // ---- Attach the reference afterwards: the tab reads it.
    await db.setReference(OUT + "/ref.chessdb");
    await until(() => db.reference?.info, "the reference database open");
    rb("Reference").click();
    await until(() => shown($("#panel-reference")), "the Reference tab showing");
    await until(() => reference.target?.reference && reference.current, "the tab reading the reference database");
    expect(text($("#panel-reference header .name")).startsWith("ref"), "the header naming it");

    // ---- Save into the reference database as it is (no list change).
    const before = db.reference.info.games;
    await game.saveInto(db.reference.path);
    await until(() => game.view.source && !game.view.dirty, "the game saved");
    expect(!db.info, "the list still empty");
    expect(game.view.source.db === db.reference.info.handle, "its source the reference database");
    await until(() => db.reference.info.games === before + 1, "the reference with one game more, got " + db.reference.info.games);

    // ---- Save again replaces in place (reference counts as open).
    await game.play("g1", "f3");
    await game.save();
    await until(() => !game.view.dirty, "the second save done");
    expect(db.reference.info.games === before + 1, "replaced, not appended, got " + db.reference.info.games);

    // ---- Another database: a new one through saveInto(create), shown after.
    await game.saveInto(OUT + "/other.chessdb", true);
    await until(() => db.info?.name === "other" && db.info.games === 1, "the new database open with the game");
    expect(game.view.source.db === db.info.handle, "its source the new database");
    expect(!game.view.dirty, "saved");

    // ---- Save as new always asks, also with a database open, listing it first.
    await game.saveAsNew();
    await until(() => shown(dialog()), "the dialog from Save as new with a database open");
    expect(db.info.games === 1, "nothing saved before choosing, got " + db.info.games);
    $$("[role='dialog'] button.target").find((b) => text($(".name strong", b)) === "other").click();
    await until(() => db.info.games === 2, "the open database picked: one game more");
    // The ribbon's button asks again, so another database can follow.
    rb("Save as new…").click();
    await until(() => shown(dialog()), "the dialog from the ribbon button");
    const names = targets();
    expect(names[0] === "other" && names.includes("ref"), "the open one first, then ref; got " + names.join());
    await shot("save-dialog-open");
    // Pick the reference: saved there with the list left alone.
    $$("[role='dialog'] button.target").find((b) => text($(".name strong", b)) === "ref").click();
    await until(() => game.view.source?.db === db.reference.info.handle && !game.view.dirty, "saved into ref");
    expect(db.info.name === "other", "the list still on other");
    await until(() => db.reference.info.games === before + 2, "ref with another game, got " + db.reference.info.games);

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e));
  }
})();
