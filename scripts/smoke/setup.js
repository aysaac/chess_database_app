// Smoke-test driver for the position setup editor (UI-07). Placing pieces
// by clicking or dragging can't be scripted (chessground ignores synthetic
// input); everything else is exercised through the dialog's controls.
(async () => {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { game, invoke } = window.__chessdb;
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
  const until = async (cond, what, ms = 4000) => {
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const $ = (s) => document.querySelector(s);
  const button = (text) => [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === text);
  const status = () => $(".editor .status")?.textContent.trim() ?? "";
  const pieces = () => document.querySelectorAll(".editor .board cg-board piece").length;
  const fenInput = () => $(".editor input.fen");
  const typeFen = async (fen) => {
    const input = fenInput();
    input.focus();
    input.value = fen;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.blur();
    await wait(300);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    await game.newGame();
    // In the ribbon, Set up position… is in the New game menu (UI-18).
    $(".ribbon > .groups button[aria-label='More ways to start a game']").click();
    await until(() => button("Set up position…"), "the New game menu");
    button("Set up position…").click();
    await until(() => pieces() === 32, "the editor shows the current position");
    await until(() => status().startsWith("32 pieces"), "a legal position: " + status());
    await shot("u01-editor");

    button("Clear board").click();
    await until(() => pieces() === 0 && status().includes("the board is empty"), "empty board rejected: " + status());
    expect(button("Start a game from here").disabled, "can't start from an empty board");

    await typeFen("8/8/8/8/8/8/8/4K3 w - - 0 1");
    await until(() => status().includes("each side needs a king"), "missing king explained: " + status());

    await typeFen("4k3/8/8/8/8/8/4P3/4K2R w K - 0 1");
    await until(() => pieces() === 4 && status().startsWith("4 pieces"), "position from the FEN: " + status());
    const castle = [...document.querySelectorAll(".editor fieldset label")];
    const box = (text) => castle.find((l) => l.textContent.trim() === text).querySelector("input");
    expect(box("White O-O").checked && !box("White O-O").disabled, "White can castle short");
    expect(box("White O-O-O").disabled && box("Black O-O").disabled, "other castling impossible");

    castle.find((l) => l.textContent.trim() === "Black").querySelector("input").click();
    await until(() => fenInput().value === "4k3/8/8/8/8/8/4P3/4K2R b K - 0 1", "FEN follows the controls: " + fenInput().value);
    await shot("u02-editor-endgame");

    button("Start a game from here").click();
    await until(() => !$(".editor") && game.view?.fen === "4k3/8/8/8/8/8/4P3/4K2R b K - 0 1",
      "a game from the position, got " + game.view?.fen);

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
