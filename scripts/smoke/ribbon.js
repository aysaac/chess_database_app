// Smoke-test driver for the ribbon (UI-18), with paste game (IO-08) and the
// reference dropdown (DB-13, DB-07): the groups and their buttons, copying
// and pasting a game through the private display's clipboard (the harness
// saves it on `clip:<name>`), paste position, the messages for a clipboard
// without a game, Ctrl+V with a PGN (a keydown, then a paste event with a
// stubbed clipboardData, as goto.js does), the New game menu and Ctrl+N,
// the reference dropdown, and the ribbon at narrower widths: labels drop,
// then groups collapse into dropdowns. Run with
// scripts/smoke/run.sh scripts/smoke/ribbon.js (GTK_THEME=Adwaita for light mode).
(async () => {
  const OUT = "__OUT__";
  const ROOT = "__ROOT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, tree, panels, notices, api, invoke } = window.__chessdb;
  // A native dialog would hang the run on the private display.
  game.confirmDiscard = async () => true;
  const log = (m) => invoke("dev_log", { message: String(m) });
  const shot = async (name) => {
    await wait(800);
    await log("shot:" + name);
    await wait(1500);
  };
  // The harness reads the clipboard after the log line; don't replace it meanwhile.
  const clip = async (name) => {
    await log("clip:" + name);
    await wait(1500);
  };
  const expect = (ok, what) => {
    if (!ok) throw new Error("expected " + what);
  };
  const until = async (cond, what, ms = 6000) => {
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const $ = (s) => document.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const notice = () => $("footer .notice")?.textContent.trim() ?? "";
  const noticeError = () => $("footer .notice")?.classList.contains("error") ?? false;
  // Notices stay up for seconds: clear the last one before waiting for the next.
  const clearNotice = () => (notices.current = null);
  // The live ribbon (not its hidden measuring copy).
  const groups = () => $$(".ribbon > .groups > .group");
  const rb = (label) => $$(".ribbon > .groups button").find((b) => b.getAttribute("aria-label") === label);
  const popupOf = (trigger) => document.getElementById(trigger.getAttribute("aria-controls") ?? "");
  const item = (popup, text) =>
    $$("[role^='menuitem']", popup).find((b) => b.textContent.replace(/\s+/g, " ").trim().startsWith(text));
  const key = (el, k, extra = {}) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...extra }));
  // WebKit refuses navigator.clipboard.writeText without a real user
  // gesture, so drivers fill the clipboard the way Copy game does.
  const setClipboard = async (text) => {
    await api.copyText(text);
    await wait(150);
  };
  // Ctrl+V outside a text box: the keydown, then the paste at whatever has the focus.
  const pressPaste = (text, from = document.body) => {
    from.dispatchEvent(new KeyboardEvent("keydown", { key: "v", ctrlKey: true, bubbles: true, cancelable: true }));
    const e = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(e, "clipboardData", { value: { getData: (t) => (t === "text/plain" ? text : "") } });
    (document.activeElement ?? document.body).dispatchEvent(e);
  };
  const mainLine = () => (game.notation?.moves ?? []).map((m) => m.san).join(" ");
  const START = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
  const THREE_GAMES =
    '[Event "Promotion and castling"]\n[SetUp "1"]\n[FEN "r3k2r/1P6/8/8/8/8/6p1/R3K2R w KQkq - 0 30"]\n\n' +
    "30. bxa8=Q+ Ke7 31. O-O-O gxh1=Q 32. Rxh1 1-0\n\n" +
    '[Event "Second"]\n\n1. e4 e5 *\n\n[Event "Third"]\n\n1. d4 d5 *\n';
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await db.createAt(OUT + "/ref.chessdb");
    await db.importFrom(ROOT + "/testdata/pgn/variations.pgn");
    await db.createAt(OUT + "/ribbon.chessdb");
    for (const f of ["lichess.pgn", "variations.pgn", "setup.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    await until(() => db.info?.index.built && db.info.index.upToDate, "the index built after the imports", 20000);

    // ---- The groups, in order, with their captions and buttons.
    await until(() => groups().length === 6, "six groups");
    const captions = groups().map((g) => g.querySelector(".caption")?.textContent.trim());
    expect(captions.join(",") === "Clipboard,Game,Reference,Analysis,Database,View", "captions, got " + captions);
    expect(groups().every((g) => g.dataset.form === "full"), "every group in full at the window's width");
    for (const label of ["Paste game", "Paste position", "Copy game", "Copy position", "Board image", "New game",
      "More ways to start a game", "Save", "Save as new", "Undo", "Redo", "Find games", "Go to position…", "Analyze",
      "Blunder check…", "Engines…", "New database", "Open database", "Recent", "Import PGN", "Export PGN",
      "Notation", "Opening tree", "Reference", "Reset layout"]) {
      const b = rb(label);
      expect(b, "a ribbon button " + label);
      expect(b.title, label + " has a tooltip");
    }
    expect(rb("Save").title === "Save (Ctrl+S)" && rb("Undo").title === "Undo (Ctrl+Z)", "shortcut hints kept");
    // Tab follows the order on screen: groups left to right.
    const lefts = groups().map((g) => g.getBoundingClientRect().left);
    expect(lefts.every((l, i) => i === 0 || l > lefts[i - 1]), "groups left to right in DOM order");
    expect($$(".ribbon > .groups button").every((b) => b.tabIndex === 0), "every ribbon button is reached by Tab");
    expect($(".ribbon .measure").inert, "the hidden measuring copy can't be reached");
    await shot("r01-ribbon");

    // ---- Copy game, then Paste game through the real clipboard.
    await game.open(db.info.handle, 0, 0);
    const original = await api.gamePgn(game.view.session);
    const moves = mainLine();
    const session = game.view.session;
    const tabs = game.tabs.length;
    clearNotice();
    rb("Copy game").click();
    await until(() => notice() === "Copied the game as PGN", "copied, got " + notice());
    await clip("r02-copied-game");
    expect((await api.clipboardText())?.trim() === original.trim(), "the clipboard holds the game's PGN");
    clearNotice();
    rb("Paste game").click();
    await until(() => notice() === "Pasted the game.", "pasted, got " + notice());
    expect(game.view.session === session && game.tabs.length === tabs, "the clean tab shows the pasted game");
    expect(!game.view.source && !game.view.dirty, "a new, unsaved game");
    expect(mainLine() === moves, "the same moves, got " + mainLine());
    expect((await api.gamePgn(game.view.session)).trim() === original.trim(), "the same PGN");
    expect(!rb("Save").disabled && !rb("Save as new").disabled, "it can be saved");
    await shot("r02-pasted-game");

    // With unsaved changes, the pasted game gets a new tab.
    await game.setHeaders({ ...game.view.headers, event: "Edited" });
    clearNotice();
    rb("Paste game").click();
    await until(() => game.tabs.length === tabs + 1, "a new tab for the pasted game");
    expect(game.tabs.some((t) => t.session === session && t.view.headers.event === "Edited"), "the edited game stays");

    // Several games: the first one opens.
    await setClipboard(THREE_GAMES);
    clearNotice();
    rb("Paste game").click();
    await until(() => notice() === "Pasted the first of 3 games.", "first of 3, got " + notice());
    expect(game.view.headers.event === "Promotion and castling" && game.notation.moves.length === 5,
      "the first game, from its set-up position");
    // A game with an illegal move: opened up to it, and the notice says where it stops.
    await setClipboard("1. e4 e5 2. Ke3 Nc6 *");
    clearNotice();
    rb("Paste game").click();
    await until(() => notice().startsWith("Pasted the game. The main line stops at 2.Ke3"), "the problem named, got " + notice());
    expect(mainLine() === "e4 e5", "opened up to the illegal move");

    // No game on the clipboard.
    await setClipboard("hello world");
    clearNotice();
    rb("Paste game").click();
    await until(() => notice() ===
      "The clipboard has no PGN game. Copy a game with its moves, or use Paste position for a FEN.",
    "no-game message, got " + notice());
    await shot("r03-no-game");
    await setClipboard('[Variant "Crazyhouse"]\n\n1. e4 *');
    clearNotice();
    rb("Paste game").click();
    await until(() => notice().startsWith("Can't open the pasted game. It's a Crazyhouse game") && noticeError(),
      "the reason, got " + notice());

    // ---- Paste position.
    const e4e5 = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
    panels.show("notation");
    await game.newGame();
    await setClipboard(e4e5);
    clearNotice();
    rb("Paste position").click();
    await until(() => game.view.fen === e4e5, "the board at the pasted FEN, got " + game.view.fen);
    expect(notice() === "Opened the position as a new game" && panels.isShowing("tree"), "as Go to position does");
    await setClipboard("hello world");
    clearNotice();
    rb("Paste position").click();
    await until(() => notice().startsWith("There's no FEN on the clipboard"), "no-FEN message, got " + notice());

    // ---- Ctrl+V: a PGN game, a game from a set-up position, and nothing.
    clearNotice();
    pressPaste('[White "Tal"]\n[Black "Botvinnik"]\n\n1. e4 c6 2. d4 d5 *');
    await until(() => game.view.headers.white === "Tal" && mainLine() === "e4 c6 d4 d5", "Ctrl+V opened the game");
    expect(notice() === "Pasted the game.", "the notice, got " + notice());
    pressPaste(THREE_GAMES);
    await until(() => game.view.headers.event === "Promotion and castling", "Ctrl+V with a [FEN] game");
    expect(game.notation.moves.length === 5, "it opened with its moves, not as the tag's position");
    pressPaste("1. d4 Nf6 2. c4 e6 *");
    await until(() => mainLine() === "d4 Nf6 c4 e6", "Ctrl+V with moves and no tags");
    clearNotice();
    pressPaste(e4e5);
    await until(() => game.view.fen === e4e5, "Ctrl+V with a FEN still goes to the position");
    clearNotice();
    pressPaste("hello world");
    await until(() => notice() ===
      "The clipboard has no game or position. Copy a PGN game, a FEN or a Lichess link and try again.",
    "nothing-to-paste message, got " + notice());

    // ---- New game ▾ and Ctrl+N.
    const arrow = rb("More ways to start a game");
    expect(arrow.getAttribute("aria-haspopup") === "menu" && arrow.getAttribute("aria-expanded") === "false", "a menu button");
    arrow.click();
    await until(() => popupOf(arrow), "the New game menu opens");
    const menu = popupOf(arrow);
    expect($$("[role='menuitem']", menu).map((b) => b.textContent.trim()).join("|") === "New game (Ctrl+N)|Set up position…",
      "its items");
    await until(() => document.activeElement === item(menu, "New game"), "the first item has the focus");
    key(document.activeElement, "ArrowDown");
    expect(document.activeElement === item(menu, "Set up position"), "↓ moves to the next item");
    key(document.activeElement, "ArrowDown");
    expect(document.activeElement === item(menu, "New game"), "↓ wraps around");
    key(document.activeElement, "End");
    expect(document.activeElement === item(menu, "Set up position"), "End goes to the last");
    await shot("r04-new-game-menu");
    key(document.activeElement, "Escape");
    await until(() => !popupOf(arrow) && document.activeElement === arrow, "Escape closes it, focus on its button");
    key(arrow, "ArrowDown");
    await until(() => popupOf(arrow) && document.activeElement === item(popupOf(arrow), "New game"), "↓ on the button opens it");
    item(popupOf(arrow), "Set up position").click();
    await until(() => $("[role='dialog'][aria-label='Set up a position']"), "Set up position… opens the editor");
    expect(!popupOf(arrow), "the menu closed");
    key($("[role='dialog']"), "Escape");
    await until(() => !$("[role='dialog']"), "the editor closes");
    await game.play("e2", "e4");
    rb("New game").click();
    await until(() => game.view.fen === START, "New game: the start position");
    // Ctrl+N, with unsaved changes: a new tab.
    await game.play("d2", "d4");
    const before = game.tabs.length;
    key(document.body, "n", { ctrlKey: true });
    await until(() => game.tabs.length === before + 1 && game.view.fen === START, "Ctrl+N: a new game in a new tab");

    // ---- The reference dropdown (DB-13, DB-07).
    const refButton = () => $(".ribbon > .groups .reference-menu button[aria-haspopup]");
    expect(refButton().textContent.trim() === "No reference database", "no reference yet, got " + refButton().textContent);
    expect(refButton().title.startsWith("Reference database:"), "its tooltip");
    refButton().click();
    await until(() => popupOf(refButton()) && item(popupOf(refButton()), "ref"), "the menu lists the recent databases");
    let refMenu = popupOf(refButton());
    const radios = $$("[role='menuitemradio']", refMenu);
    expect(radios.map((r) => r.textContent.trim()).join(",") === "ribbon,ref", "ribbon then ref, got " + radios.map((r) => r.textContent.trim()));
    expect(radios.every((r) => r.getAttribute("aria-checked") === "false"), "none ticked");
    expect(item(refMenu, "Use the open database (ribbon)"), "use the open database");
    expect(item(refMenu, "Stop using a reference database").disabled, "nothing to stop using");
    await shot("r05-reference-menu");
    item(refMenu, "ref").click();
    await until(() => db.reference?.info && db.reference.name === "ref" && refButton().textContent.trim() === "ref",
      "ref is the reference");
    expect(!popupOf(refButton()), "picking closes the menu");
    // The tree follows when its source is the reference; its select has two choices.
    panels.show("tree");
    tree.source = "reference";
    await until(() => tree.data && tree.target?.name === "ref", "the tree reads ref");
    const options = $$(".tree select option").map((o) => o.textContent.trim());
    expect(options.join("|") === "ribbon|ref (reference)", "the tree's two choices, got " + options.join("|"));
    refButton().click();
    await until(() => popupOf(refButton()), "the menu again");
    refMenu = popupOf(refButton());
    expect(item(refMenu, "ref").getAttribute("aria-checked") === "true", "ref ticked");
    item(refMenu, "Use the open database (ribbon)").click();
    await until(() => db.reference?.info?.handle === db.info.handle && refButton().textContent.trim() === "ribbon",
      "the open database is the reference");
    expect(tree.target?.handle === db.info.handle, "the tree follows");
    refButton().click();
    await until(() => popupOf(refButton()), "the menu once more");
    refMenu = popupOf(refButton());
    expect(!item(refMenu, "Use the open database"), "no 'use the open database' when it's the reference");
    expect(item(refMenu, "ribbon").getAttribute("aria-checked") === "true", "ribbon ticked");
    item(refMenu, "Stop using a reference database").click();
    await until(() => !db.reference && refButton().textContent.trim() === "No reference database", "no reference");
    // Find games in the reference: it opens in the list with the games reaching the position.
    await db.setReference(OUT + "/ref.chessdb");
    await until(() => db.reference?.info?.index.upToDate, "ref indexed");
    await game.newGame();
    await game.play("e2", "e4");
    rb("Find games").click();
    await until(() => db.info?.name === "ref" && db.info.filter, "Find games opened ref with the games reaching 1.e4");
    await db.openAt(OUT + "/ribbon.chessdb");
    await db.setReference(null);
    // A database that's gone is listed but can't be picked.
    const loadRecent = db.loadRecent;
    db.loadRecent = async () => {};
    db.recent = [...db.recent, { path: OUT + "/gone.chessdb", name: "gone", exists: false }];
    refButton().click();
    await until(() => popupOf(refButton()) && item(popupOf(refButton()), "gone"), "the gone database listed");
    const gone = item(popupOf(refButton()), "gone");
    expect(gone.disabled && gone.textContent.includes("Not found"), "disabled and marked Not found");
    key(document.activeElement, "Escape");
    await until(() => !popupOf(refButton()), "closed");
    db.loadRecent = loadRecent;
    await db.loadRecent();

    // ---- Narrower windows: labels go first, then groups collapse from the right.
    const app = document.getElementById("app");
    let seenCompact = false;
    let seenCollapsed = false;
    let iconOnly = false;
    // Below about 440 px every group is collapsed and the row can't shrink further.
    for (const w of [1500, 1200, 1000, 800, 640, 560, 480]) {
      app.style.width = w + "px";
      await wait(300);
      const ribbon = $(".ribbon").getBoundingClientRect();
      const forms = groups().map((g) => g.dataset.form);
      await log(`  ${w}px: ${forms.join(", ")}`);
      for (const g of groups()) {
        const r = g.getBoundingClientRect();
        expect(r.right <= ribbon.right + 0.5 && Math.abs(r.top - ribbon.top - 0) < 2, `${w}px: ${g.dataset.group} on the row, inside`);
      }
      expect(ribbon.height < 110, `${w}px: one row`);
      // From the right: no full group after a compact one, no compact after a collapsed one.
      const rank = { full: 0, compact: 1, collapsed: 2 };
      expect(forms.every((f, i) => i === 0 || rank[f] >= rank[forms[i - 1]]), `${w}px: from the right, got ${forms}`);
      if (forms.includes("compact")) seenCompact = true;
      if (forms.includes("collapsed")) seenCollapsed = true;
      if (forms[5] === "compact" && !iconOnly) {
        iconOnly = true;
        // Icon-only buttons keep their name and tooltip.
        const b = rb("Opening tree");
        const label = b.querySelector(".label").getBoundingClientRect();
        expect(label.width <= 1 && b.getAttribute("aria-label") === "Opening tree" && b.title, "icon only, named");
      }
      if (w === 800) await shot("r06-compact-800");
    }
    expect(seenCompact && seenCollapsed && iconOnly, "compact and collapsed groups were seen");

    // A collapsed group opens as a dropdown; its buttons have their labels.
    app.style.width = "520px";
    await wait(300);
    const viewButton = rb("View");
    expect(viewButton && viewButton.getAttribute("aria-expanded") === "false", "View collapsed into a button");
    viewButton.click();
    await until(() => popupOf(viewButton), "the View group opens");
    const panel = popupOf(viewButton);
    const notationButton = $$("button", panel).find((b) => b.getAttribute("aria-label") === "Notation");
    const treeButton = $$("button", panel).find((b) => b.getAttribute("aria-label") === "Opening tree");
    expect(treeButton && treeButton.querySelector(".label").getBoundingClientRect().width > 20, "labelled in the dropdown");
    await until(() => document.activeElement === notationButton, "its first button focused");
    await shot("r07-collapsed-view");
    panels.show("notation");
    treeButton.click();
    await until(() => panels.isShowing("tree") && !popupOf(viewButton), "the button works and the dropdown closes");
    // A menu inside a collapsed group: Escape closes one at a time.
    app.style.width = "480px";
    await wait(300);
    const clipButton = rb("Clipboard");
    expect(clipButton, "the Clipboard group collapsed too");
    clipButton.click();
    await until(() => popupOf(clipButton), "the Clipboard group opens");
    const imageButton = $$("button", popupOf(clipButton)).find((b) => b.getAttribute("aria-label") === "Board image");
    imageButton.click();
    await until(() => popupOf(imageButton) && item(popupOf(imageButton), "Copy as PNG"), "the Board image menu inside it");
    await shot("r08-nested-menu");
    key(document.activeElement, "Escape");
    await until(() => !popupOf(imageButton) && popupOf(clipButton) && document.activeElement === imageButton,
      "Escape closed the inner menu only");
    key(document.activeElement, "Escape");
    await until(() => !popupOf(clipButton) && document.activeElement === clipButton, "then the group");
    app.style.width = "";
    await wait(300);
    expect(groups().every((g) => g.dataset.form === "full"), "back to full at the window's width");

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
