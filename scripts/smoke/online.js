// Smoke-test driver for NET-01, downloading a player's games, against the
// real Lichess and Chess.com (needs the internet; a few hundred games at
// most): the empty list's "Download games from Lichess or Chess.com…", the
// checks before anything is asked (no username, a player who doesn't
// exist: no database made), DrNykterstein's Lichess games since
// 2026-10-01 into a new database with their clock times, the ribbon's
// "Download games…" filled in again for the open database with new games
// only (none added twice), Hikaru's rated Chess.com blitz and rapid games
// of the last two days into the same database (no Classical box there),
// and Stop download partway through all of DrNykterstein's games, the
// games that came staying.
//
// The main window is the library (DB-07): there the driver only opens an
// editor window, which runs the same script and does the checks.
(async () => {
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke, online, jobs } = window.__chessdb;
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

  const dialog = () => document.querySelector('.dialog[aria-label="Download games from Lichess or Chess.com"]');
  const button = (text) => [...(dialog()?.querySelectorAll("button") ?? [])].find((b) => b.textContent.trim() === text);
  const status = () => dialog()?.querySelector(".status")?.textContent.replace(/\s+/g, " ").trim() ?? "";
  const daysAgo = (n) => new Date(Date.now() - n * 864e5).toISOString().slice(0, 10);
  /** Clicks Download and waits until the dialog is done with it. */
  const download = async (what, ms = 180_000) => {
    online.result = null;
    online.error = "";
    button("Download").click();
    // A short download can be over before a poll sees it busy.
    await until(() => !online.busy && (online.result || online.error), what + ": finished", ms);
    await log(what + ": " + status());
  };
  const rows = async () => (await api.listGames(db.info.handle, { key: "id", descending: false }, 0, 1000)).rows;

  try {
    await log("start");
    await until(() => document.querySelector(".empty"), "the empty game list");
    const fromEmpty = [...document.querySelectorAll(".empty button")].find((b) =>
      b.textContent.includes("Download games from Lichess or Chess.com"),
    );
    expect(!!fromEmpty, "the empty list's download button");
    fromEmpty.click();
    await until(() => dialog(), "the dialog");
    expect(online.into === "new", "into a new database with none open");
    const openChoice = [...dialog().querySelectorAll('input[name="online-into"]')][1];
    expect(openChoice.disabled, "the open database can't be picked with none open");
    expect(document.activeElement === dialog().querySelector(".field input"), "the username has the focus");
    await shot("01-dialog");

    // Checks before anything is asked.
    online.username = "";
    button("Download").click();
    await until(() => online.error, "an error without a username");
    expect(online.error.includes("Enter the player's username"), "asks for the username: " + online.error);
    online.username = "no_such_user_zz9q";
    await download("an unknown player", 30_000);
    expect(online.error.includes("has no player called no_such_user_zz9q"), "no such player: " + online.error);
    expect(!db.info, "no database made for a player who doesn't exist");
    await shot("02-no-such-player");

    // Closing the dialog while the player is looked up: nothing is made.
    online.username = "DrNykterstein";
    online.target = OUT + "/Closed during lookup.chessdb";
    online.error = "";
    button("Download").click();
    await wait(50);
    expect(online.busy && online.job === null, "looking up");
    expect(!button("Stop download").disabled, "Stop works while looking up");
    button("Close").click();
    await until(() => !dialog(), "closed while looking up");
    await wait(4000);
    expect(!db.info && !jobs.running.length, "no database and no download after closing during the lookup");
    fromEmpty.click();
    await until(() => dialog(), "the dialog again");

    // Lichess, into a new database (in the run's folder, not Documents).
    online.username = "DrNykterstein";
    online.period = "since";
    online.since = "2026-10-01";
    online.target = OUT + "/DrNykterstein (Lichess).chessdb";
    await download("Lichess since 2026-10-01");
    expect(online.result?.state === "done", "done: " + JSON.stringify(online.result));
    expect(online.result.message.startsWith("Downloaded"), "downloaded: " + online.result.message);
    expect(db.info?.name === "DrNykterstein (Lichess)", "the new database is open: " + db.info?.name);
    const first = db.info.games;
    expect(first >= 5, "5 games at least: " + first);
    const lichess = await rows();
    expect(lichess.every((r) => /lichess\.org\//.test(r.site)), "each game's page as its site");
    expect(lichess.every((r) => r.white === "DrNykterstein" || r.black === "DrNykterstein"), "the player's games");
    expect(lichess.every((r, i) => i === 0 || r.date >= lichess[i - 1].date), "oldest first");
    await game.open(db.info.handle, 0, 0);
    await until(() => game.notation, "the first game open");
    const pgn = await api.gamePgn(game.view.session);
    expect(/%clk/.test(pgn), "clock times kept");
    await shot("03-lichess");

    // The ribbon's button, filled in again: new games only, none added twice.
    button("Close").click();
    await until(() => !dialog(), "the dialog closed");
    document.querySelector('.ribbon > .groups button[aria-label="Download games…"]').click();
    await until(() => dialog(), "the dialog from the ribbon");
    expect(online.username === "DrNykterstein" && online.site === "lichess", "the player remembered");
    expect(online.into === "open" && online.period === "newOnly", "new games only into the open database");
    await download("Lichess, new games only", 60_000);
    expect(/New ones only: from where the last download/.test(dialog().textContent), "the new-ones choice says what it counts");
    expect(online.result?.state === "done", "done again: " + JSON.stringify(online.result));
    expect(
      /^No new games of DrNykterstein on Lichess|^Downloaded \d+ new/.test(online.result.message),
      "from where the first download got to: " + online.result.message,
    );
    expect(db.info.games >= first, "no game lost");
    const after = await rows();
    expect(new Set(after.map((r) => r.site)).size === after.length, "no game twice");
    await shot("04-new-only");

    // Chess.com: rated blitz and rapid games of the last two days.
    online.site = "chessCom";
    await wait(100);
    const boxes = [...dialog().querySelectorAll('[aria-label="Time controls"] label')];
    expect(!boxes.some((b) => b.textContent.includes("Classical")), "no Classical on Chess.com");
    expect(boxes.some((b) => b.textContent.includes("Daily")), "Daily on Chess.com");
    for (const name of ["Bullet", "Daily"]) {
      const box = boxes.find((b) => b.textContent.trim() === name).querySelector("input");
      if (box.checked) box.click();
    }
    online.ratedOnly = true;
    online.username = "hikaru";
    online.period = "since";
    online.since = daysAgo(2);
    const before = db.info.games;
    await download("Chess.com, rated blitz and rapid of 2 days");
    expect(online.result?.state === "done", "Chess.com done: " + JSON.stringify(online.result));
    const all = await rows();
    const chessCom = all.filter((r) => r.site === "Chess.com");
    expect(db.info.games === before + chessCom.length, "the Chess.com games added: " + chessCom.length);
    expect(chessCom.every((r) => /hikaru/i.test(r.white + r.black)), "Hikaru's games");
    await shot("05-chess-com");

    // Stop partway: all of DrNykterstein's Lichess games (thousands).
    online.site = "lichess";
    online.username = "DrNykterstein";
    online.ratedOnly = false;
    online.speeds = ["bullet", "blitz", "rapid", "classical", "correspondence"];
    online.period = "all";
    const beforeStop = db.info.games;
    button("Download").click();
    await until(() => online.progress && online.progress.done > 60, "the download under way", 60_000);
    await until(() => dialog().querySelector("progress"), "a progress bar");
    await shot("06-downloading");
    // One download at a time, whatever starts the second.
    const second = await api
      .downloadGames(db.info.handle, { site: "chessCom", username: "hikaru", speeds: [], ratedOnly: false, period: "all", since: null })
      .then(() => "started", (e) => e.message);
    expect(/Another download is running/.test(second), "a second download refused: " + second);
    button("Stop download").click();
    await until(() => !online.busy, "stopped", 30_000);
    await log("stopped: " + status());
    expect(online.result?.state === "cancelled", "cancelled: " + JSON.stringify(online.result));
    expect(db.info.games > beforeStop, "the games that came stay: " + (db.info.games - beforeStop));
    await shot("07-stopped");

    dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await until(() => !dialog(), "Escape closes the dialog");
    await log("done");
  } catch (e) {
    const state = { busy: online.busy, job: online.job, error: online.error, result: online.result, into: online.into, period: online.period, open: !!dialog(), status: status(), focus: document.activeElement?.outerHTML?.slice(0, 120) };
    await log("state: " + JSON.stringify(state));
    await shot("99-failed");
    // WebKit's stack leaves out the message.
    await log("FAILED: " + (e?.message ?? e) + (e?.stack ? "\n" + e.stack : ""));
  }
})();
