// Smoke-test driver for the Sync button (NET-13), against chess-net's
// stand-in Lichess, never the real one:
//
//   cargo run -p chess-net --example lichess_standin -- 18431 &
//   CHESSDB_LICHESS_URL=http://127.0.0.1:18431 CHESSDB_BROWSER=approve \
//     scripts/smoke/run.sh scripts/smoke/study-sync.js
//
// A database of the six annotated round-trip games: the ribbon's Link to a
// study… (a new private study, both ways), its preview ("6 new games here
// to send") and Link and sync; then Sync with four games added here (the
// preview says so, one confirm sends them); a comment changed here and on
// Lichess (the stand-in's collaborator, see examples/lichess_standin.rs):
// the preview leaves it for the user, nothing is overwritten, the ribbon
// says Sync (1), the resolve panel shows both comments and Keep both
// keeps a marked copy; a change on Lichess only comes here on the next
// Sync; Follow only shows what stays here. Shots of each step.
//
// The main window is the library (DB-07): there the driver only opens an
// editor window, which runs the same script and does the checks.
(async () => {
  const OUT = "__OUT__";
  const ROOT = "__ROOT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke, lichess } = window.__chessdb;
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
  const until = async (cond, what, ms = 15000) => {
    const end = Date.now() + ms;
    while (!cond() && Date.now() < end) await wait(100);
    expect(cond(), what);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  if (document.querySelector(".home")) {
    await log("library window: opening an editor window for the checks");
    await api.openEditorWindow(null);
    return;
  }

  const ribbonButton = (label) =>
    [...document.querySelectorAll(".ribbon > .groups button")].find((b) => b.getAttribute("aria-label") === label);
  const dialog = () =>
    document.querySelector('.dialog[aria-label="Link to a Lichess study"], .dialog[aria-label="Sync with a Lichess study"]');
  const text = (el) => el?.textContent.replace(/\s+/g, " ").trim() ?? "";
  const button = (label) => [...(dialog()?.querySelectorAll("button") ?? [])].find((b) => text(b) === label);
  const status = () => text(dialog()?.querySelector(".status"));
  /** Opens the dialog from the ribbon's button `label`. */
  const openSync = async (label) => {
    await until(() => ribbonButton(label), "the ribbon's " + label + ": " + [...document.querySelectorAll(".ribbon > .groups button")].map((b) => b.getAttribute("aria-label")).join(", "));
    ribbonButton(label).click();
    await until(dialog, "the sync dialog");
  };
  const close = async () => {
    button("Close").click();
    await until(() => !dialog(), "the dialog closed");
  };
  /** Edits a game's first comment here, saved, its tab closed. */
  const editHere = async (index, comment) => {
    await game.open(db.info.handle, index, index);
    await until(() => game.view && game.view.source?.game === index, "game " + index + " open");
    const session = game.view.session;
    await api.setComment(session, 0, "after", comment);
    await game.save();
    await wait(300);
    await game.closeTab(session);
    await wait(300);
  };

  try {
    await log("start");
    // ---- A database of the round-trip games ---------------------------------
    await db.createAt(OUT + "/Synced games.chessdb");
    await until(() => db.info?.name === "Synced games", "the database made");
    expect(await db.importFrom(ROOT + "/testdata/pgn/study-roundtrip.pgn"), "the games imported");
    await until(() => db.info.games === 6, "6 games: " + db.info.games);

    // Not linked: the ribbon offers Link to a study….
    await openSync("Link to a study…");
    expect(text(dialog()).includes("Link Synced games to a study"), "says what linking does: " + text(dialog()));
    await until(() => button("Log in to Lichess…"), "the login offered");
    button("Log in to Lichess…").click();
    await until(() => lichess.username === "SmokeUser", "logged in as SmokeUser: " + lichess.loginError, 20000);
    const nameField = dialog().querySelector(".details input");
    expect(nameField?.value === "Synced games", "the new study named after the database: " + nameField?.value);
    expect(dialog().querySelector('input[name="sync-visibility"]').checked, "private by default");
    expect(dialog().querySelector('input[name="sync-link-mode"]').checked, "both ways by default");
    await shot("01-link-dialog");
    button("Show what it does").click();
    await until(() => status().includes("6 new games here to send"), "the preview: " + status());
    expect(status().includes("a new private study on Lichess, both ways"), "where they go: " + status());
    await shot("02-link-preview");
    button("Link and sync").click();
    await until(
      () => status().includes("Synced with Synced games, a new study on Lichess: 6 games sent as new chapters"),
      "linked: " + status(),
      30000,
    );
    await until(() => text(dialog()).includes("Linked to Synced games"), "the linked study shown: " + text(dialog()));
    await shot("03-linked");
    await close();

    // ---- New games here go to the study -------------------------------------
    expect(await db.importFrom(ROOT + "/testdata/pgn/lichess-study.pgn"), "four more games");
    await until(() => db.info.games === 10, "10 games: " + db.info.games);
    await openSync("Sync");
    await until(() => status().includes("4 new games here to send"), "the preview at once: " + status());
    await shot("04-new-games-preview");
    button("Sync now").click();
    await until(() => status().includes("4 games sent as new chapters"), "sent: " + status(), 30000);
    await close();

    // ---- A comment changed on both sides: left for the user -----------------
    await api.lichessStudies("SmokeEditFirst");
    await wait(1200);
    await editHere(0, "Changed here in the smoke run.");
    await openSync("Sync");
    await until(() => status().includes("1 for you to decide"), "the conflict in the preview: " + status());
    expect(status().includes("the start: comment"), "what's in conflict: " + status());
    await shot("05-conflict-preview");
    button("Sync now").click();
    await until(() => status().includes("1 game waits for you"), "left for the user: " + status(), 30000);
    const here = await api.gamePgn((await game.open(db.info.handle, 0, 0), game.view.session));
    expect(here.includes("Changed here in the smoke run."), "nothing overwritten here: " + here.slice(0, 300));
    await game.closeTab(game.view.session);
    await until(() => ribbonButton("Sync (1)"), "the ribbon says a game waits");
    await until(() => button("Resolve…"), "Resolve… offered");
    await shot("06-waiting");
    button("Resolve…").click();
    await until(() => dialog().querySelector(".resolve"), "the resolve panel");
    const panel = text(dialog().querySelector(".resolve"));
    expect(panel.includes("Changed here in the smoke run.") && panel.includes("(edited on Lichess)"), "both comments: " + panel);
    await shot("07-resolve");
    button("Keep both").click();
    await until(() => !dialog().querySelector(".resolve"), "resolved", 30000);
    await until(() => status().includes("marked as a copy") || status().includes("Nothing to do") || status().includes("not sent"), "after Keep both: " + status(), 30000);
    await until(() => db.info.games === 11, "the copy kept: " + db.info.games);
    await until(() => ribbonButton("Sync"), "nothing waits now");
    await shot("08-kept-both");
    await close();

    // ---- A change on Lichess only comes here ---------------------------------
    await api.lichessStudies("SmokeEditSecond");
    await wait(1200);
    await openSync("Sync");
    await until(() => status().includes("1 changed on Lichess"), "the pull in the preview: " + status());
    button("Sync now").click();
    await until(() => status().includes("1 game changed on Lichess brought in"), "pulled: " + status(), 30000);
    await game.open(db.info.handle, 1, 1);
    await until(() => game.view?.source?.game === 1, "game 1 open");
    const pulled = await api.gamePgn(game.view.session);
    expect(pulled.includes("(edited on Lichess)"), "Lichess's edit here: " + pulled.slice(0, 400));
    await game.closeTab(game.view.session);

    // ---- Follow only ----------------------------------------------------------
    const follow = dialog().querySelectorAll('input[name="sync-mode"]')[1];
    follow.click();
    await until(() => status().includes("Nothing to do") || dialog().querySelector(".preview"), "looked again following only: " + status());
    await until(() => dialog().querySelectorAll('input[name="sync-mode"]')[1].checked, "follow only chosen");
    await shot("09-follow-only");
    await close();
    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e));
    await shot("failed");
  }
})();
