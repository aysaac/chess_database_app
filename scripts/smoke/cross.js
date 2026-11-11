// Smoke-test driver for SRCH-13, searching several databases at once: two
// databases open (one in the list, the other as the reference database),
// the search dialog's "Search in" choice, the Search results tab with its
// Database column and counts, sorting it by a header (names compared
// across the databases), a row selected by a click and by the keys, a row
// of the other database opened from its own database by a double-click,
// a search limited to the position on the board (its count matching the
// list's own position search), and closing the results.
//
// The main window is the library (DB-07): there the driver only opens an
// editor window, which runs the same script and does the checks.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb?.cross) await wait(100);
  const { db, game, api, invoke, cross } = window.__chessdb;
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
  const until = async (cond, what, tries = 100) => {
    for (let i = 0; i < tries && !cond(); i++) await wait(50);
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

  const fold = (s) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();
  const rows = () => [...document.querySelectorAll('section[aria-label="Search results"] .viewport .row')];
  const cellText = (row, i) => row.querySelectorAll(".cell")[i]?.textContent ?? "";

  try {
    await log("start");
    const a = OUT + "/Reference games.chessdb";
    const b = OUT + "/Club games.chessdb";
    await db.createAt(a);
    await db.importFrom(ROOT + "/testdata/pgn/reference.pgn");
    const aGames = db.info.games;
    await db.createAt(b);
    await db.importFrom(ROOT + "/testdata/pgn/crosstable.pgn");
    const bGames = db.info.games;
    await db.setReference(a);
    await until(() => db.reference?.info, "the reference database open");
    const open = await api.openDatabases();
    expect(open.length === 2, "two open databases, not " + JSON.stringify(open));
    await log(`open: ${open.map((d) => `${d.name} ${d.games}${d.reference ? " (reference)" : ""}`).join(", ")}`);

    // The search dialog, from the filter bar.
    const searchButton = [...document.querySelectorAll(".bar button")].find((x) => x.textContent.trim() === "Search…");
    expect(!!searchButton, "the Search… button");
    searchButton.click();
    await until(() => document.querySelector("fieldset.dbs"), "the Search in choice");
    const boxes = () => [...document.querySelectorAll("fieldset.dbs label.check")];
    expect(boxes().length === 3, "two databases and the position option");
    const box = (name) => boxes().find((l) => l.textContent.includes(name))?.querySelector("input");
    expect(box("Club games").checked && !box("Reference games").checked, "this window's database chosen at first");
    box("Reference games").click();
    await wait(100);
    const white = [...document.querySelectorAll('[role="dialog"] label')].find((l) => l.textContent.startsWith("White"));
    const input = white.querySelector("input");
    input.value = "a";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await shot("01-dialog");
    const go = [...document.querySelectorAll('[role="dialog"] button')].find((x) => x.textContent.trim() === "Search");
    go.click();

    await until(() => cross.info && !cross.searching, "the search's results");
    const info = cross.info;
    await log(`results: ${info.total} games, ${info.databases.map((d) => `${d.name} ${d.count}`).join(", ")}`);
    expect(info.databases.length === 2, "both databases searched");
    expect(info.total === info.databases.reduce((s, d) => s + d.count, 0), "the total is the sum");
    expect(info.total > 0 && info.total < aGames + bGames, "some games, not all");
    const tab = [...document.querySelectorAll(".tabs button")].find((t) => t.textContent === "Search results");
    expect(tab?.getAttribute("aria-selected") === "true", "the Search results tab shown");
    await until(() => rows().length > 3 && rows()[0].querySelector(".cell"), "rows");
    const head = [...document.querySelectorAll('section[aria-label="Search results"] .head .cell')].map((c) => c.textContent);
    expect(head[0] === "Database", "a Database column first, not " + head.join("|"));
    const names = new Set(rows().map((r) => cellText(r, 0)).filter(Boolean));
    await log("databases in the first rows: " + [...names].join(", "));
    // Natural order: the first database's games, then the other's (sorted by name).
    expect(cellText(rows()[0], 0) === "Club games", "Club games first (by name)");
    await shot("02-results");

    // Sort by White: names merged across databases, case-insensitively.
    const sortBy = (label) =>
      [...document.querySelectorAll('section[aria-label="Search results"] .head button')]
        .find((x) => x.textContent.startsWith(label))
        .click();
    sortBy("White");
    await until(() => cross.sort.key === "white" && cross.sortingKey === null, "sorted by White");
    const page = await api.searchPage(info.search, cross.sort, 0, 1000);
    expect(page.rows.length === info.total, "a page of every row");
    const whites = page.rows.map((r) => r.row.white.toLowerCase());
    for (let i = 1; i < whites.length; i++) {
      expect(!whites[i - 1] || !whites[i] ? true : whites[i - 1] <= whites[i], `White in order at ${i}: ${whites[i - 1]} ${whites[i]}`);
    }
    expect(page.rows.every((r) => fold(r.row.white).includes("a")), "every White has an a");
    expect(new Set(page.rows.map((r) => r.handle)).size === 2, "rows from both databases");
    await until(() => cellText(rows()[0], 2).toLowerCase() === whites[0], "the list shows the sorted rows");
    await shot("03-by-white");

    // Clicking selects; the keys move the selection.
    rows()[2].dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
    await wait(100);
    expect(rows()[2].classList.contains("selected"), "the clicked row selected");
    const viewport = document.querySelector('section[aria-label="Search results"] .viewport');
    viewport.focus();
    viewport.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    await wait(100);
    expect(rows()[3].classList.contains("selected") && !rows()[2].classList.contains("selected"), "ArrowDown moved it");

    // A row of the reference database opens from its own database.
    const refHandle = info.databases.find((d) => d.name === "Reference games").handle;
    const index = page.rows.findIndex((r) => r.handle === refHandle);
    expect(index >= 0, "a reference row");
    const target = page.rows[index];
    viewport.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true }));
    for (let i = 0; i < index; i++) viewport.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    await wait(200);
    const row = rows().find((r) => r.dataset.index === String(index));
    expect(!!row && row.classList.contains("selected"), "the reference row selected");
    row.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await until(() => game.view?.source?.db === refHandle && game.view.source.game === target.row.id, "the game opened from the reference database");
    expect(game.view.headers.white === target.row.white, "the right game");
    await until(() => row.classList.contains("open"), "the open game marked in the list");
    await log(`opened ${target.row.white} – ${target.row.black} (game ${target.row.id + 1} of Reference games)`);
    await shot("04-opened");

    // Sort by date, newest first.
    sortBy("Date");
    await until(() => cross.sort.key === "date" && cross.sortingKey === null, "sorted by date");
    sortBy("Date");
    await until(() => cross.sort.key === "date" && cross.sort.descending && cross.sortingKey === null, "newest first");
    const byDate = (await api.searchPage(info.search, cross.sort, 0, 1000)).rows.map((r) => r.row.date);
    const known = byDate.filter((d) => !d.startsWith("?"));
    for (let i = 1; i < known.length; i++) expect(known[i - 1] >= known[i], `dates descending at ${i}`);

    // The position on the board: after 1.e4 in the opened game.
    await game.goto(game.notation.moves[0].id);
    await wait(200);
    await cross.run(info.databases.map((d) => d.handle), {}, game.session);
    const pos = cross.info;
    await log(`after 1.e4: ${pos.total} games, ${pos.databases.map((d) => `${d.name} ${d.count}`).join(", ")}`);
    expect(pos.description.startsWith("Position"), "described by the position");
    await db.searchPosition(game.session);
    const clubCount = pos.databases.find((d) => d.name === "Club games").count;
    expect(db.info.filter.count === clubCount, `the list's own position search agrees: ${db.info.filter.count} vs ${clubCount}`);
    await db.clearFilter();
    await shot("05-position");

    // Closing the results takes the tab away.
    document.querySelector('button[aria-label="Close the search results"]').click();
    await until(() => !cross.info, "the results closed");
    await until(() => ![...document.querySelectorAll(".tabs button")].some((t) => t.textContent === "Search results"), "the tab gone");
    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e) + " " + (e?.stack ?? ""));
  }
})();
