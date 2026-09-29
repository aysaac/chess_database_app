// smoke: first-run
// Smoke-test driver for onboarding (QOL-08). The line above makes run.sh start
// the app with no settings, as on a first run, so the welcome guide opens by
// itself. It picks the sample games, checks the engine page (adding the test
// engine when none is found), finishes the guide, then runs the import wizard
// from the status bar's "?" button: a new database from fixture PGNs, and the
// error when that database already exists. Placeholders as in basic.js.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, analysis, onboarding, api, invoke } = window.__chessdb;
  // Never ask about unsaved changes: a native dialog can't be clicked here.
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
  const until = async (cond, what, ms = 10000) => {
    for (let t = 0; t < ms && !cond(); t += 50) await wait(50);
    expect(cond(), what);
  };
  const dialog = () => document.querySelector("[role='dialog']");
  const button = (label) =>
    [...(dialog()?.querySelectorAll("button") ?? [])].find((b) => b.textContent.trim() === label);
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");

    // A first run: the guide opens by itself, with the first choice focused
    // (so Escape reaches the dialog).
    await until(() => onboarding.open && dialog(), "the welcome guide to open on a first run");
    expect(dialog().getAttribute("aria-label") === "Welcome to Chess Database", "the welcome page");
    await until(() => document.activeElement?.classList.contains("choice"), "the first choice focused");
    const status = await api.onboardingStatus();
    expect(status.showWelcome, "a first run in the settings");
    expect(status.sampleGames === 12, "12 sample games, got " + status.sampleGames);
    expect(status.samplePath.startsWith(OUT), "the sample database in the run's folder: " + status.samplePath);
    expect(!db.info, "no database open yet");
    await shot("01-welcome");

    // The sample games: built on first use, opened in the list, first game on the board.
    const choices = [...dialog().querySelectorAll(".choice")];
    expect(choices.length === 3, "three ways to start");
    const t0 = performance.now();
    choices[1].click();
    await until(() => onboarding.step === "engine" && db.info, "the engine page after the sample games");
    await log(`sample database opened in ${Math.round(performance.now() - t0)} ms`);
    expect(db.info.name === "Sample games" && db.info.games === 12, "the sample database, got " + db.info?.name);
    expect(!db.importReport, "no import warnings in the sample games");
    await until(() => game.view?.source?.game === 0, "the first sample game on the board");
    expect(game.view.headers.white === "Anderssen, Adolf", "the Immortal Game, got " + game.view.headers.white);

    // The engine page: the bundled or installed engine, or a way to add one.
    await until(() => !analysis.loadingEngines && dialog()?.getAttribute("aria-label") === "Your engine",
      "the engine list", 30000);
    if (!analysis.engines.length) {
      expect(dialog().textContent.includes("No engine was found"), "the no-engine message");
      expect(button("Add engine…") && button("Look again"), "ways to add an engine");
      await shot("02-no-engine");
      // What "Add engine…" does once a program is picked.
      expect(await analysis.add(ROOT + "/target/debug/fake-uci"), "the test engine added");
    }
    await until(() => dialog().textContent.includes("is ready"), "an engine ready");
    await log("engine: " + analysis.defaultEngine.name + " (" + analysis.defaultEngine.path + ")");
    await shot("03-engine");

    button("Next").click();
    await until(() => onboarding.step === "tips" && dialog()?.getAttribute("aria-label") === "A few things to try",
      "the tips page");
    await shot("04-tips");
    button("Start").click();
    await until(() => !onboarding.open && !dialog(), "the guide closed");
    await wait(200);
    expect(!(await api.onboardingStatus()).showWelcome, "the guide not opening again");
    await shot("05-sample-open");

    // Closed and opened again: the same sample, not built twice.
    await db.close();
    await until(() => [...document.querySelectorAll(".empty button")].some((b) => b.textContent === "Open the sample games"),
      "the sample games offered in the empty list");
    await shot("06-empty-list");
    expect(await db.openSample(), "the sample games reopened");
    expect(db.info.games === 12, "still 12 sample games, got " + db.info.games);

    // The import wizard, from the status bar's "?" button.
    document.querySelector("button[aria-label='Getting started']").click();
    await until(() => onboarding.open && onboarding.step === "start", "the guide from the ? button");
    dialog().querySelectorAll(".choice")[0].click();
    await until(() => dialog()?.getAttribute("aria-label") === "Import your games", "the import page");
    expect(document.activeElement?.textContent === "Choose PGN files…", "Choose PGN files focused");
    expect(button("Import").disabled, "Import disabled without files");
    await shot("07-import-empty");

    // What "Choose PGN files…" does once files are picked (no native dialog here).
    const files = ["variations.pgn", "lichess.pgn", "malformed.pgn"].map((f) => ROOT + "/testdata/pgn/" + f);
    await onboarding.setFiles(files);
    expect(/variations( \d+)?\.chessdb$/.test(onboarding.target), "a database named after the file: " + onboarding.target);
    await log("suggested " + onboarding.target);
    // What "Change…" does: the run's folder instead of Documents.
    onboarding.target = OUT + "/Imported games.chessdb";
    await until(() => dialog().querySelectorAll(".files li").length === 3, "the chosen files listed");
    expect(dialog().querySelector(".target strong")?.textContent === "Imported games", "the database name shown");
    await shot("08-import-ready");
    button("Import").click();
    await until(() => onboarding.result !== null && !onboarding.importing, "the import to finish", 60000);
    const r = onboarding.result;
    expect(r.complete && r.name === "Imported games", "a complete import, got " + JSON.stringify(r));
    expect(db.info.name === "Imported games" && db.info.games === r.games && r.games >= 3,
      "the new database with the games, got " + db.info.games);
    expect(r.warnings > 0 && r.skipped === 1, "malformed.pgn's problems counted, got " + JSON.stringify(r));
    expect(dialog().textContent.replace(/\s+/g, " ").includes("and 1 game couldn't be read, so it was skipped."),
      "the skipped game mentioned");
    expect(db.importReport, "the warnings in the status bar");
    await until(() => document.activeElement === button("Next"), "Next focused");
    await until(() => game.view?.source?.db === db.info.handle, "its first game on the board");
    await log(`imported ${r.games} games with ${r.warnings} warnings`);
    await shot("09-import-done");

    // The same database again: the wizard says what went wrong and what to do.
    onboarding.resetImport();
    await onboarding.setFiles(files.slice(0, 1));
    onboarding.target = OUT + "/Imported games.chessdb";
    await onboarding.runImport();
    expect(onboarding.error.startsWith("There's already a database called Imported games"),
      "the error: " + onboarding.error);
    expect(db.info.name === "Imported games", "the open database unchanged");
    await until(() => dialog().querySelector(".error"), "the error shown");
    await shot("10-import-error");

    // Escape closes the guide.
    dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await until(() => !onboarding.open, "Escape closes the guide");

    // Opened again (the empty list's "Import PGN files…"), the wizard starts over.
    onboarding.show("import");
    await until(() => dialog()?.getAttribute("aria-label") === "Import your games", "the import page again");
    expect(!onboarding.files.length && !onboarding.result && !onboarding.error, "a fresh import wizard");
    dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await until(() => !onboarding.open, "closed again");

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e) + "\n" + (e?.stack ?? ""));
  }
})();
