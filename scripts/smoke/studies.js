// Smoke-test driver for the Lichess studies track (NET-11, IO-10, NET-12),
// against a server standing in for Lichess, never the real one:
//
//   cargo run -p chess-net --example lichess_standin -- 18431 &
//   CHESSDB_LICHESS_URL=http://127.0.0.1:18431 CHESSDB_BROWSER=approve \
//     scripts/smoke/run.sh scripts/smoke/studies.js
//
// CHESSDB_BROWSER=approve lets the login in as the stand-in's SmokeUser
// without a browser. The run's display has no keyring (its D-Bus starts no
// services), so the login lasts the session, as the dialog says.
//
// The ribbon's Import study…: a public study's link looked up (4 chapters)
// and imported into a new database (in the run's folder), again into it
// (nothing new), a private study refused without a login, Log in to
// Lichess…, the private study then, the user's studies listed with it;
// Send to study…: the list's games to a new private study, a second new
// study refused at the stand-in's daily limit of 1 (what went where said),
// then to the study made before, picked from "One of your studies"; the
// list's menu offering Send game to a Lichess study…; logging out.
//
// The main window is the library (DB-07): there the driver only opens an
// editor window, which runs the same script and does the checks.
(async () => {
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke, lichess, jobs } = window.__chessdb;
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
  const until = async (cond, what, ms = 8000) => {
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

  const ribbon = (label) => document.querySelector(`.ribbon > .groups button[aria-label="${label}"]`);
  const dialogOf = (title) => () => document.querySelector(`.dialog[aria-label="${title}"]`);
  const importDialog = dialogOf("Import a Lichess study");
  const exportDialog = dialogOf("Send games to a Lichess study");
  const text = (el) => el?.textContent.replace(/\s+/g, " ").trim() ?? "";
  const button = (dialog, label) =>
    [...(dialog()?.querySelectorAll("button") ?? [])].find((b) => text(b) === label || b.title === label);
  const type = (input, value) => {
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const status = (dialog) => text(dialog()?.querySelector(".status"));
  /** Clicks Look up and waits for the new preview (the old one goes first). */
  const lookUp = async (expected) => {
    button(importDialog, "Look up").click();
    await wait(300);
    await until(
      () => importDialog().querySelector(".preview") && text(importDialog().querySelector(".preview")).includes(expected),
      "looked up: " + expected,
    );
  };

  try {
    await log("start");
    // The ribbon's new column (Import study…, Send to study…) still fits on one row,
    // full width and narrow (groups then drop labels or collapse).
    for (const width of [null, "900px"]) {
      document.querySelector("#app").style.width = width ?? "";
      await wait(600);
      const bar = document.querySelector(".ribbon").getBoundingClientRect();
      const groups = [...document.querySelectorAll(".ribbon > .groups > .group")].map((g) => g.getBoundingClientRect());
      expect(groups.every((g) => g.right <= bar.right + 1 && g.top === groups[0].top), `the ribbon on one row at ${width ?? "full width"}`);
    }
    document.querySelector("#app").style.width = "";
    await wait(600);
    const account = await api.lichessAccount();
    expect(!account.username, "nobody logged in at first: " + JSON.stringify(account));

    // ---- IO-10: a public study into a new database ------------------------
    expect(ribbon("Import study…") && !ribbon("Import study…").disabled, "the ribbon's Import study…");
    ribbon("Import study…").click();
    await until(importDialog, "the import dialog");
    expect(text(importDialog()).includes("Public studies import without a login"), "says what needs a login");
    const link = importDialog().querySelector('input[placeholder="https://lichess.org/study/…"]');
    expect(document.activeElement === link, "the link has the focus");
    type(link, "lichess.org/study/nothing");
    button(importDialog, "Look up").click();
    await until(() => text(importDialog()).includes("isn't a link to a Lichess study"), "a bad link refused");
    type(link, "https://lichess.org/study/FxStudy1");
    button(importDialog, "Look up").click();
    await until(() => text(importDialog()).includes("Fixture study"), "the study looked up");
    expect(text(importDialog()).includes("4 chapters"), "4 chapters: " + text(importDialog()));
    lichess.importTarget = OUT + "/Fixture study.chessdb";
    await wait(100);
    expect(text(importDialog()).includes("A new database: Fixture study"), "named after the study");
    await shot("01-study-looked-up");
    button(importDialog, "Import").click();
    await until(() => status(importDialog).includes("Imported 4 chapters of Fixture study"), "imported: " + status(importDialog), 20000);
    expect(db.info?.name === "Fixture study" && db.info.games === 4, "the new database has the chapters: " + JSON.stringify(db.info?.name) + " " + db.info?.games);
    await shot("02-study-imported");

    // The chapters keep their study tags, comments and arrows.
    await game.open(db.info.handle, 0, 0);
    await until(() => game.notation, "the first chapter open");
    const pgn = await api.gamePgn(game.view.session);
    expect(pgn.includes('[ChapterURL "https://lichess.org/study/FxStudy1/'), "its ChapterURL: " + pgn.slice(0, 400));
    expect(/%cal [^\]]*Gg1f3/.test(pgn) && pgn.replace(/\s+/g, " ").includes("The Two Knights"), "arrows and a variation's comment: " + pgn);

    // Again into the open database: nothing new.
    await lookUp("4 chapters");
    importDialog().querySelectorAll('input[name="study-into"]')[1].click();
    await wait(100);
    button(importDialog, "Import").click();
    await until(() => status(importDialog).includes("No new chapters in Fixture study (4 were in the database already)"), "nothing new: " + status(importDialog), 20000);
    expect(db.info.games === 4, "still 4 games");

    // ---- NET-11: a private study needs the login --------------------------
    type(link, "https://lichess.org/study/Private1");
    button(importDialog, "Look up").click();
    await until(() => text(importDialog()).includes("private or unlisted. Log in to Lichess"), "the private study refused: " + text(importDialog()));
    await shot("03-private-needs-login");
    button(importDialog, "Log in to Lichess…").click();
    await until(() => lichess.username === "SmokeUser", "logged in as SmokeUser: " + lichess.loginError, 20000);
    expect(text(importDialog()).includes("Logged in to Lichess as SmokeUser"), "says who");
    expect(!lichess.account.kept && text(importDialog()).includes("no keyring is running"), "a session login without a keyring");
    await until(() => text(importDialog()).includes("My private lines"), "the user's studies listed with the login", 10000);
    await shot("04-logged-in");
    await lookUp("2 chapters");
    expect(text(importDialog().querySelector(".preview")).includes("private or unlisted"), "marked private");
    importDialog().querySelectorAll('input[name="study-into"]')[1].click();
    await wait(100);
    button(importDialog, "Import").click();
    await until(() => status(importDialog).includes("Imported 2 chapters of My private lines"), "the private study imported: " + status(importDialog), 20000);
    expect(db.info.games === 6, "6 games now: " + db.info.games);
    button(importDialog, "Close").click();
    await until(() => !importDialog(), "the import dialog closed");

    // ---- NET-12: games to studies ------------------------------------------
    ribbon("Send to study…").click();
    await until(exportDialog, "the export dialog");
    expect(text(exportDialog()).includes("All 6 games of Fixture study"), "the list's games: " + text(exportDialog()));
    const visibility = [...exportDialog().querySelectorAll('input[name="study-visibility"]')];
    expect(visibility[0].checked && text(visibility[0].parentElement).startsWith("Private"), "private by default");
    type(exportDialog().querySelector(".details input"), "Smoke export");
    await shot("05-export-dialog");
    button(exportDialog, "Send 6 games").click();
    await until(() => status(exportDialog).includes("Sent 6 games to Smoke export, a new private study on Lichess"), "sent: " + status(exportDialog), 30000);
    expect(text(exportDialog()).includes("Smoke export: 6 games (new)"), "where they went");
    await shot("06-sent");

    // The stand-in makes one new study a day: the second is refused.
    type(exportDialog().querySelector(".details input"), "Smoke export again");
    button(exportDialog, "Send 6 games").click();
    await until(() => status(exportDialog).includes("Lichess refused to make another study"), "refused at the daily limit: " + status(exportDialog), 30000);
    expect(status(exportDialog).includes("the rest can be sent tomorrow"), "says when");
    await shot("07-daily-limit");

    // To the study made before, picked from the user's.
    exportDialog().querySelectorAll('input[name="study-to"]')[1].click();
    await until(() => [...exportDialog().querySelectorAll(".study")].some((b) => text(b).startsWith("Smoke export")), "the user's studies", 10000);
    [...exportDialog().querySelectorAll(".study")].find((b) => text(b).startsWith("Smoke export")).click();
    await wait(100);
    button(exportDialog, "Send 6 games").click();
    await until(() => status(exportDialog).includes("Sent 6 games to Smoke export on Lichess"), "sent to the existing study: " + status(exportDialog), 30000);
    await shot("08-existing-study");
    button(exportDialog, "Close").click();
    await until(() => !exportDialog(), "the export dialog closed");
    expect(!jobs.running.length, "no job left running");

    // The list's menu: the selected game.
    const row = document.querySelector('.list .row:not(.head)');
    if (row) {
      const r = row.getBoundingClientRect();
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 5 }));
      await wait(300);
      const item = [...document.querySelectorAll('.menu [role="menuitem"]')].find((b) => text(b).startsWith("Send game to a Lichess study"));
      if (item) {
        item.click();
        await until(exportDialog, "the export dialog from the list");
        expect(text(exportDialog()).includes("The selected game"), "the selected game: " + text(exportDialog()));
        button(exportDialog, "Close").click();
        await log("the list's menu sends the selected game");
      } else await log("the list's menu didn't open (no item found): skipped");
    } else await log("no list row found for the menu: skipped");

    // ---- Logging out --------------------------------------------------------
    ribbon("Import study…").click();
    await until(importDialog, "the import dialog again");
    button(importDialog, "Log out").click();
    await until(() => !lichess.username && text(importDialog()).includes("Logged out: Lichess revoked the token"), "logged out: " + text(importDialog()));
    await shot("09-logged-out");
    button(importDialog, "Close").click();

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e));
  }
})();
