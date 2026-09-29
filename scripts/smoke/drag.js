// Smoke-test driver for dragging games out of the list (QOL-04) and
// copying or moving them between databases (DB-06): selecting several rows
// (click, Shift+click, Ctrl+click, Shift+arrows, Ctrl+A), dragging them onto
// another database (Shift moves), onto the board and the tab strip, Escape
// cancelling, the list menu's copy dialog, copying into a new database and
// into the reference database. The drags are pointer events at the
// elements' real positions, so hit testing, the drag threshold and the
// modifier keys go through the same code as a mouse. With the 1M-game file
// present it also times copying all of it.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden display: never ask about unsaved changes.
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
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  // Rows by their # column (id + 1; the list is in id order here).
  const row = (n) =>
    [...document.querySelectorAll(".viewport .row")].find((r) => r.querySelector(".cell")?.textContent === String(n));
  const picked = () => [...document.querySelectorAll(".viewport .row.selected")].map((r) => r.querySelector(".cell").textContent).join(",");
  const center = (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  };
  const pointer = (type, x, y, mods = {}) => {
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
        ...mods,
      }),
    );
  };
  const click = async (el, mods) => {
    const p = center(el);
    pointer("pointerdown", p.x, p.y, mods);
    pointer("pointerup", p.x, p.y, mods);
    await wait(60);
  };
  /**
   * Presses on `from`, drags past the threshold and on to `to` (not
   * released). `mods` (Shift to move) are held while dragging, not on the
   * press, where Shift would change the selection.
   */
  const dragTo = async (from, to, mods = {}) => {
    const a = center(from);
    pointer("pointerdown", a.x, a.y);
    pointer("pointermove", a.x + 4, a.y - 3, mods);
    expect(!document.querySelector(".shelf"), "no drag before the pointer moves 6 px");
    pointer("pointermove", a.x + 12, a.y - 12, mods);
    await until(() => document.querySelector(".shelf"), "the drop shelf shows once dragging");
    const target = typeof to === "function" ? to() : to;
    expect(target, "a drop target");
    const b = center(target);
    pointer("pointermove", b.x, b.y, mods);
    await wait(60);
    return b;
  };
  const shelfTarget = (name) =>
    [...document.querySelectorAll(".shelf [data-drop='db']")].find((t) => t.dataset.name === name);
  const hint = () => document.querySelector(".ghost span")?.textContent.trim();
  const notice = () => document.querySelector("footer .notice")?.textContent.trim() ?? "";
  const key = (el, k, mods = {}) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...mods }));
  const pgnOf = async (handle, id) => {
    const view = await api.openGame(handle, id);
    const pgn = await api.gamePgn(view.session);
    await api.closeSession(view.session);
    return pgn;
  };

  try {
    await log("start");
    await db.createAt(OUT + "/target.chessdb");
    await db.createAt(OUT + "/source.chessdb");
    for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    expect(db.info.games === 6, "6 games in source, got " + db.info.games);
    const source = db.info.handle;
    const originals = [];
    for (let id = 0; id < 6; id++) originals.push(await pgnOf(source, id));
    await db.loadRecent();
    expect(db.copyTargets.some((t) => t.name === "target") && !db.copyTargets.some((t) => t.name === "source"),
      "target is a copy target, the open database isn't: " + db.copyTargets.map((t) => t.name));
    await until(() => row(6), "six rows listed");

    // Selecting several rows.
    await click(row(1));
    await click(row(3), { shiftKey: true });
    expect(picked() === "1,2,3", "Shift+click selects 1-3, got " + picked());
    await click(row(2), { ctrlKey: true });
    expect(picked() === "1,3", "Ctrl+click drops 2, got " + picked());
    await click(row(2), { ctrlKey: true });
    expect(picked() === "1,2,3", "Ctrl+click adds 2 back, got " + picked());
    // Ctrl+click moved the anchor to row 2.
    const viewport = document.querySelector(".viewport");
    viewport.focus();
    key(viewport, "ArrowDown", { shiftKey: true });
    key(viewport, "ArrowDown", { shiftKey: true });
    await wait(60);
    expect(picked() === "2,3,4", "Shift+Down extends from the anchor, got " + picked());
    key(viewport, "a", { ctrlKey: true });
    await wait(60);
    expect(picked() === "1,2,3,4,5,6", "Ctrl+A selects all, got " + picked());
    await click(row(3));
    expect(picked() === "3", "a plain click selects one row, got " + picked());
    await click(row(1));
    await click(row(3), { shiftKey: true });

    // Drag three games onto the other database: copied.
    let at = await dragTo(row(2), () => shelfTarget("target"));
    expect(picked() === "1,2,3", "pressing a selected row keeps the selection for the drag");
    expect(document.querySelector(".ghost strong")?.textContent === "3 games", "the label says 3 games");
    expect(hint() === "Copy to target", "the hint says where it goes, got " + hint());
    expect(shelfTarget("target").classList.contains("over"), "the target under the pointer is highlighted");
    await shot("d01-drag-to-database");
    pointer("pointerup", at.x, at.y);
    await until(() => notice() === "Copied 3 games to target", "copied notice, got " + notice(), 8000);
    expect(!document.querySelector(".shelf") && !document.body.classList.contains("dragging-games"), "drag over");

    // Shift moves: the originals are marked as deleted.
    await click(row(4));
    await click(row(5), { shiftKey: true });
    at = await dragTo(row(4), () => shelfTarget("target"), { shiftKey: true });
    expect(hint() === "Move to target", "Shift moves, got " + hint());
    expect(document.querySelector(".shelf .title")?.textContent.startsWith("Move 2 games"), "the shelf says move");
    await shot("d02-move-with-shift");
    pointer("pointerup", at.x, at.y, { shiftKey: true });
    await until(() => notice().startsWith("Moved 2 games to target"), "moved notice, got " + notice(), 8000);
    await until(() => row(4)?.classList.contains("deleted") && row(5)?.classList.contains("deleted"), "originals struck through");
    expect(db.info.deleted === 2, "2 deleted in source, got " + db.info.deleted);

    // Deleted games aren't copied.
    await click(row(3));
    await click(row(5), { shiftKey: true });
    at = await dragTo(row(3), () => shelfTarget("target"));
    pointer("pointerup", at.x, at.y);
    await until(() => notice() === "Copied 1 game to target (skipped 2 games marked as deleted)",
      "deleted games skipped, got " + notice(), 8000);

    // Escape cancels a drag: nothing is copied.
    await click(row(1));
    at = await dragTo(row(1), () => shelfTarget("target"));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await until(() => !document.querySelector(".shelf"), "Escape ends the drag");
    pointer("pointerup", at.x, at.y);
    await wait(500);
    expect(notice().startsWith("Copied 1 game to target (skipped"), "no copy after Escape, got " + notice());

    // Onto the board: the game opens.
    await click(row(6));
    at = await dragTo(row(6), () => document.querySelector("[data-drop='board']"));
    expect(document.querySelector("[data-drop='board']").classList.contains("drop-over"), "board highlighted");
    expect(hint() === "Open on the board", "board hint, got " + hint());
    await shot("d03-drag-to-board");
    pointer("pointerup", at.x, at.y);
    await until(() => game.view?.source?.game === 5 && game.view.source.db === source, "game 6 opened on the board");

    // Onto the tab strip: each game in a new tab, the first one shown.
    const tabs = game.tabs.length;
    await click(row(1));
    await click(row(2), { shiftKey: true });
    at = await dragTo(row(1), () => document.querySelector("[data-drop='tabs']"));
    expect(hint() === "Open 2 games in new tabs", "tabs hint, got " + hint());
    pointer("pointerup", at.x, at.y);
    await until(() => game.tabs.length === tabs + 2, "two new tabs");
    expect(game.view.source.game === 0, "the first dropped game is on screen");

    // The list menu copies too (and is the keyboard way): Copy 2 games to database…
    row(1).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: center(row(1)).y }));
    expect(picked() === "1,2", "the selection survives opening its games, got " + picked());
    await until(() => document.querySelector("[role='menu']"), "list menu open");
    const items = [...document.querySelectorAll("[role='menu'] [role='menuitem']")];
    const copyItem = items.find((b) => b.textContent.trim() === "Copy 2 games to database…");
    expect(copyItem, "menu offers to copy the 2 selected games: " + items.map((b) => b.textContent.trim()).join(" | "));
    expect(document.activeElement === items[0], "the menu takes focus");
    copyItem.click();
    await until(() => document.querySelector("[role='dialog'] .target"), "copy dialog lists databases");
    await shot("d04-copy-dialog");
    [...document.querySelectorAll("[role='dialog'] .target")].find((b) => b.textContent.includes("target")).click();
    await until(() => notice() === "Copied 2 games to target", "copied from the dialog, got " + notice(), 8000);
    viewport.focus();
    key(viewport, "F10", { shiftKey: true });
    await until(() => document.activeElement?.getAttribute("role") === "menuitem", "Shift+F10 opens the menu, focused");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await until(() => !document.querySelector("[role='menu']") && document.activeElement === viewport, "Escape closes it");

    // Into a new database (the dialog's path given directly): listed second
    // among the recent ones, so the open database is still reopened first.
    let done = await db.copyRows([[0, 6]], OUT + "/clip", "copy", true);
    expect(done?.message === "Copied 4 games to clip (skipped 2 games marked as deleted)", "copied to a new database: " + done?.message);
    const recent = await api.recentDatabases();
    expect(recent[0].name === "source" && recent[1].name === "clip", "clip remembered second: " + recent.map((r) => r.name));
    // Copying into the same database is refused.
    const same = await api.copyGames(source, { sort: db.sort, ranges: [[0, 1]] }, db.info.path, false, "copy").then(
      () => "",
      (e) => e.message,
    );
    expect(same.includes("already in source"), "no copy into itself: " + same);

    // Into the reference database: it's refreshed afterwards.
    await db.setReference(OUT + "/clip.chessdb");
    await until(() => db.reference?.info?.games === 4, "clip is the reference with 4 games");
    await click(row(6));
    at = await dragTo(row(6), () => shelfTarget("clip"));
    expect(document.querySelector(".shelf .tag")?.textContent === "reference", "the reference is marked");
    pointer("pointerup", at.x, at.y);
    await until(() => db.reference?.info?.games === 5, "reference refreshed: " + db.reference?.info?.games, 8000);

    // The copies are the games, exactly; moves too.
    await db.openAt(OUT + "/target.chessdb");
    expect(db.info.games === 8, "8 games in target, got " + db.info.games);
    const expected = [0, 1, 2, 3, 4, 2, 0, 1];
    for (const [copy, id] of expected.entries()) {
      expect((await pgnOf(db.info.handle, copy)) === originals[id], `target game ${copy + 1} is source game ${id + 1}`);
    }
    await until(() => row(8), "target lists 8 games");
    await click(row(1));
    await click(row(8), { shiftKey: true });
    await shot("d05-target");

    if (BIG) {
      await db.createAt(OUT + "/big.chessdb");
      await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
      const n = db.info.games;
      let t = performance.now();
      done = await db.copyRows([[0, n]], OUT + "/bigcopy", "copy", true);
      await log(`copying ${n} games to a new database: ${Math.round(performance.now() - t)} ms (${done?.message})`);
      expect(done?.state === "done", "big copy done");
      await db.openAt(OUT + "/bigcopy.chessdb");
      expect(db.info.games === n, "all games copied");
      const a = await pgnOf(db.info.handle, n - 1);
      await db.openAt(OUT + "/big.chessdb");
      expect(a === (await pgnOf(db.info.handle, n - 1)), "the last game copied exactly");
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
