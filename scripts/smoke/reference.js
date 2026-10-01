// Smoke-test driver for the Reference tab (OPEN-14), on the fixture
// testdata/pgn/reference.pgn whose counts are known: the third panel tab and
// its ribbon button, the moves table with Hot and Best players, the main
// line and the lines, the top games' order, a move played from the table,
// the mini board stepping through a game or a line without moving the main
// board (and going back when the main board moves), opening a top game
// where it reaches the position, queries only while the tab shows, the
// section dividers (remembered), the narrow (stacked) layout, the note when
// no reference database is set, and the reference database read while
// another is open. With the 1M-game file (__BIG__) it times the tab's
// commands at several positions and a cancelled lines job.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";
  const BIG = "__BIG__" === "1";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, panels, reference, jobs, notices, api, invoke } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden monitor: never ask about unsaved changes.
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
  const untilAsync = async (cond, what, ms = 5000) => {
    const end = performance.now() + ms;
    while (performance.now() < end) {
      if (await cond()) return;
      await wait(100);
    }
    expect(await cond(), what);
  };
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const text = (el) => (el?.textContent ?? "").replace(/\s+/g, " ").trim();
  const shown = (el) => !!el && el.getClientRects().length > 0;
  const key = (el, k, opts = {}) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...opts }));
  const pointer = (type, x, y) => {
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
      }),
    );
  };
  const rb = (label) => $$(".ribbon > .groups button").find((b) => b.getAttribute("aria-label") === label);
  const P = "#panel-reference ";
  const tab = (id) => $(`[role='tab'][data-panel='${id}']`);
  const cur = () => reference.current;
  const lines = () => reference.currentLines;
  const moveList = () => cur().moves.map((m) => `${m.row.san ?? "end"}:${m.row.games}`).join(" ");
  const sans = (positions) => positions.filter((p) => p.san).map((p) => p.san).join(" ");
  const lineList = () => lines().lines.map((l) => `${sans(l.positions)}:${l.games}`);
  const moveRows = () => $$(P + "section.moves tbody tr");
  const lineRows = () => $$(P + ".lines tbody tr");
  const gameRows = () => $$(P + ".top-games tbody tr");
  const grid = () => $(P + ".top-games table");
  const caption = () => text($(P + ".mini .caption"));
  const miniButton = (label) => $(P + `.mini button[aria-label="${label}"]`);
  const miniPieces = () => $$(P + ".mini .board piece").map((p) => `${p.cgKey}:${p.className}`);
  const indexed = () => db.info?.index.built && db.info.index.upToDate;
  const START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  // Every query the tab makes.
  let views = 0;
  const referenceView = api.referenceView;
  api.referenceView = (...args) => {
    views++;
    return referenceView(...args);
  };

  try {
    await log("start");
    await db.createAt(OUT + "/reference.chessdb");
    await db.importFrom(ROOT + "/testdata/pgn/reference.pgn");
    await until(indexed, "the index built after the import", 20000);
    expect(db.info.games === 56, "56 games, got " + db.info.games);
    await game.newGame();

    // ---- The third panel tab, shown from the ribbon; nothing queried before.
    const labels = $$(".game-column [role='tablist'] [role='tab']").map((t) => text($(".label:not([aria-hidden])", t)));
    expect(labels.join(" | ") === "Notation | Opening tree | Reference", "three panel tabs, got " + labels.join(" | "));
    await wait(300);
    expect(!panels.isShowing("reference") && views === 0, "no Reference query while hidden, got " + views);
    rb("Reference").click();
    await until(() => panels.isShowing("reference") && shown($("#panel-reference")), "the Reference tab showing");
    expect(rb("Reference").getAttribute("aria-pressed") === "true", "the ribbon's Reference button pressed");
    expect(tab("reference").getAttribute("aria-selected") === "true", "its tab selected");
    await until(() => cur()?.fen === START, "the view of the starting position");
    expect(views === 1, "one query when the tab showed, got " + views);

    // ---- No reference database: it reads the open one and says so.
    const note = text($(P + ".fallback"));
    expect(note === "Reading reference: no reference database is set. Choose one in the ribbon.", "the note, got " + note);
    const header = text($(P + "header .name"));
    expect(header === "reference · 56 games", "the header, got " + header);

    // ---- 3a: moves, Hot, Best players.
    expect(moveList() === "e4:39 d4:11 c4:5 Nf3:1", "the moves, got " + moveList());
    await until(() => moveRows().length === 4, "four rows in the moves table");
    const rows = moveRows();
    expect($(".trend.down", rows[0]) && !$(".trend.up", rows[0]), "1.e4 is played less lately (↓)");
    expect($(".trend.up", rows[1]), "1.d4 is played more lately (↑)");
    expect(!$(".trend", rows[2]) && !$(".trend", rows[3]), "no arrow for 1.c4 and 1.Nf3");
    const hotTitle = $(".hot", rows[0]).title;
    expect(hotTitle === "38% of games from 2018–2020 (70% over all years); played less lately", "Hot's tooltip, got " + hotTitle);
    const best = (r) => text($("td.best-col", r));
    expect(best(rows[0]) === "Carlsen, Caruana, So, Nakamura", "1.e4's best players, got " + best(rows[0]));
    expect($("td.best-col .player", rows[0]).title === "Carlsen, Magnus (2850)", "the full name and Elo as tooltip");
    expect(best(rows[1]) === "Kramnik, Anand, Giri" && best(rows[2]) === "Kasparov, Short, Timman" && best(rows[3]) === "",
      "the other moves' best players, got " + rows.slice(1).map(best).join(" / "));

    // ---- 3b: the main line and the lines.
    await until(() => lines(), "the lines");
    const main = lines().mainLine;
    expect(sans(main.positions) === "e4 e5 Nf3 Nc6 Bb5 a6" && main.games === 20, "the main line, got " + sans(main.positions));
    const mainText = text($(P + ".main-line"));
    expect(mainText === "Main line: 1.e4 e5 2.♘f3 ♘c6 3.♗b5 a6 (20 games)", "the main line shown, got " + mainText);
    const expected = [
      "e4 e5 Nf3 Nc6 Bb5 a6:20",
      "e4 e6 d4 d5 Nc3 Bb4:6",
      "e4 c6 d4 d5:5",
      "d4 d5 c4 e6 Nc3 Nf6:6",
      "d4 Nf6:5",
      "c4 e5 Nc3 Nf6:5",
      "Nf3:1",
    ];
    expect(lineList().join(" | ") === expected.join(" | "), "the lines, got " + lineList().join(" | "));
    await until(() => lineRows().length === 7, "seven line rows");
    const groups = lineRows().map((r, i) => (r.classList.contains("group") ? i : -1)).filter((i) => i >= 0);
    expect(groups.join() === "3,5,6", "groups start at 1.d4, 1.c4 and 1.Nf3, got " + groups);
    expect(text($("td", lineRows()[1])) === "1.e4 e6 2.d4 d5 3.♘c3 ♗b4", "a line's moves, got " + text($("td", lineRows()[1])));

    // ---- 3c: top games, strongest first.
    const whites = gameRows().slice(0, 8).map((r) => text($("td", r)));
    expect(whites.join(" | ") === ["Carlsen, Magnus", "Kasparov, Garry", "Kramnik, Vladimir", "Carlsen, Magnus",
      "So, Wesley", "Anand, Viswanathan", "Nakamura, Hikaru", "Caruana, Fabiano"].join(" | "), "the top games' order, got " + whites.join(" | "));
    expect(gameRows().length === 56 && text($("td", gameRows()[55])) === "Club player 87", "all 56, the undated game without Elos last");
    await shot("ref01-start");

    // ---- Clicking a move plays it on the main board.
    $("button.move", moveRows()[1]).click();
    await until(() => game.view.cursorPath.length === 1 && game.view.turn === "black", "1.d4 played");
    const afterD4 = game.view.fen;
    await until(() => cur()?.fen === afterD4 && moveList() === "d5:6 Nf6:5", "the moves after 1.d4, got " + (cur() && moveList()));
    await until(() => moveRows().length === 2 && best(moveRows()[0]) === "Topalov, Club player 69", "Black's best players");

    // ---- Selecting a game: the mini board steps on its own.
    gameRows()[0].click();
    await until(() => reference.mini?.kind === "game", "the game on the mini board");
    expect(reference.mini.start === 1 && reference.mini.index === 1, "at the board's position (ply 1)");
    expect(caption() === "1.d4 · Kramnik – Topalov, 2020", "the caption, got " + caption());
    expect(gameRows()[0].getAttribute("aria-selected") === "true", "the row selected");
    const mini = $(P + ".mini");
    mini.focus();
    key(mini, "ArrowRight");
    await until(() => reference.mini.index === 2 && caption() === "1…d5 · Kramnik – Topalov, 2020", "→ steps the mini board, got " + caption());
    await until(() => miniPieces().includes("d5:black pawn"), "the mini board drawing 1…d5");
    expect(game.view.fen === afterD4, "the main board didn't move");
    key(mini, "End");
    await until(() => caption() === "4…♗e7 · Kramnik – Topalov, 2020", "End: the game's end, got " + caption());
    key(mini, "Home");
    await until(() => caption() === "Start · Kramnik – Topalov, 2020", "Home: its start, got " + caption());
    miniButton("Next move").click();
    await until(() => reference.mini.index === 1, "the Next move button");
    miniButton("Last move").click();
    miniButton("Previous move").click();
    await until(() => reference.mini.index === 7, "Last, then Previous move");
    expect(miniButton("First move") && !miniButton("Next move").disabled, "the buttons");
    expect(game.view.fen === afterD4 && game.view.cursorPath.length === 1, "the main board still after 1.d4");
    // ↑/↓ in the top games select the next game; ←/→ there step the mini board.
    grid().focus();
    expect(document.activeElement === grid(), "the top games focused");
    key(grid(), "ArrowDown");
    await until(() => reference.mini?.title === "Anand – Unknown, 2018", "↓ selects the next game, got " + reference.mini?.title);
    key(grid(), "ArrowRight");
    await until(() => reference.mini.index === 2, "→ in the top games steps the mini board");
    expect(game.view.fen === afterD4, "and not the main board");
    await shot("ref02-mini-game");
    // ⟲ goes back to the main board's position.
    miniButton("Back to the board's position").click();
    await until(() => reference.mini === null && caption() === "The position on the board", "⟲ back to the board");

    // ---- A move on the main board takes the mini board back.
    gameRows()[0].click();
    await until(() => reference.mini?.kind === "game", "a game on the mini board again");
    await game.step("back");
    await until(() => reference.mini === null && caption() === "The position on the board", "reset by a main-board move");
    await until(() => cur()?.fen === START && lines(), "back at the start, with its lines");

    // ---- A line on the mini board.
    $(P + ".main-line button.main").click();
    await until(() => reference.mini?.kind === "line" && reference.mini.index === 0, "the main line on the mini board");
    expect(caption() === "The board's position · Main line (20 games)", "its caption, got " + caption());
    for (let i = 0; i < 3; i++) miniButton("Next move").click();
    await until(() => caption() === "2.♘f3 · Main line (20 games)", "stepping the main line, got " + caption());
    $("button.line", lineRows()[3]).click();
    await until(() => reference.mini?.key === "line:3", "line 4 on the mini board");
    miniButton("Last move").click();
    await until(() => caption() === "3…♘f6 · Line 4 (6 games)", "line 4's end, got " + caption());
    expect(lineRows()[3].classList.contains("shown"), "line 4 marked as shown");
    expect(game.view.fen === START, "the main board didn't move");
    await shot("ref03-mini-line");

    // ---- Opening a top game: Enter opens it where it reaches the position.
    $("button.move", moveRows()[1]).click();
    await until(() => cur()?.fen === game.view.fen && game.view.cursorPath.length === 1, "1.d4 again");
    const top = cur().topGames[0];
    gameRows()[0].click();
    await until(() => reference.mini?.kind === "game", "selected");
    grid().focus();
    key(grid(), "Enter");
    await until(() => game.view.source?.game === top.id && game.view.cursorPath.length === top.ply,
      `Enter opened game ${top.id} at ply ${top.ply}, got ${JSON.stringify(game.view.source)} at ${game.view.cursorPath.length}`);
    // The menu: Open in a new tab.
    const tabs = game.tabs.length;
    await until(() => cur()?.fen === game.view.fen && gameRows().length > 1, "the tab for the opened game's position");
    const second = cur().topGames[1];
    gameRows()[1].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 300 }));
    await until(() => $(".menu[aria-label='Top game']"), "the top game's menu");
    expect(document.activeElement?.textContent === "Open game", "its first item focused");
    $$(".menu[aria-label='Top game'] button").find((b) => b.textContent === "Open in a new tab").click();
    await until(() => game.tabs.length === tabs + 1 && game.view.source?.game === second.id && game.view.cursorPath.length === second.ply,
      "opened in a new tab where it reaches the position");
    await game.newGame();

    // ---- Queries only while the tab shows, and on each move while it does.
    await until(() => cur()?.fen === START, "the start");
    panels.show("notation");
    let before = views;
    await game.playUci("e2e4");
    await game.step("back");
    await wait(400);
    expect(views === before, "no query while hidden, got " + (views - before));
    panels.show("reference");
    await until(() => views === before + 1 && cur()?.fen === game.view.fen, "one query when shown again, got " + (views - before));
    before = views;
    await game.playUci("e2e4");
    await until(() => views === before + 1 && cur()?.fen === game.view.fen, "one query per move");
    await game.step("back");
    await until(() => cur()?.fen === START, "back at the start");

    // ---- The dividers: drag, keys, double-click; remembered.
    const dividers = $$(P + "[role='separator']");
    expect(dividers.length === 2, "two dividers");
    const movesH = () => $(P + "section.moves").getBoundingClientRect().height;
    const linesH = () => $(P + "section.lines").getBoundingClientRect().height;
    const h0 = movesH();
    const l0 = linesH();
    const d = dividers[0].getBoundingClientRect();
    const x = d.left + d.width / 2;
    const y = d.top + d.height / 2;
    pointer("pointerdown", x, y);
    pointer("pointermove", x, y + 30);
    pointer("pointermove", x, y + 60);
    pointer("pointerup", x, y + 60);
    await until(() => Math.abs(movesH() - (h0 + 60)) < 3 && Math.abs(linesH() - (l0 - 60)) < 3,
      `the moves 60px taller, the lines 60px shorter: ${h0}→${movesH()}, ${l0}→${linesH()}`);
    await untilAsync(async () => {
      const s = (await api.uiPrefs()).referenceSizes;
      return s && Math.abs(s.moves - reference.sizes.moves) < 0.002 && s.moves > 0.3;
    }, "the sizes saved");
    const l1 = linesH();
    dividers[1].focus();
    key(dividers[1], "ArrowUp");
    await until(() => Math.abs(linesH() - (l1 - 16)) < 3, "↑ on the second divider: 16px off the lines");
    await shot("ref04-resized");
    dividers[0].dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await until(() => reference.sizes.moves === 0.3 && reference.sizes.lines === 0.25, "double-click: the default sizes");

    // ---- Wide: the top games and the mini board side by side; narrow: stacked.
    const bottom = () => getComputedStyle($(P + "section.bottom")).display;
    expect(bottom() === "grid", "side by side at the window's width, got " + bottom());
    const app = document.getElementById("app");
    app.style.width = "900px";
    await until(() => bottom() === "flex", "stacked in a narrow tab, got " + bottom());
    const miniBox = $(P + ".mini .board").getBoundingClientRect();
    const gamesBox = $(P + ".games-cell").getBoundingClientRect();
    expect(miniBox.top >= gamesBox.bottom - 1, "the mini board under the games");
    expect(Math.abs(miniBox.width - miniBox.height) < 1 && miniBox.width >= 120, "a square mini board, got " + miniBox.width);
    await shot("ref05-narrow");
    app.style.width = "";
    await until(() => bottom() === "grid", "side by side again");
    await wait(300);
    const wideBox = $(P + ".mini .board").getBoundingClientRect();
    expect(wideBox.width <= 280.5 && wideBox.width >= 120 && Math.abs(wideBox.width - wideBox.height) < 1,
      "the mini board 120–280px and square, got " + wideBox.width);
    await log(`  mini board ${Math.round(wideBox.width)}px wide, ${Math.round(miniBox.width)}px stacked`);
    const cg = $(P + ".mini .board cg-container").getBoundingClientRect();
    expect(Math.abs(cg.width - wideBox.width) < 1, `chessground fills the mini board: ${cg.width} in ${wideBox.width}`);

    // ---- A reference database: read even with another database in the list.
    const refPath = db.info.path;
    await db.setReference(refPath);
    await until(() => !$(P + ".fallback"), "no note with a reference database");
    await db.createAt(OUT + "/other.chessdb");
    await db.importFrom(ROOT + "/testdata/pgn/variations.pgn");
    await until(indexed, "the other database indexed", 20000);
    await game.newGame();
    await until(() => reference.target?.reference && reference.target.handle !== db.info.handle && cur()?.total.games === 56,
      "the tab reads the reference, not the open database");
    expect(text($(P + "header .name")) === "reference · 56 games", "its name in the header");
    await shot("ref06-reference-db");
    await reference.findGames();
    expect(db.info.path === refPath && db.info.filter?.count === 56, "Find games opened the reference with its 56 games");
    await db.setReference(null);
    await until(() => text($(P + ".fallback")).startsWith("Reading reference: no reference database is set"),
      "the note again, got " + text($(P + ".fallback")));

    // ---- Past the indexed plies: the tree's note and the full search.
    await game.newGame("8/8/4k3/8/8/4K3/8/8 w - - 0 60");
    await until(() => cur()?.fen === game.view.fen, "an endgame");
    expect(cur().total.games === 0 && cur().beyondIndex, "no games, beyond the index");
    const deep = text($(P + ".note"));
    expect(deep.includes("within the first 25 moves") && $(P + ".note button")?.textContent === "Search every move of every game",
      "the note and the full search, got " + deep);

    if (BIG) {
      // Timed straight through the commands, with the tab hidden so it
      // doesn't count lines meanwhile.
      panels.show("notation");
      await db.createAt(OUT + "/big.chessdb");
      let t = performance.now();
      await db.importFrom(ROOT + "/testdata/large/lichess_1m.pgn");
      await log(`imported 1M games in ${Math.round((performance.now() - t) / 1000)}s`);
      await until(indexed, "1M games indexed", 300000);
      const handle = db.info.handle;
      const time = async (f) => {
        const t0 = performance.now();
        const out = await f();
        return [out, Math.round(performance.now() - t0)];
      };
      await game.newGame();
      let [v, ms] = await time(() => api.referenceView(handle, game.view.session));
      await log(`reference view at the start, first use (loads the per-game stats): ${v.total.games} games, ${ms} ms`);
      const positions = [
        ["the start", []],
        ["1.e4", ["e2e4"]],
        ["1.d4 d5 2.c4", ["d2d4", "d7d5", "c2c4"]],
        ["1.e4 c5 2.Nf3 d6", ["e2e4", "c7c5", "g1f3", "d7d6"]],
        ["the Najdorf", ["e2e4", "c7c5", "g1f3", "d7d6", "d2d4", "c5d4", "f3d4", "g8f6", "b1c3", "a7a6"]],
        ["move 15 of a game", null],
      ];
      for (const [label, ucis] of positions) {
        if (ucis) {
          await game.newGame();
          for (const u of ucis) await game.playUci(u);
        } else {
          // A position from the middle of a real game: open the first game at ply 30.
          await game.open(handle, 0, null, "auto", 30);
        }
        const session = game.view.session;
        [v, ms] = await time(() => api.referenceView(handle, session));
        let [l, lms] = await time(() => api.referenceLines(handle, session));
        let how = "counted";
        if (l.kind === "job") {
          const t0 = performance.now();
          const done = await jobs.waitFor(l.job);
          expect(done.state === "done", "the lines job finished, got " + done.state);
          lms += Math.round(performance.now() - t0);
          l = await api.referenceLines(handle, session);
          how = "a job";
        }
        expect(l.kind === "ready", "the lines ready");
        const [, cached] = await time(() => api.referenceLines(handle, session));
        await log(`reference at ${label}: ${v.total.games} games; moves, Hot, Best players and top games ${ms} ms ` +
          `(target 200); lines ${lms} ms as ${how}${v.total.games < 50000 ? " (target 500)" : ""}, ${cached} ms cached; ` +
          `${l.lines.lines.length} lines, main line ${l.lines.mainLine ? l.lines.mainLine.positions.length - 1 : 0} plies`);
        // A generous bound so a slow machine doesn't fail the run; the
        // numbers above are what to compare with the targets.
        expect(ms < 800, `the view at ${label} under 800 ms, got ${ms}`);
        if (v.total.games < 50000) expect(lms < 2000 && how === "counted", `the lines at ${label} counted directly, quickly: ${lms} ms`);
      }

      // The tab itself at the start (its lines cached by now).
      await game.newGame();
      panels.show("reference");
      t = performance.now();
      await until(() => cur()?.fen === START && cur().total.games > 900000 && lines(), "the tab on 1M games", 20000);
      await log(`the Reference tab at the start, shown: ${Math.round(performance.now() - t)} ms`);
      await shot("ref07-big");

      // A common position starts a job; moving on cancels it, quietly.
      // (1.d4's lines take well under a second: step back at once.)
      notices.current = null;
      await game.playUci("d2d4");
      await until(() => reference.linesJob, "a job counting the lines after 1.d4", 10000);
      const job = reference.linesJob.id;
      await game.step("back");
      const done = await jobs.waitFor(job);
      expect(done.state === "cancelled", "the job cancelled when the board moved on, got " + done.state);
      await wait(300);
      expect(!notices.current, "no notice for it, got " + JSON.stringify(notices.current));
      await until(() => cur()?.fen === START && lines(), "the start's lines again");
      // Its progress in the tab, on another common position (shot at once).
      await game.newGame();
      await game.playUci("e2e4");
      await game.playUci("e7e5");
      expect(game.view.cursorPath.length === 2, "1.e4 e5 on the board");
      await until(() => reference.linesJob || lines(), "the lines after 1.e4 e5 being counted", 10000);
      if (reference.linesJob) {
        expect(text($(P + ".main-line")).startsWith("Counting the lines of"), "its progress in the tab");
        await log("shot:ref08-counting");
      }
      await until(() => lines(), "the lines after 1.e4 e5", 20000);
      await log(`  lines after 1.e4 e5: ${lines().games} games, ${lines().lines.length} lines`);
    }

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
