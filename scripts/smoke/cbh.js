// Smoke-test driver for importing a ChessBase database (IO-03): the
// ribbon's Import games, a `.cbh` whose files are missing, and, when the
// databases that ship with ChessBase are linked into
// testdata/large/cbh-samples/ (they aren't committed; see AGENTS.md),
// Openings (comments in four languages) and Marathon (set-up positions).
(async () => {
  const OUT = "__OUT__";
  const ROOT = "__ROOT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, panels, api } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden monitor: never ask about unsaved changes.
  game.confirmDiscard = async () => true;
  const log = (m) => window.__chessdb.invoke("dev_log", { message: String(m) });
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
  const $ = (s) => document.querySelector(s);
  const notice = () => $("footer .notice")?.textContent.trim() ?? "";
  const notation = () => $(".notation")?.textContent ?? "";
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await db.createAt(OUT + "/cbh.chessdb");
    const importButton = () => $('.ribbon > .groups button[aria-label="Import games"]');
    const labels = () => [...document.querySelectorAll(".ribbon > .groups button")].map((b) => b.getAttribute("aria-label"));
    await until(
      () => importButton() && !importButton().disabled,
      "an enabled Import games button in the ribbon, among " + JSON.stringify(labels()),
    );

    // A .cbh without its other files: the import fails and says which one.
    expect(!(await db.importFrom(OUT + "/Missing.cbh")), "importing a missing database fails");
    await until(() => notice().includes("Missing.cbh is missing"), "the notice naming the missing file");
    await log("missing: " + notice());
    expect(db.info.games === 0, "no games from it");

    const samples = ROOT + "/testdata/large/cbh-samples/";
    if (!(await db.importFrom(samples + "Openings.cbh"))) {
      await log("no sample databases in testdata/large/cbh-samples/: the rest is skipped");
      await log("done");
      return;
    }
    expect(db.info.games === 150, "150 games from Openings, got " + db.info.games);
    expect(db.importReport === null, "no warnings, got " + JSON.stringify(db.importReport));

    // Its comments come in German, Spanish, French and Russian: one language.
    panels.show("notation");
    await game.open(db.info.handle, 0);
    await until(() => notation().includes("Aljechin-Verteidigung"), "the opening's name in the notation");
    expect(!notation().includes("Defensa Alekhine"), "only one language of the comment");
    await shot("cbh01-openings");

    expect(await db.importFrom(samples + "Marathon.cbh"), "Marathon imported");
    expect(db.info.games === 360, "210 more games from Marathon, got " + db.info.games);
    expect(db.importReport === null, "no warnings from Marathon");
    await game.open(db.info.handle, 150);
    await until(() => game.view?.fen && !game.view.fen.startsWith("rnbqkbnr/pppppppp/"), "a set-up start position");
    await shot("cbh02-marathon");

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
