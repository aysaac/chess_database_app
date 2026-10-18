// Smoke-test driver for tournament cross-tables (DB-16), on the fixture
// testdata/pgn/crosstable.pgn whose numbers are known: the Events list's
// Cross-table button, the round-robin grid (points, performance,
// Sonneborn-Berger, a forfeit, an unfinished game), the rounds view,
// opening a game from a result, the game header's Cross-table button with
// that game highlighted and its Show games bringing the list's Games tab
// forward, a Swiss with byes and a tie on every tie-break (both rows show
// the places they share), a knock-out with a third-place match, an event
// with two editions (a double round robin) and Show games listing the one
// shown, and light mode. With testdata/large/twic1600.pgn it tries real
// TWIC events (a round robin, a 597-player Swiss, a knock-out with level
// matches), and with the 1M Lichess games (__BIG__) an event too big for a
// table, one too big for its results (standings only) and an online arena,
// timed.
//
// The main window is the library (DB-07): there the driver only opens an
// editor window, which runs the same script and does the checks.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, appearance, api, invoke } = window.__chessdb;
  // A native dialog would open where nobody can click it.
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
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const text = (el) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();
  const key = (el, k) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  if (document.querySelector(".home")) {
    await log("library window: opening an editor window for the checks");
    await api.openEditorWindow(null);
    return;
  }

  // ---- Helpers for the Events list and the dialog.
  const listTab = (id) => $(`[role='tab'][data-tab='${id}']`);
  const nameRows = () => $$(".names .rows .row");
  const nameOf = (row) => text($(".cell", row));
  const barButton = (label) => $$(".names .bar button").find((b) => text(b) === label);
  const dialog = () => $(".cross")?.closest("[role='dialog']");
  const title = () => dialog()?.getAttribute("aria-label");
  const summary = () => text($(".cross .summary"));
  const bodyRows = () => $$(".cross tbody tr");
  const names = () => bodyRows().map((r) => text($("td.name", r)));
  const column = (cls) => bodyRows().map((r) => text($(`td.${cls}`, r)));
  // The stats after the results: Pts, Games, %, Opp. Elo, Perf, tie-breaks.
  const stats = (r) => $$("td.first-stat, td.first-stat ~ td", r).map(text);
  const cell = (r, c) => $$("td.g", bodyRows()[r])[c];
  const symbols = (td) => $$("button.res .sym", td).map(text).join(" ");
  const view = (label) => $$(".cross .views button").find((b) => text(b) === label);
  const showGames = () => $$("[role='dialog'] button").find((b) => text(b) === "Show games");
  const closeDialog = async () => {
    key($(".cross button.res") ?? $(".cross"), "Escape");
    await until(() => !dialog(), "the dialog closed");
  };

  async function eventsTab() {
    listTab("event").click();
    await until(() => $(".names"), "the Events list");
  }

  async function findEvent(name) {
    const input = $(".names .bar input");
    input.value = name;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => nameRows().some((r) => nameOf(r) === name), "the event " + name + " listed");
  }

  async function openCrossTable(name) {
    await findEvent(name);
    const row = nameRows().find((r) => nameOf(r) === name);
    row.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await wait(50);
    const t0 = performance.now();
    barButton("Cross-table").click();
    await until(() => title() === name && (bodyRows().length > 0 || $(".cross .message:not(.soft)")), "the cross-table of " + name, 30000);
    return performance.now() - t0;
  }

  try {
    await log("start");
    await db.createAt(OUT + "/crosstable.chessdb");
    await db.importFrom(ROOT + "/testdata/pgn/crosstable.pgn");
    await until(() => db.info?.games === 47, "47 games imported");

    // ---- The Events list: a Cross-table button for events only.
    listTab("player").click();
    await until(() => $(".names"), "the Players list");
    expect(!barButton("Cross-table"), "no Cross-table button in the Players list");
    await eventsTab();
    await until(() => nameRows().length === 4, "four events");
    expect(nameRows().map(nameOf).join(" | ") === "Spring Round Robin | Autumn Open | Club Championship | Summer Cup",
      "the events by games, got " + nameRows().map(nameOf).join(" | "));

    // ---- A round robin: the grid.
    await openCrossTable("Spring Round Robin");
    expect(summary() === "Round robin · 6 players · 5 rounds · 15 games · Leiden · 2024.04.06 – 2024.04.10",
      "the summary, got " + summary());
    expect(view("Grid")?.getAttribute("aria-pressed") === "true", "the grid view first");
    expect(names().join(" | ") === "Alder, Anna | Cedar, Clara | Elm, Eva | Brook, Ben | Dale, David | Fern, Felix",
      "the standings, got " + names().join(" | "));
    expect(column("pts").join(" ") === "4 3 3 2½ 1½ 0", "the points, got " + column("pts").join(" "));
    const heads = $$(".cross thead th").map(text);
    expect(heads.join(" ") === "# Player Elo 1 2 3 4 5 6 Pts Games % Opp. Elo Perf SB Wins", "the headings, got " + heads.join(" "));
    // Alder: 4/5, 80%, opponents 2317 on average, performance 2557, SB 7.25, 3 wins.
    expect(stats(bodyRows()[0]).join(" ") === "4 5 80% 2317 2557 7.25 3", "Alder's line, got " + stats(bodyRows()[0]).join(" "));
    // Cedar and Elm, level on points, split by Sonneborn-Berger.
    expect(stats(bodyRows()[1]).join(" ") === "3 4 75% 2351 2544 7.5 2", "Cedar's line, got " + stats(bodyRows()[1]).join(" "));
    // Elm's forfeit win counts for the points (3 of 5) but not for the performance (2 of 4).
    expect(stats(bodyRows()[2]).join(" ") === "3 5 60% 2366 2366 4.25 2", "Elm's line, got " + stats(bodyRows()[2]).join(" "));
    expect($$("td.self", bodyRows()[0]).length === 1 && cell(0, 0).classList.contains("self"), "the diagonal");
    // Row Elm (3rd), column Fern (6th): a forfeit win; the other way a forfeit loss.
    expect(symbols(cell(2, 5)) === "+" && symbols(cell(5, 2)) === "−", "the forfeit, got " + symbols(cell(2, 5)) + " / " + symbols(cell(5, 2)));
    expect(symbols(cell(1, 5)) === "*" && symbols(cell(5, 1)) === "*", "the unfinished game");
    expect(symbols(cell(0, 1)) === "½" && symbols(cell(3, 4)) === "½" && symbols(cell(0, 4)) === "1", "results by opponent");
    const tip = $("button.res", cell(0, 1)).title;
    expect(tip === "Round 3 · Black against Cedar, Clara (2350) · drew. Opens the game.", "the tooltip, got " + tip);
    const notes = text($(".cross .notes"));
    expect(notes === "1 unfinished game (*) counts for nothing. 1 forfeit (+ −) counts for points, not for the performance.",
      "the notes, got " + notes);
    await shot("01-round-robin-grid");

    // ---- The rounds view of the same table.
    view("Rounds").click();
    await until(() => $(".cross td.rounds"), "the rounds view");
    const rheads = $$(".cross thead th.g").map(text).join(" ");
    expect(rheads === "R1 R2 R3 R4 R5", "a column per round, got " + rheads);
    const alder = $$("td.rounds", bodyRows()[0]).map((td) => text(td));
    // Opponent's place, colour (not text), result.
    expect(alder.join(" | ") === "61 | 4½ | 2½ | 51 | 31", "Alder's rounds, got " + alder.join(" | "));
    expect($("td.rounds .colour.white", bodyRows()[0]) && $$("td.rounds .colour:not(.white)", bodyRows()[0]).length === 2,
      "colour marks: three white, two black");
    await shot("02-round-robin-rounds");

    // ---- Opening a game from a result.
    view("Grid").click();
    await until(() => !$(".cross td.rounds"), "the grid again");
    $("button.res", cell(0, 1)).click();
    await until(() => !dialog(), "the dialog closed on opening a game");
    await until(() => game.view?.headers.white === "Cedar, Clara" && game.view.headers.black === "Alder, Anna",
      "Cedar - Alder opened");
    const opened = game.view.source.game;

    // ---- From the game header: the same event, that game highlighted.
    const headerButton = $$("#panel-notation header button").find((b) => text(b) === "Cross-table");
    expect(headerButton, "the game header's Cross-table button");
    headerButton.click();
    await until(() => title() === "Spring Round Robin" && bodyRows().length === 6, "the cross-table from the game");
    const current = $$(".cross button.res.current");
    expect(current.length === 2, "the game highlighted in both players' rows, got " + current.length);
    expect(document.activeElement === current[0], "the focus on the game on screen");
    await shot("03-from-the-game");
    // Show games from here: the list pane is on its Events tab, and brings
    // its Games tab forward with the event's games.
    expect(listTab("event").getAttribute("aria-selected") === "true", "the Events tab showing");
    showGames().click();
    await until(() => !dialog() && db.info?.filter?.count === 15, "the list limited to the event's 15 games");
    await until(() => listTab("games").getAttribute("aria-selected") === "true", "the Games tab brought forward");
    expect(db.info.filter.description === "Event: Spring Round Robin", "the filter, got " + db.info.filter.description);
    expect(game.view.source.game === opened, "Show games leaves the game on screen");
    await db.clearFilter();
    await eventsTab();

    // A new game has no source in the database: no button.
    await game.newGame(null, "tab");
    await until(() => !game.view?.source, "a new game");
    expect(!$$("#panel-notation header button").some((b) => text(b) === "Cross-table"), "no Cross-table button for a new game");
    await game.closeTab(game.activeSession);

    // ---- A Swiss with byes, three players on 2½ and two level on everything.
    await openCrossTable("Autumn Open");
    expect(summary() === "Swiss · 7 players · 3 rounds · 12 games · Delft · 2024.10.01 – 2024.10.03", "the summary, got " + summary());
    expect(view("Rounds")?.getAttribute("aria-pressed") === "true", "the rounds view first for a Swiss");
    expect(names().join(" | ") === "Heath, Hana | Gale, Gus | Juniper, Jon | Iris, Ida | Kestrel, Kim | Linden, Lea | Moss, Max",
      "the Swiss standings, got " + names().join(" | "));
    const sheads = $$(".cross thead th").map(text).slice(-2).join(" ");
    expect(sheads === "BH-C1 BH", "Buchholz tie-breaks, got " + sheads);
    expect(stats(bodyRows()[0]).slice(-2).join(" ") === "4 4.5" && stats(bodyRows()[1]).slice(-2).join(" ") === "3.5 4.5",
      "Buchholz cut 1 splits Heath and Gale");
    const pos = (r) => text($("td.pos", bodyRows()[r]));
    expect(pos(4) === "5–6" && pos(5) === "5–6" && pos(3) === "4", "the shared places on both rows, got " + [pos(3), pos(4), pos(5)]);
    expect($("td.pos", bodyRows()[4]).title === "Shares places 5–6: level on points and every tie-break", "the shared places' tooltip");
    const kestrel = $$("td.rounds", bodyRows()[4]).map(text);
    expect(kestrel.join(" | ") === "bye1 | 20 | 30", "Kestrel's bye and losses, got " + kestrel.join(" | "));
    const moss = $$("td.rounds", bodyRows()[6]).map(text);
    expect(moss[1] === "bye½", "a half-point bye, got " + moss[1]);
    expect(text($(".cross .notes")) === "3 byes count for points, not for the performance.", "the note on byes");
    await shot("04-swiss");
    await closeDialog();

    // ---- A knock-out: the third-place match is in the final's round.
    await openCrossTable("Summer Cup");
    expect(summary() === "Knock-out · 4 players · 2 rounds · 8 games · Utrecht · 2024.07.13 – 2024.07.14",
      "the summary, got " + summary());
    expect(names().join(" | ") === "Thorn, Tess | Umber, Uma | Vale, Victor | Willow, Wes",
      "the champion, the final's loser, then third and fourth, got " + names().join(" | "));
    expect(column("pos").join(" ") === "1 2 3 4", "four places, none shared, got " + column("pos").join(" "));
    expect(column("pts").join(" ") === "3 2 2 1", "the points, got " + column("pts").join(" "));
    expect($$(".cross thead th").map(text).at(-1) === "Perf", "no tie-breaks in a knock-out");
    await shot("04b-knock-out");
    await closeDialog();

    // ---- Two editions: 2024 first, then the 2023 double round robin.
    await openCrossTable("Club Championship");
    const select = $(".cross .edition select");
    expect(select, "an edition picker");
    const options = $$("option", select).map(text);
    expect(options.join(" | ") === "2023 · 6 games | 2024 · 6 games", "the editions, got " + options.join(" | "));
    expect(select.value === "1" && summary().startsWith("Round robin · 4 players"), "the newest edition first, got " + summary());
    select.value = "0";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await until(() => summary().startsWith("Double round robin · 3 players"), "the 2023 edition");
    expect(names().join(" | ") === "Oak, Olga | Pine, Paul | Rowan, Rita", "the 2023 standings, got " + names().join(" | "));
    expect(symbols(cell(0, 1)) === "1 ½" && symbols(cell(1, 0)) === "0 ½", "both games of a pairing, in the order played");
    await shot("05-double-round-robin");

    // Light mode, for the look.
    appearance.theme = "light";
    await until(() => document.documentElement.dataset.theme === "light", "light mode");
    await shot("06-light");
    appearance.theme = "system";

    // ---- Show games: the list limited to the edition shown (2023), not the
    // event's 12 games.
    showGames().click();
    await until(() => !dialog() && db.info?.filter?.count === 6, "the list limited to the edition's 6 games");
    expect(listTab("games").getAttribute("aria-selected") === "true", "the game list showing");
    expect(db.info.filter.description === "Event: Club Championship (2023)", "the filter, got " + db.info.filter.description);
    await db.clearFilter();

    // ---- Real events from TWIC, when the file is there.
    await db.createAt(OUT + "/twic.chessdb");
    if (await db.importFrom(ROOT + "/testdata/large/twic1600.pgn")) {
      expect(db.info?.games > 8000, "TWIC imported, got " + db.info?.games);
      await eventsTab();
      let ms = await openCrossTable("Ceske Budejovice GMA 2025");
      expect(summary().startsWith("Round robin · 10 players · 9 rounds · 45 games"), "a TWIC round robin, got " + summary());
      await log(`TWIC round robin: ${ms.toFixed(0)} ms`);
      await shot("07-twic-round-robin");
      await closeDialog();
      ms = await openCrossTable("Titled Tue 1st Jul Early");
      expect(summary().startsWith("Swiss · 597 players · 11 rounds · 2,561 games"), "a big Swiss, got " + summary());
      expect(!view("Grid"), "no grid for 597 players");
      await log(`TWIC Swiss of 597 players: ${ms.toFixed(0)} ms`);
      await shot("08-twic-swiss");
      await closeDialog();
      await openCrossTable("FIDE Women's World Cup");
      expect(summary().startsWith("Knock-out · 86 players · 1 round"), "a knock-out, got " + summary());
      // Ranked by the round reached and the match won there: no tie-breaks;
      // those through to round 2 share the top places.
      const kheads = $$(".cross thead th").map(text);
      expect(kheads.at(-1) === "Perf", "no tie-break columns for a knock-out, got " + kheads.at(-1));
      const through = $("td.pos", bodyRows()[0]).title;
      expect(/^Shares places 1–\d+:/.test(through) && column("pts")[0] === "2", "the winners of round 1 first, got " + through);
      // Round 1's tie-breaks aren't in this issue: its level matches are noted.
      const knotes = text($(".cross .notes"));
      expect(/^\d+ matches ended level on points/.test(knotes), "the note on level matches, got " + knotes);
      await shot("09-twic-knock-out");
      await closeDialog();
    } else {
      await log("no testdata/large/twic1600.pgn: skipping the TWIC events");
    }

    // ---- The 1M Lichess games: too big for a table, and an arena.
    if (BIG) {
      await db.createAt(OUT + "/big.chessdb");
      expect(await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn"), "the 1M games imported");
      expect(db.info?.games >= 1000000, "1M games, got " + db.info?.games);
      await eventsTab();
      let ms = await openCrossTable("Rated Blitz game");
      const message = text($(".cross .message"));
      expect(/^This edition has [\d,]+ players and [\d,]+ games: too many for a cross-table/.test(message),
        "the too-big message, got " + message);
      await log(`1M games, Rated Blitz game (too big): ${ms.toFixed(0)} ms`);
      await shot("10-too-big");
      await closeDialog();
      // 1,130 players without rounds, one with 138 games: the standings only.
      ms = await openCrossTable("Rated UltraBullet game");
      const only = text($(".cross .standings-only"));
      expect(/^Standings only: [\d,]+ players with up to [\d,]+ games each are too many/.test(only),
        "the standings-only message, got " + only);
      expect(!$(".cross td.g") && !$(".cross button.res") && bodyRows().length > 1000, "no result columns, every player");
      await log(`1M games, Rated UltraBullet game (standings only): ${ms.toFixed(0)} ms`);
      await shot("10b-standings-only");
      await closeDialog();
      ms = await openCrossTable("Rated Blitz tournament https://lichess.org/tournament/WYkYnRWN");
      expect(summary().startsWith("No rounds · "), "an arena has no rounds, got " + summary());
      expect($(".cross td.rounds") && $$(".cross thead th.g").map(text).slice(0, 3).join(" ") === "1 2 3",
        "the arena's games in order, as columns 1, 2, 3…");
      await log(`1M games, an arena: ${ms.toFixed(0)} ms`);
      await shot("11-arena");
      await closeDialog();
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e && e.stack ? e.stack : e));
  }
})();
