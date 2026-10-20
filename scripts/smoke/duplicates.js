// Smoke-test driver for duplicate games (DB-08): the ribbon's Find
// duplicates… button and its dialog, exact then fuzzy copies of
// testdata/pgn/duplicates.pgn plus a file imported twice, showing the groups
// in the game list (each kept game first) and that filter not coming back
// after a reload (it isn't saved), and from PR #16's review: a report the
// database changed under refused (A: the kept game deleted in the list), an
// action on a report another search replaced refused (B: as from a second
// window), Find again keeping Restore (C); then deleting the copies with the
// kept game 0 open in a tab (its group is left as it is, and the dialog and
// the job say so, R5-1), closing the tab and deleting the rest (game 0 first
// takes its copy's board: round 1 becomes 1.3, R4-1), Escape,
// restoring them (round 1 again), the report going stale after a compaction,
// finding again, and the dialog in light mode. Deleting and restoring are
// jobs.
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
  const { db, game, api, invoke, appearance, jobs, notices } = window.__chessdb;
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
  const warn = () => dialog()?.querySelector(".warn")?.textContent.replace(/\s+/g, " ").trim() ?? "";
  const done = () => dialog()?.querySelector(".done")?.textContent.replace(/\s+/g, " ").trim() ?? "";
  const report = () => api.duplicatesReport(db.info.handle);
  // Waits for an async condition.
  const untilAsync = async (cond, what, ms = 10000) => {
    const end = performance.now() + ms;
    while (!(await cond()) && performance.now() < end) await wait(100);
    expect(await cond(), what);
  };
  async function find(fuzzy, what, ms = 10000) {
    radios()[fuzzy ? 1 : 0].click();
    const go = button("Find");
    expect(go && !go.disabled, "a Find button");
    const before = (await report())?.scan ?? 0;
    go.click();
    // A new report, then the dialog showing it (the old one may read alike).
    await untilAsync(async () => ((await report())?.scan ?? 0) > before, "a new report", ms);
    await until(() => !dialog().querySelector(".progress") && summary().includes(what), `“${what}”, got “${summary()}”`, ms);
  }
  const deletedIds = async () => (await api.listGames(db.info.handle, db.sort, 0, 50)).rows.filter((r) => r.deleted).map((r) => r.id);
  const roundOf = async (id) => (await api.listGames(db.info.handle, db.sort, 0, 50)).rows.find((r) => r.id === id)?.round;

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
      expect(db.info.filter.description === "Duplicates (exact and fuzzy): 4 groups",
        "the filter's description: " + db.info.filter.description);
      await until(() => document.querySelector(".bar .status")?.textContent.includes("10 of 13"), "the filter bar: 10 of 13");
      await shot("d05-list");
      // R1-6: not saved, so an older version can read the settings and
      // reopening doesn't search again.
      expect((await api.dbView(db.info.handle)).filter === null, "the duplicates filter isn't saved");

      // Closing the database drops the filter and the searches in Rust.
      await db.close();
      sessionStorage.setItem(PHASE, "reloaded");
      await wait(500);
      location.reload();
      return;
    }

    if (phase === "reloaded") {
      sessionStorage.removeItem(PHASE);
      await db.openAt(OUT + "/dups.chessdb");
      await wait(1500);
      expect(!db.info.filter && !db.restoring, "no filter after reopening");
      const handle = db.info.handle;
      await shot("d06-reopened");

      // A database opened again has no report yet; fuzzy is still chosen.
      await openDialog();
      expect(!summary() && button("Find duplicates"), "no report after reopening");
      expect(radios()[1].checked, "fuzzy copies chosen, as last time");

      // A (R1-4): the kept game deleted in the list after the search.
      await find(false, "4 copies to delete, in 3 groups");
      button("Close").click();
      await until(() => !dialog(), "closed");
      await db.setDeleted([4], true);
      await openDialog();
      expect(warn().includes("changed after the last search"), "the report says the database changed: " + warn());
      expect(!button("Delete") && !button("Show in game list"), "nothing to act on");
      await shot("d07-changed");
      await find(false, "3 copies to delete, in 3 groups");
      button("Close").click();
      await until(() => !dialog(), "closed");
      await db.setDeleted([4], false);

      // B (R1-5): another search replaced the report on screen, as one in a
      // second window on this database would.
      await openDialog();
      await find(true, "6 copies to delete, in 4 groups");
      await jobs.waitFor(await api.findDuplicates(handle, false));
      expect(summary().includes("6 copies to delete"), "the dialog still shows the fuzzy report");
      button("Delete 6 copies").click();
      await until(() => notices.current?.text.includes("searched for again"), "the delete refused: " + notices.current?.text);
      await until(() => summary().includes("4 copies to delete, in 3 groups"), "the new report shown: " + summary());
      expect(db.info.deleted === 0 && (await deletedIds()).length === 0, "nothing deleted");
      await shot("d08-replaced");

      await find(true, "6 copies to delete, in 4 groups");
      // R5-1: the kept game 0 open in a tab. Its group is left as it is:
      // saving the tab would undo what it takes from its copies.
      await game.open(handle, 0);
      const tab = game.activeSession;
      expect(game.active?.view.source?.game === 0, "game 0 open in a tab");
      button("Delete 6 copies").click();
      await until(() => db.info.deleted === 4 && button("Delete 2 copies"), "4 of 6 copies deleted: " + summary());
      expect(warn().includes("1 group was left as it is, as its kept game is open in a tab"), "says why: " + warn());
      expect(summary().includes("4 of 6 copies marked as deleted, in 4 groups"), "the report: " + summary());
      await until(() => notices.current?.text.includes("1 group left as it is, as its kept game is open in a tab"),
        "the job says so: " + notices.current?.text);
      expect(same(await deletedIds(), [5, 6, 11, 12]), "the other groups' copies deleted");
      expect((await roundOf(0)) === "1", "game 0 not filled in, got " + (await roundOf(0)));
      await shot("d09a-open-in-a-tab");
      await game.closeTab(tab);
      expect(!game.tabs.some((t) => t.view.source?.game === 0), "its tab closed");
      button("Delete 2 copies").click();
      await until(() => db.info.deleted === 6 && button("Restore 6 copies"), "6 copies marked as deleted");
      expect(!warn(), "nothing left: " + warn());
      expect(done().includes("Marked 6 copies as deleted, after filling in what 1 kept game lacked from them"),
        "says what it did: " + done());
      expect(summary().includes("6 copies marked as deleted, in 4 groups"), "the report: " + summary());
      expect(same(await deletedIds(), [1, 2, 5, 6, 11, 12]), "the copies deleted, not the kept games");
      // R4-1: the kept game took the board of its copy that stops early.
      expect((await roundOf(0)) === "1.3", "game 0 took round 1.3, got " + (await roundOf(0)));
      await shot("d09-deleted");

      // Escape closes it; the list shows the copies struck through.
      dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await until(() => !dialog(), "Escape closes the dialog");
      await until(() => document.querySelectorAll(".viewport .row.deleted").length > 0, "rows struck through");
      await log(`struck through: ${document.querySelectorAll(".viewport .row.deleted").length} rows shown`);
      await shot("d10-list-deleted");

      // C (R1-7): Find again after deleting still offers Restore.
      await openDialog();
      await find(true, "No exact or fuzzy copies among 7 games");
      expect(button("Restore 6 copies") && done().includes("Marked 6 copies as deleted"), "Restore still offered");
      await shot("d11-find-again");
      button("Restore 6 copies").click();
      await until(() => db.info.deleted === 0 && !button("Restore"), "the copies restored");
      expect((await roundOf(0)) === "1", "the fill taken out too, got " + (await roundOf(0)));
      // The report lacked them: it says so.
      await until(() => warn().includes("changed after the last search"), "the report out of date: " + warn());
      await find(true, "6 copies to delete, in 4 groups");
      button("Delete 6 copies").click();
      await until(() => db.info.deleted === 6 && button("Restore 6 copies"), "deleted again");
      button("Restore 6 copies").click();
      // Its own copies back: the report holds. The board is out again.
      await until(() => db.info.deleted === 0 && button("Delete 6 copies"), "restored, Delete offered again");
      expect((await roundOf(0)) === "1", "game 0 back to round 1, got " + (await roundOf(0)));
      await shot("d12-restored");

      // A compaction renumbers the games: the report goes stale.
      button("Delete 6 copies").click();
      await until(() => db.info.deleted === 6 && button("Restore"), "deleted again");
      button("Close").click();
      await until(() => !dialog(), "closed");
      await db.compact();
      expect(db.info.games === 7 && !db.info.filter, "compacted to 7 games, the filter cleared");
      await openDialog();
      expect(warn().includes("compacted"), "the stale report says so: " + warn());
      expect(!button("Delete") && !button("Show in game list") && !button("Restore"), "nothing to act on");
      await shot("d13-stale");
      await find(true, "No exact or fuzzy copies among 7 games");
      await shot("d14-none");
      button("Close").click();

      // Light mode.
      await db.importFrom(ROOT + "/testdata/pgn/duplicates.pgn");
      appearance.theme = "light";
      await openDialog();
      await find(true, "9 copies to delete, in 5 groups");
      await shot("d15-light");
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
        await shot("d16-big-none");
        button("Close").click();

        t = performance.now();
        await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
        await log(`imported again: ${db.info.games} games in ${Math.round(performance.now() - t)} ms`);
        await openDialog();
        t = performance.now();
        await find(true, "1,000,000 copies to delete, in 1,000,000 groups", 300000);
        await log(`fuzzy search over 2M games: ${Math.round(performance.now() - t)} ms; ${summary()}`);
        await shot("d17-big-twice");

        t = performance.now();
        button("Show in game list").click();
        await until(() => !dialog() && db.info.filter?.count === 2000000, "2M games listed", 60000);
        await log(`showing the groups: ${Math.round(performance.now() - t)} ms`);
        expect(same(await listIds(4), [0, 1000000, 1, 1000001]), "each game, then its copy");
        await shot("d18-big-list");

        await openDialog();
        t = performance.now();
        button("Delete 1,000,000 copies").click();
        // A job: the list pages meanwhile.
        await until(() => dialog().querySelector(".progress"), "the delete's progress", 10000);
        const page = performance.now();
        await api.listGames(db.info.handle, db.sort, 500000, 50);
        await log(`a list page while deleting: ${Math.round(performance.now() - page)} ms`);
        await shot("d19-big-deleting");
        await until(() => db.info.deleted === 1000000 && button("Restore"), "1M copies deleted", 120000);
        await log(`deleting 1M copies: ${Math.round(performance.now() - t)} ms`);
        await shot("d20-big-deleted");
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
