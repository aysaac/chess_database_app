// Smoke-test driver for "go to position" (QOL-10): Ctrl+V with a FEN
// outside a text box, and the "Go to position" dialog. Real keys can't be
// pressed here, so the driver sends what Ctrl+V causes: a keydown, then a
// paste event at whatever has the focus after the keydown handlers ran.
(async () => {
  const OUT = "__OUT__";
  const ROOT = "__ROOT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, tree, panels, invoke } = window.__chessdb;
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
  const $ = (s) => document.querySelector(s);
  const button = (text) => [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === text);
  const notice = () => $("footer .notice")?.textContent.trim() ?? "";
  const sink = () => $("textarea.sink");
  let focusDuringPaste = null;
  // Ctrl+V: the keydown goes to `from`; the paste to the element focused then.
  const pressPaste = (text, from = document.activeElement ?? document.body) => {
    from.dispatchEvent(new KeyboardEvent("keydown", { key: "v", ctrlKey: true, bubbles: true, cancelable: true }));
    focusDuringPaste = document.activeElement;
    const e = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(e, "clipboardData", { value: { getData: (t) => (t === "text/plain" ? text : "") } });
    (document.activeElement ?? document.body).dispatchEvent(e);
    return e.defaultPrevented;
  };
  const fields = (fen) => fen.split(" ");
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await db.createAt(OUT + "/goto.chessdb");
    for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
      await db.importFrom(ROOT + "/testdata/pgn/" + f);
    }
    await until(() => db.info?.index.built && db.info.index.upToDate, "the index built after the imports", 20000);
    panels.show("notation");
    await game.newGame();
    const tabs = game.tabs.length;

    // A Lichess link on the clipboard, pasted with the game list focused: a
    // new game from the position (in this clean tab), the tree opens on it,
    // and the focus goes back to the list.
    const list = $(".games .viewport");
    list.focus();
    const e4e5 = "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2";
    const link = "https://lichess.org/analysis/standard/" + e4e5.replaceAll(" ", "_");
    expect(pressPaste(link, list), "the paste was taken");
    expect(focusDuringPaste === sink(), "Ctrl+V moved the focus to the paste target");
    expect(document.activeElement === list, "the focus went back to the list");
    await until(() => game.view?.fen === e4e5, "the board shows the pasted position, got " + game.view?.fen);
    expect(game.tabs.length === tabs && !game.view.source, "a new game in the same tab");
    expect(panels.isShowing("tree"), "the Opening tree tab showing");
    await until(() => tree.data?.fen === e4e5 && tree.data.total.games === 2,
      "2 games reach 1.e4 e5, got " + tree.data?.total.games);
    expect(notice().includes("Opened the position as a new game"), "a notice, got " + notice());
    await shot("g01-pasted-link");

    // A position the game on screen reaches: the cursor goes there (other
    // move counters), in the same tab.
    await game.open(db.info.handle, 0, 0);
    const session = game.view.session;
    const target = game.notation.moves[9];
    await game.goto(target.id);
    const deep = fields(game.view.fen);
    await game.step("start");
    pressPaste([...deep.slice(0, 4), "0", "1"].join(" "), document.body);
    await until(() => game.view.cursor === target.id, "the cursor at the pasted position, got " + game.view.cursor);
    expect(game.view.session === session, "same game, same tab");
    expect(notice().includes("This game reaches that position at 5"), "the move named, got " + notice());
    await until(() => tree.data?.fen === game.view.fen, "the tree follows");
    await shot("g02-in-game");

    // A FEN the game doesn't reach while it has unsaved changes: a new tab.
    await game.setHeaders({ ...game.view.headers, event: "Edited" });
    const before = game.tabs.length;
    pressPaste("8/8/4k3/8/8/4K3/8/8 w - - 0 1", document.body);
    await until(() => game.tabs.length === before + 1, "a new tab next to the edited game");
    expect(game.view.fen === "8/8/4k3/8/8/4K3/8/8 w - - 0 1", "the new tab shows the position");
    expect(game.tabs.some((t) => t.session === session && t.view.dirty), "the edited game stays open");
    // A set-up position's move number says nothing about its depth, so the
    // tree offers the full search when the index has no game with it.
    await until(() => tree.data?.fen === game.view.fen, "tree for the pasted endgame");
    expect(tree.data.total.games === 0 && tree.data.beyondIndex, "no games, full search offered");
    expect(button("Search every move of every game"), "the full-search link");
    await shot("g03-endgame-new-tab");

    // Text with neither a FEN nor a game (a PGN game opens since IO-08; see
    // ribbon.js), a FEN of an illegal position, an empty clipboard.
    const fen = game.view.fen;
    pressPaste("hello world", document.body);
    await until(() => notice().startsWith("The clipboard has no game or position"), "nothing-to-paste notice, got " + notice());
    pressPaste("8/8/8/8/8/8/8/4K3 w - - 0 1", document.body);
    await until(() => notice() === "Can't go to the pasted position: each side needs a king.",
      "illegal-position notice, got " + notice());
    expect($("footer .notice.error"), "shown as an error");
    expect(pressPaste("", document.body) && document.activeElement !== sink(), "an empty paste changes nothing");
    await wait(300);
    expect(game.view.fen === fen, "the board kept its position");

    // In a text box, Ctrl+V pastes there as usual.
    const filter = $("input.quick");
    filter.focus();
    expect(!pressPaste(e4e5, filter) && document.activeElement === filter, "the filter box keeps its paste");
    await wait(300);
    expect(game.view.fen === fen, "a paste into the filter box doesn't move the board");
    // Ctrl+V without a paste after it: the focus comes back anyway.
    list.focus();
    list.dispatchEvent(new KeyboardEvent("keydown", { key: "v", ctrlKey: true, bubbles: true }));
    expect(document.activeElement === sink(), "the paste target has the focus");
    await until(() => document.activeElement === list, "the focus back on the list", 1500);

    // The dialog: errors in place, then a position.
    button("Go to position…").click();
    await until(() => $("[role='dialog'] input"), "the dialog opened");
    const input = $("[role='dialog'] input");
    const submit = () => [...document.querySelectorAll("[role='dialog'] button")].find((b) => b.textContent.trim() === "Go to position");
    // Waits for the button to follow the text (it's disabled while empty).
    const type = async (text) => {
      input.value = text;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await wait(100);
    };
    expect(submit().disabled, "nothing to go to yet");
    // A paste while the dialog is open goes nowhere else.
    pressPaste(e4e5);
    await wait(300);
    expect(game.view.fen === fen && $("[role='dialog']"), "the dialog is left alone");
    await type("hello");
    submit().click();
    await until(() => $("#fen-dialog-error")?.textContent.startsWith("That isn't a FEN"), "not-a-FEN message");
    await type("4k3/8/8/8/8/8/8/4K2R w KQ - 0 1");
    submit().click();
    await until(() => $("#fen-dialog-error")?.textContent.includes("castling needs the king and that rook"),
      "illegal castling explained, got " + $("#fen-dialog-error")?.textContent);
    await shot("g04-dialog-error");
    await type(`[FEN "${e4e5}"]`);
    submit().click();
    await until(() => !$("[role='dialog']") && game.view.fen === e4e5, "the dialog went to the FEN tag's position");
    await until(() => tree.data?.fen === e4e5 && tree.data.total.games === 2, "the tree for it");

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
