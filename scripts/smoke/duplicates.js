// Smoke-test driver for duplicate games (DB-08): the ribbon's Find
// duplicates… button and its dialog, exact then fuzzy copies of
// testdata/pgn/duplicates.pgn plus a file imported twice, showing the groups
// in the game list (each kept game first) and that filter coming back after
// a reload, deleting the copies, Escape, restoring them, the report going
// stale after a compaction, finding again, and the dialog in light mode.
// With the 1M-game file present it times a search over it, imports it again
// and times finding, showing, deleting and restoring a million copies.
//
// The ribbon and the list are an editor's (DB-07): in the library window
// the driver only opens an editor window, which runs the same script. An
// editor doesn't reopen its database after a reload, so the driver does.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke, appearance } = window.__chessdb;
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
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  const PHASE = "duplicates-phase";
  const phase = sessionStorage.getItem(PHASE) || "start";

  if (document.querySelector(".home")) {
    await log("library window: opening an editor window for the checks");
    await api.openEditorWindow(null);
    return;
  }

  // The live ribbon, not its hidden copy (app/AGENTS.md).
  const ribbonButton = (label) => document.querySelector(`.ribbon > .groups button[aria-label="${label}"]`);
  const dialog = () => document.querySelector('[role="dialog"][aria-label="Find duplicate games"]');
  const button = (text) =>
    [...(dialog()?.querySelectorAll(".actions button") ?? [])].find((b) => b.textContent.trim().startsWith(text));
  const summary = () => dialog()?.querySelector(".summary")?.textContent.replace(/\s+/g, " ").trim() ?? "";
  const radios = () => [...(dialog()?.querySelectorAll('input[name="duplicate-kind"]') ?? [])];
  const listIds = async (n) => (await api.listGames(db.info.handle, db.sort, 0, n)).rows.map((r) => r.id);
  async function openDialog() {
    const b = ribbonButton("Find duplicates…");
    expect(b && !b.disabled, "the ribbon's Find duplicates… button, enabled");
    b.click();
    await until(() => dialog(), "the dialog open");
    // It loads the last report of this database.
    await wait(300);
  }
  async function find(fuzzy, what, ms = 10000) {
    radios()[fuzzy ? 1 : 0].click();
    const go = button("Find");
    expect(go && !go.disabled, "a Find button");
    go.click();
    await until(() => !dialog().querySelector(".progress") && summary().includes(what), `“${what}”, got “${summary()}”`, ms);
  }

  try {
    await log("start, phase=" + phase);

    if (phase === "start") {
      await db.createAt(OUT + "/dups.chessdb");
      // 0–8: the fixture (copies of 0 in 1 and 2, of 4 in 5 and 6); 9–10
      // and 11–12: variations.pgn twice.
      await db.importFrom(ROOT + "/testdata/pgn/duplicates.pgn");
      await db.importFrom(ROOT + "/testdata/pgn/variations.pgn");
      await db.importFrom(ROOT + "/testdata/pgn/variations.pgn");
      expect(db.info.games === 13, "13 games, got " + db.info.games);
      await shot("d01-imported");

      await openDialog();
      expect(radios().length === 2 && radios()[0].checked, "exact copies chosen at first");
      expect(button("Find duplicates")?.classList.contains("primary"), "Find duplicates is the main action");
      await shot("d02-dialog");

      // Game 1 lacks game 0's ratings: a fuzzy copy only.
      await find(false, "4 copies to delete, in 3 groups");
      expect(dialog().querySelectorAll("tbody tr").length === 3, "3 groups previewed");
      expect(!dialog().querySelector(".tag"), "no fuzzy groups");
      const first = dialog().querySelector("tbody tr td.players").textContent;
      expect(first.includes("Berg, Anna") && first.includes("Holm, Sara"), "the first kept game: " + first);
      await shot("d03-exact");

      await find(true, "6 copies to delete, in 4 groups");
      expect(dialog().querySelectorAll("tbody .tag").length === 1, "one group with fuzzy copies");
      expect(dialog().textContent.includes("3 groups of exact copies, 1 with fuzzy ones"), "the split by kind");
      await shot("d04-fuzzy");

      button("Show in game list").click();
      await until(() => !dialog() && db.info.filter?.count === 10, "the list filtered to the 10 games of the groups");
      expect(same(await listIds(20), [0, 1, 2, 4, 5, 6, 9, 11, 10, 12]), "each kept game, then its copies");
      expect(db.info.filter.description === "Duplicates (fuzzy too): 4 groups, kept game first",
        "the filter's description: " + db.info.filter.description);
      await until(() => document.querySelector(".bar .status")?.textContent.includes("10 of 13"), "the filter bar: 10 of 13");
      await shot("d05-list");

      // The filter is saved with the database's list (QOL-07). Closing the
      // database drops it (and this search) in Rust, so it has to be made
      // again when the database is opened after the reload.
      await db.close();
      sessionStorage.setItem(PHASE, "reloaded");
      await wait(500);
      location.reload();
      return;
    }

    if (phase === "reloaded") {
      sessionStorage.removeItem(PHASE);
      await db.openAt(OUT + "/dups.chessdb");
      await until(() => db.info?.filter?.count === 10, "the duplicates filter made again after a reload", 10000);
      expect(same(await listIds(20), [0, 1, 2, 4, 5, 6, 9, 11, 10, 12]), "the same order after the reload");
      await shot("d06-filter-restored");

      // A database opened again has no report yet; fuzzy is still chosen.
      await openDialog();
      expect(!summary() && button("Find duplicates"), "no report after reopening");
      expect(radios()[1].checked, "fuzzy copies chosen, as last time");
      await find(true, "6 copies to delete, in 4 groups");
      button("Delete 6 copies").click();
      await until(() => db.info.deleted === 6 && button("Restore 6 copies"), "6 copies marked as deleted");
      expect(dialog().querySelector(".done")?.textContent.includes("Marked 6 copies as deleted"), "says what it did");
      const rows = (await api.listGames(db.info.handle, db.sort, 0, 20)).rows;
      expect(same(rows.filter((r) => r.deleted).map((r) => r.id), [1, 2, 5, 6, 11, 12]), "the copies deleted, not the kept games");
      await shot("d07-deleted");

      // Escape closes it; the list shows the copies struck through.
      dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await until(() => !dialog(), "Escape closes the dialog");
      await until(() => document.querySelectorAll(".viewport .row.deleted").length > 0, "rows struck through");
      await log(`struck through: ${document.querySelectorAll(".viewport .row.deleted").length} rows shown`);
      await shot("d08-list-deleted");

      await openDialog();
      button("Restore 6 copies").click();
      await until(() => db.info.deleted === 0 && button("Delete 6 copies"), "the copies restored");
      await shot("d09-restored");

      // A compaction renumbers the games: the report goes stale.
      button("Delete 6 copies").click();
      await until(() => db.info.deleted === 6, "deleted again");
      button("Close").click();
      await until(() => !dialog(), "closed");
      await db.compact();
      expect(db.info.games === 7 && !db.info.filter, "compacted to 7 games, the filter cleared");
      await openDialog();
      expect(dialog().querySelector(".warn")?.textContent.includes("compacted"), "the stale report says so");
      expect(!button("Delete") && !button("Show in game list"), "nothing to act on");
      await shot("d10-stale");
      await find(true, "No exact or fuzzy copies among 7 games");
      await shot("d11-none");
      button("Close").click();

      // Light mode.
      await db.importFrom(ROOT + "/testdata/pgn/duplicates.pgn");
      appearance.theme = "light";
      await openDialog();
      await find(true, "9 copies to delete, in 5 groups");
      await shot("d12-light");
      button("Close").click();
      appearance.theme = "system";

      if (BIG) {
        await db.createAt(OUT + "/big.chessdb");
        let t = performance.now();
        await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
        await log(`imported ${db.info.games} games in ${Math.round(performance.now() - t)} ms`);
        await openDialog();
        t = performance.now();
        await find(true, "No exact or fuzzy copies among 1,000,000 games", 120000);
        await log(`fuzzy search over 1M games: ${Math.round(performance.now() - t)} ms`);
        await shot("d13-big-none");
        button("Close").click();

        t = performance.now();
        await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
        await log(`imported again: ${db.info.games} games in ${Math.round(performance.now() - t)} ms`);
        await openDialog();
        t = performance.now();
        await find(true, "1,000,000 copies to delete, in 1,000,000 groups", 300000);
        await log(`fuzzy search over 2M games: ${Math.round(performance.now() - t)} ms; ${summary()}`);
        await shot("d14-big-twice");

        t = performance.now();
        button("Show in game list").click();
        await until(() => !dialog() && db.info.filter?.count === 2000000, "2M games listed", 60000);
        await log(`showing the groups: ${Math.round(performance.now() - t)} ms`);
        expect(same(await listIds(4), [0, 1000000, 1, 1000001]), "each game, then its copy");
        await shot("d15-big-list");

        await openDialog();
        t = performance.now();
        button("Delete 1,000,000 copies").click();
        await until(() => db.info.deleted === 1000000 && button("Restore"), "1M copies deleted", 120000);
        await log(`deleting 1M copies: ${Math.round(performance.now() - t)} ms`);
        await shot("d16-big-deleted");
        t = performance.now();
        button("Restore 1,000,000 copies").click();
        await until(() => db.info.deleted === 0 && button("Delete"), "1M copies restored", 120000);
        await log(`restoring 1M copies: ${Math.round(performance.now() - t)} ms`);
        button("Close").click();
      }

      await log("done");
    }
  } catch (e) {
    sessionStorage.removeItem(PHASE);
    await log("FAILED: " + (e?.message ? `${e.message}\n${e.stack}` : JSON.stringify(e)));
  }
})();
